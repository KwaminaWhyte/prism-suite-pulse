//! The on-canvas transform gizmo: drag the selected layer directly in the
//! preview to move, scale, or rotate it (After Effects' / Affinity's selection
//! handles), instead of only nudging the Properties sliders.
//!
//! The gizmo edits a layer's **local** transform properties (position
//! `X`/`Y`, uniform `Scale`, `Rotation`, and the `Anchor` point), which are
//! applied in the layer's **parent space** (`world = parent_world · local`, see
//! [`Comp::world_matrix`](crate::comp::Comp::world_matrix)). All the drag math
//! therefore happens in **parent-local comp space**: a pointer position is
//! mapped screen → comp → parent-local, the property delta is computed there,
//! and the result is keyed back at the playhead. This keeps a parented layer's
//! handles dragging correctly under its parent's rotation/scale.
//!
//! This module is pure (no egui types): it converts geometry to property
//! deltas so the conversion can be unit-tested. The preview panel
//! ([`crate::preview`]) owns hit-testing the screen handles and painting them.

use crate::comp::{Affine2, Comp, Prop, Transform};

/// The half-extent of a layer's base quad as a fraction of the comp width/height
/// — mirrors the renderer's `LAYER_HALF_FRAC` so the gizmo box matches the drawn
/// solid quad exactly.
pub const LAYER_HALF_FRAC: f32 = 0.22;

/// Which part of the gizmo the pointer grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// The layer body — translate (`X`/`Y` position).
    Move,
    /// One of the four bounding-box corners — uniform scale about the anchor.
    /// Indexed `0..4` in the order top-left, top-right, bottom-right, bottom-left.
    Scale(u8),
    /// The rotation knob above the box — rotate about the anchor.
    Rotate,
    /// The anchor-point cross — move the anchor (`Anchor X`/`Anchor Y`).
    Anchor,
}

/// The gizmo geometry for a layer, in **comp space** (origin at the comp center,
/// `+y` downward — the same space `world.apply` outputs and the preview maps to
/// screen). Built from the layer's resolved world matrix so it overlays the
/// drawn quad exactly, including any parent transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoGeom {
    /// The four bounding-box corners (TL, TR, BR, BL), comp px.
    pub corners: [(f32, f32); 4],
    /// The anchor point (the scale/rotation pivot), comp px.
    pub anchor: (f32, f32),
    /// The rotation-knob position (above the box's top edge), comp px.
    pub rotate_knob: (f32, f32),
}

impl GizmoGeom {
    /// Build the gizmo geometry for layer `idx` at time `t`.
    ///
    /// The box is the layer's base quad (`±half` of the comp size) mapped through
    /// its world matrix; the anchor is the layer's anchor point mapped the same
    /// way; the rotation knob sits a fixed comp-space distance beyond the
    /// top-edge midpoint along the box's local "up" direction.
    pub fn build(comp: &Comp, idx: usize, t: f32) -> Option<Self> {
        let layer = comp.layers.get(idx)?;
        let world = comp.world_matrix(idx, t);
        let half_w = comp.width as f32 * LAYER_HALF_FRAC;
        let half_h = comp.height as f32 * LAYER_HALF_FRAC;
        let local = [
            (-half_w, -half_h),
            (half_w, -half_h),
            (half_w, half_h),
            (-half_w, half_h),
        ];
        let corners = local.map(|(lx, ly)| world.apply(lx, ly));

        let tf = layer.transform(t);
        let anchor = world.apply(tf.anchor_x, tf.anchor_y);

        // Knob: beyond the top-edge midpoint, along the box's "up" edge (TL→TR
        // gives the local x axis; the local-up is perpendicular). We extend from
        // the midpoint of the top edge outward by a fraction of the box height.
        let top_mid = (
            (corners[0].0 + corners[1].0) * 0.5,
            (corners[0].1 + corners[1].1) * 0.5,
        );
        let bot_mid = (
            (corners[3].0 + corners[2].0) * 0.5,
            (corners[3].1 + corners[2].1) * 0.5,
        );
        // Up direction = from bottom-mid toward top-mid, normalized.
        let (ux, uy) = {
            let dx = top_mid.0 - bot_mid.0;
            let dy = top_mid.1 - bot_mid.1;
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            (dx / len, dy / len)
        };
        let knob_dist = (half_h * tf.scale.max(0.0)).max(8.0) * 0.35 + 24.0;
        let rotate_knob = (top_mid.0 + ux * knob_dist, top_mid.1 + uy * knob_dist);

        Some(GizmoGeom {
            corners,
            anchor,
            rotate_knob,
        })
    }
}

/// Map a screen point to comp space (origin at the comp center, `+y` down):
/// `comp = (screen - center) / scale`.
pub fn screen_to_comp(sx: f32, sy: f32, cx: f32, cy: f32, scale: f32) -> (f32, f32) {
    let s = scale.max(1e-6);
    ((sx - cx) / s, (sy - cy) / s)
}

/// The parent-space matrix for layer `idx`: the world matrix of its parent (the
/// space the layer's *local* transform is applied in), or the identity for an
/// unparented layer. Position/scale/rotation deltas are computed in this space.
pub fn parent_matrix(comp: &Comp, idx: usize, t: f32) -> Affine2 {
    match comp.layers.get(idx).and_then(|l| l.parent) {
        Some(p) if p < comp.layers.len() && p != idx => comp.world_matrix(p, t),
        _ => Affine2::IDENTITY,
    }
}

/// Squared distance between two points.
fn dist2(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// Hit-test the gizmo handles against a pointer, all in **comp space**.
///
/// Corners and the rotation knob (point handles) are matched within `tol`
/// (comp px). If no point handle is hit but the pointer is inside the box, the
/// body grabs a [`Handle::Move`]. The anchor is matched before the box body so
/// it stays grabbable even when it sits inside the quad. Returns `None` when the
/// pointer is outside everything.
pub fn hit_test(geom: &GizmoGeom, p: (f32, f32), tol: f32) -> Option<Handle> {
    let tol2 = tol * tol;
    // Rotation knob first (it sits outside the box, easiest to claim).
    if dist2(geom.rotate_knob, p) <= tol2 {
        return Some(Handle::Rotate);
    }
    // Corners (scale).
    let mut best: Option<(u8, f32)> = None;
    for (i, &c) in geom.corners.iter().enumerate() {
        let d = dist2(c, p);
        if d <= tol2 && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i as u8, d));
        }
    }
    if let Some((i, _)) = best {
        return Some(Handle::Scale(i));
    }
    // Anchor cross.
    if dist2(geom.anchor, p) <= tol2 {
        return Some(Handle::Anchor);
    }
    // Body interior → move.
    if point_in_quad(p, &geom.corners) {
        return Some(Handle::Move);
    }
    None
}

/// Even-odd point-in-polygon test for the (possibly rotated/sheared) gizmo quad.
fn point_in_quad(p: (f32, f32), quad: &[(f32, f32); 4]) -> bool {
    let mut inside = false;
    let mut j = quad.len() - 1;
    for i in 0..quad.len() {
        let (xi, yi) = quad[i];
        let (xj, yj) = quad[j];
        let intersect = ((yi > p.1) != (yj > p.1))
            && (p.0 < (xj - xi) * (p.1 - yi) / (yj - yi + f32::EPSILON.copysign(yj - yi)) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The result of a gizmo drag: the new sampled values for each property that
/// changed at the playhead, ready to key. `None` fields are untouched.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DragResult {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub scale: Option<f32>,
    pub rotation: Option<f32>,
    pub anchor_x: Option<f32>,
    pub anchor_y: Option<f32>,
}

impl DragResult {
    /// The `(Prop, value)` pairs to key, skipping untouched properties.
    pub fn keys(&self) -> Vec<(Prop, f32)> {
        let mut out = Vec::new();
        if let Some(v) = self.x {
            out.push((Prop::X, v));
        }
        if let Some(v) = self.y {
            out.push((Prop::Y, v));
        }
        if let Some(v) = self.scale {
            out.push((Prop::Scale, v));
        }
        if let Some(v) = self.rotation {
            out.push((Prop::Rotation, v));
        }
        if let Some(v) = self.anchor_x {
            out.push((Prop::AnchorX, v));
        }
        if let Some(v) = self.anchor_y {
            out.push((Prop::AnchorY, v));
        }
        out
    }
}

/// Compute the property change for a drag of `handle`, given the transform at
/// the **grab time** (`start`), the parent matrix, and the pointer's start /
/// current positions in **comp space**.
///
/// Both pointer positions are mapped into parent-local space (where the layer's
/// local transform lives) before the delta is taken, so the math is correct
/// under a rotated/scaled parent. Returns the new property values to key.
///
/// - **Move**: add the parent-local pointer delta to position `(X, Y)`.
/// - **Scale**: ratio of (current pointer ↔ anchor) to (start pointer ↔ anchor)
///   distances, applied to the start scale (clamped non-negative). Degenerate
///   when the grab started on the anchor.
/// - **Rotate**: signed angle swept about the anchor (parent-local), added to
///   the start rotation.
/// - **Anchor**: move the anchor by the parent-local delta *also adjusting
///   position* so the layer doesn't visually jump (After Effects keeps the
///   layer put when the anchor moves). The position compensation is the anchor
///   delta pushed through the layer's rotation+scale.
pub fn drag(
    handle: Handle,
    start: Transform,
    parent: Affine2,
    start_comp: (f32, f32),
    cur_comp: (f32, f32),
) -> DragResult {
    let Some(inv) = parent.inverse() else {
        return DragResult::default();
    };
    let p0 = inv.apply(start_comp.0, start_comp.1);
    let p1 = inv.apply(cur_comp.0, cur_comp.1);
    let mut out = DragResult::default();

    match handle {
        Handle::Move => {
            out.x = Some(start.x + (p1.0 - p0.0));
            out.y = Some(start.y + (p1.1 - p0.1));
        }
        Handle::Scale(_) => {
            // Anchor in parent-local space is the layer's position (the local
            // matrix maps the anchor point onto `(x, y)`).
            let pivot = (start.x, start.y);
            let d0 = (dist2(p0, pivot)).sqrt();
            let d1 = (dist2(p1, pivot)).sqrt();
            if d0 > 1e-4 {
                let factor = d1 / d0;
                out.scale = Some((start.scale * factor).max(0.0));
            }
        }
        Handle::Rotate => {
            let pivot = (start.x, start.y);
            let a0 = (p0.1 - pivot.1).atan2(p0.0 - pivot.0);
            let a1 = (p1.1 - pivot.1).atan2(p1.0 - pivot.0);
            let mut delta = (a1 - a0).to_degrees();
            // Normalize the swept delta to (-180, 180] so a small drag never
            // produces a near-360° jump across the atan2 branch cut.
            while delta > 180.0 {
                delta -= 360.0;
            }
            while delta <= -180.0 {
                delta += 360.0;
            }
            out.rotation = Some(start.rotation_deg + delta);
        }
        Handle::Anchor => {
            // Anchor lives in the layer's *local* space; convert the parent-local
            // pointer delta into local space by undoing rotation+scale.
            let dx_par = p1.0 - p0.0;
            let dy_par = p1.1 - p0.1;
            let s = start.scale.max(1e-4);
            let (sin, cos) = (-start.rotation_deg).to_radians().sin_cos();
            // Inverse of Rotate·Scale applied to the parent-space delta.
            let lx = (cos * dx_par - sin * dy_par) / s;
            let ly = (sin * dx_par + cos * dy_par) / s;
            out.anchor_x = Some(start.anchor_x + lx);
            out.anchor_y = Some(start.anchor_y + ly);
            // Keep the layer visually put: position shifts by the same parent-
            // space amount the anchor moved (the anchor maps to position).
            out.x = Some(start.x + dx_par);
            out.y = Some(start.y + dy_par);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(x: f32, y: f32, scale: f32, rot: f32) -> Transform {
        Transform {
            anchor_x: 0.0,
            anchor_y: 0.0,
            x,
            y,
            scale,
            rotation_deg: rot,
            opacity: 1.0,
        }
    }

    #[test]
    fn screen_comp_roundtrip() {
        let (cx, cy, scale) = (400.0, 300.0, 2.0);
        let (sx, sy) = (500.0, 360.0);
        let (cmx, cmy) = screen_to_comp(sx, sy, cx, cy, scale);
        assert!((cmx - 50.0).abs() < 1e-4);
        assert!((cmy - 30.0).abs() < 1e-4);
    }

    #[test]
    fn move_adds_parent_local_delta_unparented() {
        // Identity parent: the comp delta is the local delta.
        let start = tf(10.0, 20.0, 1.0, 0.0);
        let r = drag(
            Handle::Move,
            start,
            Affine2::IDENTITY,
            (0.0, 0.0),
            (30.0, -5.0),
        );
        assert_eq!(r.x, Some(40.0));
        assert_eq!(r.y, Some(15.0));
        assert_eq!(r.scale, None);
        assert_eq!(r.rotation, None);
    }

    #[test]
    fn move_respects_rotated_parent() {
        // Parent rotated +90° (clockwise on screen): a comp +x drag becomes a
        // local -y drag (because the inverse rotates the delta back).
        let parent = Affine2::rotate_deg(90.0);
        let start = tf(0.0, 0.0, 1.0, 0.0);
        let r = drag(Handle::Move, start, parent, (0.0, 0.0), (10.0, 0.0));
        // rotate_deg(90): (x,y)->(-y, x); inverse maps (10,0)->(0,-10).
        assert!((r.x.unwrap() - 0.0).abs() < 1e-3);
        assert!((r.y.unwrap() - (-10.0)).abs() < 1e-3);
    }

    #[test]
    fn scale_uses_distance_ratio_about_pivot() {
        // Pivot at the layer position (50, 0); grab 100px out, drag to 150px out.
        let start = tf(50.0, 0.0, 1.0, 0.0);
        let r = drag(
            Handle::Scale(2),
            start,
            Affine2::IDENTITY,
            (150.0, 0.0), // 100 px from pivot
            (200.0, 0.0), // 150 px from pivot
        );
        assert!((r.scale.unwrap() - 1.5).abs() < 1e-3);
    }

    #[test]
    fn scale_clamps_nonnegative_and_ignores_grab_on_pivot() {
        let start = tf(0.0, 0.0, 2.0, 0.0);
        // Grab exactly on the pivot → degenerate, no scale change.
        let r = drag(
            Handle::Scale(0),
            start,
            Affine2::IDENTITY,
            (0.0, 0.0),
            (10.0, 0.0),
        );
        assert_eq!(r.scale, None);
    }

    #[test]
    fn rotate_sweeps_signed_angle_about_pivot() {
        let start = tf(0.0, 0.0, 1.0, 10.0);
        // From the +x axis to the +y axis is +90° (screen, +y down → clockwise).
        let r = drag(
            Handle::Rotate,
            start,
            Affine2::IDENTITY,
            (100.0, 0.0),
            (0.0, 100.0),
        );
        assert!((r.rotation.unwrap() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn rotate_normalizes_across_branch_cut() {
        let start = tf(0.0, 0.0, 1.0, 0.0);
        // A tiny clockwise step just below the -x axis to just above it should be
        // a small positive sweep, not ~-360.
        let r = drag(
            Handle::Rotate,
            start,
            Affine2::IDENTITY,
            (-100.0, -1.0),
            (-100.0, 1.0),
        );
        let d = r.rotation.unwrap();
        assert!(d.abs() < 5.0, "expected small sweep, got {d}");
    }

    #[test]
    fn anchor_moves_anchor_and_compensates_position() {
        // No rotation/scale: anchor delta equals local delta equals position delta.
        let start = tf(100.0, 50.0, 1.0, 0.0);
        let r = drag(
            Handle::Anchor,
            start,
            Affine2::IDENTITY,
            (0.0, 0.0),
            (20.0, 10.0),
        );
        assert_eq!(r.anchor_x, Some(20.0));
        assert_eq!(r.anchor_y, Some(10.0));
        assert_eq!(r.x, Some(120.0));
        assert_eq!(r.y, Some(60.0));
    }

    #[test]
    fn anchor_undoes_scale_for_local_delta() {
        // Scale 2: a 20px parent-space drag is a 10px local-space anchor move,
        // but position still shifts the full 20px (parent space).
        let start = tf(0.0, 0.0, 2.0, 0.0);
        let r = drag(
            Handle::Anchor,
            start,
            Affine2::IDENTITY,
            (0.0, 0.0),
            (20.0, 0.0),
        );
        assert!((r.anchor_x.unwrap() - 10.0).abs() < 1e-3);
        assert!((r.anchor_y.unwrap() - 0.0).abs() < 1e-3);
        assert!((r.x.unwrap() - 20.0).abs() < 1e-3);
    }

    #[test]
    fn drag_result_keys_skips_untouched() {
        let mut r = DragResult::default();
        r.x = Some(1.0);
        r.rotation = Some(45.0);
        let keys = r.keys();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&(Prop::X, 1.0)));
        assert!(keys.contains(&(Prop::Rotation, 45.0)));
    }

    #[test]
    fn hit_test_prefers_knob_then_corner_then_body() {
        let geom = GizmoGeom {
            corners: [(-50.0, -50.0), (50.0, -50.0), (50.0, 50.0), (-50.0, 50.0)],
            anchor: (0.0, 0.0),
            rotate_knob: (0.0, -90.0),
        };
        assert_eq!(hit_test(&geom, (0.0, -90.0), 8.0), Some(Handle::Rotate));
        assert_eq!(hit_test(&geom, (50.0, -50.0), 8.0), Some(Handle::Scale(1)));
        assert_eq!(hit_test(&geom, (0.0, 0.0), 8.0), Some(Handle::Anchor));
        // Inside the box but away from the (centered) anchor → move.
        assert_eq!(hit_test(&geom, (20.0, 20.0), 8.0), Some(Handle::Move));
        // Well outside everything → nothing.
        assert_eq!(hit_test(&geom, (500.0, 500.0), 8.0), None);
    }

    #[test]
    fn build_geom_overlays_unparented_layer() {
        let comp = Comp::new();
        // Solid 1 (index 0): centered base quad, no parent.
        let geom = GizmoGeom::build(&comp, 0, 0.0).unwrap();
        // The four corners should form the ±half box around the layer position.
        let half_w = comp.width as f32 * LAYER_HALF_FRAC;
        let half_h = comp.height as f32 * LAYER_HALF_FRAC;
        // At t=0 Solid 1 sits at x=-300 (its first key), no rotation/scale.
        let cx = -300.0;
        assert!((geom.corners[0].0 - (cx - half_w)).abs() < 1.0);
        assert!((geom.corners[2].1 - half_h).abs() < 1.0);
    }
}
