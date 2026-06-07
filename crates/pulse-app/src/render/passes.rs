//! The per-layer rasterization passes the frame compositor drives: solid-quad
//! coverage, motion-blur snapshot averaging, adjustment-layer regrade, track
//! mattes, mask carving, and the spatial-effect bridge.

use super::{over, render_comp, Geom, Lin, RenderCtx};
use crate::comp::{
    apply_effects, apply_spatial_effects, mask_stack_coverage, Affine2, Comp, DecodedFrame,
    MatteMode, PulseLayer,
};
use prism_core::color::srgb_to_linear;

/// Rasterize a **shape layer**'s vector content into the (assumed-clear)
/// isolated `out` buffer, in the compositor's premultiplied linear-light form.
///
/// The shape stack lives in the layer's local frame; `world` maps that frame
/// into comp space (own transform + parent chain). The pixel loop is bounded by
/// the layer-local shape bounds mapped through `world` to a comp-space AABB, and
/// each candidate pixel is inverse-mapped back into local space where the shape
/// stack's straight-RGBA coverage is sampled, then converted to premultiplied
/// linear and scaled by the layer's `opacity`. Color is straight sRGB → linear
/// at the boundary, matching the solid path. A singular `world` (zero scale) or
/// empty bounds leaves the buffer clear.
pub(super) fn composite_shape(
    out: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    opacity: f32,
) {
    let &Geom { w, cx, cy, .. } = geom;
    if opacity <= 0.0 {
        return;
    }
    let Some(inv) = world.inverse() else {
        return;
    };
    // Pre-flatten each item once for the per-pixel sampling.
    let polys: Vec<Vec<(f32, f32)>> = layer.shape.items.iter().map(|it| it.polygon()).collect();
    let Some((lx0, ly0, lx1, ly1)) = layer.shape.local_bounds() else {
        return;
    };
    // Map the local-space bounds corners through `world` to a comp-space AABB.
    let Some((x0, x1, y0, y1)) = geom.aabb_of_local_box(world, lx0, ly0, lx1, ly1) else {
        return;
    };

    for py in y0..=y1 {
        let comp_y = py as f32 + 0.5 - cy;
        for px in x0..=x1 {
            let comp_x = px as f32 + 0.5 - cx;
            let (llx, lly) = inv.apply(comp_x, comp_y);
            let straight = layer.shape.coverage_at(&polys, llx, lly);
            let cov = straight[3] * opacity;
            if cov <= 0.0 {
                continue;
            }
            // Straight sRGB color -> linear; carry straight color + coverage and
            // composite source-over (the buffer is premultiplied-free `Lin`).
            let src = Lin {
                r: srgb_to_linear(straight[0].clamp(0.0, 1.0)),
                g: srgb_to_linear(straight[1].clamp(0.0, 1.0)),
                b: srgb_to_linear(straight[2].clamp(0.0, 1.0)),
                a: cov,
            };
            let idx = (py as u32 * w + px as u32) as usize;
            out[idx] = over(src, out[idx]);
        }
    }
}

/// Rasterize a **text layer**'s glyph strokes into the (assumed-clear) isolated
/// `out` buffer, in the compositor's premultiplied linear-light form.
///
/// The mirror of [`composite_shape`] for text: the string is laid out into
/// layer-local stroke segments once, the pixel loop is bounded by the text's
/// local bounds mapped through `world` to a comp-space AABB, and each candidate
/// pixel is inverse-mapped back into local space where the text's straight-RGBA
/// coverage (thickened pen band + optional outline) is sampled, then converted to
/// linear and scaled by the layer's `opacity`. A singular `world` (zero scale) or
/// empty text leaves the buffer clear.
pub(super) fn composite_text(
    out: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    opacity: f32,
) {
    let &Geom { w, cx, cy, .. } = geom;
    if opacity <= 0.0 {
        return;
    }
    let Some(inv) = world.inverse() else {
        return;
    };
    // Lay the string out into layer-local stroke segments once for sampling.
    let segs = layer.text.segments();
    if segs.is_empty() {
        return;
    }
    let Some((lx0, ly0, lx1, ly1)) = layer.text.local_bounds() else {
        return;
    };
    let Some((x0, x1, y0, y1)) = geom.aabb_of_local_box(world, lx0, ly0, lx1, ly1) else {
        return;
    };

    for py in y0..=y1 {
        let comp_y = py as f32 + 0.5 - cy;
        for px in x0..=x1 {
            let comp_x = px as f32 + 0.5 - cx;
            let (llx, lly) = inv.apply(comp_x, comp_y);
            let straight = layer.text.coverage_at(&segs, llx, lly);
            let cov = straight[3] * opacity;
            if cov <= 0.0 {
                continue;
            }
            let src = Lin {
                r: srgb_to_linear(straight[0].clamp(0.0, 1.0)),
                g: srgb_to_linear(straight[1].clamp(0.0, 1.0)),
                b: srgb_to_linear(straight[2].clamp(0.0, 1.0)),
                a: cov,
            };
            let idx = (py as u32 * w + px as u32) as usize;
            out[idx] = over(src, out[idx]);
        }
    }
}

/// Rasterize a **footage layer**'s decoded image into the (assumed-clear)
/// isolated `out` buffer, in the compositor's premultiplied linear-light form.
///
/// `frame` is the already-decoded footage frame for this comp time `t` (straight
/// linear-light RGBA, fetched from the [`FrameCache`](crate::comp::FrameCache) by
/// the caller). The footage fills the layer's base quad (the same
/// `±half_w/±half_h` extents a solid uses), so the layer's transform / anchor /
/// scale / rotation position it exactly like every other layer kind. Each
/// candidate comp pixel is inverse-mapped into local space, converted to a UV
/// over the quad, and the frame is bilinearly sampled; the straight linear RGBA
/// is run through the layer's effect stack (so color grading applies per pixel,
/// the footage analogue of the solid's single-color grade), scaled by the
/// layer's `opacity`, and composited source-over. A singular `world` (zero scale)
/// or empty frame leaves the buffer clear.
pub(super) fn composite_footage(
    out: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    frame: &DecodedFrame,
    opacity: f32,
) {
    let &Geom {
        w,
        cx,
        cy,
        half_w,
        half_h,
        ..
    } = geom;
    if opacity <= 0.0 || frame.width == 0 || frame.height == 0 {
        return;
    }
    let Some(inv) = world.inverse() else {
        return;
    };
    // The footage fills the base quad; bound the pixel loop to that quad's
    // transformed comp-space AABB exactly like the solid path.
    let Some((x0, x1, y0, y1)) = geom.quad_bounds(world) else {
        return;
    };
    let has_effects = !layer.effects.is_empty();

    for py in y0..=y1 {
        let comp_y = py as f32 + 0.5 - cy;
        for px in x0..=x1 {
            let comp_x = px as f32 + 0.5 - cx;
            let (lx, ly) = inv.apply(comp_x, comp_y);
            if lx.abs() > half_w || ly.abs() > half_h {
                continue;
            }
            // Local quad -> UV (top-left origin): u over x, v over y.
            let u = (lx + half_w) / (2.0 * half_w);
            let v = (ly + half_h) / (2.0 * half_h);
            let mut texel = frame.sample(u, v); // straight linear RGBA
            if texel[3] <= 0.0 {
                continue;
            }
            // The layer's effect stack grades the (linear, straight) footage color
            // per pixel — the footage twin of the solid's constant-color grade.
            if has_effects {
                texel = apply_effects(&layer.effects, texel);
            }
            let cov = texel[3].clamp(0.0, 1.0) * opacity;
            if cov <= 0.0 {
                continue;
            }
            let src = Lin {
                r: texel[0],
                g: texel[1],
                b: texel[2],
                a: cov,
            };
            let idx = (py as u32 * w + px as u32) as usize;
            out[idx] = over(src, out[idx]);
        }
    }
}

/// Render a **precomp layer**'s referenced (nested) comp into the (assumed-clear)
/// isolated `out` buffer, in the compositor's premultiplied-free linear-light
/// form.
///
/// The layer's [`PrecompLayer`](crate::comp::PrecompLayer) names a target comp
/// id; `ctx` resolves it against the project's comps (and refuses a reference
/// **cycle** — a target already on the render stack — yielding nothing). The
/// target comp is rendered **recursively** via [`render_comp`] at the
/// time-offset–mapped time, producing a native-resolution sRGB frame; that frame
/// fills the layer's base quad (the same `±half_w/±half_h` extents footage uses),
/// so the layer's transform / anchor / scale / rotation position the nested comp
/// exactly like any other layer kind. Each candidate comp pixel is inverse-mapped
/// into local space, converted to a UV over the quad, the rendered frame is
/// nearest-sampled there, its straight sRGB → linear color (alpha carried) is run
/// through the layer's effect stack and scaled by `opacity`, and composited
/// source-over. An unset reference, a cycle, or a singular `world` (zero scale)
/// leaves the buffer clear.
#[allow(clippy::too_many_arguments)]
pub(super) fn composite_precomp(
    out: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    cache: &mut crate::comp::FrameCache,
    t: f32,
    opacity: f32,
    ctx: RenderCtx,
) {
    let &Geom {
        w,
        cx,
        cy,
        half_w,
        half_h,
        ..
    } = geom;
    if opacity <= 0.0 {
        return;
    }
    let Some(src_id) = layer.precomp.source else {
        return;
    };
    // Resolve the target comp; `resolve` returns `None` for a missing target or a
    // reference cycle (the comp is already on the render stack) — either way the
    // precomp simply draws nothing, so it can't recurse forever.
    let Some(nested) = ctx.resolve(src_id) else {
        return;
    };
    let Some(inv) = world.inverse() else {
        return;
    };
    // Render the nested comp recursively at the mapped time. `render_comp` pushes
    // the nested comp's id onto the visited stack, so a precomp inside it that
    // points back at an ancestor is caught by the same guard.
    let nt = layer.precomp.nested_time(t);
    let frame = render_comp(nested, nt, cache, ctx);
    if frame.width == 0 || frame.height == 0 {
        return;
    }
    let Some((x0, x1, y0, y1)) = geom.quad_bounds(world) else {
        return;
    };
    let has_effects = !layer.effects.is_empty();
    let fw = frame.width as f32;
    let fh = frame.height as f32;

    for py in y0..=y1 {
        let comp_y = py as f32 + 0.5 - cy;
        for px in x0..=x1 {
            let comp_x = px as f32 + 0.5 - cx;
            let (lx, ly) = inv.apply(comp_x, comp_y);
            if lx.abs() > half_w || ly.abs() > half_h {
                continue;
            }
            // Local quad -> UV (top-left origin), then nearest-sample the rendered
            // nested frame.
            let u = (lx + half_w) / (2.0 * half_w);
            let v = (ly + half_h) / (2.0 * half_h);
            let sx = ((u * fw) as i32).clamp(0, frame.width as i32 - 1) as u32;
            let sy = ((v * fh) as i32).clamp(0, frame.height as i32 - 1) as u32;
            let texel = frame.pixel(sx, sy); // straight sRGB 8-bit RGBA
            let a = texel[3] as f32 / 255.0;
            if a <= 0.0 {
                continue;
            }
            // sRGB byte -> linear straight RGBA (alpha is already straight
            // coverage). Match the footage path's color boundary.
            let mut lin = [
                srgb_to_linear(texel[0] as f32 / 255.0),
                srgb_to_linear(texel[1] as f32 / 255.0),
                srgb_to_linear(texel[2] as f32 / 255.0),
                a,
            ];
            if has_effects {
                lin = apply_effects(&layer.effects, lin);
            }
            let cov = lin[3].clamp(0.0, 1.0) * opacity;
            if cov <= 0.0 {
                continue;
            }
            let src = Lin {
                r: lin[0],
                g: lin[1],
                b: lin[2],
                a: cov,
            };
            let idx = (py as u32 * w + px as u32) as usize;
            out[idx] = over(src, out[idx]);
        }
    }
}

/// Render motion-blurred solid layer `idx` into an isolated `out` buffer
/// (assumed clear) as the average of sub-frame snapshots across the shutter.
///
/// Each of the comp's [`MotionBlur::sample_times`](crate::comp::MotionBlur::sample_times) is rasterized into a scratch
/// buffer via [`composite_layer`] (which yields the buffer's standard
/// *premultiplied* color + coverage-alpha form), and the snapshots are averaged
/// component-wise — the float-compositor motion-blur recipe. Averaging in
/// premultiplied space is what keeps partly-covered edges from bleeding the quad
/// color into the transparent samples. The result is left in the same
/// premultiplied representation `composite_layer` produces, so the caller
/// composites it over the accumulator identically to a crisp matte buffer.
pub(super) fn composite_motion_blur(
    out: &mut [Lin],
    geom: &Geom,
    comp: &Comp,
    idx: usize,
    cache: &mut crate::comp::FrameCache,
    t: f32,
    ctx: RenderCtx,
) {
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
        // Opacity is expression-aware and resampled per sub-frame time, so an
        // animated/expressed opacity blurs across the shutter too.
        let op = comp.layer_opacity(idx, st);
        // `scratch` is cleared each sample, so each pixel is the snapshot's
        // premultiplied (color·coverage, coverage) output; accumulate it
        // directly. Shape and text layers rasterize their vector content; footage
        // samples its decoded frame; any other pixel-drawing layer (a solid)
        // rasterizes its quad. The footage frame is sampled at each sub-frame
        // time `st`, so a sequence that advances across the shutter blurs across
        // its own frames too.
        if layer.has_shape() {
            composite_shape(&mut scratch, geom, world, layer, op);
        } else if layer.has_text() {
            composite_text(&mut scratch, geom, world, layer, op);
        } else if layer.has_footage() {
            if let Some(path) = layer.footage.path_at(st, comp.fps) {
                if let Some(frame) = cache.get(&path, layer.footage.alpha) {
                    let frame = frame.clone();
                    composite_footage(&mut scratch, geom, world, layer, &frame, op);
                }
            }
        } else if layer.has_precomp() {
            // Re-render the nested comp at each sub-frame time, so a moving
            // precomp blurs across the shutter (and the nested comp's own
            // animation advances across it too).
            composite_precomp(&mut scratch, geom, world, layer, cache, st, op, ctx);
        } else {
            composite_layer(&mut scratch, geom, world, layer, op);
        }
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
pub(super) fn composite_layer(
    acc: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    opacity: f32,
) {
    let &Geom {
        w,
        cx,
        cy,
        half_w,
        half_h,
        ..
    } = geom;
    if opacity <= 0.0 {
        return;
    }

    // Layer straight sRGB color -> linear; premultiply happens implicitly via
    // the source-over math below (we carry straight color + coverage alpha).
    let lr = srgb_to_linear(layer.color[0].clamp(0.0, 1.0));
    let lg = srgb_to_linear(layer.color[1].clamp(0.0, 1.0));
    let lb = srgb_to_linear(layer.color[2].clamp(0.0, 1.0));
    let src_a = (layer.color[3].clamp(0.0, 1.0)) * opacity;
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
pub(super) fn apply_adjustment(
    acc: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    opacity: f32,
) {
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
    let mix = opacity.clamp(0.0, 1.0);
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
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_track_matte(
    layer_buf: &mut [Lin],
    geom: &Geom,
    comp: &Comp,
    src_idx: usize,
    cache: &mut crate::comp::FrameCache,
    mode: MatteMode,
    t: f32,
    ctx: RenderCtx,
) {
    // Render the matte source into its own isolated buffer (so its alpha/luma is
    // measured in isolation, not on top of anything below it).
    let mut matte = vec![Lin::CLEAR; (geom.w * geom.h) as usize];
    let src_world = comp.world_matrix(src_idx, t);
    let src_op = comp.layer_opacity(src_idx, t);
    if let Some(src_layer) = comp.layers.get(src_idx) {
        if src_layer.has_shape() {
            composite_shape(&mut matte, geom, src_world, src_layer, src_op);
        } else if src_layer.has_text() {
            composite_text(&mut matte, geom, src_world, src_layer, src_op);
        } else if src_layer.has_footage() {
            if let Some(path) = src_layer.footage.path_at(t, comp.fps) {
                if let Some(frame) = cache.get(&path, src_layer.footage.alpha) {
                    let frame = frame.clone();
                    composite_footage(&mut matte, geom, src_world, src_layer, &frame, src_op);
                }
            }
        } else if src_layer.has_precomp() {
            composite_precomp(&mut matte, geom, src_world, src_layer, cache, t, src_op, ctx);
        } else {
            composite_layer(&mut matte, geom, src_world, src_layer, src_op);
        }
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
pub(super) fn apply_masks(layer_buf: &mut [Lin], geom: &Geom, world: Affine2, layer: &PulseLayer) {
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
pub(super) fn apply_spatial(layer_buf: &mut [Lin], geom: &Geom, layer: &PulseLayer) {
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
