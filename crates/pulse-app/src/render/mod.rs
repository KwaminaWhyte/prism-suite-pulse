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
use crate::comp::{Affine2, Comp, LayerKind};
use prism_core::color::linear_to_srgb;

mod export;
mod passes;

pub use export::export_sequence;
use passes::{
    apply_adjustment, apply_masks, apply_spatial, apply_track_matte, composite_layer,
    composite_motion_blur,
};

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

/// Encode a linear-light component to an 8-bit sRGB byte.
fn enc(v: f32) -> u8 {
    (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8
}

#[cfg(test)]
mod tests;
