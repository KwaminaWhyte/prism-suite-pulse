// `GenerateEffect` has a single variant today, so destructuring it with `let`
// is irrefutable; the generate tests keep that form so they stay correct when
// more generate variants are added.
#![allow(irrefutable_let_patterns)]
use super::effect::{curve_eval, hsl_to_rgb, rgb_to_hsl, smoothstep};
use super::keyframe::{cubic_bezier, solve_bezier_x};
use super::mask::{dist_to_polygon, point_in_polygon};
use super::spatial::{gaussian_blur, gaussian_kernel};
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

// --- Layer kinds --------------------------------------------------------

#[test]
fn only_solid_draws_own_pixels() {
    assert!(LayerKind::Solid.draws_own_pixels());
    assert!(!LayerKind::Null.draws_own_pixels());
    assert!(!LayerKind::Adjustment.draws_own_pixels());
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

#[test]
fn generate_at_uses_static_evolution_when_track_empty() {
    // No evolution keys → generate_at returns the static field unchanged.
    let mut gen = GenerateEffect::defaults()[0];
    let GenerateEffect::FractalNoise { evolution, .. } = &mut gen;
    *evolution = 3.0;
    let mut layer = PulseLayer::new("L", [1.0; 4]);
    layer.generate = Some(gen);
    let GenerateEffect::FractalNoise { evolution, .. } = layer.generate_at(0.0).unwrap();
    assert_eq!(evolution, 3.0);
    let GenerateEffect::FractalNoise { evolution, .. } = layer.generate_at(5.0).unwrap();
    assert_eq!(evolution, 3.0, "static evolution is constant over time");
}

#[test]
fn generate_at_track_overrides_static_evolution() {
    // A keyed evolution track overrides the static field at the sampled time.
    let mut layer = PulseLayer::new("L", [1.0; 4]);
    layer.generate = Some(GenerateEffect::defaults()[0]);
    layer.generate_evolution.set_key(0.0, 0.0);
    layer.generate_evolution.set_key(2.0, 10.0);
    let GenerateEffect::FractalNoise { evolution, .. } = layer.generate_at(0.0).unwrap();
    assert!((evolution - 0.0).abs() < 1e-5);
    let GenerateEffect::FractalNoise { evolution, .. } = layer.generate_at(1.0).unwrap();
    assert!((evolution - 5.0).abs() < 1e-4, "linear interp at midpoint, got {evolution}");
    let GenerateEffect::FractalNoise { evolution, .. } = layer.generate_at(2.0).unwrap();
    assert!((evolution - 10.0).abs() < 1e-4);
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
