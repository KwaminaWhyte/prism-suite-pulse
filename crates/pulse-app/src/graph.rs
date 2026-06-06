//! The graph editor: an After-Effects-style **value-curve** view of the
//! selected layer's animated properties.
//!
//! Where the [`timeline`](crate::timeline) shows keyframes as marks on a lane,
//! the graph editor plots each property as a curve of *value over time* and lets
//! you shape the motion directly:
//!
//! - **drag a keyframe** to retime it (x) and revalue it (y);
//! - **drag a Bézier ease handle** to shape the segment leaving / arriving at a
//!   key — the same `(out_x, out_y, in_x, in_y)` control points the sampler
//!   already evaluates. Dragging a handle on a Linear/Hold segment promotes it
//!   to an editable [`Ease`](crate::comp::Ease) (seeded at the straight diagonal,
//!   so the conversion is value-neutral).
//!
//! Multiple properties can be shown at once; each gets its own color and is
//! framed to a shared value axis so curves are comparable. Interaction is
//! pointer-driven through egui's painter (no widgets), matching the timeline.

use crate::comp::{Comp, Ease, Handle, Interp, Prop};
use crate::theme;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// Pixels reserved on the left for the value-axis labels.
const AXIS_W: f32 = 44.0;
/// Top/bottom padding inside the plot so curves and handles don't clip.
const PAD_Y: f32 = 16.0;
/// Hit radius (screen px) for grabbing a keyframe or handle.
const GRAB_R: f32 = 7.0;

/// Per-property plot color, distinct from the layer swatch so curves read on
/// the dark plot. Indexed by [`Prop::ALL`] order.
fn prop_color(prop: Prop) -> Color32 {
    match prop {
        Prop::X => Color32::from_rgb(0xE5, 0x6B, 0x6B), // red
        Prop::Y => Color32::from_rgb(0x6B, 0xC2, 0x7A), // green
        Prop::Scale => Color32::from_rgb(0x4E, 0x9B, 0xE6), // blue
        Prop::Rotation => Color32::from_rgb(0xE6, 0xA1, 0x3C), // amber
        Prop::Opacity => Color32::from_rgb(0xB8, 0x7B, 0xE6), // violet
    }
}

/// Which curve element a drag is currently grabbing. Persisted across frames in
/// [`GraphState`] so a drag keeps tracking its element even as it moves and the
/// keyframe list re-sorts.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A keyframe body (`prop`, key index): drag retimes + revalues it.
    Key { prop: Prop, idx: usize },
    /// A Bézier ease handle on the segment leaving key `idx` of `prop`.
    Handle {
        prop: Prop,
        idx: usize,
        which: Handle,
    },
}

/// Persistent graph-editor state held by the app across frames.
#[derive(Default)]
pub struct GraphState {
    /// Which properties are plotted. Empty = "all that have ≥1 keyframe".
    pub shown: Vec<Prop>,
    /// The element currently being dragged, if any.
    grab: Option<Grab>,
}

impl GraphState {
    /// Toggle a property's visibility in the plot.
    pub fn toggle(&mut self, prop: Prop) {
        if let Some(i) = self.shown.iter().position(|&p| p == prop) {
            self.shown.remove(i);
        } else {
            self.shown.push(prop);
        }
    }

    pub fn is_shown(&self, prop: Prop) -> bool {
        self.shown.contains(&prop)
    }
}

/// What the graph editor changed this frame; the app applies scrubbing if set.
pub struct GraphResponse {
    /// New playhead time if the user scrubbed the time axis, else `None`.
    pub scrub_time: Option<f32>,
}

/// Draw the graph editor for the selected layer and handle pointer interaction.
///
/// `time` is the playhead (drawn as a vertical guide). Returns a
/// [`GraphResponse`]; the layer model is mutated in place when keyframes/handles
/// are dragged.
pub fn show(
    ui: &mut Ui,
    comp: &mut Comp,
    selected: Option<usize>,
    time: f32,
    state: &mut GraphState,
) -> GraphResponse {
    let mut out = GraphResponse { scrub_time: None };

    let Some(layer_idx) = selected else {
        ui.weak("Select a layer to edit its value curves.");
        return out;
    };
    if layer_idx >= comp.layers.len() {
        return out;
    }

    // Which properties to plot: explicit selection, else every keyed property.
    let plotted: Vec<Prop> = if state.shown.is_empty() {
        Prop::ALL
            .into_iter()
            .filter(|&p| !comp.layers[layer_idx].track(p).keys.is_empty())
            .collect()
    } else {
        state.shown.clone()
    };

    let dur = comp.duration.max(0.001);

    // --- Shared value axis: union of plotted tracks' bounds -----------------
    let (mut vlo, mut vhi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &p in &plotted {
        if let Some((lo, hi)) = comp.layers[layer_idx].track(p).value_bounds() {
            vlo = vlo.min(lo);
            vhi = vhi.max(hi);
        }
    }
    if !vlo.is_finite() || !vhi.is_finite() {
        ui.weak("This layer has no keyframes yet. Keyframe a property to graph it.");
        return out;
    }
    // Guarantee a non-degenerate range and a little headroom.
    if (vhi - vlo).abs() < 1e-3 {
        vlo -= 1.0;
        vhi += 1.0;
    }
    let vpad = (vhi - vlo) * 0.08;
    vlo -= vpad;
    vhi += vpad;

    // --- Allocate the plot canvas -------------------------------------------
    let desired = Vec2::new(ui.available_width(), ui.available_height().max(120.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);

    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + AXIS_W, rect.top() + PAD_Y),
        Pos2::new(rect.right() - 8.0, rect.bottom() - PAD_Y),
    );
    painter.rect_filled(rect, 0.0, theme::track_bg());

    // Coordinate maps between (time, value) and screen.
    let pw = plot.width().max(1.0);
    let ph = plot.height().max(1.0);
    let t_to_x = |t: f32| plot.left() + (t / dur).clamp(0.0, 1.0) * pw;
    let x_to_t = |x: f32| ((x - plot.left()) / pw).clamp(0.0, 1.0) * dur;
    // Value increases upward, so invert y.
    let v_to_y = move |v: f32| plot.bottom() - ((v - vlo) / (vhi - vlo)) * ph;
    let y_to_v = move |y: f32| vlo + ((plot.bottom() - y) / ph) * (vhi - vlo);

    draw_grid(&painter, plot, dur, vlo, vhi, t_to_x, v_to_y);

    // --- Curves + keyframes + handles ---------------------------------------
    // Collect interactive points so we can resolve a fresh grab on press.
    let mut key_hits: Vec<(Prop, usize, Pos2)> = Vec::new();
    let mut handle_hits: Vec<(Prop, usize, Handle, Pos2)> = Vec::new();

    for &prop in &plotted {
        let color = prop_color(prop);
        draw_curve(&painter, comp, layer_idx, prop, color, dur, t_to_x, v_to_y);

        let keys = &comp.layers[layer_idx].track(prop).keys;
        for (i, k) in keys.iter().enumerate() {
            let kp = Pos2::new(t_to_x(k.t), v_to_y(k.value));

            // Draw ease handles only for the active grab's segment or eased
            // segments, so the plot stays readable. A segment's handles attach
            // to its outgoing key (i) and incoming key (i+1).
            if let Interp::Ease(e) = k.interp {
                if let Some(next) = keys.get(i + 1) {
                    let a = (k.t, k.value);
                    let b = (next.t, next.value);
                    let (hout, hin) = handle_screen_pos(e, a, b, t_to_x, v_to_y);
                    // Stems.
                    painter.line_segment([kp, hout], Stroke::new(1.0, color.gamma_multiply(0.5)));
                    let np = Pos2::new(t_to_x(next.t), v_to_y(next.value));
                    painter.line_segment([np, hin], Stroke::new(1.0, color.gamma_multiply(0.5)));
                    painter.circle_filled(hout, 3.0, color);
                    painter.circle_filled(hin, 3.0, color);
                    handle_hits.push((prop, i, Handle::Out, hout));
                    handle_hits.push((prop, i, Handle::In, hin));
                }
            }

            // Keyframe dot.
            let is_grabbed =
                matches!(state.grab, Some(Grab::Key { prop: gp, idx }) if gp == prop && idx == i);
            let r = if is_grabbed { GRAB_R } else { 4.5 };
            painter.circle(
                kp,
                r,
                color,
                Stroke::new(1.0, Color32::from_rgb(0x10, 0x11, 0x13)),
            );
            key_hits.push((prop, i, kp));
        }
    }

    // --- Playhead guide ------------------------------------------------------
    let px = t_to_x(time);
    painter.line_segment(
        [Pos2::new(px, plot.top()), Pos2::new(px, plot.bottom())],
        Stroke::new(1.0, theme::accent().gamma_multiply(0.7)),
    );

    // --- Interaction ---------------------------------------------------------
    if let Some(pos) = response.interact_pointer_pos() {
        if response.drag_started() || (response.clicked() && state.grab.is_none()) {
            // Resolve what was grabbed: handles take priority (smaller, on top),
            // then keyframes. Nearest within GRAB_R wins.
            state.grab = pick(pos, &handle_hits, &key_hits);
        }
    }

    if response.dragged() {
        if let (Some(grab), Some(pos)) = (state.grab, response.interact_pointer_pos()) {
            apply_drag(comp, layer_idx, grab, pos, x_to_t, y_to_v, &mut state.grab);
        }
    } else if response.drag_stopped() || (!response.dragged() && state.grab.is_some()) {
        state.grab = None;
    }

    // Empty-area click on the plot scrubs time (only when nothing was grabbed).
    if response.clicked() && state.grab.is_none() {
        if let Some(pos) = response.interact_pointer_pos() {
            if plot.contains(pos) {
                out.scrub_time = Some(x_to_t(pos.x));
            }
        }
    }

    out
}

/// Screen positions of a segment's two ease handles. The handles live in
/// normalized curve space `(x∈[0,1], y∈[0,1])` mapped onto the segment's
/// `[a, b]` time/value rectangle (AE attaches handles to the value rect).
fn handle_screen_pos(
    e: Ease,
    a: (f32, f32),
    b: (f32, f32),
    t_to_x: impl Fn(f32) -> f32,
    v_to_y: impl Fn(f32) -> f32,
) -> (Pos2, Pos2) {
    let (at, av) = a;
    let (bt, bv) = b;
    let lerp = |s: f32, lo: f32, hi: f32| lo + (hi - lo) * s;
    let out = Pos2::new(t_to_x(lerp(e.out_x, at, bt)), v_to_y(lerp(e.out_y, av, bv)));
    let inp = Pos2::new(t_to_x(lerp(e.in_x, at, bt)), v_to_y(lerp(e.in_y, av, bv)));
    (out, inp)
}

/// Pick the nearest grabbable element under `pos` (handles before keys).
fn pick(
    pos: Pos2,
    handles: &[(Prop, usize, Handle, Pos2)],
    keys: &[(Prop, usize, Pos2)],
) -> Option<Grab> {
    let mut best: Option<(f32, Grab)> = None;
    let mut consider = |d: f32, g: Grab| {
        if d <= GRAB_R && best.map(|(bd, _)| d < bd).unwrap_or(true) {
            best = Some((d, g));
        }
    };
    for &(prop, idx, which, hp) in handles {
        consider(pos.distance(hp), Grab::Handle { prop, idx, which });
    }
    // Keys only win if no handle was close enough (handles checked first, but a
    // key can still beat a far handle since we compare distances).
    for &(prop, idx, kp) in keys {
        consider(pos.distance(kp), Grab::Key { prop, idx });
    }
    best.map(|(_, g)| g)
}

/// Apply a drag of the grabbed element to the model. Updates `grab` in place
/// when a keyframe re-sorts so the drag keeps following it.
fn apply_drag(
    comp: &mut Comp,
    layer_idx: usize,
    grab: Grab,
    pos: Pos2,
    x_to_t: impl Fn(f32) -> f32,
    y_to_v: impl Fn(f32) -> f32,
    grab_slot: &mut Option<Grab>,
) {
    let dur = comp.duration;
    match grab {
        Grab::Key { prop, idx } => {
            let new_t = x_to_t(pos.x).clamp(0.0, dur);
            let new_v = y_to_v(pos.y);
            let track = comp.layers[layer_idx].track_mut(prop);
            let landed = track.move_key(idx, new_t, new_v);
            if landed != idx {
                *grab_slot = Some(Grab::Key { prop, idx: landed });
            }
        }
        Grab::Handle { prop, idx, which } => {
            let track = comp.layers[layer_idx].track_mut(prop);
            // Segment spans key idx .. idx+1; need both endpoints.
            let (a, b) = match (track.keys.get(idx), track.keys.get(idx + 1)) {
                (Some(ka), Some(kb)) => ((ka.t, ka.value), (kb.t, kb.value)),
                _ => return,
            };
            // Pointer -> normalized segment coordinates.
            let (at, av) = a;
            let (bt, bv) = b;
            let nx = if (bt - at).abs() > f32::EPSILON {
                (x_to_t(pos.x) - at) / (bt - at)
            } else {
                0.0
            };
            let ny = if (bv - av).abs() > f32::EPSILON {
                (y_to_v(pos.y) - av) / (bv - av)
            } else {
                0.0
            };
            // Promote a non-eased segment to an editable ease (seeded straight).
            let base = match track.keys[idx].interp {
                Interp::Ease(e) => e,
                _ => Ease::LINEAR,
            };
            let updated = match which {
                Handle::Out => base.with_out(nx, ny),
                Handle::In => base.with_in(nx, ny),
            };
            if let Some(k) = track.key_mut(idx) {
                k.interp = Interp::Ease(updated);
            }
        }
    }
}

/// Draw a property's value curve by densely sampling the track.
#[allow(clippy::too_many_arguments)]
fn draw_curve(
    painter: &egui::Painter,
    comp: &Comp,
    layer_idx: usize,
    prop: Prop,
    color: Color32,
    dur: f32,
    t_to_x: impl Fn(f32) -> f32,
    v_to_y: impl Fn(f32) -> f32,
) {
    let track = comp.layers[layer_idx].track(prop);
    if track.keys.is_empty() {
        return;
    }
    let default = prop.default_value();
    let n = 240;
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = dur * (i as f32 / n as f32);
        let v = track.sample(t, default);
        pts.push(Pos2::new(t_to_x(t), v_to_y(v)));
    }
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
}

/// Draw the value-axis labels, baseline gridlines, and per-second time ticks.
#[allow(clippy::too_many_arguments)]
fn draw_grid(
    painter: &egui::Painter,
    plot: Rect,
    dur: f32,
    vlo: f32,
    vhi: f32,
    t_to_x: impl Fn(f32) -> f32,
    v_to_y: impl Fn(f32) -> f32,
) {
    let grid = theme::stroke_subtle().gamma_multiply(0.5);
    // Horizontal value gridlines (5 divisions) + labels.
    for i in 0..=4 {
        let v = vlo + (vhi - vlo) * (i as f32 / 4.0);
        let y = v_to_y(v);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0, grid),
        );
        painter.text(
            Pos2::new(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            format!("{v:.1}"),
            FontId::monospace(10.0),
            theme::muted(),
        );
    }
    // Vertical per-second ticks.
    let secs = dur.ceil() as i32;
    for s in 0..=secs {
        let t = s as f32;
        if t > dur + 1e-3 {
            break;
        }
        let x = t_to_x(t);
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(1.0, grid),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_screen_pos_maps_normalized_to_rect() {
        // Segment from (t=0,v=0) to (t=2,v=10); identity screen maps.
        let e = Ease {
            out_x: 0.5,
            out_y: 0.25,
            in_x: 0.75,
            in_y: 0.5,
        };
        let (out, inp) = handle_screen_pos(e, (0.0, 0.0), (2.0, 10.0), |t| t, |v| v);
        // out handle at (0.5*2, 0.25*10) = (1.0, 2.5)
        assert!((out.x - 1.0).abs() < 1e-4);
        assert!((out.y - 2.5).abs() < 1e-4);
        // in handle at (0.75*2, 0.5*10) = (1.5, 5.0)
        assert!((inp.x - 1.5).abs() < 1e-4);
        assert!((inp.y - 5.0).abs() < 1e-4);
    }

    #[test]
    fn pick_prefers_nearest_within_radius() {
        let p = Pos2::new(10.0, 10.0);
        let handles = vec![(Prop::X, 0, Handle::Out, Pos2::new(12.0, 10.0))];
        let keys = vec![(Prop::X, 0, Pos2::new(10.5, 10.0))];
        // Key is closer than the handle -> key wins.
        let g = pick(p, &handles, &keys).unwrap();
        assert_eq!(
            g,
            Grab::Key {
                prop: Prop::X,
                idx: 0
            }
        );
    }

    #[test]
    fn pick_returns_none_when_far() {
        let p = Pos2::new(0.0, 0.0);
        let keys = vec![(Prop::X, 0, Pos2::new(100.0, 100.0))];
        assert!(pick(p, &[], &keys).is_none());
    }

    #[test]
    fn pick_grabs_handle_when_closest() {
        let p = Pos2::new(20.0, 20.0);
        let handles = vec![(Prop::Scale, 1, Handle::In, Pos2::new(21.0, 20.0))];
        let keys = vec![(Prop::Scale, 1, Pos2::new(25.0, 20.0))];
        let g = pick(p, &handles, &keys).unwrap();
        assert_eq!(
            g,
            Grab::Handle {
                prop: Prop::Scale,
                idx: 1,
                which: Handle::In
            }
        );
    }
}
