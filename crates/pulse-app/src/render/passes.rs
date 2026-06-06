//! The per-layer rasterization passes the frame compositor drives: solid-quad
//! coverage, motion-blur snapshot averaging, adjustment-layer regrade, track
//! mattes, mask carving, and the spatial-effect bridge.

use super::{over, Geom, Lin};
use crate::comp::{
    apply_effects, apply_spatial_effects, mask_stack_coverage, Affine2, Comp, MatteMode, PulseLayer,
};
use prism_core::color::srgb_to_linear;

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
pub(super) fn composite_motion_blur(out: &mut [Lin], geom: &Geom, comp: &Comp, idx: usize, t: f32) {
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
pub(super) fn composite_layer(
    acc: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    t: f32,
) {
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
pub(super) fn apply_adjustment(
    acc: &mut [Lin],
    geom: &Geom,
    world: Affine2,
    layer: &PulseLayer,
    t: f32,
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
pub(super) fn apply_track_matte(
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
