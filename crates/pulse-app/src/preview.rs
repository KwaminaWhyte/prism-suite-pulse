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
use crate::gizmo::{GizmoGeom, Handle};
use crate::onion::{Ghost, OnionSkin};
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

/// The comp's on-screen center and comp-pixels→screen scale for `avail` — the
/// mapping the preview uses to place layers. Exposed so the panel can convert
/// pointer positions (screen) into comp space for the transform gizmo, using
/// exactly the same fit as the paint pass.
pub fn comp_fit(avail: Rect, width: u32, height: u32) -> (Pos2, f32) {
    let (frame, scale) = fit(avail, width, height);
    (frame.center(), scale)
}

/// Paint the on-canvas transform gizmo for the selected layer: the bounding
/// box, corner scale handles, the rotation knob (with its connector), and the
/// anchor-point cross. `hot` highlights the handle the pointer is over (or is
/// dragging). Drawn on top of the preview so the layer stays visible.
pub fn paint_gizmo(
    painter: &Painter,
    geom: &GizmoGeom,
    center: Pos2,
    scale: f32,
    hot: Option<Handle>,
) {
    let to_screen = |(cx, cy): (f32, f32)| center + Vec2::new(cx * scale, cy * scale);
    let corners: Vec<Pos2> = geom.corners.iter().map(|&c| to_screen(c)).collect();
    let knob = to_screen(geom.rotate_knob);
    let anchor = to_screen(geom.anchor);
    let accent = theme::accent();

    // Bounding box outline.
    let mut box_pts = corners.clone();
    box_pts.push(corners[0]);
    painter.add(egui::Shape::line(box_pts, Stroke::new(1.5, accent)));

    // Connector from the top-edge midpoint to the rotation knob.
    let top_mid = corners[0].lerp(corners[1], 0.5);
    painter.line_segment(
        [top_mid, knob],
        Stroke::new(1.0, accent.gamma_multiply(0.8)),
    );

    // Rotation knob (hollow circle, filled when hot).
    let knob_hot = hot == Some(Handle::Rotate);
    painter.circle(
        knob,
        5.0,
        if knob_hot {
            accent
        } else {
            Color32::from_rgb(0x16, 0x18, 0x1c)
        },
        Stroke::new(1.5, accent),
    );

    // Corner scale handles (small squares).
    for (i, &c) in corners.iter().enumerate() {
        let h = hot == Some(Handle::Scale(i as u8));
        let r = Rect::from_center_size(c, Vec2::splat(7.0));
        painter.rect(
            r,
            1.0,
            if h {
                accent
            } else {
                Color32::from_rgb(0x16, 0x18, 0x1c)
            },
            Stroke::new(1.5, accent),
            egui::StrokeKind::Middle,
        );
    }

    // Anchor cross (the scale/rotation pivot).
    let anchor_hot = hot == Some(Handle::Anchor);
    let ac = if anchor_hot {
        accent
    } else {
        accent.gamma_multiply(0.9)
    };
    let s = 7.0;
    painter.line_segment(
        [anchor - Vec2::new(s, 0.0), anchor + Vec2::new(s, 0.0)],
        Stroke::new(1.5, ac),
    );
    painter.line_segment(
        [anchor - Vec2::new(0.0, s), anchor + Vec2::new(0.0, s)],
        Stroke::new(1.5, ac),
    );
    painter.circle_stroke(anchor, 4.0, Stroke::new(1.5, ac));
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
            i,
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

/// Paint **onion-skin ghost frames** behind the live comp: for each [`Ghost`]
/// (the comp sampled at a neighbouring frame), draw every visible layer as a
/// flat, tinted, faded quad/shape — a legible silhouette of where the motion was
/// (cool tint) or is going (warm tint). Drawn *before* the live frame in
/// [`paint_comp`], so the real frame composites on top.
///
/// Ghosts are intentionally cheap: each layer is a single tinted quad (or shape
/// outline) at the ghost's sampled world transform, with no effects / masks /
/// mattes — onion skinning is a *timing* aid, not a render preview. The list is
/// pre-ordered farthest → nearest, so nearer (more opaque) ghosts paint last.
pub fn paint_onion(painter: &Painter, avail: Rect, comp: &Comp, onion: &OnionSkin, t: f32) {
    let ghosts = onion.ghosts(t, comp.fps, comp.duration);
    if ghosts.is_empty() {
        return;
    }
    let (frame, scale) = fit(avail, comp.width, comp.height);
    let center = frame.center();
    for ghost in ghosts {
        paint_ghost_frame(painter, comp, &ghost, center, scale);
    }
}

/// Paint one ghost frame: every visible, pixel-drawing layer of `comp` sampled at
/// `ghost.time`, as a flat quad/outline tinted toward `ghost.tint` and faded by
/// `ghost.opacity`.
fn paint_ghost_frame(painter: &Painter, comp: &Comp, ghost: &Ghost, center: Pos2, scale: f32) {
    let half_w = comp.width as f32 * 0.22;
    let half_h = comp.height as f32 * 0.22;
    let local = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];
    for (i, layer) in comp.layers.iter().enumerate() {
        if !layer.visible || comp.is_matte_source(i) {
            continue;
        }
        // Nulls / adjustments draw nothing of their own — skip them in ghosts
        // (their effect on the frame is render-only and not part of a timing aid).
        if !layer.kind.draws_own_pixels() {
            continue;
        }
        let world = comp.world_matrix(i, ghost.time);
        let op = (comp.layer_opacity(i, ghost.time) * ghost.opacity).clamp(0.0, 1.0);
        if op <= 0.0 {
            continue;
        }
        let corners: Vec<Pos2> = local
            .iter()
            .map(|&(lx, ly)| {
                let (wx, wy) = world.apply(lx, ly);
                center + Vec2::new(wx * scale, wy * scale)
            })
            .collect();
        let fill = Color32::from_rgba_unmultiplied(
            (ghost.tint[0] * 255.0).round() as u8,
            (ghost.tint[1] * 255.0).round() as u8,
            (ghost.tint[2] * 255.0).round() as u8,
            (op * 255.0).round() as u8,
        );
        painter.add(egui::Shape::convex_polygon(corners, fill, Stroke::NONE));
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
    let src_a = c[3].clamp(0.0, 1.0) * comp.layer_opacity(src_idx, t);
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
    idx: usize,
    layer: &PulseLayer,
    center: Pos2,
    scale: f32,
    world: Affine2,
    comp: &Comp,
    t: f32,
    matte: f32,
    selected: bool,
) {
    // Expression-aware opacity; the track matte (coarsely) scales it in the
    // preview.
    let opacity = comp.layer_opacity(idx, t) * matte.clamp(0.0, 1.0);
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
        LayerKind::Text => {
            // Text layers paint each glyph stroke as a thick line segment through
            // the world matrix into screen space (a cheap legible twin of the
            // offline pen-band rasterizer).
            paint_text(painter, layer, center, scale, world, opacity);
        }
        LayerKind::Footage => {
            // Footage layers paint a flat placeholder quad in the layer's swatch
            // (the decoded image shows in the offline render / export, not the
            // coarse preview), with a thin outline so the source quad reads.
            let fill = to_color32(layer.color, opacity);
            painter.add(egui::Shape::convex_polygon(
                corners.clone(),
                fill,
                Stroke::new(1.0, theme::muted().gamma_multiply(0.8)),
            ));
        }
        LayerKind::Precomp => {
            // Precomp layers paint a flat placeholder quad in the layer's swatch
            // (the nested comp is rendered recursively only in the offline render /
            // export, not the coarse preview), with a thin outline so the precomp's
            // source quad reads and stays selectable.
            let fill = to_color32(layer.color, opacity);
            painter.add(egui::Shape::convex_polygon(
                corners.clone(),
                fill,
                Stroke::new(1.0, theme::accent().gamma_multiply(0.7)),
            ));
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

/// Paint a **text layer**'s glyph strokes: each laid-out segment drawn as a
/// thick line through the `world` matrix into screen space, faded by `opacity`.
///
/// Mirrors the offline text rasterizer's geometry (the same stroke segments,
/// pen width as the line thickness) so the preview matches an exported frame.
/// The fill color is the pen body; if the layer has an outline stroke the body
/// line is drawn slightly thicker in the stroke color underneath, so the glyph
/// reads as filled-then-outlined.
fn paint_text(
    painter: &Painter,
    layer: &PulseLayer,
    center: Pos2,
    scale: f32,
    world: Affine2,
    opacity: f32,
) {
    let text = &layer.text;
    let segs = text.segments();
    if segs.is_empty() {
        return;
    }
    let pen_w = (text.pen_half() * 2.0 * scale).max(1.0);
    let to_screen = |lx: f32, ly: f32| {
        let (wx, wy) = world.apply(lx, ly);
        center + Vec2::new(wx * scale, wy * scale)
    };

    // Outline stroke underlay (drawn first, thicker), so the body sits over it.
    if let Some(s) = text.stroke.filter(|s| s.width > 0.0) {
        let w = ((text.pen_half() * 2.0 + s.width) * scale).max(1.0);
        let col = to_color32(
            [s.color[0], s.color[1], s.color[2], 1.0],
            opacity * s.opacity,
        );
        for &((ax, ay), (bx, by)) in &segs {
            painter.line_segment([to_screen(ax, ay), to_screen(bx, by)], Stroke::new(w, col));
        }
    }
    if let Some(f) = text.fill {
        let col = to_color32(
            [f.color[0], f.color[1], f.color[2], 1.0],
            opacity * f.opacity,
        );
        for &((ax, ay), (bx, by)) in &segs {
            painter.line_segment(
                [to_screen(ax, ay), to_screen(bx, by)],
                Stroke::new(pen_w, col),
            );
        }
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
        let opacity = comp.layer_opacity(i, st) * matte.clamp(0.0, 1.0) / count;
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
