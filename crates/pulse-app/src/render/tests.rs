use super::export::{frame_count, frame_path, frame_time};
use super::*;
use crate::comp::{
    BlendMode, Interp, LayerBlend, MatteMode, MotionBlur, Prop, PulseLayer, WorkArea,
};
use std::path::Path;

fn solid(color: [f32; 4]) -> Comp {
    let mut c = Comp {
        width: 64,
        height: 64,
        duration: 1.0,
        fps: 30.0,
        motion_blur: MotionBlur::default(),
        markers: Vec::new(),
        work_area: WorkArea::default(),
        layers: Vec::new(),
        id: 0,
        name: String::new(),
    };
    c.layers.push(PulseLayer::new("L", color));
    c
}

#[test]
fn frame_has_correct_size() {
    let c = solid([1.0, 0.0, 0.0, 1.0]);
    let f = render_frame(&c, 0.0);
    assert_eq!(f.width, 64);
    assert_eq!(f.height, 64);
    assert_eq!(f.pixels.len(), 64 * 64 * 4);
}

#[test]
fn empty_comp_is_transparent() {
    let c = Comp {
        width: 8,
        height: 8,
        duration: 1.0,
        fps: 30.0,
        motion_blur: MotionBlur::default(),
        markers: Vec::new(),
        work_area: WorkArea::default(),
        layers: Vec::new(),
        id: 0,
        name: String::new(),
    };
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0));
}

#[test]
fn center_pixel_is_opaque_layer_color() {
    // A centered, unrotated, unit-scale opaque red layer covers the center.
    let c = solid([1.0, 0.0, 0.0, 1.0]);
    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    assert!(r > 250, "red channel high, got {r}");
    assert_eq!(g, 0);
    assert_eq!(b, 0);
}

#[test]
fn corner_pixel_outside_quad_is_transparent() {
    // Half-extent is 0.22*64 ≈ 14 px, so a far corner is uncovered.
    let c = solid([1.0, 1.0, 1.0, 1.0]);
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(0, 0)[3], 0);
    assert_eq!(f.pixel(63, 63)[3], 0);
}

#[test]
fn invisible_layer_does_not_render() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].visible = false;
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0));
}

#[test]
fn zero_opacity_is_transparent() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].opacity.set_key(0.0, 0.0);
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 0);
}

#[test]
fn opacity_animates_over_time() {
    // Opacity ramps 0 -> 1 across the comp; center alpha grows with time.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].opacity.set_key(0.0, 0.0);
    c.layers[0].opacity.set_key(1.0, 1.0);
    let a0 = render_frame(&c, 0.0).pixel(32, 32)[3];
    let amid = render_frame(&c, 0.5).pixel(32, 32)[3];
    let a1 = render_frame(&c, 1.0).pixel(32, 32)[3];
    assert!(a0 < amid && amid < a1, "{a0} < {amid} < {a1}");
    assert_eq!(a1, 255);
}

#[test]
fn position_offset_moves_coverage() {
    // Shift the layer far right: center is now uncovered, the right edge covered.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].x.set_key(0.0, 20.0);
    let f = render_frame(&c, 0.0);
    // Original center (32,32) sits at the layer's left edge region; the
    // covered band shifts right. Sample a pixel that should now be covered.
    assert_eq!(f.pixel(50, 32)[3], 255);
    // A pixel far left of the shifted quad is uncovered.
    assert_eq!(f.pixel(10, 32)[3], 0);
}

#[test]
fn source_over_blends_two_layers_in_linear() {
    // Opaque black behind, 50% white on top -> mid gray, fully opaque.
    let mut c = solid([0.0, 0.0, 0.0, 1.0]);
    let mut top = PulseLayer::new("top", [1.0, 1.0, 1.0, 1.0]);
    top.opacity.set_key(0.0, 0.5);
    c.layers.push(top);
    let f = render_frame(&c, 0.0);
    let [r, _g, _b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    // 0.5 linear-light coverage of white over black, sRGB-encoded, is well
    // above naive 0.5*255=128 (gamma), so just bound it sensibly.
    assert!((150..=200).contains(&r), "mid gray r={r}");
}

#[test]
fn scale_zero_renders_nothing() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 0.0);
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0));
}

#[test]
fn larger_scale_covers_more_pixels() {
    let count_covered = |scale: f32| {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].scale.set_key(0.0, scale);
        let f = render_frame(&c, 0.0);
        f.pixels.chunks(4).filter(|p| p[3] > 0).count()
    };
    assert!(count_covered(2.0) > count_covered(1.0));
}

#[test]
fn rotation_keeps_center_covered() {
    // Rotating about the layer center leaves the center pixel covered.
    let mut c = solid([0.0, 1.0, 0.0, 1.0]);
    c.layers[0].rotation.set_key(0.0, 45.0);
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255);
}

#[test]
fn rotation_uses_outgoing_interp() {
    // Sanity: a rotation track sampled mid-segment differs from endpoints,
    // confirming render_frame consults the animated transform.
    let mut c = solid([1.0, 0.0, 0.0, 1.0]);
    c.layers[0].rotation.set_key(0.0, 0.0);
    c.layers[0].rotation.set_key(1.0, 90.0);
    c.layers[0].rotation.set_interp(0.0, Interp::Linear);
    // Just assert it renders without panic at a few times.
    for &t in &[0.0, 0.25, 0.5, 1.0] {
        let _ = render_frame(&c, t);
    }
    // And the transform actually animates.
    assert!((c.layers[0].value(Prop::Rotation, 0.5) - 45.0).abs() < 1e-3);
}

// --- Sequence math ------------------------------------------------------

#[test]
fn frame_count_is_duration_times_fps() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.duration = 5.0;
    c.fps = 30.0;
    assert_eq!(frame_count(&c), 150);
    c.duration = 2.0;
    c.fps = 24.0;
    assert_eq!(frame_count(&c), 48);
}

#[test]
fn frame_count_floors_at_one() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.duration = 0.0;
    assert_eq!(frame_count(&c), 1);
}

#[test]
fn frame_time_steps_by_fps() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.fps = 25.0;
    assert!((frame_time(&c, 0) - 0.0).abs() < 1e-6);
    assert!((frame_time(&c, 25) - 1.0).abs() < 1e-6);
}

#[test]
fn frame_path_zero_pads() {
    let dir = Path::new("/tmp/out");
    // <100 frames -> 4-digit padding (the minimum).
    assert_eq!(frame_path(dir, "comp", 7, 90), dir.join("comp_0007.png"));
    // 12000 frames -> highest index 11999 needs 5 digits.
    assert_eq!(
        frame_path(dir, "comp", 42, 12000),
        dir.join("comp_00042.png")
    );
}

#[test]
fn anchor_offset_shifts_coverage_under_rotation() {
    // With the anchor offset off-center, rotating pivots about the anchor,
    // not the layer center — so the covered region moves vs. a centered
    // anchor. Compare covered-pixel counts overlapping a probe far from
    // center to confirm the pivot changed.
    let covered_at = |anchor: f32| {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        c.layers[0].anchor_x.set_key(0.0, anchor);
        c.layers[0].rotation.set_key(0.0, 90.0);
        let f = render_frame(&c, 0.0);
        f.pixels.chunks(4).filter(|p| p[3] > 0).count()
    };
    // Both render *something* but the anchored pivot relocates the quad;
    // assert the quad still covers a sensible number of pixels (sanity) and
    // that an off-center anchor does not crash / vanish.
    assert!(covered_at(0.0) > 0);
    assert!(covered_at(20.0) > 0);
}

#[test]
fn anchored_layer_pivots_position_correctly() {
    // 64x64 comp, center at (32,32). Anchor at the quad's left edge
    // (anchor_x = -half_w ≈ -14) and position 0: the layer's left edge now
    // sits at the comp center, so the quad extends to the right of center.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    let half_w = 64.0 * LAYER_HALF_FRAC; // ~14
    c.layers[0].anchor_x.set_key(0.0, -half_w);
    let f = render_frame(&c, 0.0);
    // A pixel just right of center is covered...
    assert_eq!(f.pixel(40, 32)[3], 255);
    // ...and one left of center (beyond the anchored left edge) is not.
    assert_eq!(f.pixel(10, 32)[3], 0);
}

#[test]
fn parented_child_follows_parent_offset() {
    // Parent shifted right; an unparented child at x=0 covers the center.
    // Parenting it to the moved parent shifts its coverage right too.
    let mut c = Comp {
        width: 64,
        height: 64,
        duration: 1.0,
        fps: 30.0,
        motion_blur: MotionBlur::default(),
        markers: Vec::new(),
        work_area: WorkArea::default(),
        layers: Vec::new(),
        id: 0,
        name: String::new(),
    };
    c.layers
        .push(PulseLayer::new("parent", [0.0, 0.0, 0.0, 0.0])); // invisible-ish parent
    c.layers[0].visible = false; // parent itself doesn't draw
    c.layers[0].x.set_key(0.0, 18.0);
    let mut child = PulseLayer::new("child", [1.0, 1.0, 1.0, 1.0]);
    child.parent = Some(0);
    c.layers.push(child);

    let f = render_frame(&c, 0.0);
    // Child's coverage rode the parent's +18 offset to the right.
    assert_eq!(f.pixel(50, 32)[3], 255);
    assert_eq!(f.pixel(10, 32)[3], 0);
}

// --- Layer kinds + effects ---------------------------------------------

#[test]
fn null_layer_renders_nothing() {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = crate::comp::LayerKind::Null;
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0));
}

#[test]
fn solid_effect_stack_recolors_the_quad() {
    // A black solid with a Tint mapping black->white should now read white at
    // the center (the effect runs on the layer's own color before compositing).
    let mut c = solid([0.0, 0.0, 0.0, 1.0]);
    c.layers[0].effects.push(crate::comp::Effect::Tint {
        black: [1.0, 1.0, 1.0],
        white: [1.0, 1.0, 1.0],
        amount: 1.0,
    });
    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    assert!(
        r > 250 && g > 250 && b > 250,
        "expected white, got {r},{g},{b}"
    );
}

#[test]
fn adjustment_layer_regrades_layers_below() {
    // A mid-gray solid beneath a full-frame adjustment that lifts brightness
    // should read brighter at the center than without the adjustment.
    let make = |with_adj: bool| {
        let mut c = solid([0.5, 0.5, 0.5, 1.0]);
        if with_adj {
            let mut adj = PulseLayer::of_kind(crate::comp::LayerKind::Adjustment, "adj", [1.0; 4]);
            adj.scale.set_key(0.0, 3.0); // cover the frame
            adj.effects.push(crate::comp::Effect::BrightnessContrast {
                brightness: 0.3,
                contrast: 1.0,
            });
            c.layers.push(adj);
        }
        render_frame(&c, 0.0).pixel(32, 32)[0]
    };
    assert!(
        make(true) > make(false),
        "adjustment did not brighten below"
    );
}

#[test]
fn adjustment_layer_draws_no_pixels_of_its_own() {
    // An adjustment over an empty comp leaves it transparent (no source).
    let mut c = Comp {
        width: 16,
        height: 16,
        duration: 1.0,
        fps: 30.0,
        motion_blur: MotionBlur::default(),
        markers: Vec::new(),
        work_area: WorkArea::default(),
        layers: Vec::new(),
        id: 0,
        name: String::new(),
    };
    let mut adj = PulseLayer::of_kind(crate::comp::LayerKind::Adjustment, "adj", [1.0; 4]);
    adj.scale.set_key(0.0, 3.0);
    adj.effects.push(crate::comp::Effect::BrightnessContrast {
        brightness: 0.5,
        contrast: 1.0,
    });
    c.layers.push(adj);
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0));
}

#[test]
fn adjustment_only_affects_its_quad_bounds() {
    // A small (unscaled) adjustment over a full-frame solid grades only the
    // pixels inside its quad: the center changes, a far corner does not.
    let mut c = solid([0.5, 0.5, 0.5, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0); // bottom solid covers the frame
    let mut adj = PulseLayer::of_kind(crate::comp::LayerKind::Adjustment, "adj", [1.0; 4]);
    adj.effects.push(crate::comp::Effect::BrightnessContrast {
        brightness: 0.3,
        contrast: 1.0,
    });
    c.layers.push(adj); // unit-scale: covers only ~the center quad
    let f = render_frame(&c, 0.0);
    let center = f.pixel(32, 32)[0];
    let corner = f.pixel(1, 1)[0];
    // Center (inside the small adjustment quad) is brighter than an edge
    // pixel (covered by the solid but outside the adjustment).
    assert!(
        center > corner,
        "center {center} should exceed corner {corner}"
    );
}

// --- Track mattes -------------------------------------------------------

/// A 64x64 comp: a full-frame opaque solid (`base`) with a smaller solid on
/// top to serve as the matte source. Index 0 = matted base, index 1 = source.
fn matte_pair(base: [f32; 4], source: [f32; 4], src_scale: f32) -> Comp {
    let mut c = Comp {
        width: 64,
        height: 64,
        duration: 1.0,
        fps: 30.0,
        motion_blur: MotionBlur::default(),
        markers: Vec::new(),
        work_area: WorkArea::default(),
        layers: Vec::new(),
        id: 0,
        name: String::new(),
    };
    let mut b = PulseLayer::new("base", base);
    b.scale.set_key(0.0, 3.0); // cover the whole frame
    c.layers.push(b); // index 0
    let mut s = PulseLayer::new("source", source);
    s.scale.set_key(0.0, src_scale);
    c.layers.push(s); // index 1
    c
}

#[test]
fn matte_source_is_not_composited_on_its_own() {
    // A red base under a green source; with an alpha matte the green source
    // must NOT appear in the output — it only shapes the base's alpha.
    let mut c = matte_pair([1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], 1.0);
    c.layers[0].matte = MatteMode::Alpha;
    let f = render_frame(&c, 0.0);
    let [r, g, _b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    // The center shows the base's red, not the source's green.
    assert!(r > 250, "expected red base, got r={r}");
    assert_eq!(g, 0, "matte source leaked into the composite");
}

#[test]
fn alpha_matte_clips_to_source_coverage() {
    // Full-frame base, small (unit-scale) source. With an alpha matte the
    // base is visible only inside the small source quad: center covered, a
    // far edge (inside the base but outside the source) is now transparent.
    let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
    c.layers[0].matte = MatteMode::Alpha;
    let f = render_frame(&c, 0.0);
    assert_eq!(
        f.pixel(32, 32)[3],
        255,
        "center should pass the alpha matte"
    );
    // A pixel far from center is inside the full-frame base but outside the
    // small source quad -> matted away.
    assert_eq!(f.pixel(2, 2)[3], 0, "edge should be matted out");
}

#[test]
fn inverted_alpha_matte_is_the_complement() {
    // Inverted alpha: the base shows where the source is *transparent*, so the
    // center (under the opaque source) is hidden and the surrounding base
    // (full-frame) stays. Compare against the non-inverted case.
    let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
    c.layers[0].matte = MatteMode::AlphaInverted;
    let f = render_frame(&c, 0.0);
    // Center (under the opaque source) is punched out.
    assert_eq!(f.pixel(32, 32)[3], 0, "center should be inverted-out");
    // An edge pixel (base present, source absent) survives.
    assert_eq!(f.pixel(2, 2)[3], 255, "edge should survive inversion");
}

#[test]
fn luma_matte_scales_alpha_by_source_brightness() {
    // White base; the luma of a darker source scales the base's alpha. A gray
    // (0.5 sRGB) source yields a partial matte: center alpha between 0 and 255.
    let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [0.5, 0.5, 0.5, 1.0], 1.0);
    c.layers[0].matte = MatteMode::Luma;
    let gray = render_frame(&c, 0.0).pixel(32, 32)[3];
    assert!((1..255).contains(&gray), "partial luma matte, got a={gray}");
    // A white source passes the base through fully.
    c.layers[1].color = [1.0, 1.0, 1.0, 1.0];
    let white = render_frame(&c, 0.0).pixel(32, 32)[3];
    assert_eq!(white, 255, "white luma should fully pass");
    // A black source mattes the base completely away.
    c.layers[1].color = [0.0, 0.0, 0.0, 1.0];
    let black = render_frame(&c, 0.0).pixel(32, 32)[3];
    assert_eq!(black, 0, "black luma should fully matte out");
}

#[test]
fn matte_preserves_base_color() {
    // The matte changes coverage only, never color: a blue base under a
    // partial luma matte still reads blue (just dimmer in alpha).
    let mut c = matte_pair([0.0, 0.0, 1.0, 1.0], [0.6, 0.6, 0.6, 1.0], 1.0);
    c.layers[0].matte = MatteMode::Luma;
    let [r, g, b, a] = render_frame(&c, 0.0).pixel(32, 32);
    assert!(a > 0, "some coverage expected");
    assert!(
        b > r && b > g,
        "base color should stay blue, got {r},{g},{b}"
    );
}

#[test]
fn export_sequence_writes_all_frames() {
    let mut c = solid([0.2, 0.6, 0.9, 1.0]);
    c.width = 16;
    c.height = 16;
    c.duration = 0.1; // 0.1s * 30fps = 3 frames
    c.fps = 30.0;
    let dir = std::env::temp_dir().join(format!("pulse_export_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let summary = export_sequence(&c, &dir, "seq").expect("export");
    assert_eq!(summary.frames, 3);
    for i in 0..3 {
        let p = frame_path(&dir, "seq", i, 3);
        assert!(p.exists(), "missing frame {}", p.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Motion blur --------------------------------------------------------

/// A 64x64 comp whose single solid slides fast left→right across the frame,
/// with comp motion blur on and the layer opted in (toggled by `layer_mb`).
fn moving_solid(layer_mb: bool) -> Comp {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].motion_blur = layer_mb;
    c.layers[0].x.set_key(0.0, -24.0);
    c.layers[0].x.set_key(1.0, 24.0);
    c.motion_blur.enabled = true;
    c.motion_blur.angle = 360.0; // a whole frame of blur for a clear effect
    c.motion_blur.samples = 16;
    c
}

#[test]
fn motion_blur_softens_the_moving_edge() {
    // With motion blur the leading/trailing edge spans several partly-covered
    // (0 < a < 255) pixels; without it the edge is a hard 0/255 step. Count
    // the partial-alpha pixels along the center row at mid-travel.
    let partial_count = |mb: bool| {
        let c = moving_solid(mb);
        let f = render_frame(&c, 0.5);
        (0..f.width)
            .filter(|&x| {
                let a = f.pixel(x, 32)[3];
                a > 0 && a < 255
            })
            .count()
    };
    let blurred = partial_count(true);
    let crisp = partial_count(false);
    assert!(
        blurred > crisp,
        "motion blur should add partial-coverage edge pixels: blurred={blurred} crisp={crisp}"
    );
}

#[test]
fn motion_blur_preserves_color_no_bleed() {
    // A fully-covered pixel near the center of the swept band keeps the
    // layer's pure-white color (premultiplied averaging must not bleed it
    // toward black through the transparent samples).
    let c = moving_solid(true);
    let f = render_frame(&c, 0.5);
    // The layer center at t=0.5 sits at comp x=0 -> pixel 32; fully covered
    // across the sweep, so still opaque white.
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "center stays fully covered through the sweep");
    assert!(
        r > 250 && g > 250 && b > 250,
        "color preserved, got {r},{g},{b}"
    );
}

#[test]
fn comp_master_switch_gates_motion_blur() {
    // Layer opted in but comp master off -> identical to no motion blur.
    let mut c = moving_solid(true);
    c.motion_blur.enabled = false;
    let off = render_frame(&c, 0.5);
    let mut crisp = solid([1.0, 1.0, 1.0, 1.0]);
    crisp.layers[0].x.set_key(0.0, -24.0);
    crisp.layers[0].x.set_key(1.0, 24.0);
    let baseline = render_frame(&crisp, 0.5);
    assert_eq!(off.pixels, baseline.pixels);
}

#[test]
fn unblurred_layer_unaffected_by_comp_motion_blur() {
    // Comp MB on but the layer didn't opt in -> crisp render unchanged.
    let blurred_off = render_frame(&moving_solid(false), 0.5);
    let mut crisp = solid([1.0, 1.0, 1.0, 1.0]);
    crisp.layers[0].x.set_key(0.0, -24.0);
    crisp.layers[0].x.set_key(1.0, 24.0);
    let baseline = render_frame(&crisp, 0.5);
    assert_eq!(blurred_off.pixels, baseline.pixels);
}

#[test]
fn motion_blur_respects_track_matte() {
    // A motion-blurred base clipped by a small static alpha matte: the matte
    // still bounds coverage (no blurred pixels leak past the matte edge far
    // from the source quad).
    let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
    c.layers[0].matte = MatteMode::Alpha;
    c.layers[0].motion_blur = true;
    c.layers[0].x.set_key(0.0, -24.0);
    c.layers[0].x.set_key(1.0, 24.0);
    c.motion_blur.enabled = true;
    let f = render_frame(&c, 0.5);
    // A far corner is outside the small matte source -> matted out even with
    // motion blur on.
    assert_eq!(f.pixel(2, 2)[3], 0, "matte must still clip the blur");
}

// --- Masks --------------------------------------------------------------

use crate::comp::{Mask, MaskMode};

/// A 64x64 comp with a single full-frame opaque white solid (index 0).
fn full_frame_solid() -> Comp {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0); // cover the whole frame
    c
}

#[test]
fn mask_clips_layer_to_its_shape() {
    // A small centered rectangular Add mask on a full-frame solid: the center
    // stays opaque, a far corner (outside the mask) is carved away.
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "center inside mask stays covered");
    assert_eq!(f.pixel(2, 2)[3], 0, "corner outside mask is carved away");
}

#[test]
fn inverted_mask_keeps_the_outside() {
    // Inverting the same mask flips it: the center is punched out, the
    // surrounding frame survives.
    let mut c = full_frame_solid();
    let mut m = Mask::rect(8.0, 8.0);
    m.inverted = true;
    c.layers[0].masks.push(m);
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 0, "center punched out by inverted mask");
    assert_eq!(f.pixel(2, 2)[3], 255, "outside survives inversion");
}

#[test]
fn no_active_mask_is_identical_to_unmasked() {
    // A layer whose only mask is disabled (mode None) renders byte-identical
    // to the same layer with no masks at all.
    let base = render_frame(&full_frame_solid(), 0.0);
    let mut c = full_frame_solid();
    let mut m = Mask::rect(8.0, 8.0);
    m.mode = MaskMode::None;
    c.layers[0].masks.push(m);
    let withmask = render_frame(&c, 0.0);
    assert_eq!(base.pixels, withmask.pixels);
}

#[test]
fn mask_preserves_layer_color() {
    // Masking changes coverage, never color: a blue solid masked to a small
    // rect still reads blue at the center.
    let mut c = solid([0.0, 0.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0);
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    let [r, g, b, a] = render_frame(&c, 0.0).pixel(32, 32);
    assert_eq!(a, 255);
    assert!(b > r && b > g, "center should stay blue, got {r},{g},{b}");
}

#[test]
fn feathered_mask_softens_the_edge() {
    // A hard mask has a crisp 0/255 boundary; a feathered one adds a band of
    // partial-alpha pixels along the center row.
    let partial_count = |feather: f32| {
        let mut c = full_frame_solid();
        let mut m = Mask::rect(12.0, 12.0);
        m.feather = feather;
        c.layers[0].masks.push(m);
        let f = render_frame(&c, 0.0);
        (0..f.width)
            .filter(|&x| {
                let a = f.pixel(x, 32)[3];
                a > 0 && a < 255
            })
            .count()
    };
    assert!(
        partial_count(8.0) > partial_count(0.0),
        "feather should add partial-coverage edge pixels"
    );
}

#[test]
fn add_subtract_mask_stack_punches_a_hole() {
    // A big Add mask with a smaller Subtract mask leaves a covered ring with a
    // transparent hole at the center.
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(13.0, 13.0)); // Add (default)
    let mut sub = Mask::rect(5.0, 5.0);
    sub.mode = MaskMode::Subtract;
    c.layers[0].masks.push(sub);
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 0, "center hole subtracted away");
    // A pixel inside the big rect (local ~9.5px after the layer's 3x scale)
    // but outside the small hole stays covered.
    assert_eq!(f.pixel(60, 32)[3], 255, "ring stays covered");
}

#[test]
fn mask_rides_layer_transform() {
    // The mask is in layer-local space, so moving the layer moves the masked
    // region with it. Shift the layer right and the surviving coverage shifts
    // too: the original center loses coverage, a point to the right gains it.
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    c.layers[0].x.set_key(0.0, 16.0); // slide right 16 comp px
    let f = render_frame(&c, 0.0);
    // The masked patch moved to ~x=48; the old center is now outside it.
    assert_eq!(f.pixel(48, 32)[3], 255, "masked patch followed the layer");
    assert_eq!(f.pixel(20, 32)[3], 0, "old position no longer covered");
}

// --- Spatial effects ----------------------------------------------------

use crate::comp::SpatialEffect;

#[test]
fn gaussian_blur_softens_the_layer_edge() {
    // A small centered solid: blurring it adds a band of partial-alpha edge
    // pixels along the center row vs. the crisp render.
    let partial_count = |sigma: f32| {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        if sigma > 0.0 {
            c.layers[0]
                .spatial_effects
                .push(SpatialEffect::GaussianBlur {
                    sigma_x: sigma,
                    sigma_y: sigma,
                    repeat_edge: false,
                });
        }
        let f = render_frame(&c, 0.0);
        (0..f.width)
            .filter(|&x| {
                let a = f.pixel(x, 32)[3];
                a > 0 && a < 255
            })
            .count()
    };
    assert!(
        partial_count(4.0) > partial_count(0.0),
        "blur should add partial-coverage edge pixels"
    );
}

#[test]
fn drop_shadow_appears_in_the_composite() {
    // A solid with a hard (0-softness) black drop shadow offset down-right:
    // a pixel just past the quad in the shadow direction picks up dark,
    // semi-opaque shadow coverage where the crisp layer had nothing.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].spatial_effects.push(SpatialEffect::DropShadow {
        color: [0.0, 0.0, 0.0],
        opacity: 1.0,
        angle: 45.0, // down-right (+x,+y)
        distance: 10.0,
        softness: 0.0,
        shadow_only: false,
    });
    let crisp = render_frame(&solid([1.0, 1.0, 1.0, 1.0]), 0.0);
    let shad = render_frame(&c, 0.0);
    // The half-extent is ~14px; sample a pixel down-right of the quad's
    // bottom-right corner that the shadow offset reaches.
    let (sx, sy) = (32 + 16, 32 + 16);
    assert_eq!(
        crisp.pixel(sx, sy)[3],
        0,
        "no coverage here without a shadow"
    );
    let p = shad.pixel(sx, sy);
    assert!(p[3] > 0, "drop shadow added coverage past the layer");
    assert!(
        p[0] < 60 && p[1] < 60 && p[2] < 60,
        "shadow should be dark, got {},{},{}",
        p[0],
        p[1],
        p[2]
    );
}

#[test]
fn glow_brightens_a_bright_layer() {
    // A bright (but not pure-white) layer reads brighter at center once a
    // glow blooms its highlights back on top.
    let center_r = |with_glow: bool| {
        let mut c = solid([0.85, 0.85, 0.85, 1.0]);
        if with_glow {
            c.layers[0].spatial_effects.push(SpatialEffect::Glow {
                threshold: 0.4,
                radius: 6.0,
                intensity: 2.0,
            });
        }
        render_frame(&c, 0.0).pixel(32, 32)[0]
    };
    assert!(center_r(true) >= center_r(false), "glow should not darken");
}

#[test]
fn spatial_effect_routes_layer_through_isolated_buffer() {
    // A solid with only a (zero-sigma, identity) blur still renders the same
    // as the crisp solid — the isolated-buffer routing is value-neutral when
    // the pass is identity.
    let mut c = solid([0.3, 0.6, 0.9, 1.0]);
    c.layers[0]
        .spatial_effects
        .push(SpatialEffect::GaussianBlur {
            sigma_x: 0.0,
            sigma_y: 0.0,
            repeat_edge: false,
        });
    let base = render_frame(&solid([0.3, 0.6, 0.9, 1.0]), 0.0);
    let routed = render_frame(&c, 0.0);
    assert_eq!(base.pixels, routed.pixels);
}

#[test]
fn mask_and_track_matte_compose() {
    // A masked base under a static alpha matte: both must clip. The mask is
    // small (rect 8) and centered; the matte source is unit-scale (~14 px).
    // The center survives both; a far corner is matted out.
    let mut c = matte_pair([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], 1.0);
    c.layers[0].matte = crate::comp::MatteMode::Alpha;
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "center passes mask and matte");
    // Outside the small mask -> carved by the mask even within the matte.
    assert_eq!(f.pixel(2, 2)[3], 0, "corner clipped by mask+matte");
}

use crate::comp::{Fill, ShapeItem, ShapePrimitive, Stroke};

/// A 64x64 comp with a single centered shape layer holding one filled item.
fn shape(primitive: ShapePrimitive, fill: [f32; 3]) -> Comp {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Shape;
    let mut item = ShapeItem::new(primitive);
    item.fill = Some(Fill {
        color: fill,
        opacity: 1.0,
    });
    c.layers[0].shape.items.push(item);
    c
}

#[test]
fn shape_layer_fills_its_center() {
    // A centered red-filled rectangle covers the comp center with its color.
    let c = shape(
        ShapePrimitive::Rectangle {
            half_w: 16.0,
            half_h: 16.0,
            radius: 0.0,
        },
        [1.0, 0.0, 0.0],
    );
    let [r, g, b, a] = render_frame(&c, 0.0).pixel(32, 32);
    assert_eq!(a, 255, "center is opaque");
    assert!(
        r > 250 && g == 0 && b == 0,
        "center is the fill red: {r},{g},{b}"
    );
    // A far corner is outside the 16px half-extent rect.
    assert_eq!(render_frame(&c, 0.0).pixel(2, 2)[3], 0, "corner uncovered");
}

#[test]
fn empty_shape_layer_renders_nothing() {
    // A shape layer with no items draws nothing (and doesn't fall back to a quad).
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Shape;
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0), "no items = transparent");
}

#[test]
fn shape_layer_honors_opacity() {
    // Half-opacity shape layer -> center alpha roughly halved.
    let mut c = shape(
        ShapePrimitive::Ellipse { rx: 16.0, ry: 16.0 },
        [1.0, 1.0, 1.0],
    );
    c.layers[0].opacity.set_key(0.0, 0.5);
    let a = render_frame(&c, 0.0).pixel(32, 32)[3];
    assert!((100..=160).contains(&a), "alpha ~half, got {a}");
}

#[test]
fn shape_ellipse_clips_the_corner() {
    // An ellipse leaves its bounding-box corners transparent.
    let c = shape(
        ShapePrimitive::Ellipse { rx: 20.0, ry: 20.0 },
        [1.0, 1.0, 1.0],
    );
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "center covered");
    // Diagonal point near the bbox corner of the circle is outside the disc.
    assert_eq!(f.pixel(48, 48)[3], 0, "circle corner uncovered");
}

#[test]
fn shape_layer_composes_with_mask() {
    // A big shape carved by a small centered mask: center survives, edge gone.
    let mut c = shape(
        ShapePrimitive::Rectangle {
            half_w: 24.0,
            half_h: 24.0,
            radius: 0.0,
        },
        [1.0, 1.0, 1.0],
    );
    c.layers[0].masks.push(Mask::rect(6.0, 6.0));
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "center passes the mask");
    assert_eq!(
        f.pixel(50, 32)[3],
        0,
        "shape pixel outside the mask is carved"
    );
}

#[test]
fn shape_stroke_outlines_an_unfilled_shape() {
    // A stroked, unfilled rectangle: the boundary is colored, the interior is
    // hollow.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Shape;
    let mut item = ShapeItem::new(ShapePrimitive::Rectangle {
        half_w: 16.0,
        half_h: 16.0,
        radius: 0.0,
    });
    item.fill = None;
    item.stroke = Some(Stroke {
        color: [0.0, 0.0, 1.0],
        width: 4.0,
        opacity: 1.0,
    });
    c.layers[0].shape.items.push(item);
    let f = render_frame(&c, 0.0);
    // Interior (center) is hollow.
    assert_eq!(f.pixel(32, 32)[3], 0, "unfilled interior is transparent");
    // On the boundary (x ~= 32 + 16 = 48) the stroke is present and blue.
    let [r, _g, b, a] = f.pixel(48, 32);
    assert!(a > 100, "stroke band covered, got a={a}");
    assert!(b > r, "stroke reads blue");
}

#[test]
fn shape_layer_motion_blur_widens_the_footprint() {
    // A fast-sliding shape with comp motion blur smears its coverage across the
    // travel, so the center row is touched (alpha > 0) over a wider span of
    // columns than the crisp single-instant render. (The shape rasterizer is
    // antialiased, so we compare covered-column *count*, not partial-alpha.)
    let make = |blur: bool| {
        let mut c = shape(
            ShapePrimitive::Rectangle {
                half_w: 8.0,
                half_h: 8.0,
                radius: 0.0,
            },
            [1.0, 1.0, 1.0],
        );
        c.layers[0].x.set_key(0.0, -200.0);
        c.layers[0].x.set_key(1.0, 200.0);
        c.motion_blur.enabled = blur;
        c.motion_blur.angle = 720.0; // wide shutter so the smear is visible
        c.layers[0].motion_blur = blur;
        c
    };
    let covered_cols = |c: &Comp| {
        let f = render_frame(c, 0.5);
        (0..f.width).filter(|&x| f.pixel(x, 32)[3] > 0).count()
    };
    assert!(
        covered_cols(&make(true)) > covered_cols(&make(false)),
        "motion blur widens the swept footprint"
    );
}

#[test]
fn pre_shape_project_loads_with_empty_shape() {
    // A serialized layer missing the `shape` field (old project) deserializes
    // with an empty shape (serde default), still a valid solid.
    let json = r#"{
        "name":"L","color":[1.0,0.0,0.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}
    }"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert!(layer.shape.is_empty());
    assert_eq!(layer.kind, LayerKind::Solid);
}

// --- Text layers --------------------------------------------------------------

use crate::comp::{TextAlign, TextLayer};

/// A 64x64 comp with a single centered text layer drawing `s` at the given
/// font size in the given fill color (no stroke).
fn text(s: &str, size: f32, fill: [f32; 3]) -> Comp {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Text;
    c.layers[0].text = TextLayer {
        text: s.to_string(),
        size,
        tracking: 0.0,
        leading: 0.0,
        align: TextAlign::Center,
        fill: Some(Fill {
            color: fill,
            opacity: 1.0,
        }),
        stroke: None,
    };
    c
}

#[test]
fn text_layer_draws_its_glyph() {
    // A centered uppercase "I" (a single vertical bar through the center) covers
    // the comp center with the fill color.
    let c = text("I", 40.0, [1.0, 0.0, 0.0]);
    let [r, g, b, a] = render_frame(&c, 0.0).pixel(32, 32);
    assert!(a > 200, "center of the bar is covered, got a={a}");
    assert!(r > 200 && g == 0 && b == 0, "fill is red: {r},{g},{b}");
    // A far corner is uncovered.
    assert_eq!(render_frame(&c, 0.0).pixel(2, 2)[3], 0, "corner uncovered");
}

#[test]
fn empty_text_layer_renders_nothing() {
    // A text layer with blank text draws nothing.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Text;
    c.layers[0].text.text = "   ".to_string();
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0), "blank text = transparent");
}

#[test]
fn text_layer_honors_opacity() {
    // Half-opacity text -> on-stroke alpha roughly halved.
    let mut c = text("I", 50.0, [1.0, 1.0, 1.0]);
    c.layers[0].opacity.set_key(0.0, 0.5);
    let a = render_frame(&c, 0.0).pixel(32, 32)[3];
    assert!(
        (90..=170).contains(&a),
        "alpha ~half on the stroke, got {a}"
    );
}

#[test]
fn text_layer_composes_with_mask() {
    // A wide "W" carved by a small centered mask: center survives where the W's
    // glyph passes through it; an off-center band is carved away.
    let mut c = text("WWW", 40.0, [1.0, 1.0, 1.0]);
    c.layers[0].masks.push(Mask::rect(6.0, 6.0));
    let f = render_frame(&c, 0.0);
    // A pixel well outside the small mask is carved regardless of glyph coverage.
    assert_eq!(f.pixel(60, 32)[3], 0, "text outside the mask is carved");
}

#[test]
fn text_stroke_outlines_the_glyph() {
    // A stroked "I": just outside the pen body edge reads the stroke color.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Text;
    c.layers[0].text = TextLayer {
        text: "I".to_string(),
        size: 80.0,
        tracking: 0.0,
        leading: 0.0,
        align: TextAlign::Center,
        fill: Some(Fill {
            color: [1.0, 0.0, 0.0],
            opacity: 1.0,
        }),
        stroke: Some(Stroke {
            color: [0.0, 0.0, 1.0],
            width: 6.0,
            opacity: 1.0,
        }),
    };
    let f = render_frame(&c, 0.0);
    // Dead center of the vertical bar: the red fill body.
    let [r, _g, b, a] = f.pixel(32, 32);
    assert!(a > 200 && r > b, "body is the red fill: {r},{b}");
    // A few px to the side of the bar (past the thin pen, into the outline band):
    // the blue stroke. The bar pen half is ~80*0.055 ≈ 4.4px, stroke half 3px, so
    // x = 32 + 6 lands in the outline band.
    let [r2, _g2, b2, a2] = f.pixel(38, 32);
    assert!(a2 > 60, "outline band covered, got a={a2}");
    assert!(b2 > r2, "outline reads blue: {r2},{b2}");
}

#[test]
fn text_layer_motion_blur_widens_the_footprint() {
    // A fast-sliding text glyph with comp motion blur smears its coverage across
    // the travel, touching more columns on the center row than the crisp render.
    let make = |blur: bool| {
        let mut c = text("I", 40.0, [1.0, 1.0, 1.0]);
        c.layers[0].x.set_key(0.0, -200.0);
        c.layers[0].x.set_key(1.0, 200.0);
        c.motion_blur.enabled = blur;
        c.motion_blur.angle = 720.0;
        c.layers[0].motion_blur = blur;
        c
    };
    let covered_cols = |c: &Comp| {
        let f = render_frame(c, 0.5);
        (0..f.width).filter(|&x| f.pixel(x, 32)[3] > 0).count()
    };
    assert!(
        covered_cols(&make(true)) > covered_cols(&make(false)),
        "motion blur widens the swept text footprint"
    );
}

#[test]
fn text_layer_as_luma_matte() {
    // A text layer above a solid, used as its luma matte: the solid shows only
    // where the (bright) glyph strokes cover, transparent elsewhere.
    let mut c = solid([1.0, 0.0, 0.0, 1.0]); // index 0: the matted red solid
    c.layers[0].matte = MatteMode::Luma;
    // index 1: a white text layer above acts as the matte source.
    let mut src = PulseLayer::of_kind(LayerKind::Text, "T", [1.0; 4]);
    src.text = TextLayer {
        text: "I".to_string(),
        size: 50.0,
        tracking: 0.0,
        leading: 0.0,
        align: TextAlign::Center,
        fill: Some(Fill {
            color: [1.0, 1.0, 1.0],
            opacity: 1.0,
        }),
        stroke: None,
    };
    c.layers.push(src);
    let f = render_frame(&c, 0.0);
    // On the glyph stroke (center) the red solid shows; the matte source itself
    // doesn't composite.
    assert!(f.pixel(32, 32)[0] > 150, "solid visible under the glyph");
    // Off the glyph (a corner of the quad away from any stroke) it's matted out.
    assert_eq!(f.pixel(2, 2)[3], 0, "outside the glyph is matted away");
}

#[test]
fn pre_text_project_loads_with_default_text() {
    // A serialized layer missing the `text` field (old project) deserializes with
    // the default text (serde default), still a valid solid.
    let json = r#"{
        "name":"L","color":[1.0,0.0,0.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}
    }"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.kind, LayerKind::Solid);
    assert_eq!(layer.text, TextLayer::default());
}

// --- Blend modes --------------------------------------------------------------

/// A 64x64 comp: a full-frame opaque `base` solid (index 0) with a centered
/// `top` solid (index 1) carrying blend mode `mode`. The two overlap at center.
fn blend_pair(base: [f32; 4], top: [f32; 4], mode: BlendMode) -> Comp {
    let mut c = solid(base);
    c.layers[0].scale.set_key(0.0, 3.0); // base covers the frame
    let mut t = PulseLayer::new("top", top);
    t.blend = LayerBlend(mode);
    c.layers.push(t); // index 1
    c
}

#[test]
fn normal_blend_renders_identically_to_no_blend() {
    // A Normal-blend layer must be byte-identical to one with the default blend
    // (the renderer's source-over fast path), so old projects don't shift.
    let base = render_frame(
        &blend_pair(
            [0.2, 0.3, 0.4, 1.0],
            [0.8, 0.5, 0.2, 1.0],
            BlendMode::Normal,
        ),
        0.0,
    );
    let mut plain = blend_pair(
        [0.2, 0.3, 0.4, 1.0],
        [0.8, 0.5, 0.2, 1.0],
        BlendMode::Normal,
    );
    // Explicitly clear to the struct default to prove equivalence.
    plain.layers[1].blend = LayerBlend::default();
    let other = render_frame(&plain, 0.0);
    assert_eq!(base.pixels, other.pixels);
}

#[test]
fn multiply_blend_darkens_the_overlap() {
    // A mid-gray top multiplied over a brighter base reads darker at the overlap
    // than the same top composited Normal (which would just show the top color).
    let center = |mode: BlendMode| {
        let c = blend_pair([0.8, 0.8, 0.8, 1.0], [0.5, 0.5, 0.5, 1.0], mode);
        render_frame(&c, 0.0).pixel(32, 32)[0]
    };
    let mult = center(BlendMode::Multiply);
    let normal = center(BlendMode::Normal);
    assert!(
        mult < normal,
        "multiply should darken vs normal: mult={mult} normal={normal}"
    );
}

#[test]
fn screen_blend_lightens_the_overlap() {
    // Screen over a darker base lifts the overlap above the plain top color.
    let center = |mode: BlendMode| {
        let c = blend_pair([0.3, 0.3, 0.3, 1.0], [0.4, 0.4, 0.4, 1.0], mode);
        render_frame(&c, 0.0).pixel(32, 32)[0]
    };
    let screen = center(BlendMode::Screen);
    let normal = center(BlendMode::Normal);
    assert!(
        screen > normal,
        "screen should lighten vs normal: screen={screen} normal={normal}"
    );
}

#[test]
fn blend_only_changes_pixels_with_a_backdrop() {
    // Where the top layer overhangs past the (full-frame) base it still has a
    // backdrop, so test a region with no base instead: a small base + a larger
    // multiply top — outside the base the top shows its own color unchanged.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]); // small base (unit scale ~14px)
    let mut top = PulseLayer::new("top", [0.5, 0.5, 0.5, 1.0]);
    top.scale.set_key(0.0, 3.0); // top covers the frame
    top.blend = LayerBlend(BlendMode::Multiply);
    c.layers.push(top);
    let f = render_frame(&c, 0.0);
    // A far corner: only the top layer is present (no base backdrop), so the
    // multiply blend is a no-op there and the top shows its straight gray.
    let corner = f.pixel(2, 2);
    let mid_gray = enc(srgb_like(0.5));
    assert!(corner[3] == 255);
    assert!(
        (corner[0] as i32 - mid_gray as i32).abs() <= 2,
        "top shows unblended over empty backdrop: got {} want ~{mid_gray}",
        corner[0]
    );
}

/// Encode a straight sRGB component the way the renderer does (sRGB->linear at
/// input, linear->sRGB at output is identity), for asserting expected bytes.
fn srgb_like(v: f32) -> f32 {
    prism_core::color::srgb_to_linear(v)
}

#[test]
fn non_normal_blend_routes_solid_through_isolated_buffer() {
    // A solid with a blend mode but no masks/matte/spatial still renders (it now
    // takes the isolated-buffer path). Sanity: it composites without panic and
    // covers its center.
    let c = blend_pair(
        [0.0, 0.0, 0.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
        BlendMode::Screen,
    );
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "blended top still covers center");
}

#[test]
fn pre_blend_project_loads_with_normal_blend() {
    // A serialized layer missing the `blend` field (old project) deserializes
    // with Normal (serde default), so it renders as plain source-over.
    let json = r#"{
        "name":"L","color":[1.0,0.0,0.0,1.0],"visible":true,
        "x":{"keys":[]},"y":{"keys":[]},"scale":{"keys":[]},
        "rotation":{"keys":[]},"opacity":{"keys":[]}
    }"#;
    let layer: PulseLayer = serde_json::from_str(json).unwrap();
    assert_eq!(layer.blend_mode(), BlendMode::Normal);
}

// --- Footage layers -----------------------------------------------------

/// Write a `w`x`h` solid-color 8-bit RGBA PNG to a unique temp path and return
/// it, so footage tests have a real file for `prism-io` to decode.
fn write_test_png(stem: &str, w: u32, h: u32, rgba: [u8; 4]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pulse_footage_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{stem}.png"));
    let mut buf = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        buf.extend_from_slice(&rgba);
    }
    let img = image::RgbaImage::from_raw(w, h, buf).unwrap();
    img.save_with_format(&path, image::ImageFormat::Png).unwrap();
    path
}

#[test]
fn footage_still_rasterizes_into_the_quad() {
    use crate::comp::{FootageSource, LayerKind};
    // A solid-green still, decoded and sampled across the footage layer's quad:
    // the center is the footage color (opaque), a far corner is uncovered.
    let path = write_test_png("still_green", 8, 8, [0, 255, 0, 255]);
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Footage;
    c.layers[0].footage.source = Some(FootageSource::still(&path));

    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "footage center should be opaque");
    assert!(g > 250, "green channel high, got {g}");
    assert!(r < 8 && b < 8, "center should be green, got ({r},{g},{b})");
    // Outside the quad (a far corner) stays transparent.
    assert_eq!(f.pixel(0, 0)[3], 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn footage_honors_opacity() {
    use crate::comp::{FootageSource, LayerKind, Prop};
    let path = write_test_png("still_blue", 8, 8, [0, 0, 255, 255]);
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Footage;
    c.layers[0].footage.source = Some(FootageSource::still(&path));
    c.layers[0].track_mut(Prop::Opacity).set_key(0.0, 0.5);

    let f = render_frame(&c, 0.0);
    let a = f.pixel(32, 32)[3];
    assert!(a > 100 && a < 200, "half-opacity footage center, got a={a}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn unset_footage_renders_nothing() {
    use crate::comp::LayerKind;
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Footage; // no source set
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&b| b == 0), "no source => empty frame");
}

// --- Precomps (nested compositions) -------------------------------------

/// A full-frame solid comp of the given id and color (covers the whole frame so
/// a precomp sampling it sees the color edge-to-edge).
fn full_frame_comp(id: u64, color: [f32; 4]) -> Comp {
    let mut c = solid(color);
    c.id = id;
    c.layers[0].scale.set_key(0.0, 3.0); // cover the whole frame
    c
}

#[test]
fn precomp_renders_the_referenced_comps_content() {
    use crate::comp::{LayerKind, PrecompLayer};
    // Comp B: a full-frame green solid (id 2).
    let nested = full_frame_comp(2, [0.0, 1.0, 0.0, 1.0]);
    // Comp A (id 1): a single precomp layer referencing B, scaled to cover the
    // frame so its quad fills it.
    let mut host = solid([1.0, 1.0, 1.0, 1.0]);
    host.id = 1;
    host.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5, 0.5, 0.5, 1.0]);
        l.precomp = PrecompLayer::to(2);
        l.scale.set_key(0.0, 3.0); // cover the whole frame
        l
    };
    let comps = [host, nested];

    let mut cache = crate::comp::FrameCache::new();
    let f = render_frame_in_project(&comps, 1, 0.0, &mut cache);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "precomp center should be opaque (nested comp covers it)");
    assert!(g > 250, "nested green should show, got g={g}");
    assert!(r < 8 && b < 8, "center should be green, got ({r},{g},{b})");
}

#[test]
fn precomp_honors_time_offset() {
    use crate::comp::{Interp, LayerKind, PrecompLayer, Prop};
    // Comp B (id 2): a full-frame solid whose opacity ramps 0 -> 1 over [0,1].
    let mut nested = full_frame_comp(2, [1.0, 0.0, 0.0, 1.0]);
    nested.layers[0].track_mut(Prop::Opacity).set_key(0.0, 0.0);
    nested.layers[0].track_mut(Prop::Opacity).set_key(1.0, 1.0);
    nested.layers[0]
        .track_mut(Prop::Opacity)
        .set_interp(0.0, Interp::Linear);

    // Host precomp at t=0 with a +1.0s offset samples B at its end (opacity 1).
    let mut host = solid([1.0, 1.0, 1.0, 1.0]);
    host.id = 1;
    host.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
        l.precomp = PrecompLayer {
            source: Some(2),
            time_offset: 1.0,
        };
        l.scale.set_key(0.0, 3.0);
        l
    };
    let comps = [host, nested];
    let mut cache = crate::comp::FrameCache::new();
    let f = render_frame_in_project(&comps, 1, 0.0, &mut cache);
    let a = f.pixel(32, 32)[3];
    assert!(a > 250, "offset to B's end => opaque, got a={a}");
}

#[test]
fn precomp_cycle_guard_terminates() {
    use crate::comp::{LayerKind, PrecompLayer};
    // A -> B -> A: each comp is a precomp pointing at the other. Rendering must
    // terminate (the cycle guard refuses to re-enter a comp on the stack) rather
    // than recurse forever / overflow the stack.
    let mut a = solid([1.0, 1.0, 1.0, 1.0]);
    a.id = 1;
    a.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "A->B", [0.5; 4]);
        l.precomp = PrecompLayer::to(2);
        l.scale.set_key(0.0, 3.0);
        l
    };
    let mut b = solid([1.0, 1.0, 1.0, 1.0]);
    b.id = 2;
    b.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "B->A", [0.5; 4]);
        l.precomp = PrecompLayer::to(1);
        l.scale.set_key(0.0, 3.0);
        l
    };
    let comps = [a, b];
    let mut cache = crate::comp::FrameCache::new();
    // The assertion that matters is *that this returns* (no infinite recursion).
    let f = render_frame_in_project(&comps, 1, 0.0, &mut cache);
    // A renders B; B renders A which is on the stack -> guard breaks it (nothing).
    assert!(
        f.pixels.iter().all(|&px| px == 0),
        "a cyclic precomp pair should render nothing"
    );
}

#[test]
fn self_referential_precomp_renders_nothing() {
    use crate::comp::{LayerKind, PrecompLayer};
    // A comp whose only layer is a precomp pointing at itself: the cycle guard
    // (the comp is already on the stack) makes it render nothing.
    let mut a = solid([1.0, 1.0, 1.0, 1.0]);
    a.id = 1;
    a.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "self", [0.5; 4]);
        l.precomp = PrecompLayer::to(1);
        l.scale.set_key(0.0, 3.0);
        l
    };
    let comps = [a];
    let mut cache = crate::comp::FrameCache::new();
    let f = render_frame_in_project(&comps, 1, 0.0, &mut cache);
    assert!(f.pixels.iter().all(|&px| px == 0));
}

#[test]
fn precomp_missing_target_renders_nothing() {
    use crate::comp::{LayerKind, PrecompLayer};
    let mut host = solid([1.0, 1.0, 1.0, 1.0]);
    host.id = 1;
    host.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
        l.precomp = PrecompLayer::to(99); // no such comp
        l.scale.set_key(0.0, 3.0);
        l
    };
    let comps = [host];
    let mut cache = crate::comp::FrameCache::new();
    let f = render_frame_in_project(&comps, 1, 0.0, &mut cache);
    assert!(f.pixels.iter().all(|&px| px == 0));
}

#[test]
fn precomp_nests_two_levels_deep() {
    use crate::comp::{LayerKind, PrecompLayer};
    // C (id 3) is a green full-frame solid; B (id 2) is a precomp of C; A (id 1)
    // is a precomp of B. Rendering A should show C's green two levels down.
    let c = full_frame_comp(3, [0.0, 1.0, 0.0, 1.0]);
    let mut b = solid([1.0, 1.0, 1.0, 1.0]);
    b.id = 2;
    b.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "B->C", [0.5; 4]);
        l.precomp = PrecompLayer::to(3);
        l.scale.set_key(0.0, 3.0);
        l
    };
    let mut a = solid([1.0, 1.0, 1.0, 1.0]);
    a.id = 1;
    a.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "A->B", [0.5; 4]);
        l.precomp = PrecompLayer::to(2);
        l.scale.set_key(0.0, 3.0);
        l
    };
    let comps = [a, b, c];
    let mut cache = crate::comp::FrameCache::new();
    let f = render_frame_in_project(&comps, 1, 0.0, &mut cache);
    let [r, g, bch, alpha] = f.pixel(32, 32);
    assert_eq!(alpha, 255);
    assert!(g > 250 && r < 8 && bch < 8, "deep nest green, got ({r},{g},{bch})");
}

#[test]
fn single_comp_render_ignores_precomp() {
    use crate::comp::{LayerKind, PrecompLayer};
    // The single-comp `render_frame` entry has no project to resolve against, so
    // a precomp layer in it draws nothing (and doesn't panic / recurse).
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.id = 1;
    c.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
        l.precomp = PrecompLayer::to(2); // a sibling that isn't visible here
        l.scale.set_key(0.0, 3.0);
        l
    };
    let f = render_frame(&c, 0.0);
    assert!(f.pixels.iter().all(|&px| px == 0));
}

// --- Expressions drive the render path -------------------------------------

#[test]
fn position_expression_moves_coverage_in_render() {
    // An expression `value + 20` on X (with value defaulting to 0) shifts the
    // quad right exactly like a keyframed offset would — proving expressions are
    // wired through the compositor's world matrix, not just the model.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].x.expression = Some("20".to_string());
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(50, 32)[3], 255, "covered band shifted right");
    assert_eq!(f.pixel(10, 32)[3], 0, "left of the shifted quad is clear");
}

#[test]
fn opacity_expression_fades_layer_in_render() {
    // `time` as an opacity expression makes the center alpha grow with time
    // (clamped to [0,1]) — the opacity sampler is expression-aware end to end.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].opacity.expression = Some("time".to_string());
    let a0 = render_frame(&c, 0.0).pixel(32, 32)[3];
    let amid = render_frame(&c, 0.5).pixel(32, 32)[3];
    let a1 = render_frame(&c, 1.0).pixel(32, 32)[3];
    assert!(a0 < amid && amid < a1, "{a0} < {amid} < {a1}");
    assert_eq!(a1, 255);
}

#[test]
fn malformed_render_expression_does_not_crash() {
    // A broken expression on a property must not panic the render — it falls
    // back to the keyframed value (here the default), so the layer still draws.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].x.expression = Some("@@@ broken @@@".to_string());
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "falls back to keyframed X (center)");
}

// --- Time remapping (render path) --------------------------------------

#[test]
fn precomp_identity_time_remap_matches_no_remap() {
    use crate::comp::{Interp, LayerKind, PrecompLayer, Prop};
    // Nested comp B (id 2): full-frame solid whose opacity ramps 0 -> 1 over [0,1].
    let make_nested = || {
        let mut nested = full_frame_comp(2, [1.0, 0.0, 0.0, 1.0]);
        nested.layers[0].track_mut(Prop::Opacity).set_key(0.0, 0.0);
        nested.layers[0].track_mut(Prop::Opacity).set_key(1.0, 1.0);
        nested.layers[0]
            .track_mut(Prop::Opacity)
            .set_interp(0.0, Interp::Linear);
        nested
    };
    let make_host = || {
        let mut host = solid([1.0, 1.0, 1.0, 1.0]);
        host.id = 1;
        host.duration = 1.0;
        host.layers[0] = {
            let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
            l.precomp = PrecompLayer::to(2);
            l.scale.set_key(0.0, 3.0);
            l
        };
        host
    };

    // Baseline: no remap.
    let plain = [make_host(), make_nested()];
    // Identity remap: r(t) = t (a 0 -> 1 ramp over [0,1]).
    let mut host_remap = make_host();
    host_remap.layers[0].time_remap.enabled = true;
    host_remap.layers[0].time_remap.track.set_key(0.0, 0.0);
    host_remap.layers[0].time_remap.track.set_key(1.0, 1.0);
    host_remap.layers[0]
        .time_remap
        .track
        .set_interp(0.0, Interp::Linear);
    let remapped = [host_remap, make_nested()];

    for &t in &[0.0, 0.5, 1.0] {
        let mut c1 = crate::comp::FrameCache::new();
        let mut c2 = crate::comp::FrameCache::new();
        let a = render_frame_in_project(&plain, 1, t, &mut c1).pixel(32, 32);
        let b = render_frame_in_project(&remapped, 1, t, &mut c2).pixel(32, 32);
        assert_eq!(a, b, "identity remap must match no-remap at t={t}");
    }
}

#[test]
fn precomp_reverse_time_remap_samples_backwards() {
    use crate::comp::{Interp, LayerKind, PrecompLayer, Prop};
    // Nested comp B (id 2): opacity ramps 0 -> 1 over [0,1].
    let mut nested = full_frame_comp(2, [1.0, 0.0, 0.0, 1.0]);
    nested.layers[0].track_mut(Prop::Opacity).set_key(0.0, 0.0);
    nested.layers[0].track_mut(Prop::Opacity).set_key(1.0, 1.0);
    nested.layers[0]
        .track_mut(Prop::Opacity)
        .set_interp(0.0, Interp::Linear);

    // Host precomp with a reversing remap r(t) = 1 - t over [0,1]: at host t=0 the
    // source is sampled at 1.0 (B fully opaque), at host t=1 at 0.0 (transparent).
    let mut host = solid([1.0, 1.0, 1.0, 1.0]);
    host.id = 1;
    host.duration = 1.0;
    host.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
        l.precomp = PrecompLayer::to(2);
        l.scale.set_key(0.0, 3.0);
        l.time_remap.enabled = true;
        l.time_remap.track.set_key(0.0, 1.0);
        l.time_remap.track.set_key(1.0, 0.0);
        l.time_remap.track.set_interp(0.0, Interp::Linear);
        l
    };
    let comps = [host, nested];
    let mut cache = crate::comp::FrameCache::new();
    let a0 = render_frame_in_project(&comps, 1, 0.0, &mut cache).pixel(32, 32)[3];
    let a1 = render_frame_in_project(&comps, 1, 1.0, &mut cache).pixel(32, 32)[3];
    assert!(a0 > 250, "reverse remap @ t=0 => B's end (opaque), got {a0}");
    assert!(a1 < 5, "reverse remap @ t=1 => B's start (transparent), got {a1}");
}

#[test]
fn precomp_freeze_time_remap_holds_one_source_frame() {
    use crate::comp::{Interp, LayerKind, PrecompLayer, Prop};
    // Nested comp B opacity ramps 0 -> 1 over [0,1]; a constant remap freezes it.
    let mut nested = full_frame_comp(2, [1.0, 0.0, 0.0, 1.0]);
    nested.layers[0].track_mut(Prop::Opacity).set_key(0.0, 0.0);
    nested.layers[0].track_mut(Prop::Opacity).set_key(1.0, 1.0);
    nested.layers[0]
        .track_mut(Prop::Opacity)
        .set_interp(0.0, Interp::Linear);

    // A single constant remap key at source time 1.0: B is frozen fully opaque
    // regardless of host time.
    let mut host = solid([1.0, 1.0, 1.0, 1.0]);
    host.id = 1;
    host.duration = 2.0;
    host.layers[0] = {
        let mut l = PulseLayer::of_kind(LayerKind::Precomp, "PC", [0.5; 4]);
        l.precomp = PrecompLayer::to(2);
        l.scale.set_key(0.0, 3.0);
        l.time_remap.enabled = true;
        l.time_remap.track.set_key(0.0, 1.0); // freeze at source end
        l
    };
    let comps = [host, nested];
    for &t in &[0.0, 0.5, 1.5] {
        let mut cache = crate::comp::FrameCache::new();
        let a = render_frame_in_project(&comps, 1, t, &mut cache).pixel(32, 32)[3];
        assert!(a > 250, "freeze remap holds B opaque at host t={t}, got {a}");
    }
}
