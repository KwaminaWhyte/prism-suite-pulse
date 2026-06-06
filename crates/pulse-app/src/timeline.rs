//! The timeline: a time ruler, one lane per layer with keyframe diamonds, and a
//! draggable playhead. Drawn into the bottom panel through egui's painter, with
//! click/drag scrubbing on the ruler and the lane area.

use crate::comp::{Comp, Interp, Prop};
use crate::theme;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// Pixels reserved on the left of the timeline for layer name labels.
const LABEL_W: f32 = 140.0;
/// Height of the time ruler strip.
const RULER_H: f32 = 22.0;
/// Height of each layer lane.
const LANE_H: f32 = 26.0;

/// Outcome of drawing/interacting with the timeline for one frame.
pub struct TimelineResponse {
    /// New playhead time if the user scrubbed, else `None`.
    pub scrub_time: Option<f32>,
    /// A layer lane was clicked (selection request).
    pub clicked_layer: Option<usize>,
}

/// Draw the timeline and handle scrub/selection input. `time` is the current
/// playhead position in seconds.
pub fn show(ui: &mut Ui, comp: &Comp, time: f32, selected: Option<usize>) -> TimelineResponse {
    let mut out = TimelineResponse {
        scrub_time: None,
        clicked_layer: None,
    };

    let lanes_h = LANE_H * comp.layers.len() as f32;
    let total_h = RULER_H + lanes_h.max(LANE_H);
    let desired = Vec2::new(ui.available_width(), total_h);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

    let track_x0 = rect.left() + LABEL_W;
    let track_w = (rect.right() - track_x0).max(1.0);
    let dur = comp.duration.max(0.001);

    // Map between time (s) and screen x within the track area.
    let time_to_x = |t: f32| track_x0 + (t / dur).clamp(0.0, 1.0) * track_w;
    let x_to_time = |x: f32| ((x - track_x0) / track_w).clamp(0.0, 1.0) * dur;

    // --- Ruler background + ticks -------------------------------------------
    let ruler_rect = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top()),
        Pos2::new(rect.right(), rect.top() + RULER_H),
    );
    painter.rect_filled(ruler_rect, 0.0, theme::track_bg());

    // One tick per second, labeled.
    let secs = dur.ceil() as i32;
    for s in 0..=secs {
        let t = s as f32;
        if t > dur + 0.001 {
            break;
        }
        let x = time_to_x(t);
        painter.line_segment(
            [
                Pos2::new(x, ruler_rect.top() + 4.0),
                Pos2::new(x, ruler_rect.bottom()),
            ],
            Stroke::new(1.0, theme::stroke_subtle()),
        );
        painter.text(
            Pos2::new(x + 3.0, ruler_rect.top() + 2.0),
            Align2::LEFT_TOP,
            format!("{s}s"),
            FontId::monospace(10.0),
            theme::muted(),
        );
    }

    // --- Layer lanes + keyframe diamonds ------------------------------------
    for (i, layer) in comp.layers.iter().enumerate() {
        let lane_top = rect.top() + RULER_H + i as f32 * LANE_H;
        let lane_rect = Rect::from_min_max(
            Pos2::new(rect.left(), lane_top),
            Pos2::new(rect.right(), lane_top + LANE_H),
        );

        // Striped / selected background.
        let bg = if selected == Some(i) {
            theme::accent().gamma_multiply(0.18)
        } else if i % 2 == 0 {
            theme::track_bg().gamma_multiply(0.5)
        } else {
            Color32::TRANSPARENT
        };
        if bg != Color32::TRANSPARENT {
            painter.rect_filled(lane_rect, 0.0, bg);
        }

        // Label (clipped to the label gutter).
        let dot = layer_swatch(layer.color);
        painter.circle_filled(
            Pos2::new(rect.left() + 12.0, lane_top + LANE_H * 0.5),
            4.0,
            dot,
        );
        painter.text(
            Pos2::new(rect.left() + 24.0, lane_top + LANE_H * 0.5),
            Align2::LEFT_CENTER,
            &layer.name,
            FontId::proportional(12.0),
            if layer.visible {
                theme::muted()
            } else {
                theme::muted().gamma_multiply(0.5)
            },
        );

        // Separator at the label/track boundary.
        painter.line_segment(
            [
                Pos2::new(track_x0, lane_top),
                Pos2::new(track_x0, lane_top + LANE_H),
            ],
            Stroke::new(1.0, theme::stroke_subtle()),
        );

        // Keyframe markers: union of all five tracks' key times. The marker
        // shape encodes the outgoing interpolation so easing reads at a glance —
        // a square for Hold, a circle for Bézier ease, a diamond for Linear
        // (matching After Effects' keyframe iconography).
        let cy = lane_top + LANE_H * 0.5;
        for prop in Prop::ALL {
            for k in &layer.track(prop).keys {
                let x = time_to_x(k.t);
                keyframe_marker(&painter, Pos2::new(x, cy), 4.0, theme::accent(), k.interp);
            }
        }
    }

    // --- Playhead ------------------------------------------------------------
    let px = time_to_x(time);
    painter.line_segment(
        [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
        Stroke::new(1.5, theme::accent()),
    );
    // Playhead handle (triangle) at the top.
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(px - 5.0, rect.top()),
            Pos2::new(px + 5.0, rect.top()),
            Pos2::new(px, rect.top() + 7.0),
        ],
        theme::accent(),
        Stroke::NONE,
    ));

    // --- Interaction ---------------------------------------------------------
    if let Some(pos) = response.interact_pointer_pos() {
        let in_tracks = pos.x >= track_x0;
        let on_ruler = pos.y <= rect.top() + RULER_H;

        if (response.clicked() || response.dragged()) && in_tracks {
            // Scrub when interacting with the ruler or the lane track area.
            out.scrub_time = Some(x_to_time(pos.x));
        }

        // Clicking a layer's label gutter selects it.
        if response.clicked() && !on_ruler && pos.x < track_x0 {
            let rel = pos.y - (rect.top() + RULER_H);
            if rel >= 0.0 {
                let idx = (rel / LANE_H) as usize;
                if idx < comp.layers.len() {
                    out.clicked_layer = Some(idx);
                }
            }
        }
    }

    out
}

/// Draw a keyframe marker whose shape reflects its outgoing interpolation:
/// diamond = linear, square = hold, circle = Bézier ease.
fn keyframe_marker(painter: &egui::Painter, c: Pos2, r: f32, color: Color32, interp: Interp) {
    let outline = Stroke::new(1.0, Color32::from_rgb(0x10, 0x11, 0x13));
    match interp {
        Interp::Linear => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x, c.y - r),
                    Pos2::new(c.x + r, c.y),
                    Pos2::new(c.x, c.y + r),
                    Pos2::new(c.x - r, c.y),
                ],
                color,
                outline,
            ));
        }
        Interp::Hold => {
            let h = r * 0.9;
            painter.rect(
                Rect::from_center_size(c, Vec2::splat(h * 2.0)),
                0.0,
                color,
                outline,
                egui::StrokeKind::Middle,
            );
        }
        Interp::Ease(_) => {
            painter.circle(c, r, color, outline);
        }
    }
}

/// A solid egui color for a layer's swatch dot (alpha ignored).
fn layer_swatch(c: [f32; 4]) -> Color32 {
    Color32::from_rgb(
        (c[0].clamp(0.0, 1.0) * 255.0) as u8,
        (c[1].clamp(0.0, 1.0) * 255.0) as u8,
        (c[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}
