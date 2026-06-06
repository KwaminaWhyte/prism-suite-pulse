//! The preview surface: paints the composition and its layers for the current
//! playhead time through egui's [`Painter`].
//!
//! The comp is shown as a centered, aspect-fit rectangle inside the central
//! panel. Each visible layer is a solid color rect, transformed by its resolved
//! [`Affine2`] world matrix (position, uniform scale, and rotation about its
//! anchor point, composed under any parent chain) and faded by opacity. Layer
//! coordinates are in comp pixels with the origin at the comp center; we map
//! them to screen via a single fitted scale factor.

use crate::comp::{apply_effects, Comp, LayerKind, MaskMode, PulseLayer};
use crate::theme;
use egui::{epaint::PathShape, Color32, Painter, Pos2, Rect, Stroke, Vec2};

use crate::comp::Affine2;

use prism_core::color::{linear_to_srgb, srgb_to_linear};

/// The solid layer's effect-processed display color (straight sRGB `[f32; 4]`).
///
/// Mirrors the offline renderer: convert to linear, run the effect stack, encode
/// back to sRGB — so the preview swatch matches an exported frame's color. Only
/// the solid's own constant color is processed here (per-pixel adjustment-layer
/// grading is render-only; the preview shows adjustments as outlines).
fn effected_color(layer: &PulseLayer) -> [f32; 4] {
    if layer.effects.is_empty() {
        return layer.color;
    }
    let lin = [
        srgb_to_linear(layer.color[0].clamp(0.0, 1.0)),
        srgb_to_linear(layer.color[1].clamp(0.0, 1.0)),
        srgb_to_linear(layer.color[2].clamp(0.0, 1.0)),
        layer.color[3],
    ];
    let out = apply_effects(&layer.effects, lin);
    [
        linear_to_srgb(out[0]),
        linear_to_srgb(out[1]),
        linear_to_srgb(out[2]),
        out[3],
    ]
}

/// Convert a straight sRGB `[f32; 4]` (0..=1) into an egui [`Color32`], scaling
/// alpha by `opacity`.
///
/// egui expects sRGB bytes, so the `srgb_to_linear`/`linear_to_srgb` round-trip
/// is value-neutral but routes color through `prism-core`'s shared boundary
/// helpers (and keeps the suite's color path consistent at the app edge).
fn to_color32(c: [f32; 4], opacity: f32) -> Color32 {
    let enc = |v: f32| (linear_to_srgb(srgb_to_linear(v.clamp(0.0, 1.0))) * 255.0).round() as u8;
    let a = (c[3].clamp(0.0, 1.0) * opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_unmultiplied(enc(c[0]), enc(c[1]), enc(c[2]), a)
}

/// Compute the centered, aspect-fit rect for a comp of `width`x`height` inside
/// `avail`, plus the comp-pixels-to-screen scale factor.
fn fit(avail: Rect, width: u32, height: u32) -> (Rect, f32) {
    let cw = width.max(1) as f32;
    let ch = height.max(1) as f32;
    let margin = 24.0;
    let aw = (avail.width() - margin * 2.0).max(1.0);
    let ah = (avail.height() - margin * 2.0).max(1.0);
    let scale = (aw / cw).min(ah / ch).max(0.01);
    let size = Vec2::new(cw * scale, ch * scale);
    let rect = Rect::from_center_size(avail.center(), size);
    (rect, scale)
}

/// Paint the whole composition (frame + visible layers) at time `t`.
pub fn paint_comp(painter: &Painter, avail: Rect, comp: &Comp, t: f32, selected: Option<usize>) {
    let (frame, scale) = fit(avail, comp.width, comp.height);

    // Comp backdrop + frame.
    painter.rect_filled(frame, 4.0, Color32::from_rgb(0x10, 0x11, 0x13));
    painter.rect_stroke(
        frame,
        4.0,
        Stroke::new(1.0, theme::stroke_subtle()),
        egui::StrokeKind::Outside,
    );

    let center = frame.center();

    for (i, layer) in comp.layers.iter().enumerate() {
        if !layer.visible {
            continue;
        }
        // A layer used as a matte source is pulled in by the layer below it and
        // doesn't paint on its own (it only contributes alpha/luma).
        if comp.is_matte_source(i) {
            continue;
        }
        // World matrix folds the layer's own transform under its parent chain.
        let world = comp.world_matrix(i, t);
        // A track matte coarsely modulates this layer's preview opacity by the
        // matte source's constant-color factor (the offline render does this
        // per-pixel; the preview's constant quads can only approximate it).
        let matte = matte_opacity(comp, i, t);
        // Motion blur: draw faint ghost quads at the shutter's sub-frame sample
        // times so the on-screen preview hints at the motion the offline render
        // integrates per-pixel. Only for solids that opt in (and the comp's
        // master switch is on).
        if comp.layer_motion_blurred(i) && layer.kind == LayerKind::Solid {
            paint_motion_blur_ghosts(painter, comp, i, t, center, scale, matte);
        }
        paint_layer(
            painter,
            layer,
            center,
            scale,
            world,
            comp,
            t,
            matte,
            selected == Some(i),
        );
    }
}

/// The coarse matte multiplier for layer `i` in the preview: the matte source's
/// constant-color [`MatteMode::factor`] (the preview can't do per-pixel mattes,
/// so it uses the source's flat color/alpha). `1.0` when the layer has no matte.
fn matte_opacity(comp: &Comp, i: usize, t: f32) -> f32 {
    let Some(src_idx) = comp.matte_source(i) else {
        return 1.0;
    };
    let mode = comp.layers[i].matte;
    let Some(src) = comp.layers.get(src_idx) else {
        return 1.0;
    };
    // The source's effect-processed straight color in linear light, scaled by its
    // own opacity — the same inputs the offline matte factor sees, flattened.
    let c = effected_color(src);
    let src_a = c[3].clamp(0.0, 1.0) * src.transform(t).opacity.clamp(0.0, 1.0);
    let lin = [
        srgb_to_linear(c[0].clamp(0.0, 1.0)),
        srgb_to_linear(c[1].clamp(0.0, 1.0)),
        srgb_to_linear(c[2].clamp(0.0, 1.0)),
        src_a,
    ];
    mode.factor(lin)
}

/// Paint a single layer as a rotated/scaled solid quad, transformed by its
/// resolved `world` matrix (own transform + parent chain).
#[allow(clippy::too_many_arguments)]
fn paint_layer(
    painter: &Painter,
    layer: &PulseLayer,
    center: Pos2,
    scale: f32,
    world: Affine2,
    comp: &Comp,
    t: f32,
    matte: f32,
    selected: bool,
) {
    let tf = layer.transform(t);
    // The track matte (coarsely) scales effective opacity in the preview.
    let opacity = tf.opacity * matte.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    // Layer base rect: a fraction of the comp, sized in comp pixels.
    let half_w = comp.width as f32 * 0.22;
    let half_h = comp.height as f32 * 0.22;

    // Local-space corners (comp px, origin at the layer's geometric center).
    let local = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];

    // Map each local corner through the world matrix into comp space, then to
    // screen: comp center + comp-space offset scaled to screen.
    let corners: Vec<Pos2> = local
        .iter()
        .map(|&(lx, ly)| {
            let (wx, wy) = world.apply(lx, ly);
            center + Vec2::new(wx * scale, wy * scale)
        })
        .collect();

    match layer.kind {
        LayerKind::Solid => {
            // Solids paint their (effect-processed) color, faded by opacity.
            let fill = to_color32(effected_color(layer), opacity);
            painter.add(egui::Shape::convex_polygon(
                corners.clone(),
                fill,
                Stroke::NONE,
            ));
        }
        LayerKind::Shape => {
            // Shape layers paint each item's flattened polygon (fill, then
            // stroke) transformed through the world matrix into screen space.
            paint_shape(painter, layer, center, scale, world, opacity);
        }
        LayerKind::Adjustment => {
            // Adjustment layers don't paint pixels (the regrade is render-only);
            // show a dashed bounds outline so they stay visible & selectable.
            let mut outline = corners.clone();
            outline.push(corners[0]);
            painter.add(egui::Shape::line(
                outline,
                Stroke::new(1.0, theme::muted().gamma_multiply(0.8)),
            ));
        }
        LayerKind::Null => {
            // Nulls are invisible reference handles: draw a small pivot marker at
            // the layer origin so the rig is locatable.
            let (ox, oy) = world.apply(0.0, 0.0);
            let o = center + Vec2::new(ox * scale, oy * scale);
            let s = 8.0;
            painter.line_segment(
                [o - Vec2::new(s, 0.0), o + Vec2::new(s, 0.0)],
                Stroke::new(1.0, theme::muted()),
            );
            painter.line_segment(
                [o - Vec2::new(0.0, s), o + Vec2::new(0.0, s)],
                Stroke::new(1.0, theme::muted()),
            );
        }
    }

    if selected {
        let mut outline = corners.clone();
        outline.push(corners[0]);
        painter.add(egui::Shape::line(
            outline,
            Stroke::new(1.5, theme::accent()),
        ));
        // Draw the layer's mask paths (in layer-local space) on top, so the
        // editable mask region is visible while the layer is selected.
        paint_masks(painter, layer, center, scale, world);
    }
}

/// Paint the selected layer's **mask** paths as outlines, transformed through
/// the layer's `world` matrix into screen space.
///
/// Each active mask's flattened polygon is stroked closed; the stroke color
/// hints at the mode (subtractive/inverted masks are dimmed) so the carve reads
/// at a glance. This is an editor overlay only — coverage itself is computed in
/// the renderer.
fn paint_masks(painter: &Painter, layer: &PulseLayer, center: Pos2, scale: f32, world: Affine2) {
    for mask in &layer.masks {
        if !mask.is_active() {
            continue;
        }
        let poly = mask.flatten();
        if poly.len() < 2 {
            continue;
        }
        let mut pts: Vec<Pos2> = poly
            .iter()
            .map(|&(lx, ly)| {
                let (wx, wy) = world.apply(lx, ly);
                center + Vec2::new(wx * scale, wy * scale)
            })
            .collect();
        pts.push(pts[0]); // close the path
                          // Subtract / inverted masks read as "removing" — dim them; Add/Intersect
                          // are the "keep" ops — draw them in the accent color.
        let removes = mask.inverted || matches!(mask.mode, MaskMode::Subtract);
        let color = if removes {
            theme::muted().gamma_multiply(0.9)
        } else {
            theme::accent().gamma_multiply(0.9)
        };
        painter.add(egui::Shape::line(pts, Stroke::new(1.0, color)));
    }
}

/// Paint a **shape layer**'s items: each item's flattened polygon, mapped from
/// layer-local space through the `world` matrix to screen, filled and/or
/// stroked. Items are drawn bottom-up (under to over), faded by `opacity`.
///
/// Mirrors the offline shape rasterizer's geometry so the preview matches an
/// exported frame; egui's tessellator handles the (possibly concave) fill, and
/// the stroke is drawn as a closed outline of the configured width.
fn paint_shape(
    painter: &Painter,
    layer: &PulseLayer,
    center: Pos2,
    scale: f32,
    world: crate::comp::Affine2,
    opacity: f32,
) {
    for item in &layer.shape.items {
        let poly = item.polygon();
        if poly.len() < 3 {
            continue;
        }
        let pts: Vec<Pos2> = poly
            .iter()
            .map(|&(lx, ly)| {
                let (wx, wy) = world.apply(lx, ly);
                center + Vec2::new(wx * scale, wy * scale)
            })
            .collect();

        let fill = item
            .fill
            .map(|f| {
                to_color32(
                    [f.color[0], f.color[1], f.color[2], 1.0],
                    opacity * f.opacity,
                )
            })
            .unwrap_or(Color32::TRANSPARENT);
        let stroke = item
            .stroke
            .filter(|s| s.width > 0.0)
            .map(|s| {
                let c = to_color32(
                    [s.color[0], s.color[1], s.color[2], 1.0],
                    opacity * s.opacity,
                );
                Stroke::new((s.width * scale).max(1.0), c)
            })
            .unwrap_or(Stroke::NONE);

        painter.add(egui::Shape::Path(PathShape {
            points: pts,
            closed: true,
            fill,
            stroke: stroke.into(),
        }));
    }
}

/// Paint faint **motion-blur ghost** quads for solid layer `i`: one reduced-
/// opacity copy of the layer at each shutter sub-frame sample time, so the
/// preview hints at the swept motion the offline renderer integrates per-pixel.
///
/// Capped to a handful of evenly-chosen samples (the real render uses the comp's
/// full count) and each drawn at `1/count` of the layer's opacity, so the stack
/// of ghosts roughly sums to one solid's worth of coverage — a cheap, legible
/// approximation, not the true integral.
fn paint_motion_blur_ghosts(
    painter: &Painter,
    comp: &Comp,
    i: usize,
    t: f32,
    center: Pos2,
    scale: f32,
    matte: f32,
) {
    let layer = &comp.layers[i];
    let times = comp.motion_blur.sample_times(t, comp.fps);
    if times.len() <= 1 {
        return;
    }
    // Show at most ~8 ghosts regardless of the render sample count.
    const MAX_GHOSTS: usize = 8;
    let step = times.len().div_ceil(MAX_GHOSTS).max(1);
    let ghosts: Vec<f32> = times.iter().copied().step_by(step).collect();
    let count = ghosts.len().max(1) as f32;

    let half_w = comp.width as f32 * 0.22;
    let half_h = comp.height as f32 * 0.22;
    let local = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];
    let base = effected_color(layer);

    for st in ghosts {
        let world = comp.world_matrix(i, st);
        let opacity = layer.transform(st).opacity * matte.clamp(0.0, 1.0) / count;
        if opacity <= 0.0 {
            continue;
        }
        let corners: Vec<Pos2> = local
            .iter()
            .map(|&(lx, ly)| {
                let (wx, wy) = world.apply(lx, ly);
                center + Vec2::new(wx * scale, wy * scale)
            })
            .collect();
        let fill = to_color32(base, opacity);
        painter.add(egui::Shape::convex_polygon(corners, fill, Stroke::NONE));
    }
}
