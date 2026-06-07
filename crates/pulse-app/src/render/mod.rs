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
use crate::comp::{blend_over, Affine2, BlendMode, BlendRgba, Comp, FrameCache, LayerKind};
use prism_core::color::linear_to_srgb;

mod export;
mod passes;

pub use export::export_sequence_in_project;
#[cfg(test)]
pub use export::export_sequence;
use passes::{
    apply_adjustment, apply_masks, apply_spatial, apply_track_matte, composite_footage,
    composite_layer, composite_motion_blur, composite_precomp, composite_shape, composite_text,
};

/// Per-render context for resolving **precomps**: the project's comps (so a
/// precomp layer can find the comp it references by id) and the visited-set of
/// comp ids currently on the render stack (so a reference cycle A → B → A is
/// detected and broken rather than recursing forever).
///
/// Copied (cheaply — it borrows the comp slice and the visited stack) into each
/// nested render call; pushing a comp id before recursing and the borrow ending
/// after keeps the visited set scoped to the active recursion path.
#[derive(Clone, Copy)]
pub(crate) struct RenderCtx<'a> {
    /// All comps in the project, addressed by [`Comp::id`].
    comps: &'a [Comp],
    /// Comp ids currently being rendered (the recursion stack), for cycle
    /// detection.
    visited: &'a [u64],
}

impl<'a> RenderCtx<'a> {
    /// A context over a single comp (no project) — the legacy/test universe where
    /// the only renderable comp is the one passed to [`render_frame`]. A precomp
    /// in such a comp can only resolve if the lone comp's id matches (which the
    /// cycle guard then refuses), so precomps render nothing here.
    fn lone(comp: &'a [Comp]) -> Self {
        Self {
            comps: comp,
            visited: &[],
        }
    }

    /// Look up a comp by id, unless rendering it would close a cycle (it is
    /// already on the render stack). Returns `None` (render nothing) for a
    /// missing target or a cyclic one.
    fn resolve(&self, id: u64) -> Option<&'a Comp> {
        if self.visited.contains(&id) {
            return None; // cycle guard: refuse to re-enter a comp on the stack
        }
        self.comps.iter().find(|c| c.id == id)
    }
}

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
    /// (A pixel accessor for callers/tests inspecting a rendered frame, and for
    /// the precomp pass sampling a nested comp's rendered frame.)
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

/// Composite straight linear-light `src` onto straight `dst` using `mode`'s
/// **blend mode**. A thin bridge over [`blend_over`] that maps the renderer's
/// [`Lin`] accumulator pixel to the blend math's [`BlendRgba`] and back.
/// [`BlendMode::Normal`] reduces exactly to [`over`], so an un-blended layer is
/// bit-identical to the prior behavior.
fn blend_lin(mode: BlendMode, src: Lin, dst: Lin) -> Lin {
    let out = blend_over(
        mode,
        BlendRgba {
            r: src.r,
            g: src.g,
            b: src.b,
            a: src.a,
        },
        BlendRgba {
            r: dst.r,
            g: dst.g,
            b: dst.b,
            a: dst.a,
        },
    );
    Lin {
        r: out.r,
        g: out.g,
        b: out.b,
        a: out.a,
    }
}

/// Render the composition at time `t` to a native-resolution [`Frame`], decoding
/// footage through a throwaway [`FrameCache`].
///
/// A convenience wrapper over [`render_frame_cached`] for callers (tests, one-off
/// renders) that don't keep a persistent footage cache. Interactive callers
/// (the export loop) should reuse a cache via [`render_frame_cached`] so a
/// sequence isn't re-decoded for every comp frame.
#[cfg_attr(not(test), allow(dead_code))]
pub fn render_frame(comp: &Comp, t: f32) -> Frame {
    let mut cache = FrameCache::new();
    render_frame_cached(comp, t, &mut cache)
}

/// Render the composition at time `t` to a native-resolution [`Frame`], decoding
/// footage layers through the supplied [`FrameCache`] (so repeated source frames
/// across comp frames / motion-blur samples decode at most once).
///
/// The comp backdrop is fully transparent black; visible layers are composited
/// back-to-front (index 0 first / behind). Coordinates follow the preview:
/// the comp origin is its center, `+y` is downward (screen space).
///
/// This single-comp entry treats `comp` as the only renderable comp, so any
/// **precomp** layer it contains has nothing to resolve (it draws nothing). Use
/// [`render_frame_in_project`] to render a comp whose precomps reference sibling
/// comps.
pub fn render_frame_cached(comp: &Comp, t: f32, cache: &mut FrameCache) -> Frame {
    let comps = std::slice::from_ref(comp);
    render_comp(comp, t, cache, RenderCtx::lone(comps))
}

/// Render comp `id` (within `comps`) at time `t`, resolving any **precomp**
/// layers against its sibling comps and breaking reference cycles.
///
/// The project-aware entry: a precomp layer in the rendered comp (or, recursively,
/// in any comp it nests) resolves its target through `comps` and is rendered into
/// its quad, with a visited-set guard so a cycle A → B → A terminates (the cyclic
/// precomp renders nothing). Returns an empty/transparent frame if `id` is not
/// found.
#[cfg_attr(not(test), allow(dead_code))]
pub fn render_frame_in_project(comps: &[Comp], id: u64, t: f32, cache: &mut FrameCache) -> Frame {
    let Some(comp) = comps.iter().find(|c| c.id == id) else {
        return Frame {
            width: 1,
            height: 1,
            pixels: vec![0; 4],
        };
    };
    render_comp(comp, t, cache, RenderCtx::lone(comps))
}

/// Core compositor: render `comp` at time `t` under render context `ctx`.
///
/// `ctx` carries the project's comps and the visited-set of comp ids on the
/// render stack; `comp`'s own id is pushed onto that stack before its precomp
/// layers recurse, so a precomp that points back at an ancestor comp is detected
/// and skipped.
pub(crate) fn render_comp(comp: &Comp, t: f32, cache: &mut FrameCache, ctx: RenderCtx) -> Frame {
    // Push this comp's id onto the recursion stack so nested precomps can detect
    // a cycle back to it. (A zero/duplicate id is harmless — it only ever causes
    // the guard to skip rendering, never to recurse.)
    let mut stack = ctx.visited.to_vec();
    stack.push(comp.id);
    let ctx = RenderCtx {
        comps: ctx.comps,
        visited: &stack,
    };
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
        let world = comp.world_matrix(i, t);
        // Expression-aware opacity for this layer at the frame time (the value the
        // rasterizers scale coverage by).
        let opacity = comp.layer_opacity(i, t);
        match layer.kind {
            // A null draws nothing — it's a transform reference (parent) only.
            LayerKind::Null => {}
            // A motion-blurred pixel-drawing layer (solid / shape / text /
            // footage / precomp) is rendered into an isolated buffer as the
            // average of sub-frame snapshots, then mask-carved, matte-clipped,
            // spatially filtered, and composited over the accumulator.
            // `composite_motion_blur` dispatches to the right rasterizer per
            // sub-frame (precomps recurse through `ctx`).
            LayerKind::Solid
            | LayerKind::Shape
            | LayerKind::Text
            | LayerKind::Footage
            | LayerKind::Precomp
                if blurred =>
            {
                let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                composite_motion_blur(&mut layer_buf, &geom, comp, i, cache, t, ctx);
                finish_layer(
                    &mut acc,
                    &mut layer_buf,
                    &geom,
                    comp,
                    cache,
                    world,
                    layer,
                    masked,
                    spatial,
                    matte_src,
                    t,
                    ctx,
                );
            }
            // A crisp shape layer rasterizes its vector content into an isolated
            // buffer (it draws arbitrary geometry, so it always routes through the
            // isolated path), then mask / matte / spatial passes apply before it
            // is composited over the accumulator.
            LayerKind::Shape => {
                let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                composite_shape(&mut layer_buf, &geom, world, layer, opacity);
                finish_layer(
                    &mut acc,
                    &mut layer_buf,
                    &geom,
                    comp,
                    cache,
                    world,
                    layer,
                    masked,
                    spatial,
                    matte_src,
                    t,
                    ctx,
                );
            }
            // A crisp text layer rasterizes its glyph strokes into an isolated
            // buffer (vector geometry like a shape), then mask / matte / spatial
            // passes apply before it is composited.
            LayerKind::Text => {
                let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                composite_text(&mut layer_buf, &geom, world, layer, opacity);
                finish_layer(
                    &mut acc,
                    &mut layer_buf,
                    &geom,
                    comp,
                    cache,
                    world,
                    layer,
                    masked,
                    spatial,
                    matte_src,
                    t,
                    ctx,
                );
            }
            // A footage layer decodes its source frame for time `t` (through the
            // shared cache) and samples it into an isolated buffer, then mask /
            // matte / spatial passes apply before it is composited. An unset or
            // failed-to-decode source draws nothing (the buffer stays clear).
            LayerKind::Footage => {
                let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                // Time remap (if enabled) drives the *source* time the footage is
                // sampled at; transforms/opacity stay on the comp time `t`.
                let src_t = comp.layer_source_time(i, t);
                if let Some(path) = layer.footage.path_at(src_t, comp.fps) {
                    if let Some(frame) = cache.get(&path, layer.footage.alpha) {
                        let frame = frame.clone();
                        composite_footage(&mut layer_buf, &geom, world, layer, &frame, opacity);
                    }
                }
                finish_layer(
                    &mut acc,
                    &mut layer_buf,
                    &geom,
                    comp,
                    cache,
                    world,
                    layer,
                    masked,
                    spatial,
                    matte_src,
                    t,
                    ctx,
                );
            }
            // A precomp layer renders its referenced comp recursively (through
            // `ctx`, which carries the project's comps + a cycle guard) at the
            // mapped time, then samples that rendered frame into an isolated buffer
            // (filling the layer's quad like footage). A missing reference or a
            // reference cycle yields nothing (the buffer stays clear). Mask /
            // matte / spatial passes then apply before it is composited.
            LayerKind::Precomp => {
                let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                // Time remap (if enabled) drives the host time fed to the nested
                // comp (the `time_offset` shift still applies on top); transforms
                // and opacity stay on the comp time `t`.
                let src_t = comp.layer_source_time(i, t);
                composite_precomp(&mut layer_buf, &geom, world, layer, cache, src_t, opacity, ctx);
                finish_layer(
                    &mut acc,
                    &mut layer_buf,
                    &geom,
                    comp,
                    cache,
                    world,
                    layer,
                    masked,
                    spatial,
                    matte_src,
                    t,
                    ctx,
                );
            }
            // A crisp solid draws its own colored quad (processed by its effect
            // stack) directly into the accumulator — or, when it has masks, a
            // track matte, spatial effects, or a non-Normal blend mode, into an
            // isolated buffer whose alpha the masks/matte modulate and whose whole
            // buffer the spatial passes filter before it is composited with the
            // layer's blend mode.
            LayerKind::Solid => {
                let blended = layer.blend_mode() != BlendMode::Normal;
                if masked || spatial || matte_src.is_some() || blended {
                    let mut layer_buf = vec![Lin::CLEAR; (w * h) as usize];
                    composite_layer(&mut layer_buf, &geom, world, layer, opacity);
                    finish_layer(
                        &mut acc,
                        &mut layer_buf,
                        &geom,
                        comp,
                        cache,
                        world,
                        layer,
                        masked,
                        spatial,
                        matte_src,
                        t,
                        ctx,
                    );
                } else {
                    composite_layer(&mut acc, &geom, world, layer, opacity);
                }
            }
            // An adjustment re-processes the composite beneath it, within its own
            // transformed quad bounds.
            LayerKind::Adjustment => {
                apply_adjustment(&mut acc, &geom, world, layer, opacity);
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

/// Finish an isolated layer buffer and composite it over the accumulator: carve
/// by the layer's masks, clip by its track matte, run its spatial-effect stack,
/// then source-over the result onto `acc`. Each pass is gated by the
/// corresponding flag the caller already resolved, so an un-effected layer just
/// composites. Shared by the solid / shape / text isolated-buffer paths.
#[allow(clippy::too_many_arguments)]
fn finish_layer(
    acc: &mut [Lin],
    layer_buf: &mut [Lin],
    geom: &Geom,
    comp: &Comp,
    cache: &mut FrameCache,
    world: Affine2,
    layer: &crate::comp::PulseLayer,
    masked: bool,
    spatial: bool,
    matte_src: Option<usize>,
    t: f32,
    ctx: RenderCtx,
) {
    if masked {
        apply_masks(layer_buf, geom, world, layer);
    }
    if let Some(src_idx) = matte_src {
        apply_track_matte(layer_buf, geom, comp, src_idx, cache, layer.matte, t, ctx);
    }
    if spatial {
        apply_spatial(layer_buf, geom, layer);
    }
    // Composite the finished isolated buffer onto the accumulator using the
    // layer's blend mode (Normal reduces exactly to source-over).
    let mode = layer.blend_mode();
    for (dst, src) in acc.iter_mut().zip(layer_buf.iter()) {
        *dst = blend_lin(mode, *src, *dst);
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

    /// The conservative comp-space pixel AABB of a layer-local rectangle
    /// `[lx0, lx1] × [ly0, ly1]` transformed by `world`, clamped to the frame.
    /// `None` when the box falls entirely outside the frame. Used to bound the
    /// shape rasterizer's pixel loop to the shape's transformed extent.
    fn aabb_of_local_box(
        &self,
        world: Affine2,
        lx0: f32,
        ly0: f32,
        lx1: f32,
        ly1: f32,
    ) -> Option<(i32, i32, i32, i32)> {
        let corners = [(lx0, ly0), (lx1, ly0), (lx1, ly1), (lx0, ly1)];
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
