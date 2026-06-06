//! The preview surface: paints the composition and its layers for the current
//! playhead time through egui's [`Painter`].
//!
//! The comp is shown as a centered, aspect-fit rectangle inside the central
//! panel. Each visible layer is a solid color rect, transformed by its sampled
//! (x, y) offset, uniform scale, and rotation about its center, and faded by
//! opacity. Layer coordinates are in comp pixels with the origin at the comp
//! center; we map them to screen via a single fitted scale factor.

use crate::comp::{Comp, PulseLayer};
use crate::theme;
use egui::{Color32, Painter, Pos2, Rect, Stroke, Vec2};

use prism_core::color::{linear_to_srgb, srgb_to_linear};

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
        paint_layer(painter, layer, center, scale, comp, t, selected == Some(i));
    }
}

/// Paint a single layer as a rotated/scaled solid quad.
fn paint_layer(
    painter: &Painter,
    layer: &PulseLayer,
    center: Pos2,
    scale: f32,
    comp: &Comp,
    t: f32,
    selected: bool,
) {
    let tf = layer.transform(t);
    if tf.opacity <= 0.0 {
        return;
    }

    // Layer base rect: a fraction of the comp, sized in comp pixels.
    let half_w = comp.width as f32 * 0.22;
    let half_h = comp.height as f32 * 0.22;

    // Local-space corners (comp px, origin at layer center), pre-rotation.
    let s = tf.scale.max(0.0);
    let local = [
        Vec2::new(-half_w, -half_h),
        Vec2::new(half_w, -half_h),
        Vec2::new(half_w, half_h),
        Vec2::new(-half_w, half_h),
    ];

    let theta = tf.rotation_deg.to_radians();
    let (sin, cos) = theta.sin_cos();

    // Layer center on screen: comp center + (x, y) offset, all scaled to screen.
    let lc = center + Vec2::new(tf.x * scale, tf.y * scale);

    let corners: Vec<Pos2> = local
        .iter()
        .map(|p| {
            let sx = p.x * s;
            let sy = p.y * s;
            let rx = sx * cos - sy * sin;
            let ry = sx * sin + sy * cos;
            lc + Vec2::new(rx * scale, ry * scale)
        })
        .collect();

    let fill = to_color32(layer.color, tf.opacity);
    painter.add(egui::Shape::convex_polygon(
        corners.clone(),
        fill,
        Stroke::NONE,
    ));

    if selected {
        let mut outline = corners.clone();
        outline.push(corners[0]);
        painter.add(egui::Shape::line(
            outline,
            Stroke::new(1.5, theme::accent()),
        ));
    }
}
