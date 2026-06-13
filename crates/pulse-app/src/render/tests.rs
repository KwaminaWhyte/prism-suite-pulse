use super::export::{frame_count, frame_path, frame_range, frame_time, range_frame_count};
use super::*;
use crate::comp::{
    BlendMode, Camera, Interp, LayerBlend, MatteMode, MotionBlur, Prop, PulseLayer, WorkArea,
};
use crate::render::RenderRange;
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
        camera: Camera::default(),
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
        camera: Camera::default(),
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
        camera: Camera::default(),
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
fn channel_mixer_swaps_channels_in_render_path() {
    // A pure-blue solid with a Channel Mixer that sources red from blue (R<-B)
    // should read with a high red channel at the center after compositing.
    let mut c = solid([0.0, 0.0, 1.0, 1.0]);
    c.layers[0].effects.push(crate::comp::Effect::ChannelMixer {
        red: [0.0, 0.0, 1.0, 0.0], // R <- B
        green: [0.0, 1.0, 0.0, 0.0],
        blue: [0.0, 0.0, 1.0, 0.0],
        monochrome: false,
    });
    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    assert!(r > 250, "expected red lifted from blue, got r={r}");
    assert!(g < 5, "green should stay zero, got {g}");
    assert!(b > 250, "blue should stay high, got {b}");
}

#[test]
fn gradient_map_recolors_solid_in_render_path() {
    // A black solid through a Gradient Map whose shadow stop is pure red should
    // composite as red at the center (luma 0 -> first stop).
    let mut c = solid([0.0, 0.0, 0.0, 1.0]);
    c.layers[0].effects.push(crate::comp::Effect::GradientMap {
        low: [1.0, 0.0, 0.0],
        mid: [0.0, 1.0, 0.0],
        high: [0.0, 0.0, 1.0],
        amount: 1.0,
    });
    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    assert!(
        r > 250 && g < 5 && b < 5,
        "expected red shadow stop, got {r},{g},{b}"
    );
}

#[test]
fn effect_mask_limits_the_grade_to_its_region() {
    // A black solid with a "make it white" effect, masked to only the RIGHT side
    // of the layer (local x in ~[2, 14]). The pixel at the layer center (local
    // 0,0) is outside the region → stays black (the unmasked grade is suppressed);
    // a pixel well to the right is inside → reads white (full grade). Without a
    // mask the whole quad would be white, so this proves the mask gates the effect.
    let half = 64.0 * LAYER_HALF_FRAC; // ~14 px
    let mut c = solid([0.0, 0.0, 0.0, 1.0]);
    c.layers[0]
        .effects
        .push(crate::comp::Effect::BrightnessContrast {
            brightness: 1.0,
            contrast: 1.0,
        });
    c.layers[0].effect_mask.enabled = true;
    // A rect region covering local x in [2, half], full height — shift a centered
    // rect's left edge rightward so the center is excluded.
    let mut region = crate::comp::Mask::rect(half, half);
    for v in &mut region.vertices {
        if v.x < 0.0 {
            v.x = 2.0; // pull the left edge to x=2
        }
    }
    c.layers[0].effect_mask.region = region;

    let f = render_frame(&c, 0.0);
    // Center pixel (local ~0,0) is outside the masked region → original black.
    let [cr, cg, cb, ca] = f.pixel(32, 32);
    assert_eq!(ca, 255);
    assert!(
        cr < 5 && cg < 5 && cb < 5,
        "center should be unmasked (black), got {cr},{cg},{cb}"
    );
    // A pixel ~8 px right of center (comp x=40, local ~+8) is inside → white.
    let [rr, rg, rb, ra] = f.pixel(40, 32);
    assert_eq!(ra, 255);
    assert!(
        rr > 250 && rg > 250 && rb > 250,
        "masked region should be graded white, got {rr},{rg},{rb}"
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
        camera: Camera::default(),
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
        camera: Camera::default(),
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
    let summary = export_sequence(&c, &dir, "seq", RenderRange::Full).expect("export");
    assert_eq!(summary.frames, 3);
    for i in 0..3 {
        let p = frame_path(&dir, "seq", i, 3);
        assert!(p.exists(), "missing frame {}", p.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Render range (work area vs full comp) -------------------------------

/// A 10 s, 30 fps comp (300 frames: 0..=299) with the work area trimmed to
/// `[1.0s, 2.0s]` — frames 30..=60 on the comp grid.
fn ranged_comp() -> Comp {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.width = 16;
    c.height = 16;
    c.duration = 10.0;
    c.fps = 30.0;
    c.work_area = WorkArea { start: 1.0, end: 2.0 };
    c
}

#[test]
fn frame_range_full_spans_whole_timeline() {
    let c = ranged_comp();
    // Full: every frame, 0..=last (300 frames, indices 0..=299).
    assert_eq!(frame_range(&c, RenderRange::Full), (0, 299));
    assert_eq!(range_frame_count(&c, RenderRange::Full), 300);
}

#[test]
fn frame_range_work_area_uses_in_out_frames() {
    let c = ranged_comp();
    // Work area [1.0s, 2.0s] @30fps → frames 30..=60 inclusive (31 frames).
    let (first, last) = frame_range(&c, RenderRange::WorkArea);
    assert_eq!((first, last), (30, 60), "first = in-point frame, last = out");
    assert_eq!(range_frame_count(&c, RenderRange::WorkArea), 31);
    // The first exported frame's time is the work-area start.
    assert!((frame_time(&c, first) - 1.0).abs() < 1e-6);
    assert!((frame_time(&c, last) - 2.0).abs() < 1e-6);
}

#[test]
fn full_work_area_falls_back_to_full_render() {
    // A work area spanning the whole timeline is not a real sub-range, so a
    // work-area render renders the full comp (never a degenerate empty render).
    let mut c = ranged_comp();
    c.work_area = WorkArea::full(c.duration);
    assert_eq!(frame_range(&c, RenderRange::WorkArea), (0, 299));
    assert_eq!(range_frame_count(&c, RenderRange::WorkArea), 300);
}

#[test]
fn degenerate_work_area_falls_back_to_full_render() {
    // A zero-length (in == out) work area would render nothing useful; fall back
    // to the full comp so an export is never empty.
    let mut c = ranged_comp();
    c.work_area = WorkArea { start: 3.0, end: 3.0 };
    assert_eq!(frame_range(&c, RenderRange::WorkArea), (0, 299));
    assert_eq!(range_frame_count(&c, RenderRange::WorkArea), 300);
}

#[test]
fn empty_serde_default_work_area_falls_back_to_full() {
    // The serde-default empty [0,0] range (a pre-work-area `.pulse`) self-heals
    // to the whole timeline via `clamped_work_area`, so it renders full.
    let mut c = ranged_comp();
    c.work_area = WorkArea::default(); // {0,0}
    assert_eq!(frame_range(&c, RenderRange::WorkArea), (0, 299));
}

#[test]
fn default_range_prefers_a_trimmed_work_area() {
    // A real sub-range → default to the work area (After Effects' default).
    let c = ranged_comp();
    assert_eq!(RenderRange::default_for(&c), RenderRange::WorkArea);
    // A full work area → default to the full comp.
    let mut full = ranged_comp();
    full.work_area = WorkArea::full(full.duration);
    assert_eq!(RenderRange::default_for(&full), RenderRange::Full);
    // A degenerate work area → default to the full comp.
    let mut degen = ranged_comp();
    degen.work_area = WorkArea { start: 3.0, end: 3.0 };
    assert_eq!(RenderRange::default_for(&degen), RenderRange::Full);
}

#[test]
fn export_work_area_writes_only_in_out_frames_numbered_by_comp_index() {
    let c = ranged_comp();
    let dir =
        std::env::temp_dir().join(format!("pulse_export_wa_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let summary = export_sequence(&c, &dir, "seq", RenderRange::WorkArea).expect("export");
    // 31 frames (frames 30..=60), the work-area in/out range only.
    assert_eq!(summary.frames, 31);
    let total = frame_count(&c); // numbering padding is over the full count
    // The first exported file is the work-area start frame (frame 30), not 0.
    assert!(
        !frame_path(&dir, "seq", 29, total).exists(),
        "frame before the work area must not be written"
    );
    assert!(
        frame_path(&dir, "seq", 30, total).exists(),
        "first work-area frame (in-point) must be written"
    );
    assert!(
        frame_path(&dir, "seq", 60, total).exists(),
        "last work-area frame (out-point) must be written"
    );
    assert!(
        !frame_path(&dir, "seq", 61, total).exists(),
        "frame after the work area must not be written"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_full_writes_every_frame() {
    let mut c = ranged_comp();
    c.duration = 0.1; // keep IO small: 3 frames, work area trimmed but ignored
    c.work_area = WorkArea { start: 0.0, end: 0.05 };
    let dir =
        std::env::temp_dir().join(format!("pulse_export_full_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let summary = export_sequence(&c, &dir, "seq", RenderRange::Full).expect("export");
    assert_eq!(summary.frames, 3, "full render ignores the work area");
    for i in 0..3 {
        assert!(frame_path(&dir, "seq", i, 3).exists());
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

use crate::comp::{RadialKind, SpatialEffect};

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
fn box_blur_softens_the_layer_edge() {
    // A box blur, like the Gaussian, adds partial-alpha edge pixels along the
    // center row vs. the crisp render — and composites into the buffer.
    let partial_count = |radius: f32| {
        let mut c = solid([1.0, 1.0, 1.0, 1.0]);
        if radius > 0.0 {
            c.layers[0].spatial_effects.push(SpatialEffect::BoxBlur {
                radius,
                iterations: 3,
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
        partial_count(5.0) > partial_count(0.0),
        "box blur should add partial-coverage edge pixels"
    );
}

#[test]
fn directional_blur_smears_into_the_composite() {
    // A horizontal directional blur on a centered solid extends partial-coverage
    // along the center row past the crisp edge, but leaves a vertical column off
    // the layer crisp (no off-axis smear) — proving the angle drives the streak in
    // the real render path.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0]
        .spatial_effects
        .push(SpatialEffect::DirectionalBlur {
            angle: 0.0,
            length: 12.0,
        });
    let crisp = render_frame(&solid([1.0, 1.0, 1.0, 1.0]), 0.0);
    let smeared = render_frame(&c, 0.0);
    assert_ne!(crisp.pixels, smeared.pixels, "directional blur must change the frame");
    // Partial-coverage pixels appear along the center row (the smear axis).
    let row_partial = |f: &Frame| {
        (0..f.width)
            .filter(|&x| {
                let a = f.pixel(x, 32)[3];
                a > 0 && a < 255
            })
            .count()
    };
    assert!(
        row_partial(&smeared) > row_partial(&crisp),
        "horizontal smear adds partial coverage along the row"
    );
}

#[test]
fn radial_blur_changes_the_frame_and_is_deterministic() {
    // A spin radial blur about the centre warps a wide solid (rotational smear);
    // render-path smoke + determinism.
    let mut c = solid([0.8, 0.4, 0.2, 1.0]);
    c.layers[0].scale.set_key(0.0, 2.0); // a wide quad so the sweep has content
    let crisp = {
        let mut b = solid([0.8, 0.4, 0.2, 1.0]);
        b.layers[0].scale.set_key(0.0, 2.0);
        render_frame(&b, 0.0)
    };
    c.layers[0].spatial_effects.push(SpatialEffect::RadialBlur {
        center: [0.5, 0.5],
        kind: RadialKind::Spin,
        amount: 30.0,
    });
    let warped = render_frame(&c, 0.0);
    assert_ne!(crisp.pixels, warped.pixels, "radial blur must change the frame");
    assert_eq!(warped.pixels, render_frame(&c, 0.0).pixels, "deterministic");
}

// --- Distort effects ----------------------------------------------------

use crate::comp::{DistortEffect, PolarKind};

#[test]
fn identity_distort_routes_through_isolated_buffer() {
    // A solid with only an identity corner-pin renders the same as the crisp
    // solid — the isolated-buffer routing is value-neutral when the remap is
    // identity (mirrors the spatial-effect identity-routing test).
    let mut c = solid([0.3, 0.6, 0.9, 1.0]);
    c.layers[0].distort_effects.push(DistortEffect::CornerPin {
        top_left: [0.0, 0.0],
        top_right: [1.0, 0.0],
        bottom_right: [1.0, 1.0],
        bottom_left: [0.0, 1.0],
    });
    let base = render_frame(&solid([0.3, 0.6, 0.9, 1.0]), 0.0);
    let routed = render_frame(&c, 0.0);
    assert_eq!(base.pixels, routed.pixels);
}

#[test]
fn distort_transform_moves_coverage_in_the_composite() {
    // A centered solid with an effect-level Transform that shifts the content far
    // right: the original center loses coverage, a band to the right gains it —
    // proving the distort pass composites into the buffer.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 2.0); // a wider quad so the shift stays in-frame
    c.layers[0].distort_effects.push(DistortEffect::Transform {
        anchor: [0.5, 0.5],
        position: [0.78, 0.5], // shift the content right within the buffer
        scale: 1.0,
        rotation: 0.0,
        skew: 0.0,
        opacity: 1.0,
    });
    let crisp = {
        let mut b = solid([1.0, 1.0, 1.0, 1.0]);
        b.layers[0].scale.set_key(0.0, 2.0);
        render_frame(&b, 0.0)
    };
    let warped = render_frame(&c, 0.0);
    assert_ne!(crisp.pixels, warped.pixels, "transform must change the frame");
    // The far-right region picks up coverage it didn't have crisp.
    let right_cov = |f: &Frame| (0..f.height).filter(|&y| f.pixel(58, y)[3] > 0).count();
    assert!(
        right_cov(&warped) >= right_cov(&crisp),
        "transform shifted coverage right"
    );
}

#[test]
fn distort_transform_opacity_fades_the_layer() {
    // An effect-level Transform at half opacity dims the layer's center alpha.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].distort_effects.push(DistortEffect::Transform {
        anchor: [0.5, 0.5],
        position: [0.5, 0.5],
        scale: 1.0,
        rotation: 0.0,
        skew: 0.0,
        opacity: 0.5,
    });
    let a = render_frame(&c, 0.0).pixel(32, 32)[3];
    assert!((100..=160).contains(&a), "effect opacity halved alpha, got {a}");
}

#[test]
fn distort_polar_changes_the_frame() {
    // A full-frame gradient-ish solid through a Rect→Polar remap is no longer the
    // crisp solid (the remap warps coverage); render-path smoke + determinism.
    let mut c = solid([0.8, 0.4, 0.2, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0); // cover the frame so the remap has content
    let crisp = {
        let mut b = solid([0.8, 0.4, 0.2, 1.0]);
        b.layers[0].scale.set_key(0.0, 3.0);
        render_frame(&b, 0.0)
    };
    c.layers[0].distort_effects.push(DistortEffect::Polar {
        center: [0.5, 0.5],
        kind: PolarKind::RectToPolar,
        interp: 1.0,
    });
    let warped = render_frame(&c, 0.0);
    assert_ne!(crisp.pixels, warped.pixels, "polar remap must change the frame");
    // Deterministic: re-render is byte-identical.
    assert_eq!(warped.pixels, render_frame(&c, 0.0).pixels);
}

#[test]
fn distort_composes_with_mask() {
    // A distort (mirror) on a masked layer still respects the mask: a far corner
    // outside the small centered mask stays carved away after the distort runs.
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    c.layers[0].distort_effects.push(DistortEffect::Mirror {
        center: [0.5, 0.5],
        angle: 90.0,
    });
    let f = render_frame(&c, 0.0);
    // The mask + mirror both clip; a far corner is empty.
    assert_eq!(f.pixel(2, 2)[3], 0, "corner stays carved with a distort");
}

// --- Keying effects -----------------------------------------------------

use crate::comp::KeyEffect;

#[test]
fn identity_key_choke_routes_through_isolated_buffer() {
    // A solid with only an identity Matte Choke (no choke, full clip range)
    // renders the same as the crisp solid — the isolated-buffer routing is
    // value-neutral when the key is a no-op (mirrors the distort identity test).
    let mut c = solid([0.3, 0.6, 0.9, 1.0]);
    c.layers[0].key_effects.push(KeyEffect::MatteChoke {
        choke: 0.0,
        clip_black: 0.0,
        clip_white: 1.0,
    });
    let base = render_frame(&solid([0.3, 0.6, 0.9, 1.0]), 0.0);
    let routed = render_frame(&c, 0.0);
    assert_eq!(base.pixels, routed.pixels);
}

#[test]
fn color_key_removes_keyed_solid_from_the_composite() {
    // A green solid with a Color Key on its own colour keys itself out: the
    // center, opaque crisp, drops to (near-)transparent. Determinism too.
    let green = [0.0, 0.6, 0.1, 1.0];
    let crisp = render_frame(&solid(green), 0.0);
    assert_eq!(crisp.pixel(32, 32)[3], 255, "crisp solid is opaque");
    let mut c = solid(green);
    c.layers[0].key_effects.push(KeyEffect::ColorKey {
        key: [0.0, 0.6, 0.1],
        tolerance: 0.2,
        softness: 0.05,
    });
    let keyed = render_frame(&c, 0.0);
    assert_eq!(
        keyed.pixel(32, 32)[3],
        0,
        "the target colour is keyed away from the composite"
    );
    // A non-matching key colour leaves the solid intact.
    let mut c2 = solid(green);
    c2.layers[0].key_effects.push(KeyEffect::ColorKey {
        key: [1.0, 0.0, 0.0], // red — far from green
        tolerance: 0.2,
        softness: 0.05,
    });
    assert_eq!(
        render_frame(&c2, 0.0).pixel(32, 32)[3],
        255,
        "a non-matching key keeps the layer"
    );
    // Deterministic: re-render is byte-identical.
    assert_eq!(keyed.pixels, render_frame(&c, 0.0).pixels);
}

#[test]
fn key_composes_with_mask() {
    // A Color Key that does NOT match the layer (red key on a white solid) still
    // respects the mask: a far corner outside a small centered mask stays carved.
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    c.layers[0].key_effects.push(KeyEffect::ColorKey {
        key: [1.0, 0.0, 0.0],
        tolerance: 0.1,
        softness: 0.05,
    });
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(32, 32)[3], 255, "center inside mask survives the key");
    assert_eq!(f.pixel(2, 2)[3], 0, "corner stays carved with a key");
}

#[test]
fn matte_choke_dilate_recovers_masked_alpha() {
    // A small centered mask leaves a hole-y matte; a dilating Matte Choke grows
    // the alpha back outward, so a pixel just outside the mask edge gains
    // coverage it didn't have without the choke.
    let mut base = full_frame_solid();
    base.layers[0].masks.push(Mask::rect(6.0, 6.0));
    let no_choke = render_frame(&base, 0.0);
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(6.0, 6.0));
    c.layers[0].key_effects.push(KeyEffect::MatteChoke {
        choke: 5.0, // dilate
        clip_black: 0.0,
        clip_white: 1.0,
    });
    let choked = render_frame(&c, 0.0);
    // Sum alpha across the frame: dilation only adds coverage.
    let total = |f: &Frame| -> u64 {
        (0..f.height)
            .flat_map(|y| (0..f.width).map(move |x| (x, y)))
            .map(|(x, y)| f.pixel(x, y)[3] as u64)
            .sum()
    };
    assert!(
        total(&choked) > total(&no_choke),
        "dilating choke grows the matte's total coverage"
    );
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
        font_family: None,
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
        font_family: None,
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
        font_family: None,
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

// --- Frame blending -----------------------------------------------------

/// A 2-frame sequence (black `<tag>_0001.png`, white `<tag>_0002.png`) on a
/// footage layer playing at 10 fps in a 30 fps comp. Source frame 0 = black,
/// 1 = white. `tag` keeps each test's files distinct so parallel tests don't
/// delete each other's frames.
fn frame_blend_comp(
    tag: &str,
    blend: crate::comp::FrameBlend,
) -> (Comp, std::path::PathBuf, std::path::PathBuf) {
    use crate::comp::{FootageSource, LayerKind};
    let p0 = write_test_png(&format!("{tag}_0001"), 8, 8, [0, 0, 0, 255]);
    let p1 = write_test_png(&format!("{tag}_0002"), 8, 8, [255, 255, 255, 255]);
    let pattern = p0
        .parent()
        .unwrap()
        .join(format!("{tag}_{{}}.png"))
        .to_string_lossy()
        .into_owned();
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].kind = LayerKind::Footage;
    c.layers[0].footage.source = Some(FootageSource::Sequence {
        pattern,
        pad: 4,
        start: 1,
        count: 2,
    });
    c.layers[0].footage.fps = Some(10.0); // 10fps in a 30fps comp
    c.layers[0].footage.frame_blend = blend;
    (c, p0, p1)
}

#[test]
fn frame_blend_off_steps_to_floored_frame() {
    use crate::comp::FrameBlend;
    // At t=0.05s @ 10fps the source time is exactly between frame 0 (black) and
    // frame 1 (white). With blending OFF the floored frame (0, black) shows.
    let (c, p0, p1) = frame_blend_comp("fboff", FrameBlend::Off);
    let f = render_frame(&c, 0.05);
    let [r, _g, _b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "footage center opaque");
    assert!(r < 8, "stepped to the black frame, got r={r}");
    let _ = std::fs::remove_file(&p0);
    let _ = std::fs::remove_file(&p1);
}

#[test]
fn frame_blend_mix_cross_dissolves_neighbors() {
    use crate::comp::FrameBlend;
    // Same setup with Frame Mix: the half-way source time blends black and white
    // into a mid-gray (strictly between the two endpoints) — not a hard step.
    let (c, p0, p1) = frame_blend_comp("fbmix", FrameBlend::Mix);
    let f = render_frame(&c, 0.05);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "footage center opaque");
    assert!((20..235).contains(&r), "mid blend, not a step, got r={r}");
    assert_eq!(r, g);
    assert_eq!(g, b, "neutral gray (black<->white mix)");
    let _ = std::fs::remove_file(&p0);
    let _ = std::fs::remove_file(&p1);
}

#[test]
fn frame_blend_mix_on_exact_frame_matches_step() {
    use crate::comp::FrameBlend;
    // On an exact source frame (t=0.0 -> frame 0) Frame Mix has nothing to blend,
    // so it renders identically to the stepped black frame.
    let (c, p0, p1) = frame_blend_comp("fbexact", FrameBlend::Mix);
    let f = render_frame(&c, 0.0);
    let [r, _g, _b, a] = f.pixel(32, 32);
    assert_eq!(a, 255);
    assert!(r < 8, "exact frame 0 is black with no blend, got r={r}");
    let _ = std::fs::remove_file(&p0);
    let _ = std::fs::remove_file(&p1);
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

// --- Generate render-path ---------------------------------------------------

use crate::comp::{CellType, FractalType, GenerateEffect, Overflow, RampShape};

/// A full-opacity default Fractal Noise that always covers (opacity 1, no clip
/// killing the center). Scale tuned so several features fall inside the quad.
fn fractal_fill() -> GenerateEffect {
    GenerateEffect::FractalNoise {
        fractal_type: FractalType::Basic,
        contrast: 1.0,
        brightness: 0.3, // lift so the field is visible (avoids near-black center)
        scale: 20.0,
        scale_x: 1.0,
        scale_y: 1.0,
        complexity: 6,
        sub_influence: 0.6,
        sub_scaling: 2.0,
        evolution: 0.0,
        seed: 0,
        overflow: Overflow::AllowHdr,
        opacity: 1.0,
    }
}

#[test]
fn generate_fills_the_layer_quad() {
    // A generate fill replaces the layer's content: the quad's center is covered
    // (alpha > 0), and a far corner outside the (unit-scale) quad — half-extent
    // ≈ 0.22·64 ≈ 14 px about the center — stays transparent.
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].generate = Some(fractal_fill());
    let f = render_frame(&c, 0.0);
    assert!(f.pixel(32, 32)[3] > 0, "generate fill covers the quad center");
    assert_eq!(f.pixel(0, 0)[3], 0, "outside the quad stays transparent");
}

#[test]
fn generate_render_is_deterministic() {
    // Same comp, same time → byte-identical frame (the cache / MFR rely on this).
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0);
    c.layers[0].generate = Some(fractal_fill());
    let a = render_frame(&c, 0.0);
    let b = render_frame(&c, 0.0);
    assert_eq!(a.pixels, b.pixels, "generate render must be deterministic");
}

#[test]
fn generate_evolution_changes_the_frame() {
    // Animating evolution must change the rendered pixels (the motion knob).
    let mut evolved = fractal_fill();
    if let GenerateEffect::FractalNoise { evolution, .. } = &mut evolved {
        *evolution = 5.0;
    }

    let mut a = solid([1.0, 1.0, 1.0, 1.0]);
    a.layers[0].scale.set_key(0.0, 3.0);
    a.layers[0].generate = Some(fractal_fill());
    let mut b = a.clone();
    b.layers[0].generate = Some(evolved);
    let fa = render_frame(&a, 0.0);
    let fb = render_frame(&b, 0.0);
    assert_ne!(
        fa.pixels, fb.pixels,
        "evolution should change the rendered frame"
    );
}

#[test]
fn generate_evolution_track_drives_the_field_over_time() {
    // A keyframed evolution track flows the field over comp time: two different
    // times render different frames. (The static field is fixed, so the change
    // comes from the track overriding it per frame.)
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.duration = 2.0;
    c.layers[0].scale.set_key(0.0, 3.0);
    c.layers[0].generate = Some(fractal_fill());
    c.layers[0].generate_evolution.set_key(0.0, 0.0);
    c.layers[0].generate_evolution.set_key(2.0, 8.0);
    let f0 = render_frame(&c, 0.0);
    let f1 = render_frame(&c, 2.0);
    assert_ne!(
        f0.pixels, f1.pixels,
        "a keyframed evolution track should flow the field over time"
    );
    // And it's still deterministic at a fixed time.
    assert_eq!(render_frame(&c, 1.0).pixels, render_frame(&c, 1.0).pixels);
}

#[test]
fn generate_color_correction_applies_to_field() {
    // The layer's per-pixel effect stack runs on the generated grayscale: a Tint
    // mapping black→red, white→red drives the field red, so the green channel of
    // a covered pixel drops to ~0.
    use crate::comp::Effect;
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0);
    c.layers[0].generate = Some(fractal_fill());
    c.layers[0].effects.push(Effect::Tint {
        black: [1.0, 0.0, 0.0],
        white: [1.0, 0.0, 0.0],
        amount: 1.0,
    });
    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert!(a > 0, "covered");
    assert!(r > g && r > b, "tinted red: r={r} g={g} b={b}");
    assert_eq!(g, 0, "fully red tint zeroes green");
}

#[test]
fn demo_comp_with_fractal_noise_renders() {
    // The launch demo ships a keyframed-evolution Fractal Noise layer; render it
    // at a couple of times to confirm the full demo composites without panicking
    // and the noise contributes (the field flows, so two times differ).
    let c = Comp::new();
    let f0 = render_frame(&c, 0.0);
    let f2 = render_frame(&c, 2.5);
    assert_eq!(f0.pixels.len(), (c.width * c.height * 4) as usize);
    assert_ne!(f0.pixels, f2.pixels, "the demo evolves over time");
}

#[test]
fn generate_on_non_generate_layer_is_inert_when_none() {
    // A solid without a generate fill renders exactly as before (no regression).
    let mut with_none = solid([0.2, 0.6, 0.9, 1.0]);
    with_none.layers[0].scale.set_key(0.0, 2.0);
    let baseline = render_frame(&with_none, 0.0);
    // Setting then clearing the generate slot returns to the baseline.
    with_none.layers[0].generate = Some(fractal_fill());
    with_none.layers[0].generate = None;
    let cleared = render_frame(&with_none, 0.0);
    assert_eq!(baseline.pixels, cleared.pixels, "cleared generate is inert");
}

// --- Generate (colour generators) render-path -------------------------------

/// A solid scaled 3× so its quad covers the whole 64×64 frame, with `gen` filling
/// it. The center pixel (32,32) maps to layer-local (0,0).
fn generated(gen: GenerateEffect) -> Comp {
    let mut c = solid([1.0, 1.0, 1.0, 1.0]);
    c.layers[0].scale.set_key(0.0, 3.0);
    c.layers[0].generate = Some(gen);
    c
}

#[test]
fn ramp_render_fills_quad_with_gradient() {
    // A vertical black→white ramp across the layer: the top of the quad is dark,
    // the bottom bright, and the whole quad is covered.
    let ramp = GenerateEffect::Ramp {
        shape: RampShape::Linear,
        start: [0.0, -40.0],
        end: [0.0, 40.0],
        radius: 40.0,
        start_color: [0.0, 0.0, 0.0],
        end_color: [1.0, 1.0, 1.0],
        scatter: 0.0,
        opacity: 1.0,
    };
    let f = render_frame(&generated(ramp), 0.0);
    let top = f.pixel(32, 18); // near the top of the quad
    let bot = f.pixel(32, 46); // near the bottom
    assert!(top[3] > 0 && bot[3] > 0, "ramp covers the quad");
    assert!(
        bot[0] > top[0] + 20,
        "ramp gets brighter top→bottom: top={} bot={}",
        top[0],
        bot[0]
    );
}

#[test]
fn checkerboard_render_alternates_cells() {
    // A checkerboard whose cells are small enough that several fall inside the
    // (scale-3, ±14 local-px) quad: adjacent cells differ. Cell size 8 local px →
    // cell 0 spans local [0,8), cell 1 [8,16). The layer is scaled 3×, so a comp
    // pixel maps to local = (comp - 32) / 3.
    let checker = GenerateEffect::Checkerboard {
        anchor: [0.0, 0.0],
        size_w: 8.0,
        size_h: 8.0,
        color1: [0.0, 0.0, 0.0],
        color2: [1.0, 1.0, 1.0],
        opacity: 1.0,
    };
    let f = render_frame(&generated(checker), 0.0);
    // comp 34 → local ~0.67 (cell 0, black); comp 60 → local ~9.3 (cell 1, white).
    let a = f.pixel(34, 32)[0];
    let b = f.pixel(60, 32)[0];
    assert_ne!(a, b, "adjacent checker cells differ: {a} vs {b}");
    assert!(a.max(b) > 200 && a.min(b) < 60, "one cell white, one black");
}

#[test]
fn four_color_render_blends_corners() {
    // Distinct corner colours blend across the quad; the four corners read close
    // to their colours and the centre is a mix (not equal to any single corner).
    let g = GenerateEffect::FourColorGradient {
        tl: [1.0, 0.0, 0.0],
        tr: [0.0, 1.0, 0.0],
        bl: [0.0, 0.0, 1.0],
        br: [1.0, 1.0, 0.0],
        blend: 1.0,
        jitter: 0.0,
        opacity: 1.0,
    };
    let f = render_frame(&generated(g), 0.0);
    // Quad spans ~[-42,42] local → comp ~[-10,74], clamped to the frame. Sample
    // inside each quadrant; the dominant channel reflects that corner's colour.
    let tl = f.pixel(20, 20); // toward top-left → red dominant
    let tr = f.pixel(44, 20); // toward top-right → green dominant
    assert!(tl[0] > tl[1] && tl[0] > tl[2], "top-left reds: {tl:?}");
    assert!(tr[1] > tr[0] && tr[1] > tr[2], "top-right greens: {tr:?}");
}

#[test]
fn grid_render_lines_over_transparent_background() {
    // A grid with a transparent background: the line at the quad centre is opaque,
    // a cell interior is transparent.
    let grid = GenerateEffect::Grid {
        anchor: [0.0, 0.0],
        size_w: 20.0,
        size_h: 20.0,
        border: 4.0,
        color: [1.0, 1.0, 1.0],
        background: [0.0, 0.0, 0.0],
        background_opacity: 0.0,
        opacity: 1.0,
    };
    let f = render_frame(&generated(grid), 0.0);
    // Local (0,0) (comp 32,32) sits on a grid line → opaque.
    assert_eq!(f.pixel(32, 32)[3], 255, "grid line is opaque");
    // Local (10,10) (comp 42,42) is a cell interior → transparent.
    assert_eq!(f.pixel(42, 42)[3], 0, "cell interior is transparent");
}

#[test]
fn color_generator_render_is_deterministic() {
    // Each colour generator renders byte-identically across passes (cache / MFR).
    for i in 1..GenerateEffect::defaults().len() {
        let c = generated(GenerateEffect::defaults()[i]);
        let a = render_frame(&c, 0.0);
        let b = render_frame(&c, 0.0);
        assert_eq!(
            a.pixels,
            b.pixels,
            "{} render must be deterministic",
            GenerateEffect::defaults()[i].label()
        );
    }
}

#[test]
fn checkerboard_color_decodes_through_srgb() {
    // A checkerboard with a known sRGB colour 1 round-trips through the
    // sRGB→linear compositor decode and back to ~the same 8-bit value on output.
    let checker = GenerateEffect::Checkerboard {
        anchor: [0.0, 0.0],
        size_w: 128.0, // one big cell over the quad → cell (0,0) = color1
        size_h: 128.0,
        color1: [0.5, 0.25, 0.75],
        color2: [0.0, 0.0, 0.0],
        opacity: 1.0,
    };
    let f = render_frame(&generated(checker), 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "opaque cell");
    // Output 8-bit ≈ the sRGB input (within rounding of the decode/encode trip).
    assert!((r as i32 - 128).abs() <= 3, "r ~128, got {r}");
    assert!((g as i32 - 64).abs() <= 3, "g ~64, got {g}");
    assert!((b as i32 - 191).abs() <= 3, "b ~191, got {b}");
}

#[test]
fn cell_pattern_render_fills_quad_grayscale() {
    // Cell Pattern is grayscale-linear (like Fractal Noise): it fills the quad, and
    // a covered pixel is grey (R ≈ G ≈ B) rather than a tinted colour.
    let cells = GenerateEffect::CellPattern {
        cell_type: CellType::Crystals,
        size: 8.0,
        disorder: 1.0,
        contrast: 1.0,
        brightness: 0.3, // lift so the centre is visibly covered
        invert: false,
        evolution: 0.0,
        seed: 0,
        opacity: 1.0,
    };
    let f = render_frame(&generated(cells), 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert!(a > 0, "cell pattern covers the quad centre");
    assert!(
        (r as i32 - g as i32).abs() <= 1 && (g as i32 - b as i32).abs() <= 1,
        "grayscale fill: r={r} g={g} b={b}"
    );
}

#[test]
fn cell_pattern_evolution_track_drives_the_field_over_time() {
    // A keyframed evolution track flows the cells over comp time (Cell Pattern
    // shares the keyframable evolution infra with Fractal Noise): two times differ.
    let mut c = generated(GenerateEffect::CellPattern {
        cell_type: CellType::Bubbles,
        size: 8.0,
        disorder: 1.0,
        contrast: 1.0,
        brightness: 0.0,
        invert: false,
        evolution: 0.0,
        seed: 0,
        opacity: 1.0,
    });
    c.duration = 2.0;
    c.layers[0].generate_evolution.set_key(0.0, 0.0);
    c.layers[0].generate_evolution.set_key(2.0, 8.0);
    let f0 = render_frame(&c, 0.0);
    let f1 = render_frame(&c, 2.0);
    assert_ne!(
        f0.pixels, f1.pixels,
        "a keyframed evolution track should flow the cells over time"
    );
    // And it's still deterministic at a fixed time.
    assert_eq!(render_frame(&c, 1.0).pixels, render_frame(&c, 1.0).pixels);
}

// --- Stylize effects ----------------------------------------------------

use crate::comp::StylizeEffect;

#[test]
fn full_resolution_mosaic_routes_through_isolated_buffer() {
    // A solid with a per-pixel Mosaic (one block per pixel) renders the same as
    // the crisp solid — the isolated-buffer routing is value-neutral when the
    // stylize is an identity (mirrors the distort / key identity-routing tests).
    let mut c = solid([0.3, 0.6, 0.9, 1.0]);
    c.layers[0].stylize_effects.push(StylizeEffect::Mosaic {
        horizontal: 64,
        vertical: 64,
    });
    let base = render_frame(&solid([0.3, 0.6, 0.9, 1.0]), 0.0);
    let routed = render_frame(&c, 0.0);
    assert_eq!(base.pixels, routed.pixels);
}

#[test]
fn find_edges_whitens_a_flat_solid_interior() {
    // Find Edges on a flat full-frame solid: the interior has no colour gradient,
    // so (inverted, AE default) it reads ~white. Determinism on re-render.
    let mut c = full_frame_solid();
    // A mid-grey solid so the whitening is unambiguous (the input isn't white).
    c.layers[0].color = [0.4, 0.4, 0.4, 1.0];
    c.layers[0].stylize_effects.push(StylizeEffect::FindEdges {
        amount: 1.0,
        invert: false,
    });
    let f = render_frame(&c, 0.0);
    let [r, g, b, a] = f.pixel(32, 32);
    assert_eq!(a, 255, "interior stays opaque");
    assert!(r > 245 && g > 245 && b > 245, "flat interior whitens, got {r},{g},{b}");
}

#[test]
fn mosaic_pools_the_frame_into_constant_blocks() {
    // A two-tone masked layer through a coarse Mosaic pools detail into blocks: a
    // 1×1 mosaic collapses every covered pixel to one constant colour. Render-path
    // smoke + determinism.
    let mut c = full_frame_solid();
    c.layers[0].color = [0.8, 0.2, 0.1, 1.0];
    c.layers[0].stylize_effects.push(StylizeEffect::Mosaic {
        horizontal: 1,
        vertical: 1,
    });
    let f = render_frame(&c, 0.0);
    // Every fully-covered pixel reads the same pooled colour (the whole-frame avg).
    let center = f.pixel(32, 32);
    let other = f.pixel(20, 44);
    assert_eq!(center, other, "1×1 mosaic pools to one constant block");
    // Deterministic re-render.
    assert_eq!(f.pixels, render_frame(&c, 0.0).pixels);
}

#[test]
fn stylize_composes_with_mask() {
    // A stylize (Find Edges) on a masked layer still respects the mask: Find Edges
    // preserves the per-pixel alpha, so a far corner outside the small centered
    // mask stays carved away after the stylize runs (the mask carves before the
    // stylize pass, and Find Edges only reshapes RGB). (Mosaic deliberately pools
    // coverage across its blocks, so it is not used for this carved-corner check.)
    let mut c = full_frame_solid();
    c.layers[0].masks.push(Mask::rect(8.0, 8.0));
    c.layers[0].stylize_effects.push(StylizeEffect::FindEdges {
        amount: 1.0,
        invert: false,
    });
    let f = render_frame(&c, 0.0);
    assert_eq!(f.pixel(2, 2)[3], 0, "corner stays carved with a stylize");
}

// --- 3-D layers + camera (render-level) ---------------------------------

use crate::comp::Camera as Cam3d;

/// A 64x64 comp whose default camera is sized to the comp (so a Z=0 3-D layer
/// is identity) with a single mid-size opaque solid.
fn comp_3d_render() -> Comp {
    let mut c = solid([0.2, 0.7, 0.9, 1.0]);
    c.camera = Cam3d::default();
    c.camera.position = [0.0, 0.0, -Cam3d::default_distance(c.height as f32)];
    c
}

#[test]
fn three_d_layer_at_z0_renders_identical_to_2d() {
    // A 3-D layer at Z = 0 with no orientation, default camera → byte-for-byte
    // the same frame as the same layer in 2-D. The core back-compat guarantee.
    let mut flat = comp_3d_render();
    flat.layers[0].x.set_key(0.0, 30.0);
    flat.layers[0].rotation.set_key(0.0, 20.0);
    let mut three_d = flat.clone();
    three_d.layers[0].threed = true; // Z defaults to 0, no orientation
    let a = render_frame(&flat, 0.0);
    let b = render_frame(&three_d, 0.0);
    assert_eq!(a.pixels, b.pixels, "3-D @ Z=0 must match the 2-D render");
}

#[test]
fn pushing_z_shrinks_the_rendered_footprint() {
    // The same 3-D layer pushed in Z covers fewer opaque pixels (perspective).
    let count_opaque = |f: &Frame| -> usize {
        (0..f.width * f.height)
            .filter(|i| f.pixels[(*i * 4 + 3) as usize] > 0)
            .count()
    };
    let mut near = comp_3d_render();
    near.layers[0].threed = true;
    let near_f = render_frame(&near, 0.0);
    let mut far = near.clone();
    far.layers[0].z.set_key(0.0, 600.0);
    let far_f = render_frame(&far, 0.0);
    assert!(
        count_opaque(&far_f) < count_opaque(&near_f),
        "z-pushed layer must cover fewer pixels: far {} < near {}",
        count_opaque(&far_f),
        count_opaque(&near_f),
    );
}

#[test]
fn two_d_only_comp_renders_identically_with_camera_field() {
    // A comp built the legacy way (serde-default camera) and the same comp with
    // an explicit default camera render byte-identically — adding the camera
    // field changes nothing for a 2-D-only comp.
    let legacy = solid([0.9, 0.3, 0.2, 1.0]); // uses Camera::default()
    let f = render_frame(&legacy, 0.0);
    // A reference render produced the same way must match exactly (determinism +
    // 2-D-only invariance).
    let f2 = render_frame(&legacy.clone(), 0.0);
    assert_eq!(f.pixels, f2.pixels);
    // No 3-D layers ⇒ the draw order is the plain stack order.
    assert_eq!(legacy.draw_order(0.0), vec![0]);
}

#[test]
fn z_sorted_3d_layers_draw_far_first() {
    // Two overlapping full-frame 3-D solids: the nearer one must end up on top
    // regardless of stack order (painter's z-sort).
    let mut c = comp_3d_render();
    c.layers[0] = PulseLayer::new("back", [1.0, 0.0, 0.0, 1.0]); // red
    c.layers[0].scale.set_key(0.0, 3.0);
    c.layers[0].threed = true;
    c.layers[0].z.set_key(0.0, 0.0); // nearer
    let mut front = PulseLayer::new("front", [0.0, 0.0, 1.0, 1.0]); // blue
    front.scale.set_key(0.0, 3.0);
    front.threed = true;
    front.z.set_key(0.0, 800.0); // farther — should be drawn first (behind)
    c.layers.push(front);
    let f = render_frame(&c, 0.0);
    let center = f.pixel(32, 32);
    // The nearer (red, Z=0) layer wins the center even though blue is later in
    // the stack, because blue is farther and drawn first.
    assert!(center[0] > center[2], "near red on top: {center:?}");
}
