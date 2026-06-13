use super::distort::sample_bilinear;
use super::effect::{curve_eval, hsl_to_rgb, rgb_to_hsl, smoothstep};
use super::keyframe::{cubic_bezier, solve_bezier_x, Keyframe};
use super::mask::{dist_to_polygon, point_in_polygon};
use super::spatial::{box_blur, directional_blur, gaussian_blur, gaussian_kernel, radial_blur};
use super::*;

#[test]
fn empty_track_uses_default() {
    let t = Track::default();
    assert_eq!(t.sample(2.0, 1.0), 1.0);
}

#[test]
fn single_key_is_constant() {
    let mut t = Track::default();
    t.set_key(1.0, 7.0);
    assert_eq!(t.sample(0.0, 0.0), 7.0);
    assert_eq!(t.sample(5.0, 0.0), 7.0);
}

#[test]
fn linear_interp_and_hold() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(2.0, 10.0);
    assert_eq!(t.sample(-1.0, 99.0), 0.0); // hold before first
    assert!((t.sample(1.0, 0.0) - 5.0).abs() < 1e-5); // midpoint
    assert_eq!(t.sample(9.0, 0.0), 10.0); // hold after last
}

#[test]
fn set_key_overwrites_and_sorts() {
    let mut t = Track::default();
    t.set_key(2.0, 1.0);
    t.set_key(0.0, 2.0);
    t.set_key(2.0, 5.0); // overwrite the key at t=2
    assert_eq!(t.keys.len(), 2);
    assert_eq!(t.keys[0].t, 0.0);
    assert_eq!(t.keys[1].value, 5.0);
}

// --- Easing math --------------------------------------------------------

#[test]
fn ease_endpoints_are_exact() {
    for e in [Ease::EASY, Ease::IN, Ease::OUT] {
        assert_eq!(e.eval(0.0), 0.0);
        assert_eq!(e.eval(1.0), 1.0);
        // Out-of-range x is clamped, not extrapolated.
        assert_eq!(e.eval(-1.0), 0.0);
        assert_eq!(e.eval(2.0), 1.0);
    }
}

#[test]
fn linear_ease_is_identity() {
    // cubic-bezier(1/3, 1/3, 2/3, 2/3) is the straight diagonal: y == x.
    let lin = Ease {
        out_x: 1.0 / 3.0,
        out_y: 1.0 / 3.0,
        in_x: 2.0 / 3.0,
        in_y: 2.0 / 3.0,
    };
    for i in 0..=10 {
        let x = i as f32 / 10.0;
        assert!((lin.eval(x) - x).abs() < 1e-4, "x={x}");
    }
}

#[test]
fn easy_ease_is_symmetric_and_slow_at_ends() {
    let e = Ease::EASY;
    // Symmetry about the midpoint: f(x) + f(1-x) == 1.
    for i in 1..10 {
        let x = i as f32 / 10.0;
        assert!((e.eval(x) + e.eval(1.0 - x) - 1.0).abs() < 1e-3, "x={x}");
    }
    // Midpoint sits exactly at 0.5 by symmetry.
    assert!((e.eval(0.5) - 0.5).abs() < 1e-4);
    // Eased curve lags behind linear early (slow start) ...
    assert!(e.eval(0.25) < 0.25);
    // ... and leads it late (fast then slow finish is the mirror).
    assert!(e.eval(0.75) > 0.75);
}

#[test]
fn ease_eval_inverts_x_correctly() {
    // For any handle config, eval(x) must equal bezier_y(s) where
    // bezier_x(s) == x. Check the x-solve round-trips.
    let e = Ease {
        out_x: 0.8,
        out_y: 0.1,
        in_x: 0.2,
        in_y: 0.9,
    };
    for i in 0..=20 {
        let x = i as f32 / 20.0;
        let s = solve_bezier_x(x, e.out_x.clamp(0.0, 1.0), e.in_x.clamp(0.0, 1.0));
        let reconstructed_x = cubic_bezier(s, e.out_x, e.in_x);
        assert!((reconstructed_x - x).abs() < 1e-3, "x={x}");
    }
}

#[test]
fn ease_is_monotonic_in_x_for_standard_handles() {
    // With monotonic y-handles the eased value never decreases as x grows.
    let e = Ease::EASY;
    let mut prev = -1.0;
    for i in 0..=50 {
        let y = e.eval(i as f32 / 50.0);
        assert!(y >= prev - 1e-4, "non-monotonic at i={i}");
        prev = y;
    }
}

#[test]
fn hold_interp_steps() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(2.0, 10.0);
    t.set_interp(0.0, Interp::Hold);
    assert_eq!(t.sample(0.0, 0.0), 0.0);
    assert_eq!(t.sample(1.0, 0.0), 0.0); // holds outgoing value across segment
    assert_eq!(t.sample(1.999, 0.0), 0.0);
    assert_eq!(t.sample(2.0, 0.0), 10.0); // snaps at the next key
}

#[test]
fn eased_segment_matches_ease_curve() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(2.0, 100.0);
    t.set_interp(0.0, Interp::Ease(Ease::EASY));
    // At the temporal midpoint the eased value lands at the curve midpoint.
    assert!((t.sample(1.0, 0.0) - 50.0).abs() < 0.5);
    // Quarter point lags linear (which would give 25).
    assert!(t.sample(0.5, 0.0) < 25.0);
    // Endpoints unchanged.
    assert_eq!(t.sample(0.0, 0.0), 0.0);
    assert_eq!(t.sample(2.0, 0.0), 100.0);
}

#[test]
fn set_key_inherits_neighbour_interp() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(4.0, 100.0);
    t.set_interp(0.0, Interp::Hold);
    // Re-keying inside the held segment inherits Hold, not Linear.
    t.set_key(2.0, 50.0);
    assert_eq!(t.interp_at(2.0), Some(Interp::Hold));
    // Overwriting an existing key keeps its own mode.
    t.set_interp(2.0, Interp::Ease(Ease::EASY));
    t.set_key(2.0, 60.0);
    assert_eq!(t.interp_at(2.0), Some(Interp::Ease(Ease::EASY)));
}

// --- Graph-editor support ----------------------------------------------

#[test]
fn ease_linear_const_is_identity() {
    // Ease::LINEAR is the straight diagonal: converting a linear segment to
    // this eased curve must be value-neutral.
    for i in 0..=10 {
        let x = i as f32 / 10.0;
        assert!((Ease::LINEAR.eval(x) - x).abs() < 1e-4, "x={x}");
    }
}

#[test]
fn with_handles_clamp_x_keep_y_free() {
    let e = Ease::EASY.with_out(1.7, -0.4).with_in(-0.3, 1.9);
    assert_eq!(e.out_x, 1.0); // x clamped into [0,1]
    assert_eq!(e.in_x, 0.0);
    assert_eq!(e.out_y, -0.4); // y free (anticipation/overshoot)
    assert_eq!(e.in_y, 1.9);
}

#[test]
fn value_bounds_none_when_empty() {
    assert_eq!(Track::default().value_bounds(), None);
}

#[test]
fn value_bounds_spans_keyframe_values() {
    let mut t = Track::default();
    t.set_key(0.0, -5.0);
    t.set_key(1.0, 10.0);
    t.set_key(2.0, 3.0);
    let (lo, hi) = t.value_bounds().unwrap();
    assert!(lo <= -5.0 + 1e-4);
    assert!(hi >= 10.0 - 1e-4);
}

#[test]
fn value_bounds_captures_ease_overshoot() {
    // An overshooting ease (out_y/in_y beyond [0,1]) pushes the sampled value
    // past the keyframe endpoints; bounds must include the overshoot.
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(1.0, 100.0);
    // Big overshoot on the incoming handle.
    t.set_interp(0.0, Interp::Ease(Ease::EASY.with_in(0.67, 1.6)));
    let (_lo, hi) = t.value_bounds().unwrap();
    assert!(hi > 100.0, "expected overshoot above 100, got {hi}");
}

#[test]
fn move_key_reorders_when_crossing_neighbour() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0); // idx 0
    t.set_key(1.0, 10.0); // idx 1
    t.set_key(2.0, 20.0); // idx 2
                          // Drag the middle key past the last one in time.
    let landed = t.move_key(1, 3.0, 99.0);
    assert_eq!(landed, 2);
    // Times stay sorted ascending.
    assert!(t.keys.windows(2).all(|w| w[0].t <= w[1].t));
    // The moved key kept its (new) value at its new slot.
    assert_eq!(t.keys[2].value, 99.0);
    assert_eq!(t.keys[2].t, 3.0);
}

#[test]
fn move_key_without_crossing_keeps_index() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(2.0, 10.0);
    let landed = t.move_key(0, 0.5, 5.0);
    assert_eq!(landed, 0);
    assert_eq!(t.keys[0].t, 0.5);
    assert_eq!(t.keys[0].value, 5.0);
}

#[test]
fn move_key_out_of_range_is_noop() {
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    assert_eq!(t.move_key(9, 5.0, 5.0), 9);
    assert_eq!(t.keys.len(), 1);
    assert_eq!(t.keys[0].t, 0.0);
}

#[test]
fn interp_serde_defaults_to_linear() {
    // Pre-easing keyframes (no `interp` field) must deserialize as Linear.
    let json = r#"{"keys":[{"t":0.0,"value":1.0},{"t":1.0,"value":2.0}]}"#;
    let track: Track = serde_json::from_str(json).unwrap();
    assert_eq!(track.keys.len(), 2);
    assert_eq!(track.keys[0].interp, Interp::Linear);
    // And it samples linearly.
    assert!((track.sample(0.5, 0.0) - 1.5).abs() < 1e-5);
}

// --- Affine2 transform math --------------------------------------------

fn approx(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
}

#[test]
fn affine_identity_is_a_noop() {
    assert!(approx(Affine2::IDENTITY.apply(3.0, -7.0), (3.0, -7.0)));
}

#[test]
fn affine_translate_scale_rotate() {
    assert!(approx(
        Affine2::translate(5.0, 2.0).apply(1.0, 1.0),
        (6.0, 3.0)
    ));
    assert!(approx(Affine2::scale(3.0).apply(2.0, -4.0), (6.0, -12.0)));
    // 90° about origin, +y down (clockwise on screen): (1,0) -> (0,1).
    assert!(approx(
        Affine2::rotate_deg(90.0).apply(1.0, 0.0),
        (0.0, 1.0)
    ));
    // 180°: (1,2) -> (-1,-2).
    assert!(approx(
        Affine2::rotate_deg(180.0).apply(1.0, 2.0),
        (-1.0, -2.0)
    ));
}

#[test]
fn affine_then_applies_rhs_first() {
    // then(rhs) = self ∘ rhs: scale by 2, THEN translate by (10,0).
    let m = Affine2::translate(10.0, 0.0).then(Affine2::scale(2.0));
    assert!(approx(m.apply(3.0, 1.0), (16.0, 2.0)));
    // Reversed order differs (translate first, then scale).
    let n = Affine2::scale(2.0).then(Affine2::translate(10.0, 0.0));
    assert!(approx(n.apply(3.0, 1.0), (26.0, 2.0)));
}

#[test]
fn affine_inverse_round_trips() {
    let m = Affine2::translate(7.0, -3.0)
        .then(Affine2::rotate_deg(37.0))
        .then(Affine2::scale(2.5));
    let inv = m.inverse().unwrap();
    let p = (4.0, -9.0);
    let mapped = m.apply(p.0, p.1);
    let back = inv.apply(mapped.0, mapped.1);
    assert!(approx(back, p), "inverse did not round-trip: {back:?}");
}

#[test]
fn affine_inverse_none_when_singular() {
    // Zero scale collapses the plane -> not invertible.
    assert!(Affine2::scale(0.0).inverse().is_none());
}

// --- Anchor point -------------------------------------------------------

#[test]
fn default_transform_pivots_about_center() {
    // No anchor, no position: the local matrix is just rotate·scale about
    // the layer center, so the center (0,0) stays put.
    let tf = Transform {
        anchor_x: 0.0,
        anchor_y: 0.0,
        x: 0.0,
        y: 0.0,
        scale: 2.0,
        rotation_deg: 90.0,
        opacity: 1.0,
    };
    let m = tf.local_matrix();
    assert!(approx(m.apply(0.0, 0.0), (0.0, 0.0)));
    // A point right of center: scaled x2 then rotated 90° (+y down).
    assert!(approx(m.apply(1.0, 0.0), (0.0, 2.0)));
}

#[test]
fn anchor_point_is_the_pivot_and_lands_on_position() {
    // Anchor offset (10,0); position (100, 50): the anchored local point
    // (10,0) must map exactly to comp-space position (100,50), and scale
    // pivots about the anchor, not the center.
    let tf = Transform {
        anchor_x: 10.0,
        anchor_y: 0.0,
        x: 100.0,
        y: 50.0,
        scale: 3.0,
        rotation_deg: 0.0,
        opacity: 1.0,
    };
    let m = tf.local_matrix();
    // The anchor maps to the position.
    assert!(approx(m.apply(10.0, 0.0), (100.0, 50.0)));
    // The center (0,0) sits anchor-distance*scale to the left of position:
    // local (0,0) is 10 left of the anchor -> 30 left after scale x3.
    assert!(approx(m.apply(0.0, 0.0), (70.0, 50.0)));
}

/// Regression: a text layer's **transform stays put across a font-family change**.
/// Position / anchor / scale / rotation live on the layer's animatable tracks,
/// wholly separate from the glyph buffer, so switching the rendered font
/// (stroke `None` ↔ outline `Some(..)`) — like changing size / align — must not
/// touch the transform or move where the layer's anchor lands on screen. (Guards
/// against the class of bug where editing a type property resets the layer to the
/// top-left / makes the text jump.)
#[test]
fn font_family_change_preserves_text_layer_transform_and_anchor() {
    let mut layer = PulseLayer::of_kind(LayerKind::Text, "T", [1.0; 4]);
    layer.text = TextLayer {
        text: "HELLO".to_string(),
        size: 120.0,
        align: TextAlign::Center,
        font_family: None, // built-in stroke font
        ..TextLayer::default()
    };
    // Place / move the layer somewhere non-trivial (a user dragging it around):
    // an offset anchor, a position away from center, plus scale + rotation.
    layer.anchor_x.set_key(0.0, 7.0);
    layer.anchor_y.set_key(0.0, -3.0);
    layer.x.set_key(0.0, 140.0);
    layer.y.set_key(0.0, -90.0);
    layer.scale.set_key(0.0, 1.5);
    layer.rotation.set_key(0.0, 30.0);

    let before = layer.transform(0.0);
    // Where the anchor point lands in comp space, before the font change.
    let anchor_before = before.local_matrix().apply(before.anchor_x, before.anchor_y);

    // Switch to a real outline family (and bump other type settings while we're
    // here) — exactly what the Properties Font dropdown does.
    layer.text.font_family = Some("Ubuntu".to_string());
    layer.text.size = 200.0;
    layer.text.align = TextAlign::Right;

    let after = layer.transform(0.0);
    // Every transform component is untouched by the type edit.
    assert_eq!(before.anchor_x, after.anchor_x);
    assert_eq!(before.anchor_y, after.anchor_y);
    assert_eq!(before.x, after.x);
    assert_eq!(before.y, after.y);
    assert_eq!(before.scale, after.scale);
    assert_eq!(before.rotation_deg, after.rotation_deg);
    // …and the anchor still lands on the exact same comp-space point: the layer
    // (and so the text) does not jump on a font change.
    let anchor_after = after.local_matrix().apply(after.anchor_x, after.anchor_y);
    assert!(approx(anchor_before, anchor_after), "anchor moved on font change");

    // The laid-out block stays centered about the layer-local origin in *both*
    // font paths, so the text rides the same anchor regardless of font: the
    // stroke path lays out segments centered on (0,0), the outline path lays out
    // contours centered on (0,0). (A path that anchored text to its own bounds
    // would shift the centroid when metrics changed and the text would appear to
    // move.)
    let mut stroke = layer.clone();
    stroke.text.font_family = None;
    let span = |xs: &[f32]| {
        let lo = xs.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (lo + hi) * 0.5 // midpoint of the extent
    };
    let stroke_xs: Vec<f32> = stroke
        .text
        .segments()
        .iter()
        .flat_map(|&(a, b)| [a.0, b.0])
        .collect();
    let stroke_ys: Vec<f32> = stroke
        .text
        .segments()
        .iter()
        .flat_map(|&(a, b)| [a.1, b.1])
        .collect();
    let outline_xs: Vec<f32> = layer
        .text
        .outline_contours()
        .iter()
        .flat_map(|c| c.iter().map(|p| p.0))
        .collect();
    let outline_ys: Vec<f32> = layer
        .text
        .outline_contours()
        .iter()
        .flat_map(|c| c.iter().map(|p| p.1))
        .collect();
    // Both font paths center the block on the origin (within a glyph-metric
    // tolerance): the visual center coincides with the layer center either way,
    // so the on-screen placement is stable across the font switch.
    assert!(
        span(&stroke_xs).abs() < 1.0,
        "stroke block centered on x=0, got {}",
        span(&stroke_xs)
    );
    assert!(
        span(&outline_xs).abs() < layer.text.size,
        "outline block centered near x=0, got {}",
        span(&outline_xs)
    );
    assert!(
        span(&stroke_ys).abs() < 1.0,
        "stroke block centered on y=0, got {}",
        span(&stroke_ys)
    );
    assert!(
        span(&outline_ys).abs() < layer.text.size,
        "outline block centered near y=0, got {}",
        span(&outline_ys)
    );
}

// --- Parenting / world matrix ------------------------------------------

fn parented_comp() -> Comp {
    let mut c = Comp {
        width: 100,
        height: 100,
        duration: 1.0,
        fps: 30.0,
        motion_blur: MotionBlur::default(),
        markers: Vec::new(),
        work_area: WorkArea::default(),
        camera: Camera::default(),
        layers: Vec::new(),
        id: 0,
        name: String::new(),
    };
    c.layers.push(PulseLayer::new("parent", [1.0; 4])); // 0
    c.layers.push(PulseLayer::new("child", [1.0; 4])); // 1
    c
}

#[test]
fn unparented_world_matrix_equals_local() {
    let mut c = parented_comp();
    c.layers[0].x.set_key(0.0, 25.0);
    c.layers[0].rotation.set_key(0.0, 45.0);
    let world = c.world_matrix(0, 0.0);
    let local = c.layers[0].transform(0.0).local_matrix();
    assert_eq!(world, local);
}

#[test]
fn child_inherits_parent_translation() {
    let mut c = parented_comp();
    c.layers[0].x.set_key(0.0, 40.0); // parent shifted right 40
    c.layers[1].x.set_key(0.0, 10.0); // child shifted right 10 in parent space
    c.layers[1].parent = Some(0);
    // Child's local center (0,0) -> parent applies its +40 offset on top of
    // the child's own +10 = +50 in comp space.
    let world = c.world_matrix(1, 0.0);
    assert!(approx(world.apply(0.0, 0.0), (50.0, 0.0)));
}

#[test]
fn child_inherits_parent_rotation_and_scale() {
    let mut c = parented_comp();
    c.layers[0].scale.set_key(0.0, 2.0); // parent scales x2
    c.layers[0].rotation.set_key(0.0, 90.0); // and rotates 90°
    c.layers[1].x.set_key(0.0, 5.0); // child offset +5 in parent space
    c.layers[1].parent = Some(0);
    // Child center: +5 in parent space, then parent scales x2 (->10) and
    // rotates 90° (+y down): (10,0) -> (0,10).
    let world = c.world_matrix(1, 0.0);
    assert!(approx(world.apply(0.0, 0.0), (0.0, 10.0)));
}

#[test]
fn world_matrix_breaks_self_cycle() {
    let mut c = parented_comp();
    c.layers[0].parent = Some(0); // self-parent (corrupt)
    c.layers[0].x.set_key(0.0, 7.0);
    // Must terminate and apply the layer's transform exactly once.
    let world = c.world_matrix(0, 0.0);
    assert!(approx(world.apply(0.0, 0.0), (7.0, 0.0)));
}

#[test]
fn world_matrix_breaks_mutual_cycle() {
    let mut c = parented_comp();
    c.layers[0].parent = Some(1);
    c.layers[1].parent = Some(0); // 0<->1 cycle
                                  // Bounded walk; just assert it returns (no hang/overflow).
    let _ = c.world_matrix(0, 0.0);
    let _ = c.world_matrix(1, 0.0);
}

#[test]
fn can_parent_rejects_self_and_cycles() {
    let mut c = parented_comp();
    c.layers.push(PulseLayer::new("grandchild", [1.0; 4])); // 2
    c.layers[1].parent = Some(0); // child(1) -> parent(0)
    c.layers[2].parent = Some(1); // grandchild(2) -> child(1)
                                  // Self-parent is illegal.
    assert!(!c.can_parent(0, 0));
    // Out-of-range parent is illegal.
    assert!(!c.can_parent(0, 9));
    // Parenting the root (0) to its own descendants (1 or 2) would cycle.
    assert!(!c.can_parent(0, 1));
    assert!(!c.can_parent(0, 2));
    // Re-pointing the tail (2) at the root (0) is acyclic and allowed.
    assert!(c.can_parent(2, 0));
}

#[test]
fn parent_assign_then_clear_returns_to_local() {
    // Assigning a parent makes the child ride the parent's offset; clearing it
    // (back to `None`) returns the child to its own local transform exactly.
    let mut c = parented_comp();
    c.layers[0].x.set_key(0.0, 30.0); // parent shifted right 30
    c.layers[1].x.set_key(0.0, 5.0); // child shifted right 5

    // Assign: child inherits +30 on top of its own +5 = +35.
    assert!(c.can_parent(1, 0));
    c.layers[1].parent = Some(0);
    assert!(approx(c.world_matrix(1, 0.0).apply(0.0, 0.0), (35.0, 0.0)));

    // Clear: world matrix collapses back to the child's local transform (+5).
    c.layers[1].parent = None;
    assert_eq!(
        c.world_matrix(1, 0.0),
        c.layers[1].transform(0.0).local_matrix()
    );
    assert!(approx(c.world_matrix(1, 0.0).apply(0.0, 0.0), (5.0, 0.0)));
}

// --- Layer kinds --------------------------------------------------------

#[test]
fn only_solid_draws_own_pixels() {
    assert!(LayerKind::Solid.draws_own_pixels());
    assert!(!LayerKind::Null.draws_own_pixels());
    assert!(!LayerKind::Adjustment.draws_own_pixels());
}

#[test]
fn null_layer_creation_is_transform_only() {
    // A fresh Null is a real, unparented layer whose transform is live but which
    // draws nothing of its own — usable purely as a parent / pivot handle.
    let null = PulseLayer::of_kind(LayerKind::Null, "Null 1", [0.6, 0.6, 0.6, 1.0]);
    assert_eq!(null.kind, LayerKind::Null);
    assert!(!null.kind.draws_own_pixels());
    assert_eq!(null.parent, None);
    // Its transform is the usual identity-at-default and animatable like any layer.
    assert_eq!(null.transform(0.0).scale, 1.0);
    assert!(null.x.keys.is_empty());
}

#[test]
fn null_layerkind_serde_roundtrips() {
    // The new `Null` variant round-trips, and a pre-Null-variant layer (no `kind`
    // field at all) still deserializes — adding the variant doesn't break old files.
    let null = PulseLayer::of_kind(LayerKind::Null, "N", [0.6, 0.6, 0.6, 1.0]);
    let json = serde_json::to_string(&null).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, LayerKind::Null);

    let old = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(old).unwrap();
    assert_eq!(layer.kind, LayerKind::Solid);
}

#[test]
fn layer_kind_serde_defaults_to_solid() {
    // A pre-kind layer (no `kind`/`effects` fields) loads as a Solid with no
    // effects.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.kind, LayerKind::Solid);
    assert!(layer.effects.is_empty());
}

#[test]
fn generate_serde_defaults_to_none() {
    // A pre-generate layer (no `generate` field) loads with an empty generate slot.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.generate.is_none());
}

/// Pull the `evolution` out of a Fractal-Noise generate (test helper).
fn evo_of(g: GenerateEffect) -> f32 {
    match g {
        GenerateEffect::FractalNoise { evolution, .. } => evolution,
        other => panic!("expected Fractal Noise, got {}", other.label()),
    }
}

#[test]
fn generate_at_uses_static_evolution_when_track_empty() {
    // No evolution keys → generate_at returns the static field unchanged.
    let mut gen = GenerateEffect::defaults()[0];
    if let GenerateEffect::FractalNoise { evolution, .. } = &mut gen {
        *evolution = 3.0;
    }
    let mut layer = PulseLayer::new("L", [1.0; 4]);
    layer.generate = Some(gen);
    assert_eq!(evo_of(layer.generate_at(0.0).unwrap()), 3.0);
    assert_eq!(
        evo_of(layer.generate_at(5.0).unwrap()),
        3.0,
        "static evolution is constant over time"
    );
}

#[test]
fn generate_at_track_overrides_static_evolution() {
    // A keyed evolution track overrides the static field at the sampled time.
    let mut layer = PulseLayer::new("L", [1.0; 4]);
    layer.generate = Some(GenerateEffect::defaults()[0]);
    layer.generate_evolution.set_key(0.0, 0.0);
    layer.generate_evolution.set_key(2.0, 10.0);
    assert!((evo_of(layer.generate_at(0.0).unwrap()) - 0.0).abs() < 1e-5);
    let mid = evo_of(layer.generate_at(1.0).unwrap());
    assert!((mid - 5.0).abs() < 1e-4, "linear interp at midpoint, got {mid}");
    assert!((evo_of(layer.generate_at(2.0).unwrap()) - 10.0).abs() < 1e-4);
}

#[test]
fn generate_at_color_generator_ignores_evolution_track() {
    // A colour generator has no evolution axis, so a keyed evolution track is a
    // no-op for it (the generate is returned unchanged).
    let mut layer = PulseLayer::new("L", [1.0; 4]);
    let ramp = GenerateEffect::defaults()[1];
    layer.generate = Some(ramp);
    layer.generate_evolution.set_key(0.0, 0.0);
    layer.generate_evolution.set_key(2.0, 10.0);
    assert_eq!(layer.generate_at(1.0).unwrap(), ramp);
}

#[test]
fn generate_at_none_without_fill() {
    let layer = PulseLayer::new("L", [1.0; 4]);
    assert!(layer.generate_at(0.0).is_none());
}

#[test]
fn generate_layer_serde_round_trips() {
    // A layer with a Fractal Noise generate fill round-trips through serde.
    let mut layer = PulseLayer::new("L", [1.0, 1.0, 1.0, 1.0]);
    layer.generate = Some(GenerateEffect::FractalNoise {
        fractal_type: FractalType::Turbulent,
        contrast: 1.5,
        brightness: 0.1,
        scale: 64.0,
        scale_x: 1.2,
        scale_y: 0.8,
        complexity: 4,
        sub_influence: 0.5,
        sub_scaling: 2.5,
        evolution: 3.0,
        seed: 99,
        overflow: Overflow::Wrap,
        opacity: 0.7,
    });
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(layer.generate, back.generate);
}

// --- Effects ------------------------------------------------------------

fn approx_rgb(a: [f32; 4], b: [f32; 3]) -> bool {
    (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4 && (a[2] - b[2]).abs() < 1e-4
}

#[test]
fn effect_preserves_alpha() {
    let px = [0.5, 0.5, 0.5, 0.37];
    for e in Effect::defaults() {
        assert_eq!(e.apply(px)[3], 0.37, "{} changed alpha", e.label());
    }
}

#[test]
fn brightness_contrast_identity_is_neutral() {
    let e = Effect::BrightnessContrast {
        brightness: 0.0,
        contrast: 1.0,
    };
    assert!(approx_rgb(e.apply([0.2, 0.5, 0.8, 1.0]), [0.2, 0.5, 0.8]));
}

#[test]
fn brightness_lifts_and_contrast_pivots_about_half() {
    // +0.1 brightness lifts everything.
    let b = Effect::BrightnessContrast {
        brightness: 0.1,
        contrast: 1.0,
    };
    assert!(approx_rgb(b.apply([0.4, 0.4, 0.4, 1.0]), [0.5, 0.5, 0.5]));
    // 2x contrast: 0.5 is the pivot (unchanged), 0.75 pushes toward white.
    let c = Effect::BrightnessContrast {
        brightness: 0.0,
        contrast: 2.0,
    };
    assert!((c.apply([0.5, 0.5, 0.5, 1.0])[0] - 0.5).abs() < 1e-4);
    assert!(c.apply([0.75, 0.75, 0.75, 1.0])[0] > 0.75);
}

#[test]
fn exposure_doubles_per_stop_and_clamps() {
    let e = Effect::Exposure {
        stops: 1.0,
        offset: 0.0,
        gamma: 1.0,
    };
    // +1 stop doubles linear value: 0.25 -> 0.5.
    assert!((e.apply([0.25, 0.25, 0.25, 1.0])[0] - 0.5).abs() < 1e-4);
    // Output is clamped into [0,1] (0.8 * 2 = 1.6 -> 1.0).
    assert_eq!(e.apply([0.8, 0.8, 0.8, 1.0])[0], 1.0);
}

#[test]
fn levels_identity_is_neutral_and_remaps_range() {
    let id = Effect::Levels {
        in_black: 0.0,
        in_white: 1.0,
        gamma: 1.0,
        out_black: 0.0,
        out_white: 1.0,
    };
    assert!(approx_rgb(id.apply([0.3, 0.6, 0.9, 1.0]), [0.3, 0.6, 0.9]));
    // Lift the input black point to 0.5: anything <=0.5 clamps to out_black 0.
    let lift = Effect::Levels {
        in_black: 0.5,
        in_white: 1.0,
        gamma: 1.0,
        out_black: 0.0,
        out_white: 1.0,
    };
    assert_eq!(lift.apply([0.5, 0.5, 0.5, 1.0])[0], 0.0);
    // The new white point (1.0) maps to out_white (1.0).
    assert!((lift.apply([1.0, 1.0, 1.0, 1.0])[0] - 1.0).abs() < 1e-4);
    // Midway (0.75) sits halfway in the remapped range.
    assert!((lift.apply([0.75, 0.75, 0.75, 1.0])[0] - 0.5).abs() < 1e-4);
}

#[test]
fn tint_maps_luma_between_black_and_white() {
    // Tint black->blue, white->red at full strength: a mid-gray maps to a
    // blend, pure black to blue, pure white to red.
    let e = Effect::Tint {
        black: [0.0, 0.0, 1.0],
        white: [1.0, 0.0, 0.0],
        amount: 1.0,
    };
    assert!(approx_rgb(e.apply([0.0, 0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]));
    assert!(approx_rgb(e.apply([1.0, 1.0, 1.0, 1.0]), [1.0, 0.0, 0.0]));
}

#[test]
fn tint_amount_zero_is_passthrough() {
    let e = Effect::Tint {
        black: [0.0, 0.0, 0.0],
        white: [1.0, 1.0, 1.0],
        amount: 0.0,
    };
    assert!(approx_rgb(e.apply([0.2, 0.5, 0.8, 1.0]), [0.2, 0.5, 0.8]));
}

#[test]
fn apply_effects_chains_in_order() {
    // Brightness +0.5 then a Levels that remaps [0,0.5]->[0,1]: order matters.
    let stack = [
        Effect::BrightnessContrast {
            brightness: 0.5,
            contrast: 1.0,
        },
        Effect::Levels {
            in_black: 0.0,
            in_white: 0.5,
            gamma: 1.0,
            out_black: 0.0,
            out_white: 1.0,
        },
    ];
    // 0.0 -> +0.5 -> remapped (0.5/0.5)=1.0.
    let out = apply_effects(&stack, [0.0, 0.0, 0.0, 1.0]);
    assert!((out[0] - 1.0).abs() < 1e-4);
    // Empty stack is a passthrough.
    let same = apply_effects(&[], [0.1, 0.2, 0.3, 0.4]);
    assert_eq!(same, [0.1, 0.2, 0.3, 0.4]);
}

// --- Effect masks -------------------------------------------------------

/// A full-strength "make it white" grade, so the effected pixel is unmistakably
/// different from the original (black).
fn whiten_stack() -> [Effect; 1] {
    [Effect::BrightnessContrast {
        brightness: 1.0,
        contrast: 1.0,
    }]
}

#[test]
fn blend_masked_lerps_orig_to_effected() {
    let orig = [0.0, 0.0, 0.0, 1.0];
    let effected = [1.0, 1.0, 1.0, 1.0];
    // Coverage 0 = original, 1 = effected, 0.5 = halfway, channel-wise.
    assert_eq!(blend_masked(orig, effected, 0.0), orig);
    assert_eq!(blend_masked(orig, effected, 1.0), effected);
    assert!(approx_rgb(blend_masked(orig, effected, 0.5), [0.5, 0.5, 0.5]));
    // Out-of-range coverage clamps.
    assert_eq!(blend_masked(orig, effected, 2.0), effected);
    assert_eq!(blend_masked(orig, effected, -1.0), orig);
}

#[test]
fn effect_mask_disabled_applies_everywhere() {
    // Default (disabled) mask: the effect applies in full at any point — exactly
    // the legacy unmasked behaviour.
    let mask = EffectMask::default();
    assert!(!mask.is_active());
    let stack = whiten_stack();
    let full = apply_effects(&stack, [0.0, 0.0, 0.0, 1.0]);
    let out = apply_effects_masked(&stack, &mask, &[], 12.0, 34.0, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(out, full);
}

#[test]
fn effect_mask_gates_inside_vs_outside() {
    // A 100x100 rect region centred at the origin (layer-local px), hard edge.
    let mut mask = EffectMask {
        enabled: true,
        region: Mask::rect(50.0, 50.0),
    };
    mask.region.feather = 0.0;
    assert!(mask.is_active());
    let poly = mask.region.flatten();
    let stack = whiten_stack();
    let black = [0.0, 0.0, 0.0, 1.0];
    let effected = apply_effects(&stack, black);

    // A point inside the region gets the full effect; a point outside is untouched.
    let inside = apply_effects_masked(&stack, &mask, &poly, 0.0, 0.0, black);
    let outside = apply_effects_masked(&stack, &mask, &poly, 200.0, 200.0, black);
    assert!(approx_rgb(inside, [effected[0], effected[1], effected[2]]));
    assert_eq!(outside, black);
}

#[test]
fn effect_mask_invert_flips_the_region() {
    let mut mask = EffectMask {
        enabled: true,
        region: Mask::rect(50.0, 50.0),
    };
    mask.region.feather = 0.0;
    mask.region.inverted = true;
    let poly = mask.region.flatten();
    let stack = whiten_stack();
    let black = [0.0, 0.0, 0.0, 1.0];
    let effected = apply_effects(&stack, black);

    // Inverted: inside is now untouched, outside gets the effect.
    let inside = apply_effects_masked(&stack, &mask, &poly, 0.0, 0.0, black);
    let outside = apply_effects_masked(&stack, &mask, &poly, 200.0, 200.0, black);
    assert_eq!(inside, black);
    assert!(approx_rgb(outside, [effected[0], effected[1], effected[2]]));
}

#[test]
fn effect_mask_feather_gives_intermediate_blend() {
    // A feathered edge ramps coverage across the boundary, so a point right on the
    // edge of the rect blends original↔effected ~halfway.
    let mut mask = EffectMask {
        enabled: true,
        region: Mask::rect(50.0, 50.0),
    };
    mask.region.feather = 40.0; // wide feather straddling the x=50 edge
    let poly = mask.region.flatten();
    let stack = whiten_stack();
    let black = [0.0, 0.0, 0.0, 1.0];

    // On the boundary the feather centres coverage at ~0.5 → mid-gray.
    let edge = apply_effects_masked(&stack, &mask, &poly, 50.0, 0.0, black);
    assert!(
        edge[0] > 0.1 && edge[0] < 0.9,
        "feathered edge should be a partial blend, got {edge:?}"
    );
    // Deep inside is full effect, far outside is untouched.
    let deep_in = apply_effects_masked(&stack, &mask, &poly, 0.0, 0.0, black);
    let far_out = apply_effects_masked(&stack, &mask, &poly, 300.0, 0.0, black);
    assert!(deep_in[0] > edge[0]);
    assert!(far_out[0] < edge[0]);
}

#[test]
fn effect_mask_serde_roundtrips_and_defaults() {
    // Round-trip a layer with an active effect mask.
    let mut layer = PulseLayer::new("L", [0.0, 0.0, 0.0, 1.0]);
    layer.effects.push(Effect::BrightnessContrast {
        brightness: 1.0,
        contrast: 1.0,
    });
    layer.effect_mask.enabled = true;
    layer.effect_mask.region = Mask::ellipse(40.0, 30.0);
    layer.effect_mask.region.feather = 12.0;
    layer.effect_mask.region.inverted = true;
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.effect_mask, layer.effect_mask);

    // A legacy file with no `effect_mask` field loads with the mask disabled, so
    // the effect applies everywhere (back-compat).
    let old = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let legacy: PulseLayer = serde_json::from_str(old).unwrap();
    assert!(!legacy.effect_mask.enabled);
    assert!(!legacy.effect_mask.is_active());
}

#[test]
fn preset_captures_and_applies_effect_mask() {
    let mut src = PulseLayer::new("Src", [0.0, 0.0, 0.0, 1.0]);
    src.effects.push(Effect::BrightnessContrast {
        brightness: 1.0,
        contrast: 1.0,
    });
    src.effect_mask.enabled = true;
    src.effect_mask.region = Mask::rect(20.0, 20.0);
    src.effect_mask.region.feather = 5.0;

    let preset = AnimationPreset::capture("p", &src);
    let mut dst = PulseLayer::new("Dst", [0.0, 0.0, 0.0, 1.0]);
    preset.apply(&mut dst);
    assert_eq!(dst.effect_mask, src.effect_mask);
}

// --- Hue / Saturation, Curves, Color Balance ----------------------------

#[test]
fn hsl_round_trips() {
    // RGB -> HSL -> RGB recovers the original across a spread of colors.
    for c in [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.5, 0.5, 0.5],
        [0.8, 0.2, 0.4],
        [0.1, 0.7, 0.3],
        [0.25, 0.4, 0.95],
    ] {
        let (h, s, l) = rgb_to_hsl(c[0], c[1], c[2]);
        let back = hsl_to_rgb(h, s, l);
        assert!(
            approx_rgb([back[0], back[1], back[2], 1.0], c),
            "round-trip failed for {c:?} -> ({h},{s},{l}) -> {back:?}"
        );
    }
}

#[test]
fn hue_saturation_identity_and_desaturate() {
    // Zeroed params are a no-op.
    let id = Effect::HueSaturation {
        hue: 0.0,
        saturation: 0.0,
        lightness: 0.0,
    };
    assert!(approx_rgb(id.apply([0.8, 0.2, 0.4, 1.0]), [0.8, 0.2, 0.4]));
    // Full desaturate (-1) collapses to gray (R==G==B at the pixel's luma-ish L).
    let gray = Effect::HueSaturation {
        hue: 0.0,
        saturation: -1.0,
        lightness: 0.0,
    };
    let out = gray.apply([0.8, 0.2, 0.4, 1.0]);
    assert!((out[0] - out[1]).abs() < 1e-4 && (out[1] - out[2]).abs() < 1e-4);
    // Alpha untouched.
    assert_eq!(gray.apply([0.8, 0.2, 0.4, 0.5])[3], 0.5);
}

#[test]
fn hue_rotation_120_cycles_channels() {
    // A pure-red pixel rotated +120° in hue becomes pure green (HSL hue wheel).
    let e = Effect::HueSaturation {
        hue: 120.0,
        saturation: 0.0,
        lightness: 0.0,
    };
    let out = e.apply([1.0, 0.0, 0.0, 1.0]);
    assert!(approx_rgb(out, [0.0, 1.0, 0.0]), "red+120 -> {out:?}");
}

#[test]
fn curves_identity_is_passthrough() {
    let id = Effect::Curves {
        points: Effect::CURVE_IDENTITY,
    };
    for v in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
        assert!(
            (id.apply([v, v, v, 1.0])[0] - v).abs() < 1e-4,
            "identity curve changed {v}"
        );
    }
}

#[test]
fn curve_eval_hits_control_points() {
    // The spline passes exactly through the five control points at 0,¼,½,¾,1.
    let pts = [0.1, 0.3, 0.4, 0.85, 0.95];
    for (i, &expect) in pts.iter().enumerate() {
        let x = i as f32 * 0.25;
        assert!(
            (curve_eval(&pts, x) - expect).abs() < 1e-4,
            "curve at {x} = {} want {expect}",
            curve_eval(&pts, x)
        );
    }
    // Out-of-range inputs clamp to the end points.
    assert!((curve_eval(&pts, -1.0) - 0.1).abs() < 1e-4);
    assert!((curve_eval(&pts, 2.0) - 0.95).abs() < 1e-4);
}

#[test]
fn curves_lift_brightens_midtones() {
    // Raise the midpoint output: a mid-gray input lands brighter, ends pinned.
    let lift = Effect::Curves {
        points: [0.0, 0.4, 0.7, 0.9, 1.0],
    };
    assert!(lift.apply([0.5, 0.5, 0.5, 1.0])[0] > 0.5);
    assert!((lift.apply([0.0, 0.0, 0.0, 1.0])[0]).abs() < 1e-4);
    assert!((lift.apply([1.0, 1.0, 1.0, 1.0])[0] - 1.0).abs() < 1e-4);
}

#[test]
fn smoothstep_endpoints_and_midpoint() {
    assert_eq!(smoothstep(0.0, 1.0, -0.5), 0.0);
    assert_eq!(smoothstep(0.0, 1.0, 1.5), 1.0);
    assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
    // Degenerate edges (e0 == e1) act as a hard step.
    assert_eq!(smoothstep(0.5, 0.5, 0.4), 0.0);
    assert_eq!(smoothstep(0.5, 0.5, 0.6), 1.0);
}

#[test]
fn color_balance_zero_is_passthrough() {
    let id = Effect::ColorBalance {
        shadows: [0.0; 3],
        midtones: [0.0; 3],
        highlights: [0.0; 3],
    };
    assert!(approx_rgb(id.apply([0.2, 0.5, 0.8, 1.0]), [0.2, 0.5, 0.8]));
    assert_eq!(id.apply([0.2, 0.5, 0.8, 0.6])[3], 0.6);
}

#[test]
fn color_balance_pushes_target_range() {
    // A highlight red push reddens a bright pixel far more than a dark one.
    let e = Effect::ColorBalance {
        shadows: [0.0; 3],
        midtones: [0.0; 3],
        highlights: [1.0, 0.0, 0.0],
    };
    let bright = e.apply([0.9, 0.9, 0.9, 1.0]);
    let dark = e.apply([0.1, 0.1, 0.1, 1.0]);
    let bright_gain = bright[0] - 0.9;
    let dark_gain = dark[0] - 0.1;
    assert!(
        bright_gain > dark_gain,
        "highlight push should weight brights: bright +{bright_gain}, dark +{dark_gain}"
    );
    // The push only moves red here; green/blue at the bright pixel are ~unchanged.
    assert!((bright[1] - 0.9).abs() < 1e-3 && (bright[2] - 0.9).abs() < 1e-3);
}

#[test]
fn new_effects_preserve_alpha() {
    // Every default effect (including the three new ones) leaves alpha intact.
    let px = [0.4, 0.55, 0.7, 0.42];
    for e in Effect::defaults() {
        assert_eq!(e.apply(px)[3], 0.42, "{} changed alpha", e.label());
    }
}

// --- Channel Mixer, Gradient Map, Tritone -------------------------------

#[test]
fn channel_mixer_identity_is_passthrough() {
    // The default channel mixer (each output = its own input) is a no-op.
    let id = Effect::ChannelMixer {
        red: [1.0, 0.0, 0.0, 0.0],
        green: [0.0, 1.0, 0.0, 0.0],
        blue: [0.0, 0.0, 1.0, 0.0],
        monochrome: false,
    };
    assert!(approx_rgb(id.apply([0.2, 0.5, 0.8, 1.0]), [0.2, 0.5, 0.8]));
    assert_eq!(id.apply([0.2, 0.5, 0.8, 0.3])[3], 0.3);
}

#[test]
fn channel_mixer_swaps_red_from_blue() {
    // Output red sourced entirely from input blue (R←B); green/blue unchanged.
    let swap = Effect::ChannelMixer {
        red: [0.0, 0.0, 1.0, 0.0],
        green: [0.0, 1.0, 0.0, 0.0],
        blue: [0.0, 0.0, 1.0, 0.0],
        monochrome: false,
    };
    let out = swap.apply([0.1, 0.4, 0.9, 1.0]);
    assert!(approx_rgb(out, [0.9, 0.4, 0.9]), "R<-B failed: {out:?}");
}

#[test]
fn channel_mixer_constant_and_clamp() {
    // A +0.5 constant lifts the channel; output stays clamped to [0,1].
    let lift = Effect::ChannelMixer {
        red: [1.0, 0.0, 0.0, 0.5],
        green: [0.0, 1.0, 0.0, 0.0],
        blue: [0.0, 0.0, 1.0, 0.0],
        monochrome: false,
    };
    assert!((lift.apply([0.2, 0.2, 0.2, 1.0])[0] - 0.7).abs() < 1e-4);
    // 0.8 + 0.5 = 1.3 -> clamped to 1.0.
    assert_eq!(lift.apply([0.8, 0.2, 0.2, 1.0])[0], 1.0);
}

#[test]
fn channel_mixer_monochrome_writes_gray_from_red_row() {
    // Monochrome collapses every output to the red row's weighted gray.
    let mono = Effect::ChannelMixer {
        red: [0.3, 0.59, 0.11, 0.0], // luma-ish weights
        green: [0.0, 1.0, 0.0, 0.0], // ignored when monochrome
        blue: [0.0, 0.0, 1.0, 0.0],  // ignored when monochrome
        monochrome: true,
    };
    let out = mono.apply([1.0, 0.0, 0.0, 1.0]);
    assert!((out[0] - out[1]).abs() < 1e-6 && (out[1] - out[2]).abs() < 1e-6);
    assert!((out[0] - 0.3).abs() < 1e-4, "gray = {}", out[0]);
}

#[test]
fn channel_mixer_matches_shared_prism_core_math() {
    // Pulse must defer to the shared prism_core ChannelMixerMatrix — assert the
    // result is bit-identical to calling the shared math directly (no reimpl).
    let red = [0.4, 0.2, 0.1, 0.05];
    let green = [0.1, 0.7, 0.2, 0.0];
    let blue = [0.0, 0.3, 0.6, -0.1];
    let e = Effect::ChannelMixer {
        red,
        green,
        blue,
        monochrome: false,
    };
    let px = [0.35, 0.6, 0.8];
    let shared = prism_core::adjust::ChannelMixerMatrix {
        r: red,
        g: green,
        b: blue,
        monochrome: false,
    }
    .apply(px);
    let out = e.apply([px[0], px[1], px[2], 1.0]);
    assert_eq!([out[0], out[1], out[2]], shared);
}

#[test]
fn gradient_map_black_white_mid() {
    // Map black->first stop, white->last stop, mid-gray->mid stop.
    let e = Effect::GradientMap {
        low: [0.0, 0.0, 1.0],  // blue shadows
        mid: [0.0, 1.0, 0.0],  // green mids
        high: [1.0, 0.0, 0.0], // red highlights
        amount: 1.0,
    };
    assert!(approx_rgb(e.apply([0.0, 0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]));
    assert!(approx_rgb(e.apply([1.0, 1.0, 1.0, 1.0]), [1.0, 0.0, 0.0]));
    // Pure gray at luma 0.5 lands on the mid stop.
    let mid = e.apply([0.5, 0.5, 0.5, 1.0]);
    assert!(approx_rgb(mid, [0.0, 1.0, 0.0]), "mid = {mid:?}");
}

#[test]
fn gradient_map_amount_zero_is_passthrough() {
    let e = Effect::GradientMap {
        low: [0.0, 0.0, 1.0],
        mid: [0.0, 1.0, 0.0],
        high: [1.0, 0.0, 0.0],
        amount: 0.0,
    };
    assert!(approx_rgb(e.apply([0.2, 0.5, 0.8, 1.0]), [0.2, 0.5, 0.8]));
    assert_eq!(e.apply([0.2, 0.5, 0.8, 0.7])[3], 0.7);
}

#[test]
fn gradient_map_interpolates_between_stops() {
    // A grayscale identity gradient (black/gray/white) maps luma->luma; a value
    // a quarter of the way up should read ~that luma on all channels.
    let e = Effect::GradientMap {
        low: [0.0, 0.0, 0.0],
        mid: [0.5, 0.5, 0.5],
        high: [1.0, 1.0, 1.0],
        amount: 1.0,
    };
    let out = e.apply([0.25, 0.25, 0.25, 1.0]);
    assert!((out[0] - 0.25).abs() < 1e-3, "out = {out:?}");
    assert!((out[0] - out[1]).abs() < 1e-6 && (out[1] - out[2]).abs() < 1e-6);
}

#[test]
fn tritone_maps_three_tones_by_luma() {
    // Tritone shares the gradient-map primitive: dark->shadows, mid->midtones,
    // bright->highlights.
    let e = Effect::Tritone {
        shadows: [0.1, 0.0, 0.3],
        midtones: [0.6, 0.4, 0.2],
        highlights: [1.0, 0.95, 0.8],
        amount: 1.0,
    };
    assert!(approx_rgb(e.apply([0.0, 0.0, 0.0, 1.0]), [0.1, 0.0, 0.3]));
    assert!(approx_rgb(e.apply([0.5, 0.5, 0.5, 1.0]), [0.6, 0.4, 0.2]));
    assert!(approx_rgb(e.apply([1.0, 1.0, 1.0, 1.0]), [1.0, 0.95, 0.8]));
    // Alpha untouched.
    assert_eq!(e.apply([0.5, 0.5, 0.5, 0.4])[3], 0.4);
}

#[test]
fn color_effects_are_deterministic() {
    // The new color effects are pure: identical inputs yield identical outputs.
    let px = [0.33, 0.61, 0.27, 0.9];
    for e in [
        Effect::defaults()[7], // Channel Mixer
        Effect::defaults()[8], // Gradient Map
        Effect::defaults()[9], // Tritone
    ] {
        assert_eq!(e.apply(px), e.apply(px), "{} not deterministic", e.label());
    }
}

// --- Track mattes -------------------------------------------------------

#[test]
fn matte_none_is_passthrough() {
    // No matte: factor is always 1 regardless of the source pixel.
    for px in [[0.0; 4], [1.0; 4], [0.3, 0.6, 0.9, 0.5]] {
        assert_eq!(MatteMode::None.factor(px), 1.0);
    }
    assert!(!MatteMode::None.is_active());
    assert!(MatteMode::Alpha.is_active());
}

#[test]
fn alpha_matte_reads_source_alpha() {
    // Color is irrelevant to an alpha matte; only the source alpha matters.
    assert_eq!(MatteMode::Alpha.factor([0.9, 0.1, 0.4, 1.0]), 1.0);
    assert_eq!(MatteMode::Alpha.factor([0.9, 0.1, 0.4, 0.0]), 0.0);
    assert!((MatteMode::Alpha.factor([0.0, 0.0, 0.0, 0.25]) - 0.25).abs() < 1e-6);
    // Inverted alpha is 1 - alpha.
    assert_eq!(MatteMode::AlphaInverted.factor([1.0, 1.0, 1.0, 1.0]), 0.0);
    assert_eq!(MatteMode::AlphaInverted.factor([1.0, 1.0, 1.0, 0.0]), 1.0);
}

#[test]
fn luma_matte_reads_weighted_brightness() {
    // Opaque white -> luma ~1; opaque black -> 0.
    assert!((MatteMode::Luma.factor([1.0, 1.0, 1.0, 1.0]) - 1.0).abs() < 1e-5);
    assert_eq!(MatteMode::Luma.factor([0.0, 0.0, 0.0, 1.0]), 0.0);
    // Green carries the most luma weight (Rec.709), blue the least.
    let g = MatteMode::Luma.factor([0.0, 1.0, 0.0, 1.0]);
    let b = MatteMode::Luma.factor([0.0, 0.0, 1.0, 1.0]);
    assert!(g > b, "green luma {g} should exceed blue luma {b}");
    // A transparent bright pixel mattes to ~0 (luma is weighted by alpha).
    assert_eq!(MatteMode::Luma.factor([1.0, 1.0, 1.0, 0.0]), 0.0);
    // Inverted luma flips a bright source to ~0.
    assert!(MatteMode::LumaInverted.factor([1.0, 1.0, 1.0, 1.0]) < 1e-5);
    assert!((MatteMode::LumaInverted.factor([0.0, 0.0, 0.0, 1.0]) - 1.0).abs() < 1e-5);
}

#[test]
fn matte_factor_is_clamped() {
    // Out-of-gamut source values can't push the factor past [0,1].
    assert_eq!(MatteMode::Luma.factor([5.0, 5.0, 5.0, 2.0]), 1.0);
    assert_eq!(MatteMode::AlphaInverted.factor([0.0, 0.0, 0.0, -1.0]), 1.0);
}

#[test]
fn matte_source_is_layer_above_when_active() {
    let mut c = parented_comp(); // layers: 0 (parent), 1 (child)
                                 // Layer 0 with an active matte borrows layer 1 (the one above it).
    c.layers[0].matte = MatteMode::Alpha;
    assert_eq!(c.matte_source(0), Some(1));
    // The top layer has nothing above to borrow -> no source.
    c.layers[1].matte = MatteMode::Luma;
    assert_eq!(c.matte_source(1), None);
    // Without an active matte there is no source even if a layer is above.
    c.layers[0].matte = MatteMode::None;
    assert_eq!(c.matte_source(0), None);
}

#[test]
fn is_matte_source_tracks_layer_below() {
    let mut c = parented_comp(); // 0, 1
                                 // Layer 0 mattes off layer 1 -> layer 1 is a matte source, layer 0 isn't.
    c.layers[0].matte = MatteMode::Alpha;
    assert!(c.is_matte_source(1));
    assert!(!c.is_matte_source(0));
    // Turning the matte off un-consumes layer 1.
    c.layers[0].matte = MatteMode::None;
    assert!(!c.is_matte_source(1));
}

#[test]
fn matte_serde_defaults_to_none() {
    // Pre-matte layers (no `matte` field) load as un-matted.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.matte, MatteMode::None);
}

#[test]
fn parent_serde_defaults_to_none() {
    // Pre-parenting layers (no `parent`/anchor fields) load as unparented.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.parent, None);
    assert!(layer.anchor_x.keys.is_empty());
    assert!(layer.anchor_y.keys.is_empty());
}

// --- Motion blur --------------------------------------------------------

#[test]
fn motion_blur_defaults_match_ae() {
    let mb = MotionBlur::default();
    assert!(!mb.enabled); // off until opted in
    assert_eq!(mb.angle, 180.0); // cinematic half-frame shutter
    assert_eq!(mb.phase, 0.0);
    assert_eq!(mb.samples, 16);
}

#[test]
fn shutter_window_width_tracks_angle() {
    let fps = 25.0; // 1 frame = 0.04 s
                    // 360° opens the shutter for a whole frame; 180° for half.
    let full = MotionBlur {
        angle: 360.0,
        ..Default::default()
    };
    let (o, c) = full.shutter_window(1.0, fps);
    assert!((o - 1.0).abs() < 1e-6); // phase 0 opens at t
    assert!((c - o - 0.04).abs() < 1e-6); // width == one frame

    let half = MotionBlur {
        angle: 180.0,
        ..Default::default()
    };
    let (o, c) = half.shutter_window(1.0, fps);
    assert!((c - o - 0.02).abs() < 1e-6); // width == half a frame
}

#[test]
fn shutter_phase_shifts_window() {
    let fps = 50.0; // 1 frame = 0.02 s
                    // phase = -angle/2 centers the window on the frame time.
    let mb = MotionBlur {
        angle: 180.0,
        phase: -90.0,
        ..Default::default()
    };
    let (o, c) = mb.shutter_window(2.0, fps);
    let mid = 0.5 * (o + c);
    assert!((mid - 2.0).abs() < 1e-6, "window not centered: mid={mid}");
}

#[test]
fn sample_times_span_window_and_count() {
    let fps = 30.0;
    let mb = MotionBlur {
        angle: 360.0,
        samples: 8,
        ..Default::default()
    };
    let times = mb.sample_times(0.5, fps);
    assert_eq!(times.len(), 8);
    let (open, close) = mb.shutter_window(0.5, fps);
    // Every sample lands strictly inside the open window, ascending.
    for w in times.windows(2) {
        assert!(w[0] < w[1]);
    }
    assert!(*times.first().unwrap() > open);
    assert!(*times.last().unwrap() < close);
    // Midpoint sampling is symmetric about the window center.
    let mid = 0.5 * (open + close);
    let first_off = mid - times.first().unwrap();
    let last_off = times.last().unwrap() - mid;
    assert!((first_off - last_off).abs() < 1e-5);
}

#[test]
fn single_sample_lands_at_window_center() {
    let mb = MotionBlur {
        samples: 1,
        angle: 200.0,
        phase: 30.0,
        ..Default::default()
    };
    let times = mb.sample_times(1.0, 24.0);
    assert_eq!(times.len(), 1);
    let (open, close) = mb.shutter_window(1.0, 24.0);
    assert!((times[0] - 0.5 * (open + close)).abs() < 1e-6);
}

#[test]
fn sample_times_clamp_count_into_range() {
    // 0 samples degrades to 1; absurd counts clamp to 64.
    let zero = MotionBlur {
        samples: 0,
        ..Default::default()
    };
    assert_eq!(zero.sample_times(0.0, 30.0).len(), 1);
    let huge = MotionBlur {
        samples: 9999,
        ..Default::default()
    };
    assert_eq!(huge.sample_times(0.0, 30.0).len(), 64);
}

#[test]
fn layer_motion_blurred_needs_both_switches() {
    let mut c = parented_comp();
    c.layers[0].motion_blur = true;
    // Comp master off -> no layer is blurred even if its flag is on.
    c.motion_blur.enabled = false;
    assert!(!c.layer_motion_blurred(0));
    // Master on, layer flag on -> blurred.
    c.motion_blur.enabled = true;
    assert!(c.layer_motion_blurred(0));
    // Master on but the layer opted out -> not blurred.
    assert!(!c.layer_motion_blurred(1));
    // Out-of-range index is never blurred.
    assert!(!c.layer_motion_blurred(99));
}

#[test]
fn motion_blur_serde_defaults_off() {
    // A pre-motion-blur comp (no `motion_blur` field) loads with MB off and a
    // layer without the flag loads un-blurred.
    let json = r#"{"width":16,"height":16,"duration":1.0,"fps":30.0,
        "layers":[{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}]}"#;
    let comp: Comp = serde_json::from_str(json).unwrap();
    assert!(!comp.motion_blur.enabled);
    assert_eq!(comp.motion_blur.angle, 180.0);
    assert!(!comp.layers[0].motion_blur);
    assert!(!comp.layer_motion_blurred(0));
}

// --- Masks --------------------------------------------------------------

#[test]
fn point_in_polygon_square() {
    // Unit square centered at origin.
    let sq = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    assert!(point_in_polygon(&sq, 0.0, 0.0)); // center inside
    assert!(point_in_polygon(&sq, 0.9, -0.9)); // near a corner, inside
    assert!(!point_in_polygon(&sq, 2.0, 0.0)); // right of the square
    assert!(!point_in_polygon(&sq, 0.0, -5.0)); // below
                                                // Degenerate polygons are never "inside".
    assert!(!point_in_polygon(&[(0.0, 0.0), (1.0, 0.0)], 0.5, 0.0));
}

#[test]
fn point_in_polygon_concave() {
    // An arrow/chevron concave shape: a notch cut into the right side.
    let poly = [(0.0, 0.0), (4.0, 0.0), (2.0, 2.0), (4.0, 4.0), (0.0, 4.0)];
    assert!(point_in_polygon(&poly, 1.0, 2.0)); // left bulk: inside
                                                // A point inside the notch (right of the chevron tip) is outside.
    assert!(!point_in_polygon(&poly, 3.5, 2.0));
}

#[test]
fn dist_to_polygon_is_zero_on_edge_and_grows_outside() {
    let sq = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    // On the right edge -> ~0 distance to boundary.
    assert!(dist_to_polygon(&sq, 1.0, 0.0) < 1e-4);
    // 1 unit right of the edge -> distance ~1.
    assert!((dist_to_polygon(&sq, 2.0, 0.0) - 1.0).abs() < 1e-4);
    // Inside, 1 unit from the nearest (right) edge -> distance ~1.
    assert!((dist_to_polygon(&sq, 0.0, 0.0) - 1.0).abs() < 1e-4);
}

#[test]
fn mask_rect_hard_coverage_is_binary() {
    let m = Mask::rect(10.0, 10.0);
    let poly = m.flatten();
    assert_eq!(poly.len(), 4); // four straight segments -> four points
    assert!((m.coverage_at(&poly, 0.0, 0.0) - 1.0).abs() < 1e-5); // inside
    assert_eq!(m.coverage_at(&poly, 50.0, 0.0), 0.0); // outside
}

#[test]
fn mask_feather_ramps_across_the_edge() {
    let mut m = Mask::rect(10.0, 10.0);
    m.feather = 4.0; // ramp over ±2 px around the edge
    let poly = m.flatten();
    // Exactly on the right edge -> half coverage.
    let on_edge = m.coverage_at(&poly, 10.0, 0.0);
    assert!((on_edge - 0.5).abs() < 1e-4, "edge cov {on_edge}");
    // Well inside -> full; well outside -> none.
    assert!((m.coverage_at(&poly, 0.0, 0.0) - 1.0).abs() < 1e-5);
    assert_eq!(m.coverage_at(&poly, 20.0, 0.0), 0.0);
}

#[test]
fn mask_inversion_complements_coverage() {
    let mut m = Mask::rect(10.0, 10.0);
    m.inverted = true;
    let poly = m.flatten();
    assert_eq!(m.coverage_at(&poly, 0.0, 0.0), 0.0); // inside -> hidden
    assert!((m.coverage_at(&poly, 50.0, 0.0) - 1.0).abs() < 1e-5); // outside -> shown
}

#[test]
fn mask_expansion_grows_and_shrinks() {
    let m_base = Mask::rect(10.0, 10.0);
    let poly = m_base.flatten();
    // A point 5 px outside the right edge is normally uncovered...
    assert_eq!(m_base.coverage_at(&poly, 15.0, 0.0), 0.0);
    // ...but +8 px expansion pulls the boundary out past it.
    let mut grown = m_base.clone();
    grown.expansion = 8.0;
    assert!((grown.coverage_at(&poly, 15.0, 0.0) - 1.0).abs() < 1e-5);
    // Negative expansion contracts: a point just inside is knocked out.
    let mut shrunk = m_base.clone();
    shrunk.expansion = -8.0;
    assert_eq!(shrunk.coverage_at(&poly, 5.0, 0.0), 0.0);
}

#[test]
fn mask_opacity_scales_coverage() {
    let mut m = Mask::rect(10.0, 10.0);
    m.opacity = 0.5;
    let poly = m.flatten();
    assert!((m.coverage_at(&poly, 0.0, 0.0) - 0.5).abs() < 1e-5);
}

#[test]
fn mask_ellipse_is_smooth_and_inside_out() {
    let m = Mask::ellipse(10.0, 10.0);
    let poly = m.flatten();
    // Flattening a 4-segment Bézier oval yields many points.
    assert!(poly.len() > 16);
    // Center inside; a point on the bounding-box corner (outside the oval)
    // is uncovered.
    assert!((m.coverage_at(&poly, 0.0, 0.0) - 1.0).abs() < 1e-5);
    assert_eq!(m.coverage_at(&poly, 9.5, 9.5), 0.0);
    // A point near the right vertex (on-axis) is inside.
    assert!(m.coverage_at(&poly, 8.0, 0.0) > 0.5);
}

#[test]
fn mask_modes_combine_as_expected() {
    // Add unions; against an empty base it reveals exactly the shape.
    assert!((MaskMode::Add.combine(0.0, 1.0) - 1.0).abs() < 1e-6);
    assert!((MaskMode::Add.combine(0.5, 1.0) - 1.0).abs() < 1e-6);
    // Subtract knocks out.
    assert!((MaskMode::Subtract.combine(1.0, 1.0)).abs() < 1e-6);
    assert!((MaskMode::Subtract.combine(1.0, 0.0) - 1.0).abs() < 1e-6);
    // Intersect keeps the overlap.
    assert!((MaskMode::Intersect.combine(1.0, 1.0) - 1.0).abs() < 1e-6);
    assert!((MaskMode::Intersect.combine(1.0, 0.0)).abs() < 1e-6);
    // Difference is the symmetric difference.
    assert!((MaskMode::Difference.combine(1.0, 1.0)).abs() < 1e-6);
    assert!((MaskMode::Difference.combine(1.0, 0.0) - 1.0).abs() < 1e-6);
    // None passes the accumulator through untouched.
    assert!((MaskMode::None.combine(0.7, 1.0) - 0.7).abs() < 1e-6);
}

#[test]
fn mask_stack_no_active_masks_is_full_coverage() {
    // No masks -> unmasked layer (full coverage sentinel).
    assert_eq!(mask_stack_coverage(&[], &[], 0.0, 0.0), 1.0);
    // A single disabled (None) mask is still "no active masks".
    let mut m = Mask::rect(10.0, 10.0);
    m.mode = MaskMode::None;
    let polys = vec![m.flatten()];
    assert_eq!(mask_stack_coverage(&[m], &polys, 0.0, 0.0), 1.0);
}

#[test]
fn mask_stack_add_then_subtract() {
    // A big Add rectangle with a smaller Subtract rectangle punched out.
    let add = Mask::rect(20.0, 20.0);
    let mut sub = Mask::rect(5.0, 5.0);
    sub.mode = MaskMode::Subtract;
    let masks = vec![add, sub];
    let polys: Vec<_> = masks.iter().map(Mask::flatten).collect();
    // Inside the big rect but outside the hole -> covered.
    assert!((mask_stack_coverage(&masks, &polys, 12.0, 0.0) - 1.0).abs() < 1e-5);
    // Inside the punched hole -> knocked out.
    assert_eq!(mask_stack_coverage(&masks, &polys, 0.0, 0.0), 0.0);
    // Fully outside everything -> uncovered.
    assert_eq!(mask_stack_coverage(&masks, &polys, 50.0, 0.0), 0.0);
}

#[test]
fn mask_is_active_needs_three_verts_and_a_mode() {
    let mut m = Mask::rect(10.0, 10.0);
    assert!(m.is_active());
    m.mode = MaskMode::None;
    assert!(!m.is_active());
    m.mode = MaskMode::Add;
    m.vertices.truncate(2); // only 2 verts -> no area
    assert!(!m.is_active());
}

#[test]
fn masks_serde_defaults_to_empty() {
    // Pre-mask layers (no `masks` field) load unmasked.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.masks.is_empty());
    assert!(!layer.has_active_masks());
}

// --- Spatial effects (Gaussian Blur / Drop Shadow / Glow) ---------------

/// A `w×h` premultiplied buffer with a single fully-opaque white pixel at
/// `(cx, cy)` (a unit impulse) and transparent everywhere else.
fn impulse(w: usize, h: usize, cx: usize, cy: usize) -> Vec<[f32; 4]> {
    let mut buf = vec![[0.0f32; 4]; w * h];
    buf[cy * w + cx] = [1.0, 1.0, 1.0, 1.0];
    buf
}

#[test]
fn gaussian_kernel_is_normalized_and_symmetric() {
    let k = gaussian_kernel(2.0);
    assert!(k.len() % 2 == 1, "kernel must be odd-length");
    let sum: f32 = k.iter().sum();
    assert!((sum - 1.0).abs() < 1e-5, "kernel sums to {sum}");
    // Symmetric about the center.
    let n = k.len();
    for i in 0..n / 2 {
        assert!((k[i] - k[n - 1 - i]).abs() < 1e-6, "asymmetric at {i}");
    }
    // The center weight is the largest.
    assert!(k[n / 2] >= k[0]);
}

#[test]
fn gaussian_kernel_zero_sigma_is_identity() {
    assert_eq!(gaussian_kernel(0.0), vec![1.0]);
    assert_eq!(gaussian_kernel(-3.0), vec![1.0]);
}

#[test]
fn gaussian_blur_conserves_alpha_mass() {
    // A blur redistributes coverage but, with no edge loss for a centered
    // impulse well inside the buffer, conserves total alpha.
    let mut buf = impulse(21, 21, 10, 10);
    let before: f32 = buf.iter().map(|p| p[3]).sum();
    gaussian_blur(&mut buf, 21, 21, 3.0, 3.0, false);
    let after: f32 = buf.iter().map(|p| p[3]).sum();
    assert!((before - after).abs() < 1e-3, "{before} vs {after}");
    // The center is no longer a hard 1.0 (energy spread to neighbours)...
    assert!(buf[10 * 21 + 10][3] < 1.0);
    // ...and a neighbour now carries some coverage.
    assert!(buf[10 * 21 + 11][3] > 0.0);
}

#[test]
fn gaussian_blur_zero_sigma_is_noop() {
    let mut buf = impulse(9, 9, 4, 4);
    let orig = buf.clone();
    gaussian_blur(&mut buf, 9, 9, 0.0, 0.0, false);
    assert_eq!(buf, orig);
}

#[test]
fn gaussian_blur_premultiplied_no_color_bleed() {
    // A blurred opaque white impulse stays white where it has coverage: in
    // premultiplied space color == rgb/alpha must remain ~white (no bleed
    // toward black from the transparent neighbours).
    let mut buf = impulse(21, 21, 10, 10);
    gaussian_blur(&mut buf, 21, 21, 2.0, 2.0, false);
    let p = buf[10 * 21 + 11];
    assert!(p[3] > 0.0);
    let (r, g, b) = (p[0] / p[3], p[1] / p[3], p[2] / p[3]);
    assert!(
        (r - 1.0).abs() < 1e-3 && (g - 1.0).abs() < 1e-3 && (b - 1.0).abs() < 1e-3,
        "color bled: {r},{g},{b}"
    );
}

#[test]
fn drop_shadow_offsets_coverage_behind_the_layer() {
    // A small opaque square; a 0-softness shadow offset right/down should put
    // shadow coverage where the layer is transparent, down-right of it.
    let w = 32;
    let mut buf = vec![[0.0f32; 4]; w * w];
    for y in 12..16 {
        for x in 12..16 {
            buf[y * w + x] = [1.0, 1.0, 1.0, 1.0];
        }
    }
    SpatialEffect::DropShadow {
        color: [0.0, 0.0, 0.0],
        opacity: 1.0,
        angle: 0.0, // straight right (+x)
        distance: 6.0,
        softness: 0.0,
        shadow_only: false,
    }
    .apply(&mut buf, w, w);
    // The layer pixel is still opaque white (shadow is behind it).
    let layer_px = buf[13 * w + 13];
    assert!((layer_px[0] / layer_px[3] - 1.0).abs() < 1e-3);
    // A pixel 6px right of the square (previously transparent) now carries
    // dark shadow coverage.
    let shadow_px = buf[13 * w + 19];
    assert!(shadow_px[3] > 0.5, "shadow alpha {}", shadow_px[3]);
    // It's dark (black tint) — rgb ~0 in premultiplied space.
    assert!(shadow_px[0] < 0.05 && shadow_px[1] < 0.05 && shadow_px[2] < 0.05);
}

#[test]
fn drop_shadow_shadow_only_drops_the_layer() {
    let w = 24;
    let mut buf = vec![[0.0f32; 4]; w * w];
    buf[10 * w + 10] = [1.0, 1.0, 1.0, 1.0];
    SpatialEffect::DropShadow {
        color: [0.0, 0.0, 0.0],
        opacity: 1.0,
        angle: 0.0,
        distance: 4.0,
        softness: 0.0,
        shadow_only: true,
    }
    .apply(&mut buf, w, w);
    // The original layer pixel is gone (replaced by shadow buffer, which is
    // transparent there since the shadow moved right).
    assert_eq!(buf[10 * w + 10][3], 0.0, "layer should be dropped");
    // The shadow lives 4px to the right.
    assert!(buf[10 * w + 14][3] > 0.5, "shadow present at the offset");
}

#[test]
fn glow_brightens_a_bright_region() {
    // A bright (but sub-white) opaque blob; glow should screen a bloom over
    // it, raising its luminance toward white, and extend coverage outward.
    let w = 32;
    let mut buf = vec![[0.0f32; 4]; w * w];
    for y in 13..19 {
        for x in 13..19 {
            buf[y * w + x] = [0.9, 0.9, 0.9, 1.0];
        }
    }
    let before = buf[15 * w + 15][0];
    SpatialEffect::Glow {
        threshold: 0.5,
        radius: 4.0,
        intensity: 2.0,
    }
    .apply(&mut buf, w, w);
    // The center got brighter (bloom screened on top).
    assert!(buf[15 * w + 15][0] > before, "glow should brighten");
    // The glow bled outside the original blob (a previously-empty pixel near
    // the edge now has some coverage).
    assert!(
        buf[15 * w + 21][3] > 0.0,
        "glow should extend past the edge"
    );
}

#[test]
fn glow_below_threshold_is_inert() {
    // A dim blob below the threshold produces no bloom -> buffer unchanged.
    let w = 16;
    let mut buf = vec![[0.0f32; 4]; w * w];
    for y in 6..10 {
        for x in 6..10 {
            buf[y * w + x] = [0.2, 0.2, 0.2, 1.0];
        }
    }
    let orig = buf.clone();
    SpatialEffect::Glow {
        threshold: 0.8,
        radius: 4.0,
        intensity: 2.0,
    }
    .apply(&mut buf, w, w);
    assert_eq!(buf, orig, "below-threshold glow must be inert");
}

#[test]
fn spatial_effect_apply_ignores_empty_buffer() {
    // Degenerate sizes are a no-op (no panic).
    let mut empty: Vec<[f32; 4]> = Vec::new();
    SpatialEffect::GaussianBlur {
        sigma_x: 3.0,
        sigma_y: 3.0,
        repeat_edge: false,
    }
    .apply(&mut empty, 0, 0);
    assert!(empty.is_empty());
}

#[test]
fn apply_spatial_effects_stacks_in_order() {
    // Two blurs spread more than one: stacking runs both passes.
    let mut one = impulse(21, 21, 10, 10);
    gaussian_blur(&mut one, 21, 21, 2.0, 2.0, false);
    let mut two = impulse(21, 21, 10, 10);
    apply_spatial_effects(
        &[
            SpatialEffect::GaussianBlur {
                sigma_x: 2.0,
                sigma_y: 2.0,
                repeat_edge: false,
            },
            SpatialEffect::GaussianBlur {
                sigma_x: 2.0,
                sigma_y: 2.0,
                repeat_edge: false,
            },
        ],
        &mut two,
        21,
        21,
    );
    // The twice-blurred center is lower (more spread) than the once-blurred.
    assert!(two[10 * 21 + 10][3] < one[10 * 21 + 10][3]);
}

// --- Box / Directional / Radial blur ------------------------------------

/// A `w×h` premultiplied buffer with a fully-opaque white square centred at
/// `(cx, cy)` of half-extent `r`, transparent elsewhere.
fn square(w: usize, h: usize, cx: usize, cy: usize, r: usize) -> Vec<[f32; 4]> {
    let mut buf = vec![[0.0f32; 4]; w * h];
    for y in cy.saturating_sub(r)..=(cy + r).min(h - 1) {
        for x in cx.saturating_sub(r)..=(cx + r).min(w - 1) {
            buf[y * w + x] = [1.0, 1.0, 1.0, 1.0];
        }
    }
    buf
}

#[test]
fn spatial_label_and_defaults_are_consistent() {
    // Defaults are one per variant, Blur family first, labels stable.
    let d = SpatialEffect::defaults();
    assert_eq!(d.len(), 6);
    assert_eq!(d[0].label(), "Gaussian Blur");
    assert_eq!(d[1].label(), "Box Blur");
    assert_eq!(d[2].label(), "Directional Blur");
    assert_eq!(d[3].label(), "Radial Blur");
    assert_eq!(d[4].label(), "Drop Shadow");
    assert_eq!(d[5].label(), "Glow");
}

#[test]
fn box_blur_conserves_alpha_mass() {
    // A box blur spreads coverage but conserves total alpha for an impulse well
    // inside the buffer (no edge loss).
    let mut buf = impulse(21, 21, 10, 10);
    let before: f32 = buf.iter().map(|p| p[3]).sum();
    box_blur(&mut buf, 21, 21, 3.0, 1, false);
    let after: f32 = buf.iter().map(|p| p[3]).sum();
    assert!((before - after).abs() < 1e-3, "{before} vs {after}");
    // The center spread to its neighbours.
    assert!(buf[10 * 21 + 10][3] < 1.0);
    assert!(buf[10 * 21 + 11][3] > 0.0);
}

#[test]
fn box_blur_one_pass_is_a_flat_average() {
    // A single box pass over an impulse is a uniform average: every covered pixel
    // in the box window carries the *same* coverage (1 / window_area), unlike a
    // Gaussian whose centre is the peak.
    let mut buf = impulse(21, 21, 10, 10);
    box_blur(&mut buf, 21, 21, 2.0, 1, false);
    // Separable 2-pass box of radius 2 → 5×5 flat window: each pixel = 1/25.
    let expect = 1.0 / 25.0;
    for (dy, dx) in [(0, 0), (1, 0), (0, 2), (-2, -1)] {
        let v = buf[(10 + dy) as usize * 21 + (10 + dx) as usize][3];
        assert!((v - expect).abs() < 1e-5, "flat box tap {dx},{dy} = {v}");
    }
}

#[test]
fn box_blur_iterations_smooth_toward_gaussian() {
    // More box iterations spread the impulse wider (the central peak drops): the
    // central-limit smoothing. 3 passes is lower-peaked than 1.
    let mut one = impulse(31, 31, 15, 15);
    box_blur(&mut one, 31, 31, 3.0, 1, false);
    let mut three = impulse(31, 31, 15, 15);
    box_blur(&mut three, 31, 31, 3.0, 3, false);
    assert!(
        three[15 * 31 + 15][3] < one[15 * 31 + 15][3],
        "3 box passes should be smoother (lower peak) than 1"
    );
    // Alpha mass is still conserved across iterations (impulse stays interior).
    let mass: f32 = three.iter().map(|p| p[3]).sum();
    assert!((mass - 1.0).abs() < 1e-2, "box iterations conserve mass: {mass}");
}

#[test]
fn box_blur_separable_matches_2d_average() {
    // A single separable box pass (H then V) equals a true 2-D mean over the
    // (2r+1)² window. Verify on an interior pixel of a constant patch where the
    // window is fully covered (so the average is exact and obvious).
    let w = 16;
    let mut buf = square(w, w, 8, 8, 4); // opaque 9×9 block centred at (8,8)
    box_blur(&mut buf, w, w, 1.0, 1, false);
    // The deep interior of the block (window entirely inside the block) stays a
    // full 1.0 — averaging all-ones is one.
    let p = buf[8 * w + 8];
    assert!((p[3] - 1.0).abs() < 1e-5, "interior box average = {}", p[3]);
}

#[test]
fn box_blur_radius_zero_is_noop() {
    let mut buf = impulse(9, 9, 4, 4);
    let orig = buf.clone();
    box_blur(&mut buf, 9, 9, 0.0, 3, false);
    assert_eq!(buf, orig);
}

#[test]
fn box_blur_premultiplied_no_color_bleed() {
    // A blurred opaque white impulse stays white where it has coverage (no bleed
    // toward black from transparent neighbours) — premultiplied averaging.
    let mut buf = impulse(21, 21, 10, 10);
    box_blur(&mut buf, 21, 21, 2.0, 2, false);
    let p = buf[10 * 21 + 11];
    assert!(p[3] > 0.0);
    let (r, g, b) = (p[0] / p[3], p[1] / p[3], p[2] / p[3]);
    assert!(
        (r - 1.0).abs() < 1e-3 && (g - 1.0).abs() < 1e-3 && (b - 1.0).abs() < 1e-3,
        "color bled: {r},{g},{b}"
    );
}

#[test]
fn directional_blur_smears_along_the_angle_only() {
    // A horizontal (0°) directional blur smears an impulse left↔right but leaves
    // the column above/below it crisp (the perpendicular axis is untouched).
    let w = 41;
    let mut buf = impulse(w, w, 20, 20);
    directional_blur(&mut buf, w, w, 0.0, 8.0);
    // Coverage spread horizontally: a pixel several px left/right now has alpha.
    assert!(buf[20 * w + 26][3] > 0.0, "smear reached right along the axis");
    assert!(buf[20 * w + 14][3] > 0.0, "smear reached left along the axis");
    // The perpendicular (vertical) neighbours stay empty — no smear off-axis.
    assert_eq!(buf[26 * w + 20][3], 0.0, "no smear vertically");
    assert_eq!(buf[14 * w + 20][3], 0.0, "no smear vertically");
}

#[test]
fn directional_blur_vertical_smears_vertically() {
    // The 90° case smears the other axis — proving the angle drives the direction.
    let w = 41;
    let mut buf = impulse(w, w, 20, 20);
    directional_blur(&mut buf, w, w, 90.0, 8.0);
    assert!(buf[26 * w + 20][3] > 0.0, "smear reached down along the axis");
    assert!(buf[14 * w + 20][3] > 0.0, "smear reached up along the axis");
    assert_eq!(buf[20 * w + 26][3], 0.0, "no smear horizontally");
    assert_eq!(buf[20 * w + 14][3], 0.0, "no smear horizontally");
}

#[test]
fn directional_blur_zero_length_is_noop() {
    let w = 16;
    let mut buf = square(w, w, 8, 8, 2);
    let orig = buf.clone();
    directional_blur(&mut buf, w, w, 30.0, 0.0);
    assert_eq!(buf, orig);
}

#[test]
fn radial_spin_blurs_tangentially_not_radially() {
    // A spin blur about the centre sweeps samples *around* the centre: an off-axis
    // impulse smears along an arc (tangential), so a tangential neighbour gains
    // coverage while a point further out along the same radius does not.
    let w = 41;
    let cx = 20usize;
    let cy = 20usize;
    // Impulse directly to the right of the centre (on the +x axis, radius 12).
    let mut buf = impulse(w, w, cx + 12, cy);
    radial_blur(&mut buf, w, w, [0.5, 0.5], RadialKind::Spin, 40.0);
    // Spin sweeps the point up/down (tangent to the circle) — a pixel above the
    // original lands on the swept arc.
    assert!(
        buf[(cy - 3) * w + (cx + 12)][3] > 0.0 || buf[(cy + 3) * w + (cx + 12)][3] > 0.0,
        "spin should smear tangentially (around the centre)"
    );
    // Radius is ~preserved: a point much further out along the +x ray stays empty.
    assert_eq!(
        buf[cy * w + (cx + 18)][3],
        0.0,
        "spin should not push coverage radially outward"
    );
}

#[test]
fn radial_zoom_blurs_radially_not_tangentially() {
    // A zoom blur about the centre sweeps samples *along the ray*: an off-axis
    // impulse smears toward/away from the centre (radial), so a point further out
    // along the same +x ray gains coverage while a tangential neighbour does not.
    let w = 41;
    let cx = 20usize;
    let cy = 20usize;
    let mut buf = impulse(w, w, cx + 12, cy);
    radial_blur(&mut buf, w, w, [0.5, 0.5], RadialKind::Zoom, 0.3);
    // Radial smear: a pixel further out along +x (away from centre) gains alpha.
    assert!(
        buf[cy * w + (cx + 15)][3] > 0.0 || buf[cy * w + (cx + 9)][3] > 0.0,
        "zoom should smear radially (along the ray)"
    );
    // Tangential neighbours (same radius, rotated) stay empty.
    assert_eq!(
        buf[(cy - 4) * w + (cx + 12)][3],
        0.0,
        "zoom should not smear tangentially"
    );
}

#[test]
fn radial_blur_zero_amount_is_noop() {
    let w = 16;
    let mut buf = square(w, w, 8, 8, 2);
    let orig = buf.clone();
    radial_blur(&mut buf, w, w, [0.5, 0.5], RadialKind::Spin, 0.0);
    assert_eq!(buf, orig);
    radial_blur(&mut buf, w, w, [0.5, 0.5], RadialKind::Zoom, 0.0);
    assert_eq!(buf, orig);
}

#[test]
fn blur_passes_are_deterministic() {
    // Re-running each blur on identical input yields byte-identical output (pure,
    // no time / IO / RNG).
    let mk = || square(24, 24, 12, 12, 4);
    let run = |mut b: Vec<[f32; 4]>, f: &dyn Fn(&mut Vec<[f32; 4]>)| {
        f(&mut b);
        b
    };
    let a = run(mk(), &|b| box_blur(b, 24, 24, 3.0, 2, false));
    let b = run(mk(), &|b| box_blur(b, 24, 24, 3.0, 2, false));
    assert_eq!(a, b, "box blur deterministic");
    let a = run(mk(), &|b| directional_blur(b, 24, 24, 35.0, 10.0));
    let b = run(mk(), &|b| directional_blur(b, 24, 24, 35.0, 10.0));
    assert_eq!(a, b, "directional blur deterministic");
    let a = run(mk(), &|b| radial_blur(b, 24, 24, [0.5, 0.5], RadialKind::Spin, 30.0));
    let b = run(mk(), &|b| radial_blur(b, 24, 24, [0.5, 0.5], RadialKind::Spin, 30.0));
    assert_eq!(a, b, "radial blur deterministic");
}

#[test]
fn new_blur_apply_ignores_empty_buffer() {
    // Degenerate sizes are a no-op (no panic) for every new blur.
    let mut empty: Vec<[f32; 4]> = Vec::new();
    SpatialEffect::BoxBlur {
        radius: 3.0,
        iterations: 2,
        repeat_edge: false,
    }
    .apply(&mut empty, 0, 0);
    SpatialEffect::DirectionalBlur {
        angle: 30.0,
        length: 10.0,
    }
    .apply(&mut empty, 0, 0);
    SpatialEffect::RadialBlur {
        center: [0.5, 0.5],
        kind: RadialKind::Zoom,
        amount: 0.2,
    }
    .apply(&mut empty, 0, 0);
    assert!(empty.is_empty());
}

// --- Distort effects (Corner Pin / Transform / Mirror / Polar) ----------

/// A `w×h` premultiplied buffer with a smooth gradient so resampling differences
/// are detectable: red ramps with x, green with y, fully opaque everywhere.
fn gradient_buf(w: usize, h: usize) -> Vec<[f32; 4]> {
    let mut buf = vec![[0.0f32; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            let r = x as f32 / (w - 1).max(1) as f32;
            let g = y as f32 / (h - 1).max(1) as f32;
            buf[y * w + x] = [r, g, 0.0, 1.0];
        }
    }
    buf
}

#[test]
fn distort_label_and_defaults_are_consistent() {
    // Each default's label is stable and the defaults array has one per variant.
    let d = DistortEffect::defaults();
    assert_eq!(d.len(), 4);
    assert_eq!(d[0].label(), "Corner Pin");
    assert_eq!(d[1].label(), "Transform");
    assert_eq!(d[2].label(), "Mirror");
    assert_eq!(d[3].label(), "Polar Coordinates");
}

#[test]
fn distort_apply_ignores_empty_buffer() {
    // Degenerate sizes are a no-op (no panic).
    let mut empty: Vec<[f32; 4]> = Vec::new();
    DistortEffect::Mirror {
        center: [0.5, 0.5],
        angle: 0.0,
    }
    .apply(&mut empty, 0, 0);
    assert!(empty.is_empty());
}

#[test]
fn bilinear_sample_hits_pixel_centers() {
    // Sampling at a pixel's center returns that pixel exactly.
    let buf = gradient_buf(4, 4);
    let s = sample_bilinear(&buf, 4, 4, 2.5, 1.5); // center of pixel (2, 1)
    assert!((s[0] - buf[4 + 2][0]).abs() < 1e-5);
    assert!((s[1] - buf[4 + 2][1]).abs() < 1e-5);
}

#[test]
fn bilinear_sample_off_buffer_is_transparent() {
    let buf = gradient_buf(4, 4);
    let s = sample_bilinear(&buf, 4, 4, -5.0, -5.0);
    assert_eq!(s, [0.0; 4]);
}

#[test]
fn corner_pin_identity_is_a_no_op() {
    // Pinning the corners where they already are leaves the buffer ~unchanged.
    let w = 16;
    let orig = gradient_buf(w, w);
    let mut buf = orig.clone();
    DistortEffect::CornerPin {
        top_left: [0.0, 0.0],
        top_right: [1.0, 0.0],
        bottom_right: [1.0, 1.0],
        bottom_left: [0.0, 1.0],
    }
    .apply(&mut buf, w, w);
    // Interior pixels (away from the edge resample border) are preserved.
    for y in 2..w - 2 {
        for x in 2..w - 2 {
            let a = orig[y * w + x];
            let b = buf[y * w + x];
            assert!((a[0] - b[0]).abs() < 0.02, "identity changed pixel {x},{y}");
            assert!((a[1] - b[1]).abs() < 0.02);
        }
    }
}

#[test]
fn corner_pin_maps_corners_to_targets() {
    // Pin the source's top-right corner to the centre of the buffer: the source's
    // bright-red region (x→1) should now appear near the centre, and the buffer's
    // own top-right corner should be empty (outside the pinned quad).
    let w = 32;
    let mut buf = gradient_buf(w, w);
    DistortEffect::CornerPin {
        top_left: [0.0, 0.0],
        top_right: [0.5, 0.5], // pull TR into the centre
        bottom_right: [1.0, 1.0],
        bottom_left: [0.0, 1.0],
    }
    .apply(&mut buf, w, w);
    // The far top-right corner of the *output* is outside the pinned quad (which
    // now bends inward at the top-right), so it's transparent.
    assert_eq!(buf[w + (w - 2)][3], 0.0, "output TR is outside the quad");
    // The source's bright-red top-right region (x≈1) was pinned toward the centre,
    // so a point a little inside the quad near the centre carries high red — well
    // above the source's red at that same buffer location without the pin.
    let probe = buf[(w / 2 + 2) * w + (w / 2)]; // just below-centre, inside the quad
    assert!(probe[3] > 0.5, "probe is inside the quad, a={}", probe[3]);
    let crisp_red = (w / 2) as f32 / (w - 1) as f32; // source red at x = w/2
    assert!(
        probe[0] > crisp_red,
        "pinned TR brought brighter red toward the centre: {} vs {}",
        probe[0],
        crisp_red
    );
}

#[test]
fn transform_identity_is_a_no_op() {
    let w = 16;
    let orig = gradient_buf(w, w);
    let mut buf = orig.clone();
    DistortEffect::Transform {
        anchor: [0.5, 0.5],
        position: [0.5, 0.5],
        scale: 1.0,
        rotation: 0.0,
        skew: 0.0,
        opacity: 1.0,
    }
    .apply(&mut buf, w, w);
    for i in 0..w * w {
        for k in 0..4 {
            assert!((orig[i][k] - buf[i][k]).abs() < 1e-4, "identity changed");
        }
    }
}

#[test]
fn transform_translation_shifts_content() {
    // Move the content so the anchor lands a quarter-buffer to the right: a pixel
    // that was at the source x picks up the colour from a pixel to its left.
    let w = 20;
    let mut buf = gradient_buf(w, w);
    DistortEffect::Transform {
        anchor: [0.5, 0.5],
        position: [0.75, 0.5], // shift right by 0.25·w = 5 px
        scale: 1.0,
        rotation: 0.0,
        skew: 0.0,
        opacity: 1.0,
    }
    .apply(&mut buf, w, w);
    // Output pixel (10, 10) now reads the source ~5 px to its left (x≈5), which is
    // darker red than the source at x=10.
    let src_x5 = 5.0 / (w - 1) as f32;
    assert!((buf[10 * w + 10][0] - src_x5).abs() < 0.06, "got {}", buf[10 * w + 10][0]);
}

#[test]
fn transform_scale_zero_collapses_to_empty() {
    let w = 8;
    let mut buf = gradient_buf(w, w);
    DistortEffect::Transform {
        anchor: [0.5, 0.5],
        position: [0.5, 0.5],
        scale: 0.0,
        rotation: 0.0,
        skew: 0.0,
        opacity: 1.0,
    }
    .apply(&mut buf, w, w);
    assert!(buf.iter().all(|p| *p == [0.0; 4]), "zero scale = empty");
}

#[test]
fn transform_opacity_fades_the_buffer() {
    let w = 8;
    let mut buf = gradient_buf(w, w);
    DistortEffect::Transform {
        anchor: [0.5, 0.5],
        position: [0.5, 0.5],
        scale: 1.0,
        rotation: 0.0,
        skew: 0.0,
        opacity: 0.5,
    }
    .apply(&mut buf, w, w);
    // A fully-opaque source pixel is now ~half coverage.
    let mid = buf[(w / 2) * w + (w / 2)];
    assert!((mid[3] - 0.5).abs() < 0.05, "opacity halved alpha, got {}", mid[3]);
}

#[test]
fn mirror_is_symmetric_across_the_line() {
    // Vertical mirror line through the centre: the far (right) side becomes the
    // reflection of the near (left) side, so mirrored columns match.
    let w = 16;
    let mut buf = gradient_buf(w, w);
    DistortEffect::Mirror {
        center: [0.5, 0.5],
        angle: 90.0, // vertical line
    }
    .apply(&mut buf, w, w);
    let y = 8;
    // Column x and its mirror (w-1-x) about the centre should be ~equal in red.
    for x in 1..w / 2 - 1 {
        let left = buf[y * w + x][0];
        let right = buf[y * w + (w - 1 - x)][0];
        assert!((left - right).abs() < 0.06, "mirror asymmetric at x={x}: {left} vs {right}");
    }
}

#[test]
fn mirror_keeps_the_near_side() {
    // With a vertical line at the centre and the line's normal pointing −x, the
    // kept (right) half passes through unchanged while the far (left) half is
    // replaced by its reflection.
    let w = 16;
    let orig = gradient_buf(w, w);
    let mut buf = orig.clone();
    DistortEffect::Mirror {
        center: [0.5, 0.5],
        angle: 90.0,
    }
    .apply(&mut buf, w, w);
    // Right-of-centre column is untouched (the kept side).
    let y = 8;
    let x = 12;
    assert!(
        (buf[y * w + x][0] - orig[y * w + x][0]).abs() < 1e-4,
        "kept side changed"
    );
    // The far (left) side is now the reflection: its red increases toward the
    // centre (mirroring the right side) instead of ramping up from 0.
    assert!(buf[y * w + 2][0] > buf[y * w + 5][0] - 0.5, "far side reflected");
}

#[test]
fn polar_round_trip_recovers_the_source() {
    // Rect→Polar then Polar→Rect about the same centre returns ~the original in
    // the well-sampled interior (corners/edges lose some precision in the
    // resample, so check a central region).
    let w = 48;
    let orig = gradient_buf(w, w);
    let mut buf = orig.clone();
    DistortEffect::Polar {
        center: [0.5, 0.5],
        kind: PolarKind::RectToPolar,
        interp: 1.0,
    }
    .apply(&mut buf, w, w);
    DistortEffect::Polar {
        center: [0.5, 0.5],
        kind: PolarKind::PolarToRect,
        interp: 1.0,
    }
    .apply(&mut buf, w, w);
    // Sample a central band away from the singular centre and the borders.
    let mut checked = 0;
    for y in (w / 2 - 6)..(w / 2 + 6) {
        for x in (w / 2 + 4)..(w / 2 + 10) {
            let a = orig[y * w + x];
            let b = buf[y * w + x];
            assert!((a[0] - b[0]).abs() < 0.12, "round-trip drift at {x},{y}: {} vs {}", a[0], b[0]);
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn polar_interp_zero_is_a_no_op() {
    let w = 8;
    let orig = gradient_buf(w, w);
    let mut buf = orig.clone();
    DistortEffect::Polar {
        center: [0.5, 0.5],
        kind: PolarKind::RectToPolar,
        interp: 0.0,
    }
    .apply(&mut buf, w, w);
    assert_eq!(buf, orig, "interp 0 must be identity");
}

#[test]
fn distort_is_deterministic() {
    // The same effect on the same buffer gives byte-identical results (needed for
    // the RAM-preview cache / golden-frame tests).
    let w = 24;
    let mut a = gradient_buf(w, w);
    let mut b = gradient_buf(w, w);
    let eff = DistortEffect::Polar {
        center: [0.5, 0.5],
        kind: PolarKind::RectToPolar,
        interp: 1.0,
    };
    eff.apply(&mut a, w, w);
    eff.apply(&mut b, w, w);
    assert_eq!(a, b, "distort must be deterministic");
}

#[test]
fn apply_distort_effects_stacks_in_order() {
    // Two mirrors (vertical then horizontal) compose into a point reflection,
    // distinct from either alone — confirming the stack runs both passes.
    let w = 16;
    let mut both = gradient_buf(w, w);
    apply_distort_effects(
        &[
            DistortEffect::Mirror {
                center: [0.5, 0.5],
                angle: 90.0,
            },
            DistortEffect::Mirror {
                center: [0.5, 0.5],
                angle: 0.0,
            },
        ],
        &mut both,
        w,
        w,
    );
    let mut one = gradient_buf(w, w);
    DistortEffect::Mirror {
        center: [0.5, 0.5],
        angle: 90.0,
    }
    .apply(&mut one, w, w);
    assert_ne!(both, one, "stacking a second mirror must change the result");
}

#[test]
fn distort_effects_serde_defaults_to_empty() {
    // Pre-distort-effect layers (no `distort_effects` field) load with none.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.distort_effects.is_empty());
    assert!(!layer.has_distort_effects());
}

#[test]
fn stylize_effects_serde_defaults_to_empty() {
    // Pre-stylize-effect layers (no `stylize_effects` field) load with none, so
    // existing project files round-trip unchanged.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.stylize_effects.is_empty());
    assert!(!layer.has_stylize_effects());
}

#[test]
fn stylize_effects_serde_round_trips() {
    // A layer with a stylize stack serializes and reloads identically.
    use crate::comp::StylizeEffect;
    let mut layer = PulseLayer::new("L", [1.0, 1.0, 1.0, 1.0]);
    layer.stylize_effects.push(StylizeEffect::FindEdges {
        amount: 2.0,
        invert: true,
    });
    layer.stylize_effects.push(StylizeEffect::Mosaic {
        horizontal: 12,
        vertical: 20,
    });
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(layer.stylize_effects, back.stylize_effects);
    assert!(back.has_stylize_effects());
}

// --- Keying effects (Color / Luma / Chroma Key, Spill, Matte Choke) -----

/// A `w×h` premultiplied, fully-opaque buffer flooded with one **linear-light**
/// colour. (The compositor buffer the keyers operate on is linear; the keyers'
/// own `key` colour is authored sRGB and decoded to linear at the pass boundary,
/// so to make a buffer pixel that *matches* a sRGB key colour, decode it here.)
fn flood_buf(w: usize, h: usize, c: [f32; 3]) -> Vec<[f32; 4]> {
    vec![[c[0], c[1], c[2], 1.0]; w * h]
}

/// Decode a straight sRGB colour to linear (mirrors the keyers' key-colour decode)
/// so a test buffer pixel can be made to sit exactly on a sRGB key colour.
fn lin3(c: [f32; 3]) -> [f32; 3] {
    [
        prism_core::color::srgb_to_linear(c[0]),
        prism_core::color::srgb_to_linear(c[1]),
        prism_core::color::srgb_to_linear(c[2]),
    ]
}

#[test]
fn key_label_and_defaults_are_consistent() {
    // Each default's label is stable and the defaults array has one per variant.
    let d = KeyEffect::defaults();
    assert_eq!(d.len(), 5);
    assert_eq!(d[0].label(), "Color Key");
    assert_eq!(d[1].label(), "Luma Key");
    assert_eq!(d[2].label(), "Chroma Key");
    assert_eq!(d[3].label(), "Spill Suppression");
    assert_eq!(d[4].label(), "Matte Choke");
}

#[test]
fn key_apply_ignores_empty_buffer() {
    // Degenerate sizes are a no-op (no panic).
    let mut empty: Vec<[f32; 4]> = Vec::new();
    KeyEffect::ColorKey {
        key: [0.0, 0.6, 0.1],
        tolerance: 0.1,
        softness: 0.0,
    }
    .apply(&mut empty, 0, 0);
    assert!(empty.is_empty());
}

#[test]
fn color_key_drops_target_keeps_others() {
    // The target colour is keyed to zero alpha; a far colour is untouched.
    let key = [0.0, 0.6, 0.1];
    // The on-key buffer pixel is the *linear* version of the key (the buffer is
    // linear-light, the key colour is sRGB-decoded to linear inside the keyer).
    let mut buf = flood_buf(2, 1, lin3(key));
    let red = lin3([0.9, 0.1, 0.1]);
    buf[1] = [red[0], red[1], red[2], 1.0]; // a red pixel, far from the green key
    KeyEffect::ColorKey {
        key,
        tolerance: 0.05,
        softness: 0.02,
    }
    .apply(&mut buf, 2, 1);
    assert!(buf[0][3] < 0.01, "on-key pixel is keyed out, got {}", buf[0][3]);
    assert!(buf[1][3] > 0.99, "off-key pixel is kept, got {}", buf[1][3]);
}

#[test]
fn color_key_softness_feathers_the_edge() {
    // A pixel just past the tolerance reads a partial (feathered) alpha in the
    // softness band, between fully keyed and fully kept. The key is black, so the
    // distance is just the (linear) grey value's magnitude.
    let key = [0.0, 0.0, 0.0];
    let mut buf = vec![[0.18f32, 0.18, 0.18, 1.0]]; // linear grey ~0.31 from black
    KeyEffect::ColorKey {
        key,
        tolerance: 0.1,
        softness: 0.3,
    }
    .apply(&mut buf, 1, 1);
    let a = buf[0][3];
    assert!(a > 0.0 && a < 1.0, "softness band gives a partial alpha, got {a}");
}

#[test]
fn luma_key_threshold_direction_and_softness() {
    // key_high=false drops the dark side: a dark pixel keys out, a bright one
    // stays. key_high=true flips that. Softness yields a partial alpha mid-band.
    let dark = [0.05f32, 0.05, 0.05];
    let bright = [0.9f32, 0.9, 0.9];

    let mut lo = vec![[dark[0], dark[1], dark[2], 1.0], [bright[0], bright[1], bright[2], 1.0]];
    KeyEffect::LumaKey {
        threshold: 0.5,
        softness: 0.05,
        key_high: false,
    }
    .apply(&mut lo, 2, 1);
    assert!(lo[0][3] < 0.01, "dark side keyed out when key_high=false");
    assert!(lo[1][3] > 0.99, "bright side kept when key_high=false");

    let mut hi = vec![[dark[0], dark[1], dark[2], 1.0], [bright[0], bright[1], bright[2], 1.0]];
    KeyEffect::LumaKey {
        threshold: 0.5,
        softness: 0.05,
        key_high: true,
    }
    .apply(&mut hi, 2, 1);
    assert!(hi[0][3] > 0.99, "dark side kept when key_high=true");
    assert!(hi[1][3] < 0.01, "bright side keyed out when key_high=true");

    // A mid-luma pixel strictly inside the softness band reads a partial alpha.
    // key_high=false ramps over [threshold - softness, threshold] = [0.1, 0.5];
    // a luma of 0.3 sits in the middle of that ramp.
    let mut mid = vec![[0.3f32, 0.3, 0.3, 1.0]];
    KeyEffect::LumaKey {
        threshold: 0.5,
        softness: 0.4,
        key_high: false,
    }
    .apply(&mut mid, 1, 1);
    let a = mid[0][3];
    assert!(a > 0.0 && a < 1.0, "softness gives a partial luma-key alpha, got {a}");
}

#[test]
fn chroma_key_distance_keying() {
    // A pixel at the key chroma keys out; an off-chroma colour (red) stays — the
    // hallmark of chroma keying (distance in the Cb/Cr plane, not RGB). The chroma
    // axes subtract luminance, so adding an equal amount to every channel leaves
    // the chroma unchanged: a *lit* version of the backing (key + white) still
    // keys, while a chroma far from the key (red) survives even at the same
    // brightness. Buffer pixels are linear-light (the key colour is sRGB-decoded
    // to linear inside the keyer).
    let key = [0.0, 0.6, 0.1];
    let on = lin3(key); // exactly the key chroma
    // Lift every channel equally → same chroma, higher luminance (a lit backing).
    let lit = [on[0] + 0.2, on[1] + 0.2, on[2] + 0.2];
    let red = lin3([0.9, 0.1, 0.1]); // a chroma far from the key
    let mut buf = vec![
        [on[0], on[1], on[2], 1.0],
        [lit[0], lit[1], lit[2], 1.0],
        [red[0], red[1], red[2], 1.0],
    ];
    KeyEffect::ChromaKey {
        key,
        gain: 1.0,
        balance: 0.5,
        softness: 0.05,
    }
    .apply(&mut buf, 3, 1);
    assert!(buf[0][3] < 0.2, "on-chroma keyed, got {}", buf[0][3]);
    assert!(
        buf[1][3] < 0.2,
        "lit backing (same chroma) still keyed regardless of luma, got {}",
        buf[1][3]
    );
    assert!(buf[2][3] > 0.9, "off-chroma kept, got {}", buf[2][3]);
}

#[test]
fn spill_suppression_reduces_the_key_channel() {
    // A pixel with green spill (green channel above the other two) has its green
    // pulled back toward the average; alpha is untouched.
    let mut buf = vec![[0.3f32, 0.8, 0.3, 1.0]]; // green-dominant
    let before = buf[0][1];
    KeyEffect::SpillSuppression {
        key: [0.0, 1.0, 0.0], // green key → dominant channel is green
        amount: 1.0,
    }
    .apply(&mut buf, 1, 1);
    assert!(buf[0][1] < before, "green spill reduced from {before} to {}", buf[0][1]);
    // Pulled toward the average of the other two (both 0.3).
    assert!((buf[0][1] - 0.3).abs() < 1e-4, "green neutralised to the others");
    assert_eq!(buf[0][3], 1.0, "spill leaves alpha alone");
}

#[test]
fn matte_choke_erodes_and_dilates_alpha() {
    // A single opaque pixel in a transparent field. Eroding (negative choke)
    // wipes it (its neighbours are transparent); dilating (positive choke) grows
    // its coverage to neighbours.
    let w = 5;
    let h = 5;
    let center = 2 * w + 2;

    let mut erode = vec![[0.0f32; 4]; w * h];
    erode[center] = [1.0, 1.0, 1.0, 1.0];
    KeyEffect::MatteChoke {
        choke: -1.0,
        clip_black: 0.0,
        clip_white: 1.0,
    }
    .apply(&mut erode, w, h);
    assert_eq!(erode[center][3], 0.0, "erode wipes the lone pixel");

    let mut dilate = vec![[0.0f32; 4]; w * h];
    dilate[center] = [1.0, 1.0, 1.0, 1.0];
    KeyEffect::MatteChoke {
        choke: 1.0,
        clip_black: 0.0,
        clip_white: 1.0,
    }
    .apply(&mut dilate, w, h);
    let grown: f32 = dilate.iter().map(|p| p[3]).sum();
    assert!(grown > 1.0, "dilate grows coverage to neighbours, total {grown}");
}

#[test]
fn matte_choke_clip_levels_crush_the_tails() {
    // Clip black raises everything at/below it to 0; clip white drops everything
    // at/above it to 1; the middle rescales.
    let mut buf = vec![
        [0.1f32, 0.1, 0.1, 0.1], // below clip_black → 0
        [0.5f32, 0.5, 0.5, 0.5], // mid → rescaled into (0,1)
        [0.95f32, 0.95, 0.95, 0.95], // above clip_white → 1
    ];
    KeyEffect::MatteChoke {
        choke: 0.0,
        clip_black: 0.2,
        clip_white: 0.8,
    }
    .apply(&mut buf, 3, 1);
    assert_eq!(buf[0][3], 0.0, "low tail clipped to 0");
    assert_eq!(buf[2][3], 1.0, "high tail clipped to 1");
    assert!(buf[1][3] > 0.0 && buf[1][3] < 1.0, "mid rescaled, got {}", buf[1][3]);
}

#[test]
fn apply_key_effects_stacks_in_order() {
    // A Color Key (pulls a soft matte) followed by a Matte Choke clip-white that
    // hardens it differs from the Color Key alone — confirming the stack runs
    // both passes in order.
    let key = [0.0, 0.6, 0.1];
    let mut buf = flood_buf(4, 1, [0.1, 0.5, 0.15]); // near, but not on, the key
    let mut one = buf.clone();
    KeyEffect::ColorKey {
        key,
        tolerance: 0.0,
        softness: 0.6,
    }
    .apply(&mut one, 4, 1);
    apply_key_effects(
        &[
            KeyEffect::ColorKey {
                key,
                tolerance: 0.0,
                softness: 0.6,
            },
            KeyEffect::MatteChoke {
                choke: 0.0,
                clip_black: 0.0,
                clip_white: 0.5,
            },
        ],
        &mut buf,
        4,
        1,
    );
    assert_ne!(buf, one, "the choke pass after the key must change the result");
}

#[test]
fn key_is_deterministic() {
    let key = [0.0, 0.6, 0.1];
    let effects = [
        KeyEffect::ChromaKey {
            key,
            gain: 1.2,
            balance: 0.5,
            softness: 0.1,
        },
        KeyEffect::MatteChoke {
            choke: 1.0,
            clip_black: 0.05,
            clip_white: 0.95,
        },
    ];
    let mut a = flood_buf(8, 8, [0.1, 0.55, 0.12]);
    let mut b = a.clone();
    apply_key_effects(&effects, &mut a, 8, 8);
    apply_key_effects(&effects, &mut b, 8, 8);
    assert_eq!(a, b, "keying must be deterministic");
}

#[test]
fn key_effects_serde_defaults_to_empty() {
    // Pre-keying layers (no `key_effects` field) load with none.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.key_effects.is_empty());
    assert!(!layer.has_key_effects());
}

#[test]
fn key_effects_serde_round_trips() {
    // A layer with a full key stack survives a JSON round-trip value-for-value.
    let mut layer = PulseLayer::new("L", [1.0, 1.0, 1.0, 1.0]);
    layer.key_effects = vec![
        KeyEffect::ColorKey {
            key: [0.0, 0.6, 0.1],
            tolerance: 0.15,
            softness: 0.1,
        },
        KeyEffect::LumaKey {
            threshold: 0.4,
            softness: 0.05,
            key_high: true,
        },
        KeyEffect::MatteChoke {
            choke: -2.0,
            clip_black: 0.1,
            clip_white: 0.9,
        },
    ];
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.key_effects, layer.key_effects);
    assert!(back.has_key_effects());
}

#[test]
fn spatial_effects_serde_defaults_to_empty() {
    // Pre-spatial-effect layers (no `spatial_effects` field) load with none.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.spatial_effects.is_empty());
    assert!(!layer.has_spatial_effects());
}

#[test]
fn footage_serde_defaults_to_empty() {
    // A pre-footage layer (no `footage` field, no `kind`) loads as a Solid with
    // an empty footage block, so old projects are unaffected.
    let json = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.kind, LayerKind::Solid);
    assert!(!layer.footage.is_set());
    assert!(!layer.has_footage());
}

#[test]
fn footage_layer_serde_round_trips() {
    // A fully-configured footage layer (sequence source + alpha / fps / loop)
    // survives a JSON round-trip byte-for-value, including the FootageSource and
    // the new LayerKind::Footage variant.
    let mut layer = PulseLayer::of_kind(LayerKind::Footage, "Plate", [0.5, 0.5, 0.5, 1.0]);
    layer.footage.source = Some(FootageSource::Sequence {
        pattern: "shot/img_{}.png".to_string(),
        pad: 4,
        start: 1,
        count: 240,
    });
    layer.footage.alpha = AlphaMode::Premultiplied;
    layer.footage.fps = Some(24.0);
    layer.footage.looping = true;
    layer.footage.hold_last = false;

    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, LayerKind::Footage);
    assert!(back.has_footage());
    assert_eq!(back.footage.alpha, AlphaMode::Premultiplied);
    assert_eq!(back.footage.fps, Some(24.0));
    assert!(back.footage.looping);
    assert!(!back.footage.hold_last);
    assert_eq!(back.footage.source, layer.footage.source);
}

#[test]
fn footage_hold_last_serde_defaults_true() {
    // A footage block missing `hold_last` (and most fields) loads with the
    // sensible default (hold the last frame), per the field's serde default.
    let json = r#"{"name":"F","kind":"Footage","color":[0.5,0.5,0.5,1.0],"visible":true,
        "footage":{"source":null},
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.kind, LayerKind::Footage);
    assert!(layer.footage.hold_last, "hold_last serde-defaults to true");
    assert!(!layer.footage.looping);
    assert_eq!(layer.footage.fps, None);
}

// --- Precomps (nested compositions) -------------------------------------

#[test]
fn precomp_layer_serde_round_trips() {
    // A precomp layer (target comp id + time offset) survives a JSON round-trip,
    // including the new LayerKind::Precomp variant and its PrecompLayer block.
    let mut layer = PulseLayer::of_kind(LayerKind::Precomp, "Nested", [0.5, 0.5, 0.5, 1.0]);
    layer.precomp = PrecompLayer {
        source: Some(7),
        time_offset: -0.5,
    };
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(back.kind, LayerKind::Precomp);
    assert!(back.has_precomp());
    assert_eq!(back.precomp.source, Some(7));
    assert!((back.precomp.time_offset - (-0.5)).abs() < 1e-6);
}

#[test]
fn precomp_serde_defaults_for_old_files() {
    // A layer block from a pre-precomp `.pulse` (no `precomp` field, no `kind`)
    // loads as a solid with an unwired precomp (source None, offset 0).
    let json = r#"{"name":"L","color":[1.0,0.0,0.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.kind, LayerKind::Solid);
    assert_eq!(layer.precomp.source, None);
    assert_eq!(layer.precomp.time_offset, 0.0);
    assert!(!layer.has_precomp());
}

#[test]
fn old_single_comp_json_loads_as_comp() {
    // An old single-comp `.pulse` (a bare Comp, no `id`/`name`) still deserializes
    // into a Comp directly, with id/name serde-defaulted.
    let json = r#"{"width":640,"height":480,"duration":2.0,"fps":24.0,"layers":[]}"#;
    let comp: Comp = serde_json::from_str(json).unwrap();
    assert_eq!(comp.id, 0);
    assert!(comp.name.is_empty());
    assert_eq!(comp.width, 640);
    assert_eq!(comp.fps, 24.0);
    // And wraps cleanly into a one-comp project with a minted id.
    let project = Project::from_comp(comp);
    assert_eq!(project.comps.len(), 1);
    assert_eq!(project.comps[0].id, 1, "from_comp mints an id for an id-less comp");
}

#[test]
fn project_serde_round_trips_with_precomp() {
    // A two-comp project — comp A holding a precomp layer referencing comp B —
    // round-trips through JSON: both comps and the reference survive.
    let mut a = Comp::empty_like("A", &Comp::new());
    a.id = 1;
    let mut pc = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
    pc.precomp = PrecompLayer::to(2);
    a.layers.push(pc);
    let mut b = Comp::empty_like("B", &Comp::new());
    b.id = 2;

    let project = Project {
        comps: vec![a, b],
        active: 0,
        next_id: 3,
        presets: Vec::new(),
    };
    let json = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back.comps.len(), 2);
    assert_eq!(back.comps[0].layers[0].precomp.source, Some(2));
    assert_eq!(back.comp_by_id(2).map(|c| c.name.clone()), Some("B".to_string()));
}

#[test]
fn project_mints_unique_ids() {
    // mint_id never reuses or collides with a live comp id, even when next_id
    // lags behind (e.g. a hand-edited file).
    let mut p = Project::new(); // comp id 1, next_id 2
    let id_a = p.mint_id();
    let id_b = p.mint_id();
    assert_ne!(id_a, id_b);
    assert!(id_a >= 2 && id_b > id_a);
    // Force next_id to lag behind a live id, then mint: it must skip past it.
    p.next_id = 1;
    p.comps.push({
        let mut c = Comp::empty_like("X", &Comp::new());
        c.id = 50;
        c
    });
    let id_c = p.mint_id();
    assert!(id_c > 50, "mint_id skips past the highest live id, got {id_c}");
}

#[test]
fn push_comp_assigns_a_fresh_id() {
    let mut p = Project::new();
    let id = p.push_comp(Comp::empty_like("New", &Comp::new()));
    assert!(id >= 2);
    assert_eq!(p.comps.last().unwrap().id, id);
    assert!(p.comp_by_id(id).is_some());
    // The active comp (index 0) is unchanged and distinct.
    assert_ne!(p.active().id, id);
}

#[test]
fn precompose_wraps_layer_into_new_comp() {
    // Model-level analogue of the app's pre-compose: a project starts with one
    // comp holding a content layer; pre-compose moves that layer into a new comp
    // and replaces it in the host with a precomp referencing the new comp.
    let mut p = Project::new();
    let host_id = p.active().id;
    // The content layer we'll wrap (index 0 of the active comp's demo).
    let content_name = p.comps[0].layers[0].name.clone();

    // Build the nested comp and move the layer into it.
    let mut nested = Comp::empty_like(format!("{content_name} Comp"), &p.comps[0]);
    let wrapped = p.comps[0].layers[0].clone();
    nested.layers.push(wrapped);
    let new_id = p.push_comp(nested);

    // Replace the host layer with a precomp referencing the new comp.
    let mut precomp = PulseLayer::of_kind(LayerKind::Precomp, content_name.clone(), [0.5; 4]);
    precomp.precomp = PrecompLayer::to(new_id);
    p.comps[0].layers[0] = precomp;

    // The host's layer is now a precomp pointing at the new comp...
    let host = p.comp_by_id(host_id).unwrap();
    assert_eq!(host.layers[0].kind, LayerKind::Precomp);
    assert_eq!(host.layers[0].precomp.source, Some(new_id));
    // ...and the new comp holds the original content.
    let made = p.comp_by_id(new_id).unwrap();
    assert_eq!(made.layers.len(), 1);
    assert_eq!(made.layers[0].name, content_name);
    assert_ne!(new_id, host_id, "the precomp target is a distinct comp");
}

// --- Expressions on properties ---------------------------------------------

#[test]
fn expression_overrides_keyframed_value() {
    // `time * 2` ignores the keyframes and is a pure function of time.
    let mut track = Track::default();
    track.set_key(0.0, 100.0); // keyframed value the expression replaces
    track.expression = Some("time * 2".to_string());
    for &t in &[0.0_f32, 1.0, 2.5, 4.0] {
        let got = track.sample_expr(t, 0.0, ExprCtx::at(t, 0.0));
        assert!((got - t * 2.0).abs() < 1e-4, "t={t} got={got}");
    }
}

#[test]
fn expression_value_sees_keyframed_sample() {
    // `value + 10` offsets the *keyframed* value at each time, proving the
    // keyframed sample is exposed to the script as `value`.
    let mut track = Track::default();
    track.set_key(0.0, 0.0);
    track.set_key(2.0, 20.0); // linear ramp 0 -> 20
    track.expression = Some("value + 10".to_string());
    // Midpoint keyframed value is 10, so the expression yields 20.
    let got = track.sample_expr(1.0, 0.0, ExprCtx::at(1.0, 0.0));
    assert!((got - 20.0).abs() < 1e-4, "got={got}");
}

#[test]
fn malformed_expression_falls_back_to_keyframed_value() {
    // A syntax error must not panic and must fall back to the keyframed value.
    let mut track = Track::default();
    track.set_key(0.0, 42.0);
    track.expression = Some("this is not valid $#@".to_string());
    let got = track.sample_expr(0.0, 0.0, ExprCtx::at(0.0, 0.0));
    assert_eq!(got, 42.0, "malformed expression should fall back");
}

#[test]
fn empty_expression_is_keyframed() {
    let mut track = Track::default();
    track.set_key(0.0, 5.0);
    track.expression = Some("   ".to_string()); // whitespace-only = no expression
    assert!(!track.has_expression());
    assert_eq!(track.sample_expr(0.0, 0.0, ExprCtx::at(0.0, 0.0)), 5.0);
}

#[test]
fn expression_serde_round_trips() {
    let mut layer = PulseLayer::new("Expr", [1.0; 4]);
    layer.x.set_key(0.0, 1.0);
    layer.x.expression = Some("value + wiggle(2, 30)".to_string());
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.x.expression.as_deref(),
        Some("value + wiggle(2, 30)"),
        "expression must survive a serde round-trip"
    );
    // A property without an expression deserializes as None (back-compat: the
    // field is skipped when empty).
    assert!(back.opacity.expression.is_none());
}

#[test]
fn missing_expression_field_defaults_to_none() {
    // A pre-expression track (no `expression` key) deserializes as None.
    let json = r#"{"keys":[{"t":0.0,"value":1.0}]}"#;
    let track: Track = serde_json::from_str(json).unwrap();
    assert!(track.expression.is_none());
    assert!(!track.has_expression());
}

#[test]
fn comp_layer_value_is_expression_aware() {
    // Through the comp-level sampler, an expression on a transform property
    // resolves with the comp's fps/duration/index context.
    let mut comp = Comp::new();
    // Put a deterministic expression on layer 0's rotation: `index * 90 + time`.
    comp.layers[0].rotation.expression = Some("index * 90 + time".to_string());
    let got = comp.layer_value(0, Prop::Rotation, 5.0);
    assert!((got - (0.0 * 90.0 + 5.0)).abs() < 1e-4, "got={got}");
    // The opacity sampler (no expression) is unaffected.
    let op = comp.layer_opacity(0, 0.0);
    assert!((0.0..=1.0).contains(&op));
}

// --- Time remapping ----------------------------------------------------

#[test]
fn time_remap_serde_defaults_to_disabled() {
    // A pre-time-remap layer (no `time_remap` field) loads with the remap off and
    // an empty track, so old projects sample their source at the comp time.
    let json = r#"{"name":"F","kind":"Footage","color":[0.5,0.5,0.5,1.0],"visible":true,
        "footage":{"source":null},
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(!layer.time_remap.enabled);
    assert!(!layer.time_remap.is_active());
    assert!(layer.time_remap.track.keys.is_empty());
}

#[test]
fn enabled_time_remap_layer_serde_round_trips() {
    // A footage layer with an enabled, keyed time-remap curve survives a JSON
    // round-trip (enable flag + the remap track's keys).
    let mut layer = PulseLayer::of_kind(LayerKind::Footage, "Plate", [0.5, 0.5, 0.5, 1.0]);
    layer.time_remap.enabled = true;
    layer.time_remap.track.set_key(0.0, 4.0);
    layer.time_remap.track.set_key(4.0, 0.0); // reverse ramp
    let json = serde_json::to_string(&layer).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert!(back.time_remap.enabled);
    assert!(back.time_remap.is_active());
    assert_eq!(back.time_remap.track.keys.len(), 2);
    assert!((back.time_remap.track.sample(1.0, 1.0) - 3.0).abs() < 1e-4);
}

#[test]
fn comp_layer_source_time_identity_when_off() {
    // With no remap, the comp's source-time sampler is the identity: source time
    // == comp time, so footage/precomp sampling is unchanged.
    let mut comp = Comp::new();
    comp.layers[0].kind = LayerKind::Footage; // any layer; remap off by default
    for &t in &[0.0, 1.0, 2.5, 5.0] {
        assert!((comp.layer_source_time(0, t) - t).abs() < 1e-6);
    }
}

#[test]
fn comp_layer_source_time_follows_active_remap() {
    // An active reversing remap drives the comp-level source time: r(t) = dur - t.
    let dur = 5.0_f32;
    let mut comp = Comp::new();
    comp.layers[0].kind = LayerKind::Footage;
    comp.layers[0].time_remap.enabled = true;
    comp.layers[0].time_remap.track.set_key(0.0, dur);
    comp.layers[0].time_remap.track.set_key(dur, 0.0);
    for &t in &[0.0, 1.0, 2.5, 5.0] {
        assert!((comp.layer_source_time(0, t) - (dur - t)).abs() < 1e-4, "t={t}");
    }
}

// --- Markers / work area -----------------------------------------------------

#[test]
fn comp_navigation_spans_comp_and_layer_markers() {
    // next/prev consider both the comp's own markers and the selected layer's.
    let mut comp = Comp::new();
    comp.markers = vec![Marker::at(1.0), Marker::at(4.0)];
    comp.layers[0].markers = vec![Marker::at(2.0)];
    // With layer 0 selected, the layer marker at 2.0 is in the set.
    assert_eq!(comp.next_marker(0.5, Some(0)), Some(1.0));
    assert_eq!(comp.next_marker(1.0, Some(0)), Some(2.0)); // the layer marker
    assert_eq!(comp.prev_marker(3.0, Some(0)), Some(2.0));
    assert_eq!(comp.next_marker(4.0, Some(0)), None);
    // With no layer selected, only comp markers count (the 2.0 layer marker is gone).
    assert_eq!(comp.next_marker(1.0, None), Some(4.0));
    assert_eq!(comp.prev_marker(3.0, None), Some(1.0));
}

#[test]
fn comp_navigation_ignores_other_layers_markers() {
    // Only the *selected* layer's markers join the comp's; another layer's don't.
    let mut comp = Comp::new();
    comp.markers.clear(); // drop the demo's comp marker to isolate layer markers
    comp.layers[0].markers = vec![Marker::at(1.0)];
    comp.layers[1].markers = vec![Marker::at(2.0)];
    assert_eq!(comp.next_marker(0.0, Some(0)), Some(1.0));
    // Selecting layer 0, layer 1's marker at 2.0 is not in the nav set.
    assert_eq!(comp.next_marker(1.0, Some(0)), None);
    // Selecting layer 1 instead surfaces its 2.0 marker.
    assert_eq!(comp.next_marker(1.0, Some(1)), Some(2.0));
}

#[test]
fn comp_clamped_work_area_stays_inside_timeline() {
    // A hand-edited / inverted work area is clamped to the comp's [0, duration].
    let mut comp = Comp::new();
    comp.duration = 5.0;
    comp.work_area = WorkArea { start: -2.0, end: 99.0 };
    let wa = comp.clamped_work_area();
    assert_eq!(wa, WorkArea { start: 0.0, end: 5.0 });
    // An inverted range collapses to an ordered (zero-length) area, never escapes.
    comp.work_area = WorkArea { start: 4.0, end: 1.0 };
    let wa = comp.clamped_work_area();
    assert_eq!(wa, WorkArea { start: 4.0, end: 4.0 });
}

#[test]
fn fresh_comp_work_area_spans_the_timeline() {
    let comp = Comp::new();
    assert!(comp.clamped_work_area().is_full(comp.duration));
}

#[test]
fn markers_and_work_area_serde_round_trip() {
    let mut comp = Comp::new();
    comp.markers = vec![{
        let mut m = Marker::at(1.5);
        m.label = "intro".to_string();
        m.duration = 0.5;
        m.color = [0.1, 0.2, 0.3];
        m
    }];
    comp.layers[0].markers = vec![Marker::at(2.0)];
    comp.work_area = WorkArea { start: 1.0, end: 3.0 };
    let json = serde_json::to_string(&comp).unwrap();
    let back: Comp = serde_json::from_str(&json).unwrap();
    assert_eq!(back.markers.len(), 1);
    assert_eq!(back.markers[0].label, "intro");
    assert!((back.markers[0].time - 1.5).abs() < 1e-6);
    assert!((back.markers[0].duration - 0.5).abs() < 1e-6);
    assert_eq!(back.markers[0].color, [0.1, 0.2, 0.3]);
    assert_eq!(back.layers[0].markers.len(), 1);
    assert_eq!(back.work_area, WorkArea { start: 1.0, end: 3.0 });
}

#[test]
fn markers_serde_default_to_empty_for_old_files() {
    // A pre-marker comp (no `markers` / `work_area` fields) loads with no markers
    // and an empty (full-on-clamp) work area.
    let json = r#"{"width":16,"height":16,"duration":2.0,"fps":30.0,
        "layers":[{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}]}"#;
    let comp: Comp = serde_json::from_str(json).unwrap();
    assert!(comp.markers.is_empty());
    assert!(comp.layers[0].markers.is_empty());
    // The stored serde-default WorkArea is the empty [0,0] range, but
    // `clamped_work_area` self-heals it to the whole timeline so a pre-work-area
    // project loops its full length (rather than a degenerate zero range).
    assert_eq!(comp.work_area, WorkArea { start: 0.0, end: 0.0 });
    assert_eq!(comp.clamped_work_area(), WorkArea { start: 0.0, end: 2.0 });
    assert!(comp.clamped_work_area().is_full(comp.duration));
}

// ---------------------------------------------------------------------------
// Spatial motion paths + auto-orient along path
// ---------------------------------------------------------------------------

/// Build a layer with the given linear position keyframes on X and Y.
fn moving_layer(keys: &[(f32, f32, f32)]) -> PulseLayer {
    let mut l = PulseLayer::new("Mover", [1.0, 1.0, 1.0, 1.0]);
    for &(t, x, y) in keys {
        l.x.set_key(t, x);
        l.y.set_key(t, y);
    }
    l
}

#[test]
fn motion_path_tangent_points_along_travel() {
    // A layer sliding straight to the right (+x): the tangent must be (1, 0) and
    // the heading 0°. Straight down the screen (+y down) heads 90°; diagonally
    // down-right heads 45°.
    let right = moving_layer(&[(0.0, -100.0, 0.0), (2.0, 100.0, 0.0)]);
    let s = sample_path(&right.x, &right.y, 1.0, 0.0, 0.0);
    let [dx, dy] = s.tangent.expect("moving → has a tangent");
    assert!((dx - 1.0).abs() < 1e-3 && dy.abs() < 1e-3, "tangent {dx},{dy}");
    assert!((s.heading_deg().unwrap() - 0.0).abs() < 1e-2);

    let down = moving_layer(&[(0.0, 0.0, -100.0), (2.0, 0.0, 100.0)]);
    assert!(
        (sample_path(&down.x, &down.y, 1.0, 0.0, 0.0)
            .heading_deg()
            .unwrap()
            - 90.0)
            .abs()
            < 1e-2
    );

    let diag = moving_layer(&[(0.0, 0.0, 0.0), (2.0, 100.0, 100.0)]);
    assert!(
        (sample_path(&diag.x, &diag.y, 1.0, 0.0, 0.0)
            .heading_deg()
            .unwrap()
            - 45.0)
            .abs()
            < 1e-2
    );
}

#[test]
fn motion_path_position_matches_transform() {
    // The path's sampled position is exactly the layer's keyframed (x, y) — the
    // tangent is auxiliary, not a separate spline that could disagree.
    let m = moving_layer(&[(0.0, -50.0, 10.0), (4.0, 50.0, -30.0)]);
    let s = sample_path(&m.x, &m.y, 2.0, 0.0, 0.0);
    let tf = m.transform(2.0);
    assert!((s.pos[0] - tf.x).abs() < 1e-4 && (s.pos[1] - tf.y).abs() < 1e-4);
}

#[test]
fn motion_path_multi_key_corner_turns_tangent() {
    // An L-shaped path: travel +x for the first segment, then +y for the second.
    // The heading in each segment's middle reflects that segment's direction.
    let m = moving_layer(&[(0.0, 0.0, 0.0), (1.0, 100.0, 0.0), (2.0, 100.0, 100.0)]);
    assert!(
        (sample_path(&m.x, &m.y, 0.5, 0.0, 0.0)
            .heading_deg()
            .unwrap())
        .abs()
            < 1e-2
    );
    assert!(
        (sample_path(&m.x, &m.y, 1.5, 0.0, 0.0)
            .heading_deg()
            .unwrap()
            - 90.0)
            .abs()
            < 1e-2
    );
}

#[test]
fn motion_path_stationary_has_no_tangent() {
    // A single key (constant position) and an unanimated layer are stationary, so
    // there is no defined heading and auto-orient contributes 0°.
    let one = moving_layer(&[(1.0, 5.0, 5.0)]);
    assert!(sample_path(&one.x, &one.y, 1.0, 0.0, 0.0).tangent.is_none());
    assert_eq!(auto_orient_deg(&one.x, &one.y, 1.0, 0.0, 0.0), 0.0);

    let empty = PulseLayer::new("Still", [1.0; 4]);
    assert!(sample_path(&empty.x, &empty.y, 0.5, 0.0, 0.0).tangent.is_none());

    // A held segment is also stationary between its keys (no interpolation).
    let mut held = moving_layer(&[(0.0, 0.0, 0.0), (2.0, 100.0, 0.0)]);
    held.x.set_interp(0.0, Interp::Hold);
    held.y.set_interp(0.0, Interp::Hold);
    assert!(sample_path(&held.x, &held.y, 1.0, 0.0, 0.0).tangent.is_none());
}

#[test]
fn auto_orient_off_leaves_rotation_unchanged() {
    // Back-compat: with auto_orient off (the serde default), the layer's effective
    // rotation is exactly its keyframed rotation regardless of its motion.
    let mut c = Comp::new();
    c.layers.clear();
    let mut l = moving_layer(&[(0.0, -100.0, 0.0), (2.0, 100.0, 0.0)]);
    l.rotation.set_key(0.0, 30.0); // a constant 30° spin
    assert!(!l.auto_orient);
    c.layers.push(l);
    assert!((c.layer_transform(0, 1.0).rotation_deg - 30.0).abs() < 1e-4);
}

#[test]
fn auto_orient_on_rotates_to_face_travel() {
    // With auto_orient on, the path heading is *added* to the keyframed rotation:
    // a layer heading +y (90°) with a 30° keyed spin reads as 120°.
    let mut c = Comp::new();
    c.layers.clear();
    let mut l = moving_layer(&[(0.0, 0.0, -100.0), (2.0, 0.0, 100.0)]); // heads +y → 90°
    l.auto_orient = true;
    l.rotation.set_key(0.0, 30.0);
    c.layers.push(l);
    assert!((c.layer_transform(0, 1.0).rotation_deg - 120.0).abs() < 1e-2);
}

#[test]
fn auto_orient_serde_roundtrips_and_legacy_off() {
    // Round-trips when set; a pre-auto-orient layer (no field) loads with it off.
    let mut l = PulseLayer::new("L", [1.0; 4]);
    l.auto_orient = true;
    let json = serde_json::to_string(&l).unwrap();
    let back: PulseLayer = serde_json::from_str(&json).unwrap();
    assert!(back.auto_orient);

    let legacy = r#"{"name":"L","color":[1.0,1.0,1.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}}"#;
    let layer: PulseLayer = serde_json::from_str(legacy).unwrap();
    assert!(!layer.auto_orient);
}

#[test]
fn motion_path_sampling_is_deterministic() {
    // The pure sampler returns bit-identical results across repeated calls.
    let m = moving_layer(&[(0.0, 0.0, 0.0), (1.0, 80.0, 20.0), (2.0, 30.0, 90.0)]);
    for &t in &[0.3_f32, 0.75, 1.4, 1.9] {
        let a = sample_path(&m.x, &m.y, t, 0.0, 0.0);
        let b = sample_path(&m.x, &m.y, t, 0.0, 0.0);
        assert_eq!(a, b);
    }
}

// --- Animation presets --------------------------------------------------------

/// Build a layer carrying a representative slice of animatable state: every
/// effect stack populated (from each family's `defaults()`), a generate fill +
/// evolution keys, and keyframes/expression across several transform tracks.
fn rigged_layer() -> PulseLayer {
    let mut l = PulseLayer::new("Source", [0.2, 0.4, 0.6, 1.0]);
    l.effects.push(Effect::defaults()[0]);
    l.effects.push(Effect::defaults()[3]);
    l.spatial_effects.push(SpatialEffect::defaults()[0]);
    l.distort_effects.push(DistortEffect::defaults()[1]);
    l.key_effects.push(KeyEffect::defaults()[4]);
    l.stylize_effects.push(StylizeEffect::defaults()[0]);
    l.generate = Some(GenerateEffect::defaults()[0]);
    l.generate_evolution.set_key(0.0, 0.0);
    l.generate_evolution.set_key(5.0, 6.0);
    // Position animation with an ease, plus a rotation expression.
    l.x.set_key(0.0, -100.0);
    l.x.set_key(2.0, 100.0);
    l.x.set_interp(0.0, Interp::Ease(Ease::EASY));
    l.scale.set_key(0.0, 0.5);
    l.scale.set_key(1.0, 1.5);
    l.rotation.expression = Some("value + time * 90.0".to_string());
    l
}

#[test]
fn preset_capture_then_apply_reproduces_state() {
    let src = rigged_layer();
    let preset = AnimationPreset::capture("Move + grade", &src);

    // Apply onto a fresh, empty layer of a different kind/name/color.
    let mut dst = PulseLayer::of_kind(LayerKind::Solid, "Target", [1.0, 1.0, 1.0, 1.0]);
    preset.apply(&mut dst);

    // Effect stacks reproduced exactly.
    assert_eq!(dst.effects, src.effects);
    assert_eq!(dst.spatial_effects, src.spatial_effects);
    assert_eq!(dst.distort_effects, src.distort_effects);
    assert_eq!(dst.key_effects, src.key_effects);
    assert_eq!(dst.stylize_effects, src.stylize_effects);
    assert_eq!(dst.generate, src.generate);
    assert_eq!(dst.generate_evolution, src.generate_evolution);

    // Captured transform tracks reproduced (values, times, easing, expression).
    assert_eq!(dst.x, src.x);
    assert_eq!(dst.scale, src.scale);
    assert_eq!(dst.rotation, src.rotation);
    assert_eq!(dst.rotation.expression.as_deref(), Some("value + time * 90.0"));

    // Identity/wiring fields untouched by the preset.
    assert_eq!(dst.name, "Target");
    assert_eq!(dst.color, [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn preset_skips_empty_tracks_and_apply_leaves_them_untouched() {
    // A preset that only animates Position.
    let mut src = PulseLayer::new("Slide", [0.0; 4]);
    src.x.set_key(0.0, 0.0);
    src.x.set_key(1.0, 50.0);
    let preset = AnimationPreset::capture("Slide", &src);

    // Only the X track was captured (Scale etc. were empty).
    assert_eq!(preset.tracks.len(), 1);
    let pt: &PresetTrack = &preset.tracks[0];
    assert_eq!(pt.prop, PropTag::X);

    // A target with its own Scale animation keeps it after apply (uncaptured
    // property is left untouched), while X is overwritten by the preset.
    let mut dst = PulseLayer::new("Target", [0.0; 4]);
    dst.scale.set_key(0.0, 2.0);
    dst.x.set_key(0.0, 999.0); // pre-existing X, should be replaced
    preset.apply(&mut dst);

    assert_eq!(dst.x, src.x); // X replaced
    assert_eq!(dst.scale.keys.len(), 1); // Scale preserved
    assert_eq!(dst.scale.keys[0].value, 2.0);
}

#[test]
fn preset_apply_replaces_effect_stacks() {
    // Source has one Levels effect; target already has two unrelated effects.
    let mut src = PulseLayer::new("Src", [0.0; 4]);
    src.effects.push(Effect::defaults()[2]);
    let preset = AnimationPreset::capture("Look", &src);

    let mut dst = PulseLayer::new("Dst", [0.0; 4]);
    dst.effects.push(Effect::defaults()[0]);
    dst.effects.push(Effect::defaults()[1]);
    preset.apply(&mut dst);

    // The whole stack is replaced by the captured one (not merged/appended).
    assert_eq!(dst.effects, src.effects);
    assert_eq!(dst.effects.len(), 1);
}

#[test]
fn preset_serde_round_trip() {
    let preset = AnimationPreset::capture("RT", &rigged_layer());
    let json = serde_json::to_string(&preset).unwrap();
    let back: AnimationPreset = serde_json::from_str(&json).unwrap();
    assert_eq!(back, preset);
}

#[test]
fn project_round_trips_presets() {
    let mut project = Project::new();
    project
        .presets
        .push(AnimationPreset::capture("In project", &rigged_layer()));
    let json = serde_json::to_string(&project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back.presets, project.presets);
}

#[test]
fn legacy_project_loads_with_empty_presets() {
    // A project JSON with no `presets` field (a pre-presets `.pulse` file).
    let legacy = r#"{"comps":[{"id":1,"name":"C","width":100,"height":100,
        "duration":1.0,"fps":30.0,"layers":[]}],"active":0,"next_id":2}"#;
    let project: Project = serde_json::from_str(legacy).unwrap();
    assert!(project.presets.is_empty());
}

#[test]
fn project_with_layers_keyframes_and_preset_round_trips_via_bytes() {
    // The acceptance bar for File ▸ Open: a project carrying a rigged layer (with
    // effects + keyframes + an expression) *and* a saved animation preset must
    // survive serialize → deserialize → re-serialize unchanged. `Project`/`Comp`
    // don't derive `PartialEq`, so we compare the canonical JSON of the loaded
    // project against the original's — a byte-exact document round-trip.
    let mut comp = Comp::new();
    comp.id = 1;
    comp.layers.push(rigged_layer());
    let mut project = Project {
        comps: vec![comp],
        active: 0,
        next_id: 2,
        presets: vec![AnimationPreset::capture("Saved", &rigged_layer())],
    };
    // A second comp so the comp tree (not just one comp) round-trips.
    let mut b = Comp::empty_like("B", &Comp::new());
    b.id = 2;
    project.comps.push(b);
    project.next_id = 3;

    let bytes = serde_json::to_vec(&project).unwrap();
    let loaded: Project = serde_json::from_slice(&bytes).expect("deserialize round-trip");

    assert_eq!(loaded.comps.len(), 2);
    assert_eq!(loaded.presets.len(), 1);
    assert_eq!(loaded.next_id, 3);
    assert_eq!(loaded.comps[0].layers[0].x, project.comps[0].layers[0].x);
    assert_eq!(loaded.presets, project.presets);
    // Whole-document equality via canonical serialization (no PartialEq needed).
    assert_eq!(
        serde_json::to_string(&loaded).unwrap(),
        serde_json::to_string(&project).unwrap(),
        "project document round-trips byte-for-byte"
    );
}

#[test]
fn malformed_project_json_returns_err_not_panic() {
    // The Open path must never panic on a bad file — it returns Err and leaves the
    // current project intact. Exercise the same deserialize the loader uses.
    assert!(serde_json::from_slice::<Project>(b"not json at all {{{").is_err());
    assert!(serde_json::from_slice::<Project>(b"").is_err());
    // Structurally valid JSON but the wrong shape (missing required `comps`).
    assert!(serde_json::from_slice::<Project>(br#"{"active":0}"#).is_err());
}

#[test]
fn preset_capture_is_deterministic() {
    let src = rigged_layer();
    let a = AnimationPreset::capture("Same", &src);
    let b = AnimationPreset::capture("Same", &src);
    assert_eq!(a, b);
    // Apply is deterministic too.
    let mut d1 = PulseLayer::new("d", [0.0; 4]);
    let mut d2 = PulseLayer::new("d", [0.0; 4]);
    a.apply(&mut d1);
    a.apply(&mut d2);
    assert_eq!(serde_json::to_string(&d1).unwrap(), serde_json::to_string(&d2).unwrap());
}

// ---------------------------------------------------------------------------
// Roving keyframes (Rove Across Time): constant-velocity spatial re-timing.
// ---------------------------------------------------------------------------

/// Build a `RoveKey` from `(t, x, y, roving)` for terse test fixtures.
fn rk(t: f32, x: f32, y: f32, roving: bool) -> RoveKey {
    RoveKey {
        t,
        pos: [x, y],
        roving,
    }
}

#[test]
fn roved_times_endpoints_never_move() {
    // Endpoints are always anchored even if (illegally) flagged roving.
    let keys = [
        rk(0.0, 0.0, 0.0, true),
        rk(1.0, 50.0, 0.0, true),
        rk(2.0, 100.0, 0.0, true),
    ];
    let times = roved_times(&keys);
    assert_eq!(times[0], 0.0, "first key pinned");
    assert_eq!(times[2], 2.0, "last key pinned");
}

#[test]
fn roved_times_even_spacing_unchanged() {
    // Evenly-spaced positions in time AND space: an interior roving key keeps its
    // time (the path is already constant-velocity).
    let keys = [
        rk(0.0, 0.0, 0.0, false),
        rk(1.0, 50.0, 0.0, true), // roves
        rk(2.0, 100.0, 0.0, false),
    ];
    let times = roved_times(&keys);
    assert!(
        (times[1] - 1.0).abs() < 1e-4,
        "even path => time ~unchanged, got {}",
        times[1]
    );
}

#[test]
fn roved_times_uneven_equalizes_velocity() {
    // Unevenly-spaced positions: the interior key sits at 10% of the distance but
    // 50% of the authored time. Roving re-times it to 10% of the time so per-leg
    // speed equalizes.
    let keys = [
        rk(0.0, 0.0, 0.0, false),
        rk(1.0, 10.0, 0.0, true), // close to start in space
        rk(2.0, 100.0, 0.0, false),
    ];
    let times = roved_times(&keys);
    // Total path 100; first leg 10 => 10% of the 2s span = 0.2s.
    assert!((times[1] - 0.2).abs() < 1e-3, "expected ~0.2, got {}", times[1]);

    // Velocity check: distance/time over each leg should be ~equal.
    let v0 = 10.0 / (times[1] - times[0]);
    let v1 = 90.0 / (times[2] - times[1]);
    assert!(
        (v0 - v1).abs() / v0 < 1e-2,
        "legs should be ~constant velocity: {v0} vs {v1}"
    );
}

#[test]
fn roved_times_roving_off_unchanged() {
    // No roving flag anywhere => authored times preserved exactly (back-compat).
    let keys = [
        rk(0.0, 0.0, 0.0, false),
        rk(0.3, 10.0, 0.0, false),
        rk(2.0, 100.0, 0.0, false),
    ];
    let times = roved_times(&keys);
    assert_eq!(times, vec![0.0, 0.3, 2.0]);
}

#[test]
fn roved_times_two_keys_is_noop() {
    let keys = [rk(0.0, 0.0, 0.0, true), rk(2.0, 100.0, 0.0, true)];
    assert_eq!(roved_times(&keys), vec![0.0, 2.0]);
}

#[test]
fn roved_times_multiple_roving_in_segment() {
    // Two interior roving keys between the same anchored pair, redistributed by
    // arc length. Positions at 10 and 40 of a 100-long path over a 2s span =>
    // times 0.2 and 0.8.
    let keys = [
        rk(0.0, 0.0, 0.0, false),
        rk(0.5, 10.0, 0.0, true),
        rk(1.0, 40.0, 0.0, true),
        rk(2.0, 100.0, 0.0, false),
    ];
    let times = roved_times(&keys);
    assert!((times[1] - 0.2).abs() < 1e-3, "got {}", times[1]);
    assert!((times[2] - 0.8).abs() < 1e-3, "got {}", times[2]);
    // Strictly increasing.
    assert!(times[0] < times[1] && times[1] < times[2] && times[2] < times[3]);
}

#[test]
fn roved_times_anchored_interior_splits_segments() {
    // An anchored interior key splits the run into two independent segments; the
    // roving key in each is re-timed within its own span only.
    let keys = [
        rk(0.0, 0.0, 0.0, false),
        rk(1.0, 10.0, 0.0, true),   // roving in [0, 2]
        rk(2.0, 100.0, 0.0, false), // anchored interior
        rk(3.0, 110.0, 0.0, true),  // roving in [2, 4]
        rk(4.0, 200.0, 0.0, false),
    ];
    let times = roved_times(&keys);
    assert_eq!(times[2], 2.0, "anchored interior pinned");
    assert!((times[1] - 0.2).abs() < 1e-3, "first segment, got {}", times[1]);
    assert!(
        (times[3] - 2.2).abs() < 1e-3,
        "second segment, got {}",
        times[3]
    );
}

#[test]
fn roved_times_coincident_positions_even_fallback() {
    // Zero-length path (all positions identical): fall back to even spacing so the
    // keys never collapse onto one instant.
    let keys = [
        rk(0.0, 5.0, 5.0, false),
        rk(0.1, 5.0, 5.0, true),
        rk(0.2, 5.0, 5.0, true),
        rk(3.0, 5.0, 5.0, false),
    ];
    let times = roved_times(&keys);
    assert!(
        (times[1] - 1.0).abs() < 1e-3,
        "even 1/3 of span, got {}",
        times[1]
    );
    assert!(
        (times[2] - 2.0).abs() < 1e-3,
        "even 2/3 of span, got {}",
        times[2]
    );
}

#[test]
fn roved_times_is_deterministic() {
    let keys = [
        rk(0.0, 0.0, 0.0, false),
        rk(1.0, 7.0, 3.0, true),
        rk(1.5, 80.0, 9.0, true),
        rk(2.0, 100.0, 0.0, false),
    ];
    assert_eq!(roved_times(&keys), roved_times(&keys));
}

#[test]
fn roved_tracks_remaps_position_for_constant_velocity() {
    // A layer-style x/y pair: the interior key is close in space to the start but
    // halfway in time. Roving re-times it so sampling reflects constant velocity.
    let mut x = Track::default();
    let mut y = Track::default();
    x.set_key(0.0, 0.0);
    x.set_key(1.0, 10.0);
    x.set_key(2.0, 100.0);
    y.set_key(0.0, 0.0);
    y.set_key(1.0, 0.0);
    y.set_key(2.0, 0.0);
    // Mark the interior x/y keys roving (the UI does this on both tracks).
    x.set_roving(1.0, true);
    y.set_roving(1.0, true);
    assert!(has_roving(&x, &y));

    let (rx, _ry) = roved_tracks(&x, &y);
    // The interior key moved from t=1.0 to ~0.2.
    let moved = rx.keys[1].t;
    assert!(
        (moved - 0.2).abs() < 1e-3,
        "interior x re-timed to ~0.2, got {moved}"
    );

    // Sampling at t=1.0 (mid authored time): with constant velocity the layer is
    // already well past x=10 (it would have stuck at 10 without roving).
    let xat1 = rx.sample(1.0, 0.0);
    assert!(
        xat1 > 40.0,
        "constant velocity puts the layer past mid-path by t=1, got {xat1}"
    );
}

#[test]
fn roved_tracks_no_roving_is_borrow_identical() {
    // Without roving flags, the roved copy equals the original (back-compat).
    let mut x = Track::default();
    let mut y = Track::default();
    x.set_key(0.0, 0.0);
    x.set_key(0.3, 10.0);
    x.set_key(2.0, 100.0);
    y.set_key(0.0, 0.0);
    y.set_key(2.0, 50.0);
    assert!(!has_roving(&x, &y));
    let (rx, ry) = roved_tracks(&x, &y);
    assert_eq!(rx, x);
    assert_eq!(ry, y);
}

#[test]
fn layer_value_honours_roving() {
    // The layer's position sampling routes through the roving re-timer.
    let mut layer = PulseLayer::new("rover", [1.0; 4]);
    layer.x.set_key(0.0, 0.0);
    layer.x.set_key(1.0, 10.0);
    layer.x.set_key(2.0, 100.0);
    layer.y.set_key(0.0, 0.0);
    layer.y.set_key(1.0, 0.0);
    layer.y.set_key(2.0, 0.0);
    let before = layer.value(Prop::X, 1.0);
    assert!((before - 10.0).abs() < 1e-4, "no roving => x=10 at t=1");

    layer.x.set_roving(1.0, true);
    layer.y.set_roving(1.0, true);
    let after = layer.value(Prop::X, 1.0);
    assert!(
        after > 40.0,
        "roving => layer is past mid-path at t=1, got {after}"
    );
}

#[test]
fn keyframe_roving_serde_roundtrips_and_defaults() {
    // A roving key round-trips; legacy JSON (no `roving` field) defaults to false.
    let mut t = Track::default();
    t.set_key(0.0, 0.0);
    t.set_key(1.0, 10.0);
    t.set_key(2.0, 100.0);
    t.set_roving(1.0, true);
    let json = serde_json::to_string(&t).unwrap();
    let back: Track = serde_json::from_str(&json).unwrap();
    assert_eq!(back, t, "roving track round-trips");
    assert!(back.keys[1].roving);
    // Exactly one key carries the flag, so it serializes once (non-roving keys
    // skip it, keeping pre-roving files byte-identical).
    assert_eq!(
        json.matches("roving").count(),
        1,
        "only the one roving key emits the flag: {json}"
    );

    // Legacy keyframe JSON without `roving` defaults to false.
    let legacy = r#"{"t":0.0,"value":5.0}"#;
    let kf: Keyframe = serde_json::from_str(legacy).unwrap();
    assert!(!kf.roving);
}

// --- 3-D layers + camera (perspective projection + z-sort) -----------------

/// A 3-D-capable comp: a single solid layer, default camera sized to the comp.
fn comp_3d() -> Comp {
    let mut c = parented_comp();
    c.layers.truncate(1);
    c.layers[0].threed = true;
    // Match `Comp::new`'s default-camera placement for the comp height.
    c.camera = Camera::default();
    c.camera.position = [0.0, 0.0, -Camera::default_distance(c.height as f32)];
    c
}

#[test]
fn default_camera_projects_z0_plane_to_identity() {
    // A point on the z = 0 plane projects to itself at unit scale under the
    // default camera — the back-compat guarantee.
    let cam = Camera {
        position: [0.0, 0.0, -Camera::default_distance(100.0)],
        ..Camera::default()
    };
    let p = cam.project(20.0, -30.0, 0.0, 100.0);
    assert!(approx(p.screen, (20.0, -30.0)), "screen {:?}", p.screen);
    assert!((p.scale - 1.0).abs() < 1e-4, "scale {}", p.scale);
}

#[test]
fn pushing_z_farther_shrinks_projected_scale() {
    let cam = Camera {
        position: [0.0, 0.0, -Camera::default_distance(100.0)],
        ..Camera::default()
    };
    let near = cam.project(10.0, 0.0, 0.0, 100.0);
    let far = cam.project(10.0, 0.0, 200.0, 100.0);
    assert!(far.scale < near.scale, "far {} < near {}", far.scale, near.scale);
    // Projected x shrinks with scale (perspective foreshortening toward center).
    assert!(far.screen.0.abs() < near.screen.0.abs());
    // Pulling closer toward the camera (negative z, still in front of it)
    // enlarges it.
    let closer = cam.project(10.0, 0.0, -50.0, 100.0);
    assert!(closer.scale > near.scale, "closer {} > near {}", closer.scale, near.scale);
}

#[test]
fn layer_world_z0_equals_2d_world_matrix() {
    // A 3-D layer at Z = 0 with no orientation projects to exactly its 2-D
    // world matrix under the default camera (back-compat at the layer level).
    let mut c = comp_3d();
    c.layers[0].x.set_key(0.0, 30.0);
    c.layers[0].y.set_key(0.0, -15.0);
    c.layers[0].scale.set_key(0.0, 1.3);
    c.layers[0].rotation.set_key(0.0, 20.0);
    let world_2d = c.world_matrix(0, 0.0);
    let world_3d = c.layer_world(0, 0.0).expect("non-degenerate");
    // Corners must coincide.
    for (lx, ly) in [(0.0, 0.0), (50.0, 0.0), (0.0, 40.0), (-30.0, 25.0)] {
        assert!(
            approx(world_2d.apply(lx, ly), world_3d.apply(lx, ly)),
            "2d {:?} vs 3d {:?}",
            world_2d.apply(lx, ly),
            world_3d.apply(lx, ly),
        );
    }
}

#[test]
fn z_depth_shrinks_layer_world_scale() {
    // A 3-D layer pushed in Z is projected smaller than at Z = 0.
    let mut c = comp_3d();
    let at0 = c.layer_world(0, 0.0).unwrap();
    c.layers[0].z.set_key(0.0, 400.0);
    let atz = c.layer_world(0, 0.0).unwrap();
    let area = |m: &Affine2| (m.a * m.d - m.b * m.c).abs();
    assert!(area(&atz) < area(&at0), "z-pushed area must shrink");
}

#[test]
fn orientation_rotates_the_projected_quad() {
    // A Z orientation rolls the projected quad; the +x edge no longer points
    // straight along comp +x.
    let mut c = comp_3d();
    c.layers[0].orient_z.set_key(0.0, 90.0);
    let m = c.layer_world(0, 0.0).unwrap();
    let o = m.apply(0.0, 0.0);
    let ex = m.apply(1.0, 0.0);
    // After a 90° roll the local +x edge maps to (≈0, +something) in comp space.
    assert!((ex.0 - o.0).abs() < 1e-3, "x-edge x-comp ~ 0");
    assert!((ex.1 - o.1).abs() > 0.5, "x-edge gained y-comp");
}

#[test]
fn draw_order_sorts_3d_layers_by_depth_regardless_of_stack() {
    // Two 3-D layers; the one with larger Z (farther) must be drawn first
    // regardless of stack order.
    let mut c = comp_3d();
    c.layers.push(PulseLayer::new("B", [1.0; 4]));
    c.layers[1].threed = true;
    // Layer 0 near (small Z), layer 1 far (large Z).
    c.layers[0].z.set_key(0.0, -100.0);
    c.layers[1].z.set_key(0.0, 500.0);
    let order = c.draw_order(0.0);
    // Far layer (1) drawn before near layer (0).
    let pos0 = order.iter().position(|&i| i == 0).unwrap();
    let pos1 = order.iter().position(|&i| i == 1).unwrap();
    assert!(pos1 < pos0, "far layer drawn first: {order:?}");
    // Swapping depths flips the order.
    c.layers[0].z.set_key(0.0, 500.0);
    c.layers[1].z.set_key(0.0, -100.0);
    let order2 = c.draw_order(0.0);
    let p0 = order2.iter().position(|&i| i == 0).unwrap();
    let p1 = order2.iter().position(|&i| i == 1).unwrap();
    assert!(p0 < p1, "depths swapped → order flips: {order2:?}");
}

#[test]
fn draw_order_identity_with_no_3d_layers() {
    // No 3-D layers → identity order (back-compat: the draw loop is unchanged).
    let c = parented_comp();
    assert_eq!(c.draw_order(0.0), vec![0, 1]);
}

#[test]
fn draw_order_keeps_2d_layers_in_place() {
    // 2-D layers keep their exact stack slots; only the 3-D slots are re-sorted.
    let mut c = comp_3d(); // layer 0 is 3-D
    c.layers.push(PulseLayer::new("two_d", [1.0; 4])); // 1: 2-D
    c.layers.push(PulseLayer::new("three_d", [1.0; 4])); // 2: 3-D
    c.layers[2].threed = true;
    c.layers[0].z.set_key(0.0, 0.0);
    c.layers[2].z.set_key(0.0, 800.0); // far → should come first among 3-D slots
    let order = c.draw_order(0.0);
    // The 2-D layer (index 1) stays in slot 1.
    assert_eq!(order[1], 1, "2-D layer stays put: {order:?}");
    // The 3-D slots {0, 2} hold the depth-sorted 3-D layers (far=2 first).
    assert_eq!(order[0], 2, "far 3-D layer fills the first 3-D slot: {order:?}");
    assert_eq!(order[2], 0);
}

#[test]
fn focal_length_round_trips_through_fov() {
    let mut cam = Camera::default();
    let (w, h) = (1280.0, 720.0);
    cam.set_focal_length(50.0, w, h);
    let back = cam.focal_length(w, h);
    assert!((back - 50.0).abs() < 0.5, "focal round-trip: {back}");
}

#[test]
fn rotate_orientation_identity_when_zero() {
    let p = rotate_orientation(3.0, -4.0, 0.0, 0.0, 0.0, 0.0);
    assert!((p.0 - 3.0).abs() < 1e-6 && (p.1 + 4.0).abs() < 1e-6 && p.2.abs() < 1e-6);
}

#[test]
fn camera_serde_round_trip_and_legacy_default() {
    // A comp with a camera + 3-D layer round-trips through JSON.
    let mut c = comp_3d();
    c.layers[0].z.set_key(0.0, 120.0);
    c.layers[0].orient_y.set_key(0.0, 30.0);
    c.camera.fov_deg = 40.0;
    let json = serde_json::to_string(&c).unwrap();
    let back: Comp = serde_json::from_str(&json).unwrap();
    assert!((back.camera.fov_deg - 40.0).abs() < 1e-4);
    assert!(back.layers[0].threed);
    assert!((back.layer_z(0, 0.0) - 120.0).abs() < 1e-4);

    // A legacy comp JSON with no `camera` / 3-D keys loads with the default
    // camera and a 2-D layer (renders unchanged).
    let legacy = r#"{
        "id":0,"name":"L","width":64,"height":64,"duration":1.0,"fps":30.0,
        "layers":[{"name":"S","color":[1.0,1.0,1.0,1.0],"visible":true,
            "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
            "rotation":{"keys":[]},"opacity":{"keys":[]}}]
    }"#;
    let lc: Comp = serde_json::from_str(legacy).unwrap();
    assert_eq!(lc.camera, Camera::default());
    assert!(!lc.layers[0].threed);
    assert!(!lc.layer_is_3d(0));
}

#[test]
fn layer_world_is_deterministic() {
    let mut c = comp_3d();
    c.layers[0].z.set_key(0.0, 250.0);
    c.layers[0].orient_x.set_key(0.0, 25.0);
    let a = c.layer_world(0, 0.0).unwrap();
    let b = c.layer_world(0, 0.0).unwrap();
    assert_eq!(a, b);
}
