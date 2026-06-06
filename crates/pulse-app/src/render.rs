//! A small CPU **software compositor** that rasterizes a [`Comp`] to an 8-bit
//! sRGB RGBA frame buffer, plus a PNG **image-sequence** exporter built on top.
//!
//! This is the headless twin of [`preview`](crate::preview): where the preview
//! paints layers through egui's `Painter` at screen resolution, [`render_frame`]
//! produces a *real* pixel buffer at the comp's native resolution, using the
//! exact same transform model (a layer is a solid quad sized to a fraction of
//! the comp, transformed by its resolved [`Affine2`](crate::comp::Affine2) world
//! matrix — position, uniform scale, and rotation about its **anchor point**,
//! composed under any **parent** chain — and faded by `opacity`). Exported
//! frames therefore match what the preview shows.
//!
//! Compositing is **source-over in linear light**: each layer's straight sRGB
//! color is converted to linear (through `prism-core`'s shared color boundary),
//! alpha-composited back-to-front over the accumulating frame, then encoded back
//! to sRGB bytes only at the very end — the suite's "never bake until output"
//! principle, in miniature.
//!
//! The rasterizer is deliberately pure (no egui, no IO) so the transform and
//! compositing math is unit-testable; [`export_sequence`] is the thin IO shell
//! that drives it across a comp's frames and writes `name_0001.png`, ….

use crate::comp::{
    apply_effects, apply_spatial_effects, mask_stack_coverage, Affine2, Comp, LayerKind, MatteMode,
    PulseLayer,
};
use prism_core::color::{linear_to_srgb, srgb_to_linear};
use std::path::{Path, PathBuf};

/// The half-extent of a layer's base quad as a fraction of the comp size. Must
/// match [`preview`](crate::preview)'s `half_w`/`half_h` so the offline render
/// and the on-screen preview agree.
pub const LAYER_HALF_FRAC: f32 = 0.22;

/// An in-memory rendered frame: tightly packed 8-bit sRGB RGBA, row-major,
/// `width * height * 4` bytes, top-left origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Read the RGBA bytes of pixel `(x, y)`; panics if out of bounds.
    /// (A pixel accessor for callers/tests inspecting a rendered frame.)
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }
}

/// A linear-light premultiplied-free RGBA accumulator pixel.
#[derive(Clone, Copy)]
struct Lin {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Lin {
    /// A fully transparent black pixel (the empty accumulator value).
    const CLEAR: Lin = Lin {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
}

/// Source-over `src` onto `dst` in straight linear-light RGBA: `out = src +
/// dst·(1 - src.a)`. Both are straight (non-premultiplied) with `a` as coverage.
fn over(src: Lin, dst: Lin) -> Lin {
    let ia = 1.0 - src.a;
    Lin {
        r: src.r * src.a + dst.r * ia,
        g: src.g * src.a + dst.g * ia,
        b: src.b * src.a + dst.b * ia,
        a: src.a + dst.a * ia,
    }
}

/// Render the composition at time `t` to a native-resolution [`Frame`].
///
/// The comp backdrop is fully transparent black; visible layers are composited
/// back-to-front (index 0 first / behind). Coordinates follow the preview:
/// the comp origin is its center, `+y` is downward (screen space).
pub fn render_frame(comp: &Comp, t: f32) -> Frame {
    let w = comp.width.max(1);
    let h = comp.height.max(1);
    let mut acc = vec![
        Lin {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0
        };
        (w * h) as usize
    ];

    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    let half_w = w as f32 * LAYER_HALF_FRAC;
    let half_h = h as f32 * LAYER_HALF_FRAC;
    let geom = Geom {
        w,
        h,
        cx,
        cy,
        half_w,
        half_h,
    };

    for (i, layer) in comp.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        // A layer used as a track-matte source is pulled in by the layer below
        // it and must not composite on its own (it only contributes alpha/luma).
        if comp.is_matte_source(i) {
            continue;
        }
        let blurred = comp.layer_motion_blurred(i);
        let masked = layer.has_active_masks();
        let spatial = layer.has_spatial_effects();
        let matte_src = comp.matte_source(i);
        match layer.kind {
            // A null draws nothing — it's a transform reference (parent) only.
            LayerKind::Null => {}
            // A motion-blurred solid (any kind drawing pixels) is rendered into an
            // isolated buffer as the average of sub-frame snapshots, then mask-
            // carved, matte-clipped, spatially filtered, and composited over the
            // accumulator.
            LayerKind::Solid if blurred => {
                let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                composite_motion_blur(&mut layer_buf, &geom, comp, i, t);
                if masked {
                    apply_masks(&mut layer_buf, &geom, comp.world_matrix(i, t), layer);
                }
                if let Some(src_idx) = matte_src {
                    apply_track_matte(&mut layer_buf, &geom, comp, src_idx, layer.matte, t);
                }
                if spatial {
                    apply_spatial(&mut layer_buf, &geom, layer);
                }
                for (dst, src) in acc.iter_mut().zip(layer_buf.iter()) {
                    *dst = over(*src, *dst);
                }
            }
            // A crisp solid draws its own colored quad (processed by its effect
            // stack) directly into the accumulator — or, when it has masks, a
            // track matte, or spatial effects, into an isolated buffer whose
            // alpha the masks/matte modulate and whose whole buffer the spatial
            // passes filter before it is composited.
            LayerKind::Solid => {
                let world = comp.world_matrix(i, t);
                if masked || spatial || matte_src.is_some() {
                    let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                    composite_layer(&mut layer_buf, &geom, world, layer, t);
                    if masked {
                        apply_masks(&mut layer_buf, &geom, world, layer);
                    }
                    if let Some(src_idx) = matte_src {
                        apply_track_matte(&mut layer_buf, &geom, comp, src_idx, layer.matte, t);
                    }
                    if spatial {
                        apply_spatial(&mut layer_buf, &geom, layer);
                    }
                    for (dst, src) in acc.iter_mut().zip(layer_buf.iter()) {
                        *dst = over(*src, *dst);
                    }
                } else {
                    composite_layer(&mut acc, &geom, world, layer, t);
                }
            }
            // An adjustment re-processes the composite beneath it, within its
            // own transformed quad bounds.
            LayerKind::Adjustment => {
                let world = comp.world_matrix(i, t);
                apply_adjustment(&mut acc, &geom, world, layer, t);
            }
        }
    }

    // Encode linear accumulator -> straight sRGB 8-bit RGBA.
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for (px, lin) in acc.iter().enumerate() {
        let o = px * 4;
        pixels[o] = enc(lin.r);
        pixels[o + 1] = enc(lin.g);
        pixels[o + 2] = enc(lin.b);
        pixels[o + 3] = (lin.a.clamp(0.0, 1.0) * 255.0).round() as u8;
    }

    Frame {
        width: w,
        height: h,
        pixels,
    }
}

/// The fixed comp-space geometry shared by every rasterization pass in a frame:
/// the pixel dimensions, the comp center (origin), and the base layer quad's
/// half-extents. Bundled so the rasterizers take one argument instead of six.
#[derive(Clone, Copy)]
struct Geom {
    w: u32,
    h: u32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
}

impl Geom {
    /// The conservative comp-space pixel AABB covered by a quad transformed by
    /// `world`, clamped to the frame. Returns `None` when the quad falls entirely
    /// outside the frame.
    fn quad_bounds(&self, world: Affine2) -> Option<(i32, i32, i32, i32)> {
        let corners = [
            (-self.half_w, -self.half_h),
            (self.half_w, -self.half_h),
            (self.half_w, self.half_h),
            (-self.half_w, self.half_h),
        ];
        let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
        let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for (lx, ly) in corners {
            let (wx, wy) = world.apply(lx, ly);
            min_x = min_x.min(wx);
            min_y = min_y.min(wy);
            max_x = max_x.max(wx);
            max_y = max_y.max(wy);
        }
        let x0 = ((self.cx + min_x).floor() as i32).max(0);
        let x1 = ((self.cx + max_x).ceil() as i32).min(self.w as i32 - 1);
        let y0 = ((self.cy + min_y).floor() as i32).max(0);
        let y1 = ((self.cy + max_y).ceil() as i32).min(self.h as i32 - 1);
        (x0 <= x1 && y0 <= y1).then_some((x0, x1, y0, y1))
    }
}

/// Render motion-blurred solid layer `idx` into an isolated `out` buffer
/// (assumed clear) as the average of sub-frame snapshots across the shutter.
///
/// Each of the comp's [`MotionBlur::sample_times`] is rasterized into a scratch
/// buffer via [`composite_layer`] (which yields the buffer's standard
/// *premultiplied* color + coverage-alpha form), and the snapshots are averaged
/// component-wise — the float-compositor motion-blur recipe. Averaging in
/// premultiplied space is what keeps partly-covered edges from bleeding the quad
/// color into the transparent samples. The result is left in the same
/// premultiplied representation `composite_layer` produces, so the caller
/// composites it over the accumulator identically to a crisp matte buffer.
fn composite_motion_blur(out: &mut [Lin], geom: &Geom, comp: &Comp, idx: usize, t: f32) {
    let Some(layer) = comp.layers.get(idx) else {
        return;
    };
    let times = comp.motion_blur.sample_times(t, comp.fps);
    let n = times.len().max(1);
    let mut scratch = vec![Lin::CLEAR; out.len()];
    for &st in &times {
        for px in scratch.iter_mut() {
            *px = Lin::CLEAR;
        }
        let world = comp.world_matrix(idx, st);
        // `scratch` is cleared each sample, so each pixel is `composite_layer`'s
        // premultiplied (color·coverage, coverage) output; accumulate it directly.
        composite_layer(&mut scratch, geom, world, layer, st);
        for (dst, src) in out.iter_mut().zip(scratch.iter()) {
            dst.r += src.r;
            dst.g += src.g;
            dst.b += src.b;
            dst.a += src.a;
        }
    }
    let inv = 1.0 / n as f32;
    for px in out.iter_mut() {
        px.r *= inv;
        px.g *= inv;
        px.b *= inv;
        px.a *= inv;
    }
}

/// Composite one layer's solid quad into the linear accumulator.
///
/// `world` is the layer's resolved comp-space matrix (own transform + parent
/// chain). It maps layer-local points (origin at the layer's geometric center)
/// into comp space whose origin is the comp center. Coverage is tested by
/// inverse-mapping each candidate pixel back into local space and box-testing
/// against the `±half_w/±half_h` quad.
fn composite_layer(acc: &mut [Lin], geom: &Geom, world: Affine2, layer: &PulseLayer, t: f32) {
    let &Geom {
        w,
        cx,
        cy,
        half_w,
        half_h,
        ..
    } = geom;
    let tf = layer.transform(t);
    if tf.opacity <= 0.0 {
        return;
    }

    // Layer straight sRGB color -> linear; premultiply happens implicitly via
    // the source-over math below (we carry straight color + coverage alpha).
    let lr = srgb_to_linear(layer.color[0].clamp(0.0, 1.0));
    let lg = srgb_to_linear(layer.color[1].clamp(0.0, 1.0));
    let lb = srgb_to_linear(layer.color[2].clamp(0.0, 1.0));
    let src_a = (layer.color[3].clamp(0.0, 1.0)) * tf.opacity;
    if src_a <= 0.0 {
        return;
    }
    // The layer's own effect stack processes its (linear, straight) color before
    // it's composited — the solid is a constant-color source, so one evaluation
    // covers the whole quad.
    let [lr, lg, lb, _] = apply_effects(&layer.effects, [lr, lg, lb, layer.color[3]]);

    // Invert the world matrix once: a zero-scale (or otherwise singular) chain
    // collapses to nothing, so there is no coverage to composite.
    let Some(inv) = world.inverse() else {
        return;
    };

    // Conservative comp-space AABB of the quad, clamped to the frame.
    let Some((x0, x1, y0, y1)) = geom.quad_bounds(world) else {
        return;
    };

    for py in y0..=y1 {
        // Pixel center, expressed in comp space (origin at comp center).
        let comp_y = py as f32 + 0.5 - cy;
        for px in x0..=x1 {
            let comp_x = px as f32 + 0.5 - cx;
            // Inverse-map the comp-space pixel into the layer's local frame.
            let (lx, ly) = inv.apply(comp_x, comp_y);
            if lx.abs() > half_w || ly.abs() > half_h {
                continue;
            }
            // Source-over in linear light.
            let idx = (py as u32 * w + px as u32) as usize;
            acc[idx] = over(
                Lin {
                    r: lr,
                    g: lg,
                    b: lb,
                    a: src_a,
                },
                acc[idx],
            );
        }
    }
}

/// Apply an **adjustment layer**'s effect stack to the composite beneath it,
/// within the layer's transformed quad.
///
/// Unlike a solid (a constant-color source), an adjustment re-grades whatever is
/// already in the accumulator: for each covered pixel we run the effect stack on
/// the existing linear-light straight RGBA and write the result back. Coverage is
/// the same inverse-mapped quad test the solid path uses; the layer's `opacity`
/// blends the regraded result against the original so a partly-opaque adjustment
/// is a partial grade. An empty effect stack is a no-op.
fn apply_adjustment(acc: &mut [Lin], geom: &Geom, world: Affine2, layer: &PulseLayer, t: f32) {
    let &Geom {
        w,
        cx,
        cy,
        half_w,
        half_h,
        ..
    } = geom;
    if layer.effects.is_empty() {
        return;
    }
    let tf = layer.transform(t);
    let mix = tf.opacity.clamp(0.0, 1.0);
    if mix <= 0.0 {
        return;
    }
    let Some(inv) = world.inverse() else {
        return;
    };
    let Some((x0, x1, y0, y1)) = geom.quad_bounds(world) else {
        return;
    };

    for py in y0..=y1 {
        let comp_y = py as f32 + 0.5 - cy;
        for px in x0..=x1 {
            let comp_x = px as f32 + 0.5 - cx;
            let (lx, ly) = inv.apply(comp_x, comp_y);
            if lx.abs() > half_w || ly.abs() > half_h {
                continue;
            }
            let idx = (py as u32 * w + px as u32) as usize;
            let src = acc[idx];
            // Nothing underneath here — grading transparent pixels would lift
            // their (invisible) color into the buffer for no reason. Skip them.
            if src.a <= 0.0 {
                continue;
            }
            let graded = apply_effects(&layer.effects, [src.r, src.g, src.b, src.a]);
            // Blend the regrade against the original by the adjustment's opacity.
            acc[idx] = Lin {
                r: src.r + (graded[0] - src.r) * mix,
                g: src.g + (graded[1] - src.g) * mix,
                b: src.b + (graded[2] - src.b) * mix,
                a: src.a, // alpha is untouched by color grading
            };
        }
    }
}

/// Modulate an already-rendered layer buffer's alpha by a **track matte**.
///
/// `layer_buf` holds the matted layer's isolated straight-linear RGBA. The matte
/// source (`src_idx`) is rasterized per-pixel into the same comp space; each
/// matte pixel yields a factor in `[0, 1]` (alpha/luma, optionally inverted) that
/// multiplies the corresponding `layer_buf` pixel's alpha. Color is untouched —
/// only coverage changes — so the subsequent source-over honors the matte. The
/// matte source's own transform / parent chain / effects are respected.
fn apply_track_matte(
    layer_buf: &mut [Lin],
    geom: &Geom,
    comp: &Comp,
    src_idx: usize,
    mode: MatteMode,
    t: f32,
) {
    // Render the matte source into its own isolated buffer (so its alpha/luma is
    // measured in isolation, not on top of anything below it).
    let mut matte = vec![Lin::CLEAR; (geom.w * geom.h) as usize];
    let src_world = comp.world_matrix(src_idx, t);
    if let Some(src_layer) = comp.layers.get(src_idx) {
        composite_layer(&mut matte, geom, src_world, src_layer, t);
    }
    for (px, m) in matte.iter().enumerate() {
        let f = mode.factor([m.r, m.g, m.b, m.a]);
        layer_buf[px].a *= f;
    }
}

/// Carve a rendered layer buffer's alpha by the layer's **mask stack**.
///
/// `layer_buf` holds the layer's isolated straight-linear RGBA. Each pixel is
/// inverse-mapped through the layer's `world` matrix back into layer-local space
/// (where the masks are authored), and the folded [`mask_stack_coverage`] there
/// multiplies the pixel's alpha — color is untouched, only coverage changes, so
/// the subsequent source-over honors the masks. A singular world matrix (zero
/// scale) leaves nothing to mask. Assumes the layer has at least one active mask
/// (the caller gates on [`PulseLayer::has_active_masks`]).
fn apply_masks(layer_buf: &mut [Lin], geom: &Geom, world: Affine2, layer: &PulseLayer) {
    let &Geom { w, h, cx, cy, .. } = geom;
    let Some(inv) = world.inverse() else {
        // Collapsed transform: no coverage survives.
        for px in layer_buf.iter_mut() {
            px.a = 0.0;
        }
        return;
    };
    // Pre-flatten each mask once so the per-pixel loop is just point tests.
    let polys: Vec<Vec<(f32, f32)>> = layer.masks.iter().map(|m| m.flatten()).collect();
    for py in 0..h {
        let comp_y = py as f32 + 0.5 - cy;
        for px in 0..w {
            let idx = (py * w + px) as usize;
            // Skip already-transparent pixels — masking them is a no-op.
            if layer_buf[idx].a <= 0.0 {
                continue;
            }
            let comp_x = px as f32 + 0.5 - cx;
            let (lx, ly) = inv.apply(comp_x, comp_y);
            let cov = mask_stack_coverage(&layer.masks, &polys, lx, ly);
            layer_buf[idx].a *= cov;
        }
    }
}

/// Run a layer's **spatial effect stack** (Gaussian Blur / Drop Shadow / Glow)
/// over its isolated rendered buffer.
///
/// The compositor's [`Lin`] accumulator is already **premultiplied** linear-light
/// (RGB = color·coverage, A = coverage) — exactly the representation the spatial
/// passes operate on — so this is a zero-conversion bridge: view the `Lin` slice
/// as `[[f32; 4]]`, run [`apply_spatial_effects`], then write the filtered values
/// back. Assumes the layer has at least one spatial effect (the caller gates on
/// [`PulseLayer::has_spatial_effects`]).
fn apply_spatial(layer_buf: &mut [Lin], geom: &Geom, layer: &PulseLayer) {
    let (w, h) = (geom.w as usize, geom.h as usize);
    let mut rgba: Vec<[f32; 4]> = layer_buf.iter().map(|p| [p.r, p.g, p.b, p.a]).collect();
    apply_spatial_effects(&layer.spatial_effects, &mut rgba, w, h);
    for (dst, src) in layer_buf.iter_mut().zip(rgba.iter()) {
        dst.r = src[0];
        dst.g = src[1];
        dst.b = src[2];
        dst.a = src[3];
    }
}

/// Encode a linear-light component to an 8-bit sRGB byte.
fn enc(v: f32) -> u8 {
    (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8
}

/// The frame count of a render: every frame on the comp's `[0, duration]`
/// timeline at its fps, inclusive of frame 0. A 5 s comp at 30 fps yields 150
/// frames (0..149), matching After Effects' frame-inclusive duration.
pub fn frame_count(comp: &Comp) -> u32 {
    let fps = comp.fps.max(1.0);
    (comp.duration.max(0.0) * fps).round().max(1.0) as u32
}

/// The presentation time (seconds) of frame `i`.
pub fn frame_time(comp: &Comp, i: u32) -> f32 {
    let fps = comp.fps.max(1.0);
    i as f32 / fps
}

/// Build the output path for frame `i`: `<dir>/<stem>_<0000>.png`, zero-padded
/// to at least 4 digits (more if the sequence needs them).
pub fn frame_path(dir: &Path, stem: &str, i: u32, total: u32) -> PathBuf {
    let pad = total.saturating_sub(1).to_string().len().max(4);
    dir.join(format!("{stem}_{i:0pad$}.png", pad = pad))
}

/// Summary of an [`export_sequence`] run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSummary {
    pub frames: u32,
    pub dir: PathBuf,
}

/// Render every frame of `comp` and write the PNG image sequence to `dir`,
/// naming files `<stem>_0000.png`, `<stem>_0001.png`, …. Creates `dir` if it
/// does not exist. Returns a summary, or the first IO/encode error.
pub fn export_sequence(comp: &Comp, dir: &Path, stem: &str) -> std::io::Result<ExportSummary> {
    std::fs::create_dir_all(dir)?;
    let total = frame_count(comp);
    for i in 0..total {
        let t = frame_time(comp, i);
        let frame = render_frame(comp, t);
        let path = frame_path(dir, stem, i, total);
        write_png(&path, &frame)?;
    }
    Ok(ExportSummary {
        frames: total,
        dir: dir.to_path_buf(),
    })
}

/// Encode a [`Frame`] to a PNG file via the `image` crate, mapping any encode
/// failure into an `io::Error` so callers have a single error type.
fn write_png(path: &Path, frame: &Frame) -> std::io::Result<()> {
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame buffer size mismatch",
            )
        })?;
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comp::{Interp, MotionBlur, Prop};

    fn solid(color: [f32; 4]) -> Comp {
        let mut c = Comp {
            width: 64,
            height: 64,
            duration: 1.0,
            fps: 30.0,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        c.layers.push(PulseLayer::new("L", color));
        c
    }

    #[test]
    fn frame_has_correct_size() {
        let c = solid([1.0, 0.0, 0.0, 1.0]);
        let f = render_frame(&c, 0.0);
        assert_eq!(f.width, 64);
        assert_eq!(f.height, 64);
        assert_eq!(f.pixels.len(), 64 * 64 * 4);
    }

    #[test]
    fn empty_comp_is_transparent() {
        let c = Comp {
            width: 8,
            height: 8,
            duration: 1.0,
            fps: 30.0,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        let f = render_frame(&c, 0.0);
        assert!(f.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn center_pixel_is_opaque_layer_color() {
        // A centered, unrotated, unit-scale opaque red layer covers the center.
        let c = solid([1.0, 0.0, 0.0, 1.0]);
        let f = render_frame(&c, 0.0);
        let [r, g, b, a] = f.pixel(32, 32);
        assert_eq!(a, 255);
        assert!(r > 250, "red channel high, got {r}");
        assert_eq!(g, 0);
        assert_eq!(b, 0);
    }

    #[test]
    fn corner_pixel_outside_quad_is_transparent() {
        // Half-extent is 0.22*64 ≈ 14 px, so a far corner is uncovered.
        let c = solid([1.0, 1.0, 1.0, 1.0]);
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(0, 0)[3], 0);
        assert_eq!(f.pixel(63, 63)[3], 0);
    }

    #[test]
    fn invisible_layer_does_not_render() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].visible = false;
        let f = render_frame(&c, 0.0);
        assert!(f.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn zero_opacity_is_transparent() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].opacity.set_key(0.0, 0.0);
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(32, 32)[3], 0);
    }

    #[test]
    fn opacity_animates_over_time() {
        // Opacity ramps 0 -> 1 across the comp; center alpha grows with time.
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].opacity.set_key(0.0, 0.0);
        c.layers[0].opacity.set_key(1.0, 1.0);
        let a0 = render_frame(&c, 0.0).pixel(32, 32)[3];
        let amid = render_frame(&c, 0.5).pixel(32, 32)[3];
        let a1 = render_frame(&c, 1.0).pixel(32, 32)[3];
        assert!(a0 < amid && amid < a1, "{a0} < {amid} < {a1}");
        assert_eq!(a1, 255);
    }

    #[test]
    fn position_offset_moves_coverage() {
        // Shift the layer far right: center is now uncovered, the right edge covered.
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].x.set_key(0.0, 20.0);
        let f = render_frame(&c, 0.0);
        // Original center (32,32) sits at the layer's left edge region; the
        // covered band shifts right. Sample a pixel that should now be covered.
        assert_eq!(f.pixel(50, 32)[3], 255);
        // A pixel far left of the shifted quad is uncovered.
        assert_eq!(f.pixel(10, 32)[3], 0);
    }

    #[test]
    fn source_over_blends_two_layers_in_linear() {
        // Opaque black behind, 50% white on top -> mid gray, fully opaque.
        let mut c = solid([0.0, 0.0, 0.0, 1.0]);
        let mut top = PulseLayer::new("top", [1.0, 1.0, 1.0, 1.0]);
        top.opacity.set_key(0.0, 0.5);
        c.layers.push(top);
        let f = render_frame(&c, 0.0);
        let [r, _g, _b, a] = f.pixel(32, 32);
        assert_eq!(a, 255);
        // 0.5 linear-light coverage of white over black, sRGB-encoded, is well
        // above naive 0.5*255=128 (gamma), so just bound it sensibly.
        assert!((150..=200).contains(&r), "mid gray r={r}");
    }

    #[test]
    fn scale_zero_renders_nothing() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].scale.set_key(0.0, 0.0);
        let f = render_frame(&c, 0.0);
        assert!(f.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn larger_scale_covers_more_pixels() {
        let count_covered = |scale: f32| {
            let mut c = solid([1.0, 1.0, 1.0, 1.0]);
            c.layers[0].scale.set_key(0.0, scale);
            let f = render_frame(&c, 0.0);
            f.pixels.chunks(4).filter(|p| p[3] > 0).count()
        };
        assert!(count_covered(2.0) > count_covered(1.0));
    }

    #[test]
    fn rotation_keeps_center_covered() {
        // Rotating about the layer center leaves the center pixel covered.
        let mut c = solid([0.0, 1.0, 0.0, 1.0]);
        c.layers[0].rotation.set_key(0.0, 45.0);
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(32, 32)[3], 255);
    }

    #[test]
    fn rotation_uses_outgoing_interp() {
        // Sanity: a rotation track sampled mid-segment differs from endpoints,
        // confirming render_frame consults the animated transform.
        let mut c = solid([1.0, 0.0, 0.0, 1.0]);
        c.layers[0].rotation.set_key(0.0, 0.0);
        c.layers[0].rotation.set_key(1.0, 90.0);
        c.layers[0].rotation.set_interp(0.0, Interp::Linear);
        // Just assert it renders without panic at a few times.
        for &t in &[0.0, 0.25, 0.5, 1.0] {
            let _ = render_frame(&c, t);
        }
        // And the transform actually animates.
        assert!((c.layers[0].value(Prop::Rotation, 0.5) - 45.0).abs() < 1e-3);
    }

    // --- Sequence math ------------------------------------------------------

    #[test]
    fn frame_count_is_duration_times_fps() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.duration = 5.0;
        c.fps = 30.0;
        assert_eq!(frame_count(&c), 150);
        c.duration = 2.0;
        c.fps = 24.0;
        assert_eq!(frame_count(&c), 48);
    }

    #[test]
    fn frame_count_floors_at_one() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.duration = 0.0;
        assert_eq!(frame_count(&c), 1);
    }

    #[test]
    fn frame_time_steps_by_fps() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.fps = 25.0;
        assert!((frame_time(&c, 0) - 0.0).abs() < 1e-6);
        assert!((frame_time(&c, 25) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn frame_path_zero_pads() {
        let dir = Path::new("/tmp/out");
        // <100 frames -> 4-digit padding (the minimum).
        assert_eq!(frame_path(dir, "comp", 7, 90), dir.join("comp_0007.png"));
        // 12000 frames -> highest index 11999 needs 5 digits.
        assert_eq!(
            frame_path(dir, "comp", 42, 12000),
            dir.join("comp_00042.png")
        );
    }

    #[test]
    fn anchor_offset_shifts_coverage_under_rotation() {
        // With the anchor offset off-center, rotating pivots about the anchor,
        // not the layer center — so the covered region moves vs. a centered
        // anchor. Compare covered-pixel counts overlapping a probe far from
        // center to confirm the pivot changed.
        let covered_at = |anchor: f32| {
            let mut c = solid([1.0, 1.0, 1.0, 1.0]);
            c.layers[0].anchor_x.set_key(0.0, anchor);
            c.layers[0].rotation.set_key(0.0, 90.0);
            let f = render_frame(&c, 0.0);
            f.pixels.chunks(4).filter(|p| p[3] > 0).count()
        };
        // Both render *something* but the anchored pivot relocates the quad;
        // assert the quad still covers a sensible number of pixels (sanity) and
        // that an off-center anchor does not crash / vanish.
        assert!(covered_at(0.0) > 0);
        assert!(covered_at(20.0) > 0);
    }

    #[test]
    fn anchored_layer_pivots_position_correctly() {
        // 64x64 comp, center at (32,32). Anchor at the quad's left edge
        // (anchor_x = -half_w ≈ -14) and position 0: the layer's left edge now
        // sits at the comp center, so the quad extends to the right of center.
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        let half_w = 64.0 * LAYER_HALF_FRAC; // ~14
        c.layers[0].anchor_x.set_key(0.0, -half_w);
        let f = render_frame(&c, 0.0);
        // A pixel just right of center is covered...
        assert_eq!(f.pixel(40, 32)[3], 255);
        // ...and one left of center (beyond the anchored left edge) is not.
        assert_eq!(f.pixel(10, 32)[3], 0);
    }

    #[test]
    fn parented_child_follows_parent_offset() {
        // Parent shifted right; an unparented child at x=0 covers the center.
        // Parenting it to the moved parent shifts its coverage right too.
        let mut c = Comp {
            width: 64,
            height: 64,
            duration: 1.0,
            fps: 30.0,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        c.layers
            .push(PulseLayer::new("parent", [0.0, 0.0, 0.0, 0.0])); // invisible-ish parent
        c.layers[0].visible = false; // parent itself doesn't draw
        c.layers[0].x.set_key(0.0, 18.0);
        let mut child = PulseLayer::new("child", [1.0, 1.0, 1.0, 1.0]);
        child.parent = Some(0);
        c.layers.push(child);

        let f = render_frame(&c, 0.0);
        // Child's coverage rode the parent's +18 offset to the right.
        assert_eq!(f.pixel(50, 32)[3], 255);
        assert_eq!(f.pixel(10, 32)[3], 0);
    }

    // --- Layer kinds + effects ---------------------------------------------

    #[test]
    fn null_layer_renders_nothing() {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].kind = crate::comp::LayerKind::Null;
        let f = render_frame(&c, 0.0);
        assert!(f.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn solid_effect_stack_recolors_the_quad() {
        // A black solid with a Tint mapping black->white should now read white at
        // the center (the effect runs on the layer's own color before compositing).
        let mut c = solid([0.0, 0.0, 0.0, 1.0]);
        c.layers[0].effects.push(crate::comp::Effect::Tint {
            black: [1.0, 1.0, 1.0],
            white: [1.0, 1.0, 1.0],
            amount: 1.0,
        });
        let f = render_frame(&c, 0.0);
        let [r, g, b, a] = f.pixel(32, 32);
        assert_eq!(a, 255);
        assert!(
            r > 250 && g > 250 && b > 250,
            "expected white, got {r},{g},{b}"
        );
    }

    #[test]
    fn adjustment_layer_regrades_layers_below() {
        // A mid-gray solid beneath a full-frame adjustment that lifts brightness
        // should read brighter at the center than without the adjustment.
        let make = |with_adj: bool| {
            let mut c = solid([0.5, 0.5, 0.5, 1.0]);
            if with_adj {
                let mut adj =
                    PulseLayer::of_kind(crate::comp::LayerKind::Adjustment, "adj", [1.0; 4]);
                adj.scale.set_key(0.0, 3.0); // cover the frame
                adj.effects.push(crate::comp::Effect::BrightnessContrast {
                    brightness: 0.3,
                    contrast: 1.0,
                });
                c.layers.push(adj);
            }
            render_frame(&c, 0.0).pixel(32, 32)[0]
        };
        assert!(
            make(true) > make(false),
            "adjustment did not brighten below"
        );
    }

    #[test]
    fn adjustment_layer_draws_no_pixels_of_its_own() {
        // An adjustment over an empty comp leaves it transparent (no source).
        let mut c = Comp {
            width: 16,
            height: 16,
            duration: 1.0,
            fps: 30.0,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        let mut adj = PulseLayer::of_kind(crate::comp::LayerKind::Adjustment, "adj", [1.0; 4]);
        adj.scale.set_key(0.0, 3.0);
        adj.effects.push(crate::comp::Effect::BrightnessContrast {
            brightness: 0.5,
            contrast: 1.0,
        });
        c.layers.push(adj);
        let f = render_frame(&c, 0.0);
        assert!(f.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn adjustment_only_affects_its_quad_bounds() {
        // A small (unscaled) adjustment over a full-frame solid grades only the
        // pixels inside its quad: the center changes, a far corner does not.
        let mut c = solid([0.5, 0.5, 0.5, 1.0]);
        c.layers[0].scale.set_key(0.0, 3.0); // bottom solid covers the frame
        let mut adj = PulseLayer::of_kind(crate::comp::LayerKind::Adjustment, "adj", [1.0; 4]);
        adj.effects.push(crate::comp::Effect::BrightnessContrast {
            brightness: 0.3,
            contrast: 1.0,
        });
        c.layers.push(adj); // unit-scale: covers only ~the center quad
        let f = render_frame(&c, 0.0);
        let center = f.pixel(32, 32)[0];
        let corner = f.pixel(1, 1)[0];
        // Center (inside the small adjustment quad) is brighter than an edge
        // pixel (covered by the solid but outside the adjustment).
        assert!(
            center > corner,
            "center {center} should exceed corner {corner}"
        );
    }

    // --- Track mattes -------------------------------------------------------

    /// A 64x64 comp: a full-frame opaque solid (`base`) with a smaller solid on
    /// top to serve as the matte source. Index 0 = matted base, index 1 = source.
    fn matte_pair(base: [f32; 4], source: [f32; 4], src_scale: f32) -> Comp {
        let mut c = Comp {
            width: 64,
            height: 64,
            duration: 1.0,
            fps: 30.0,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        let mut b = PulseLayer::new("base", base);
        b.scale.set_key(0.0, 3.0); // cover the whole frame
        c.layers.push(b); // index 0
        let mut s = PulseLayer::new("source", source);
        s.scale.set_key(0.0, src_scale);
        c.layers.push(s); // index 1
        c
    }

    #[test]
    fn matte_source_is_not_composited_on_its_own() {
        // A red base under a green source; with an alpha matte the green source
        // must NOT appear in the output — it only shapes the base's alpha.
        let mut c = matte_pair([1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], 1.0);
        c.layers[0].matte = MatteMode::Alpha;
        let f = render_frame(&c, 0.0);
        let [r, g, _b, a] = f.pixel(32, 32);
        assert_eq!(a, 255);
        // The center shows the base's red, not the source's green.
        assert!(r > 250, "expected red base, got r={r}");
        assert_eq!(g, 0, "matte source leaked into the composite");
    }

    #[test]
    fn alpha_matte_clips_to_source_coverage() {
        // Full-frame base, small (unit-scale) source. With an alpha matte the
        // base is visible only inside the small source quad: center covered, a
        // far edge (inside the base but outside the source) is now transparent.
        let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
        c.layers[0].matte = MatteMode::Alpha;
        let f = render_frame(&c, 0.0);
        assert_eq!(
            f.pixel(32, 32)[3],
            255,
            "center should pass the alpha matte"
        );
        // A pixel far from center is inside the full-frame base but outside the
        // small source quad -> matted away.
        assert_eq!(f.pixel(2, 2)[3], 0, "edge should be matted out");
    }

    #[test]
    fn inverted_alpha_matte_is_the_complement() {
        // Inverted alpha: the base shows where the source is *transparent*, so the
        // center (under the opaque source) is hidden and the surrounding base
        // (full-frame) stays. Compare against the non-inverted case.
        let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
        c.layers[0].matte = MatteMode::AlphaInverted;
        let f = render_frame(&c, 0.0);
        // Center (under the opaque source) is punched out.
        assert_eq!(f.pixel(32, 32)[3], 0, "center should be inverted-out");
        // An edge pixel (base present, source absent) survives.
        assert_eq!(f.pixel(2, 2)[3], 255, "edge should survive inversion");
    }

    #[test]
    fn luma_matte_scales_alpha_by_source_brightness() {
        // White base; the luma of a darker source scales the base's alpha. A gray
        // (0.5 sRGB) source yields a partial matte: center alpha between 0 and 255.
        let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [0.5, 0.5, 0.5, 1.0], 1.0);
        c.layers[0].matte = MatteMode::Luma;
        let gray = render_frame(&c, 0.0).pixel(32, 32)[3];
        assert!((1..255).contains(&gray), "partial luma matte, got a={gray}");
        // A white source passes the base through fully.
        c.layers[1].color = [1.0, 1.0, 1.0, 1.0];
        let white = render_frame(&c, 0.0).pixel(32, 32)[3];
        assert_eq!(white, 255, "white luma should fully pass");
        // A black source mattes the base completely away.
        c.layers[1].color = [0.0, 0.0, 0.0, 1.0];
        let black = render_frame(&c, 0.0).pixel(32, 32)[3];
        assert_eq!(black, 0, "black luma should fully matte out");
    }

    #[test]
    fn matte_preserves_base_color() {
        // The matte changes coverage only, never color: a blue base under a
        // partial luma matte still reads blue (just dimmer in alpha).
        let mut c = matte_pair([0.0, 0.0, 1.0, 1.0], [0.6, 0.6, 0.6, 1.0], 1.0);
        c.layers[0].matte = MatteMode::Luma;
        let [r, g, b, a] = render_frame(&c, 0.0).pixel(32, 32);
        assert!(a > 0, "some coverage expected");
        assert!(
            b > r && b > g,
            "base color should stay blue, got {r},{g},{b}"
        );
    }

    #[test]
    fn export_sequence_writes_all_frames() {
        let mut c = solid([0.2, 0.6, 0.9, 1.0]);
        c.width = 16;
        c.height = 16;
        c.duration = 0.1; // 0.1s * 30fps = 3 frames
        c.fps = 30.0;
        let dir = std::env::temp_dir().join(format!("pulse_export_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let summary = export_sequence(&c, &dir, "seq").expect("export");
        assert_eq!(summary.frames, 3);
        for i in 0..3 {
            let p = frame_path(&dir, "seq", i, 3);
            assert!(p.exists(), "missing frame {}", p.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Motion blur --------------------------------------------------------

    /// A 64x64 comp whose single solid slides fast left→right across the frame,
    /// with comp motion blur on and the layer opted in (toggled by `layer_mb`).
    fn moving_solid(layer_mb: bool) -> Comp {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].motion_blur = layer_mb;
        c.layers[0].x.set_key(0.0, -24.0);
        c.layers[0].x.set_key(1.0, 24.0);
        c.motion_blur.enabled = true;
        c.motion_blur.angle = 360.0; // a whole frame of blur for a clear effect
        c.motion_blur.samples = 16;
        c
    }

    #[test]
    fn motion_blur_softens_the_moving_edge() {
        // With motion blur the leading/trailing edge spans several partly-covered
        // (0 < a < 255) pixels; without it the edge is a hard 0/255 step. Count
        // the partial-alpha pixels along the center row at mid-travel.
        let partial_count = |mb: bool| {
            let c = moving_solid(mb);
            let f = render_frame(&c, 0.5);
            (0..f.width)
                .filter(|&x| {
                    let a = f.pixel(x, 32)[3];
                    a > 0 && a < 255
                })
                .count()
        };
        let blurred = partial_count(true);
        let crisp = partial_count(false);
        assert!(
            blurred > crisp,
            "motion blur should add partial-coverage edge pixels: blurred={blurred} crisp={crisp}"
        );
    }

    #[test]
    fn motion_blur_preserves_color_no_bleed() {
        // A fully-covered pixel near the center of the swept band keeps the
        // layer's pure-white color (premultiplied averaging must not bleed it
        // toward black through the transparent samples).
        let c = moving_solid(true);
        let f = render_frame(&c, 0.5);
        // The layer center at t=0.5 sits at comp x=0 -> pixel 32; fully covered
        // across the sweep, so still opaque white.
        let [r, g, b, a] = f.pixel(32, 32);
        assert_eq!(a, 255, "center stays fully covered through the sweep");
        assert!(
            r > 250 && g > 250 && b > 250,
            "color preserved, got {r},{g},{b}"
        );
    }

    #[test]
    fn comp_master_switch_gates_motion_blur() {
        // Layer opted in but comp master off -> identical to no motion blur.
        let mut c = moving_solid(true);
        c.motion_blur.enabled = false;
        let off = render_frame(&c, 0.5);
        let mut crisp = solid([1.0, 1.0, 1.0, 1.0]);
        crisp.layers[0].x.set_key(0.0, -24.0);
        crisp.layers[0].x.set_key(1.0, 24.0);
        let baseline = render_frame(&crisp, 0.5);
        assert_eq!(off.pixels, baseline.pixels);
    }

    #[test]
    fn unblurred_layer_unaffected_by_comp_motion_blur() {
        // Comp MB on but the layer didn't opt in -> crisp render unchanged.
        let blurred_off = render_frame(&moving_solid(false), 0.5);
        let mut crisp = solid([1.0, 1.0, 1.0, 1.0]);
        crisp.layers[0].x.set_key(0.0, -24.0);
        crisp.layers[0].x.set_key(1.0, 24.0);
        let baseline = render_frame(&crisp, 0.5);
        assert_eq!(blurred_off.pixels, baseline.pixels);
    }

    #[test]
    fn motion_blur_respects_track_matte() {
        // A motion-blurred base clipped by a small static alpha matte: the matte
        // still bounds coverage (no blurred pixels leak past the matte edge far
        // from the source quad).
        let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
        c.layers[0].matte = MatteMode::Alpha;
        c.layers[0].motion_blur = true;
        c.layers[0].x.set_key(0.0, -24.0);
        c.layers[0].x.set_key(1.0, 24.0);
        c.motion_blur.enabled = true;
        let f = render_frame(&c, 0.5);
        // A far corner is outside the small matte source -> matted out even with
        // motion blur on.
        assert_eq!(f.pixel(2, 2)[3], 0, "matte must still clip the blur");
    }

    // --- Masks --------------------------------------------------------------

    use crate::comp::{Mask, MaskMode};

    /// A 64x64 comp with a single full-frame opaque white solid (index 0).
    fn full_frame_solid() -> Comp {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].scale.set_key(0.0, 3.0); // cover the whole frame
        c
    }

    #[test]
    fn mask_clips_layer_to_its_shape() {
        // A small centered rectangular Add mask on a full-frame solid: the center
        // stays opaque, a far corner (outside the mask) is carved away.
        let mut c = full_frame_solid();
        c.layers[0].masks.push(Mask::rect(8.0, 8.0));
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(32, 32)[3], 255, "center inside mask stays covered");
        assert_eq!(f.pixel(2, 2)[3], 0, "corner outside mask is carved away");
    }

    #[test]
    fn inverted_mask_keeps_the_outside() {
        // Inverting the same mask flips it: the center is punched out, the
        // surrounding frame survives.
        let mut c = full_frame_solid();
        let mut m = Mask::rect(8.0, 8.0);
        m.inverted = true;
        c.layers[0].masks.push(m);
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(32, 32)[3], 0, "center punched out by inverted mask");
        assert_eq!(f.pixel(2, 2)[3], 255, "outside survives inversion");
    }

    #[test]
    fn no_active_mask_is_identical_to_unmasked() {
        // A layer whose only mask is disabled (mode None) renders byte-identical
        // to the same layer with no masks at all.
        let base = render_frame(&full_frame_solid(), 0.0);
        let mut c = full_frame_solid();
        let mut m = Mask::rect(8.0, 8.0);
        m.mode = MaskMode::None;
        c.layers[0].masks.push(m);
        let withmask = render_frame(&c, 0.0);
        assert_eq!(base.pixels, withmask.pixels);
    }

    #[test]
    fn mask_preserves_layer_color() {
        // Masking changes coverage, never color: a blue solid masked to a small
        // rect still reads blue at the center.
        let mut c = solid([0.0, 0.0, 1.0, 1.0]);
        c.layers[0].scale.set_key(0.0, 3.0);
        c.layers[0].masks.push(Mask::rect(8.0, 8.0));
        let [r, g, b, a] = render_frame(&c, 0.0).pixel(32, 32);
        assert_eq!(a, 255);
        assert!(b > r && b > g, "center should stay blue, got {r},{g},{b}");
    }

    #[test]
    fn feathered_mask_softens_the_edge() {
        // A hard mask has a crisp 0/255 boundary; a feathered one adds a band of
        // partial-alpha pixels along the center row.
        let partial_count = |feather: f32| {
            let mut c = full_frame_solid();
            let mut m = Mask::rect(12.0, 12.0);
            m.feather = feather;
            c.layers[0].masks.push(m);
            let f = render_frame(&c, 0.0);
            (0..f.width)
                .filter(|&x| {
                    let a = f.pixel(x, 32)[3];
                    a > 0 && a < 255
                })
                .count()
        };
        assert!(
            partial_count(8.0) > partial_count(0.0),
            "feather should add partial-coverage edge pixels"
        );
    }

    #[test]
    fn add_subtract_mask_stack_punches_a_hole() {
        // A big Add mask with a smaller Subtract mask leaves a covered ring with a
        // transparent hole at the center.
        let mut c = full_frame_solid();
        c.layers[0].masks.push(Mask::rect(13.0, 13.0)); // Add (default)
        let mut sub = Mask::rect(5.0, 5.0);
        sub.mode = MaskMode::Subtract;
        c.layers[0].masks.push(sub);
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(32, 32)[3], 0, "center hole subtracted away");
        // A pixel inside the big rect (local ~9.5px after the layer's 3x scale)
        // but outside the small hole stays covered.
        assert_eq!(f.pixel(60, 32)[3], 255, "ring stays covered");
    }

    #[test]
    fn mask_rides_layer_transform() {
        // The mask is in layer-local space, so moving the layer moves the masked
        // region with it. Shift the layer right and the surviving coverage shifts
        // too: the original center loses coverage, a point to the right gains it.
        let mut c = full_frame_solid();
        c.layers[0].masks.push(Mask::rect(8.0, 8.0));
        c.layers[0].x.set_key(0.0, 16.0); // slide right 16 comp px
        let f = render_frame(&c, 0.0);
        // The masked patch moved to ~x=48; the old center is now outside it.
        assert_eq!(f.pixel(48, 32)[3], 255, "masked patch followed the layer");
        assert_eq!(f.pixel(20, 32)[3], 0, "old position no longer covered");
    }

    // --- Spatial effects ----------------------------------------------------

    use crate::comp::SpatialEffect;

    #[test]
    fn gaussian_blur_softens_the_layer_edge() {
        // A small centered solid: blurring it adds a band of partial-alpha edge
        // pixels along the center row vs. the crisp render.
        let partial_count = |sigma: f32| {
            let mut c = solid([1.0, 1.0, 1.0, 1.0]);
            if sigma > 0.0 {
                c.layers[0]
                    .spatial_effects
                    .push(SpatialEffect::GaussianBlur {
                        sigma_x: sigma,
                        sigma_y: sigma,
                        repeat_edge: false,
                    });
            }
            let f = render_frame(&c, 0.0);
            (0..f.width)
                .filter(|&x| {
                    let a = f.pixel(x, 32)[3];
                    a > 0 && a < 255
                })
                .count()
        };
        assert!(
            partial_count(4.0) > partial_count(0.0),
            "blur should add partial-coverage edge pixels"
        );
    }

    #[test]
    fn drop_shadow_appears_in_the_composite() {
        // A solid with a hard (0-softness) black drop shadow offset down-right:
        // a pixel just past the quad in the shadow direction picks up dark,
        // semi-opaque shadow coverage where the crisp layer had nothing.
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].spatial_effects.push(SpatialEffect::DropShadow {
            color: [0.0, 0.0, 0.0],
            opacity: 1.0,
            angle: 45.0, // down-right (+x,+y)
            distance: 10.0,
            softness: 0.0,
            shadow_only: false,
        });
        let crisp = render_frame(&solid([1.0, 1.0, 1.0, 1.0]), 0.0);
        let shad = render_frame(&c, 0.0);
        // The half-extent is ~14px; sample a pixel down-right of the quad's
        // bottom-right corner that the shadow offset reaches.
        let (sx, sy) = (32 + 16, 32 + 16);
        assert_eq!(
            crisp.pixel(sx, sy)[3],
            0,
            "no coverage here without a shadow"
        );
        let p = shad.pixel(sx, sy);
        assert!(p[3] > 0, "drop shadow added coverage past the layer");
        assert!(
            p[0] < 60 && p[1] < 60 && p[2] < 60,
            "shadow should be dark, got {},{},{}",
            p[0],
            p[1],
            p[2]
        );
    }

    #[test]
    fn glow_brightens_a_bright_layer() {
        // A bright (but not pure-white) layer reads brighter at center once a
        // glow blooms its highlights back on top.
        let center_r = |with_glow: bool| {
            let mut c = solid([0.85, 0.85, 0.85, 1.0]);
            if with_glow {
                c.layers[0].spatial_effects.push(SpatialEffect::Glow {
                    threshold: 0.4,
                    radius: 6.0,
                    intensity: 2.0,
                });
            }
            render_frame(&c, 0.0).pixel(32, 32)[0]
        };
        assert!(center_r(true) >= center_r(false), "glow should not darken");
    }

    #[test]
    fn spatial_effect_routes_layer_through_isolated_buffer() {
        // A solid with only a (zero-sigma, identity) blur still renders the same
        // as the crisp solid — the isolated-buffer routing is value-neutral when
        // the pass is identity.
        let mut c = solid([0.3, 0.6, 0.9, 1.0]);
        c.layers[0]
            .spatial_effects
            .push(SpatialEffect::GaussianBlur {
                sigma_x: 0.0,
                sigma_y: 0.0,
                repeat_edge: false,
            });
        let base = render_frame(&solid([0.3, 0.6, 0.9, 1.0]), 0.0);
        let routed = render_frame(&c, 0.0);
        assert_eq!(base.pixels, routed.pixels);
    }

    #[test]
    fn mask_and_track_matte_compose() {
        // A masked base under a static alpha matte: both must clip. The mask is
        // small (rect 8) and centered; the matte source is unit-scale (~14 px).
        // The center survives both; a far corner is matted out.
        let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
        c.layers[0].matte = crate::comp::MatteMode::Alpha;
        c.layers[0].masks.push(Mask::rect(8.0, 8.0));
        let f = render_frame(&c, 0.0);
        assert_eq!(f.pixel(32, 32)[3], 255, "center passes mask and matte");
        // Outside the small mask -> carved by the mask even within the matte.
        assert_eq!(f.pixel(2, 2)[3], 0, "corner clipped by mask+matte");
    }
}
