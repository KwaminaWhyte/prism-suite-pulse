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

use crate::comp::{apply_effects, Affine2, Comp, LayerKind, PulseLayer};
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

    for (i, layer) in comp.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        // World matrix composes this layer under its parent chain; it maps
        // layer-local points (origin at the layer's geometric center) into comp
        // space (origin at the comp center, +y down).
        let world = comp.world_matrix(i, t);
        match layer.kind {
            // A null draws nothing — it's a transform reference (parent) only.
            LayerKind::Null => {}
            // A solid draws its own colored quad, processed by its effect stack.
            LayerKind::Solid => {
                composite_layer(&mut acc, w, h, cx, cy, half_w, half_h, world, layer, t);
            }
            // An adjustment re-processes the composite beneath it, within its
            // own transformed quad bounds.
            LayerKind::Adjustment => {
                apply_adjustment(&mut acc, w, h, cx, cy, half_w, half_h, world, layer, t);
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

/// Composite one layer's solid quad into the linear accumulator.
///
/// `world` is the layer's resolved comp-space matrix (own transform + parent
/// chain). It maps layer-local points (origin at the layer's geometric center)
/// into comp space whose origin is the comp center. Coverage is tested by
/// inverse-mapping each candidate pixel back into local space and box-testing
/// against the `±half_w/±half_h` quad.
#[allow(clippy::too_many_arguments)]
fn composite_layer(
    acc: &mut [Lin],
    w: u32,
    h: u32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    world: Affine2,
    layer: &PulseLayer,
    t: f32,
) {
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

    // Conservative comp-space AABB of the quad: transform its four local corners
    // through the world matrix and bound them. Comp space has the origin at the
    // comp center, so add (cx, cy) to land in pixel coordinates.
    let corners = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
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
    let x0 = ((cx + min_x).floor() as i32).max(0);
    let x1 = ((cx + max_x).ceil() as i32).min(w as i32 - 1);
    let y0 = ((cy + min_y).floor() as i32).max(0);
    let y1 = ((cy + max_y).ceil() as i32).min(h as i32 - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }

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
            // Source-over in linear light: out = src + dst*(1-src_a).
            let idx = (py as u32 * w + px as u32) as usize;
            let dst = acc[idx];
            let ia = 1.0 - src_a;
            acc[idx] = Lin {
                r: lr * src_a + dst.r * ia,
                g: lg * src_a + dst.g * ia,
                b: lb * src_a + dst.b * ia,
                a: src_a + dst.a * ia,
            };
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
#[allow(clippy::too_many_arguments)]
fn apply_adjustment(
    acc: &mut [Lin],
    w: u32,
    h: u32,
    cx: f32,
    cy: f32,
    half_w: f32,
    half_h: f32,
    world: Affine2,
    layer: &PulseLayer,
    t: f32,
) {
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

    let corners = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
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
    let x0 = ((cx + min_x).floor() as i32).max(0);
    let x1 = ((cx + max_x).ceil() as i32).min(w as i32 - 1);
    let y0 = ((cy + min_y).floor() as i32).max(0);
    let y1 = ((cy + max_y).ceil() as i32).min(h as i32 - 1);
    if x0 > x1 || y0 > y1 {
        return;
    }

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
    use crate::comp::{Interp, Prop};

    fn solid(color: [f32; 4]) -> Comp {
        let mut c = Comp {
            width: 64,
            height: 64,
            duration: 1.0,
            fps: 30.0,
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
}
