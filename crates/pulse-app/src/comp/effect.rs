//! Layer kinds and the per-pixel color-correction effect stack.

use prism_core::adjust::ChannelMixerMatrix;
use prism_core::gradient::{ColorStop, Gradient, GradientType};
use serde::{Deserialize, Serialize};

/// What a layer *is* — its source/role in the composite, in the After-Effects
/// sense.
///
/// - [`LayerKind::Solid`] — a solid color quad (the v0 layer): it draws its own
///   pixels and its effect stack processes those pixels.
/// - [`LayerKind::Null`] — an invisible reference layer: it renders nothing, but
///   its transform is real, so it's useful purely as a **parent** (a controllable
///   pivot/rig handle). Matches AE's null object.
/// - [`LayerKind::Adjustment`] — draws nothing of its own; instead its **effect
///   stack** is applied to the composite of every layer *below* it, within the
///   layer's transformed bounds. Matches AE's adjustment layer.
/// - [`LayerKind::Shape`] — draws a parametric vector **shape** stack (its
///   [`shape`](super::PulseLayer::shape) field): rectangles / ellipses /
///   polygons / stars with fills and strokes, rasterized in the layer's local
///   frame. Matches AE's shape layer.
/// - [`LayerKind::Text`] — draws a string with the built-in stroke vector font
///   (its [`text`](super::PulseLayer::text) field): font size, tracking,
///   leading, alignment, and a fill / stroke, rasterized in the layer's local
///   frame. Matches AE's text layer.
/// - [`LayerKind::Footage`] — draws decoded image **footage** (a single still or
///   a numbered image sequence, its [`footage`](super::PulseLayer::footage)
///   field), sampled at comp time `t` and rasterized into the layer's quad.
///   Matches AE's footage layer (stills + sequences; real video decode is
///   deferred to the shared `prism-media` crate).
/// - [`LayerKind::Precomp`] — draws a **nested composition**: it references
///   another [`Comp`](super::Comp) by id (its
///   [`precomp`](super::PulseLayer::precomp) field) and, at comp time `t`, that
///   referenced comp is rendered recursively (through the same render path, at a
///   time-offset mapping) into the layer's quad, then composited like footage.
///   Reference cycles are detected and broken by the renderer's visited-set
///   guard. Matches AE's precomp / nested composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    #[default]
    Solid,
    Null,
    Adjustment,
    Shape,
    Text,
    Footage,
    Precomp,
}

impl LayerKind {
    /// All kinds, in menu order.
    pub const ALL: [LayerKind; 7] = [
        LayerKind::Solid,
        LayerKind::Text,
        LayerKind::Shape,
        LayerKind::Footage,
        LayerKind::Precomp,
        LayerKind::Null,
        LayerKind::Adjustment,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LayerKind::Solid => "Solid",
            LayerKind::Null => "Null",
            LayerKind::Adjustment => "Adjustment",
            LayerKind::Shape => "Shape",
            LayerKind::Text => "Text",
            LayerKind::Footage => "Footage",
            LayerKind::Precomp => "Precomp",
        }
    }

    /// Whether a layer of this kind draws its own pixels. A null draws nothing;
    /// an adjustment draws nothing of its own (it only re-processes the layers
    /// beneath it). A solid, a shape, text, footage, and a precomp all draw their
    /// own pixels (a precomp draws the rendered nested comp).
    pub fn draws_own_pixels(self) -> bool {
        matches!(
            self,
            LayerKind::Solid
                | LayerKind::Shape
                | LayerKind::Text
                | LayerKind::Footage
                | LayerKind::Precomp
        )
    }
}

/// A single non-destructive **effect** in a layer's effect stack.
///
/// Effects are pure color-correction passes evaluated in **linear light** on a
/// straight (non-premultiplied) RGBA pixel: `apply` maps a linear-light RGBA in
/// `[0,1]` (alpha carried through unchanged) to a new linear-light RGBA. They
/// stack in order — the output of one feeds the next. Kept Pulse-side and pure
/// (no GPU, no time) so each is unit-testable; they'll migrate to the suite's
/// `prism-fx` host when that lands.
///
/// These are the After-Effects color-correction staples: **Tint**, **Brightness
/// & Contrast**, **Exposure**, **Levels** (input/output black & white + gamma),
/// **Hue/Saturation** (HSL rotate/saturate/lighten), **Curves** (a master tone
/// curve), and **Color Balance** (per-range shadow/midtone/highlight pushes).
/// All parameters are plain scalars (not yet animatable `Property`s — that
/// arrives with the typed-property rebuild), so the stack is a fixed look per
/// layer for now.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// Map black→`black`, white→`white`, blending by per-pixel luminance and
    /// `amount` (0 = original, 1 = fully tinted). The classic two-color Tint.
    Tint {
        black: [f32; 3],
        white: [f32; 3],
        amount: f32,
    },
    /// Linear brightness offset + contrast pivot about 0.5.
    /// `out = (in - 0.5) * contrast + 0.5 + brightness`.
    BrightnessContrast { brightness: f32, contrast: f32 },
    /// Photographic exposure in stops: `out = in * 2^stops`, then an `offset`
    /// lift and a `gamma` (so it doubles as a simple grade).
    Exposure { stops: f32, offset: f32, gamma: f32 },
    /// Levels: remap `[in_black, in_white]` to `[0,1]` with a midtone `gamma`,
    /// then to the `[out_black, out_white]` output range. The motion-graphics
    /// contrast workhorse.
    Levels {
        in_black: f32,
        in_white: f32,
        gamma: f32,
        out_black: f32,
        out_white: f32,
    },
    /// Hue / Saturation / Lightness (After Effects' "Hue/Saturation"): rotate the
    /// hue by `hue` degrees, scale saturation by `1 + saturation` and lightness
    /// by `1 + lightness` (so `0` is a no-op, `+1` doubles, `-1` zeroes). The
    /// pixel round-trips through HSL; alpha is untouched.
    HueSaturation {
        /// Hue rotation in degrees (wrapped).
        hue: f32,
        /// Saturation adjustment, `-1..=1` (0 = unchanged, -1 = grayscale).
        saturation: f32,
        /// Lightness adjustment, `-1..=1` (0 = unchanged, -1 = black, +1 = white).
        lightness: f32,
    },
    /// A master tone **Curve** defined by five control points at inputs
    /// `0, 0.25, 0.5, 0.75, 1.0` (the AE Curves default grid). Each output is in
    /// `[0,1]`; the curve is a monotone Catmull-Rom spline through the points,
    /// applied to every RGB channel. The straight identity ramp is a no-op.
    Curves {
        /// Output values at inputs 0, ¼, ½, ¾, 1 (identity = `[0, 0.25, 0.5, 0.75, 1.0]`).
        points: [f32; 5],
    },
    /// Color Balance (After Effects' "Color Balance"): independent
    /// red/green/blue pushes for **shadows**, **midtones**, and **highlights**.
    /// Each push is `-1..=1`; the per-range weight is a smooth function of the
    /// pixel's luma so the three ranges blend (shadows weight darks, highlights
    /// weight brights, midtones peak at mid-gray). A no-op when all are zero.
    ColorBalance {
        /// Shadow R/G/B push, each `-1..=1`.
        shadows: [f32; 3],
        /// Midtone R/G/B push, each `-1..=1`.
        midtones: [f32; 3],
        /// Highlight R/G/B push, each `-1..=1`.
        highlights: [f32; 3],
    },
    /// Channel Mixer (After Effects / Photoshop "Channel Mixer"): each output
    /// channel is a linear mix of the input R/G/B plus a constant. Each of `red`,
    /// `green`, `blue` is `[from_r, from_g, from_b, constant]`; `monochrome`
    /// collapses to the **red** row written to all three outputs (a custom
    /// grayscale). The mix math is the shared
    /// [`prism_core::adjust::ChannelMixerMatrix::apply`] — Pulse only wires it.
    /// Identity (each output = its own input) is a no-op.
    ChannelMixer {
        /// Red output row: `[from_r, from_g, from_b, constant]`.
        red: [f32; 4],
        /// Green output row: `[from_r, from_g, from_b, constant]`.
        green: [f32; 4],
        /// Blue output row: `[from_r, from_g, from_b, constant]`.
        blue: [f32; 4],
        /// Collapse to a single weighted gray (the `red` row → all outputs).
        monochrome: bool,
    },
    /// Gradient Map (After Effects / Photoshop "Gradient Map"): map each pixel's
    /// luminance through a three-stop color gradient — `low` at luma 0, `mid` at
    /// luma 0.5, `high` at luma 1 — so black takes the first stop, white the last,
    /// mid-gray the middle. Built from the shared multi-stop
    /// [`prism_core::gradient::Gradient`] and sampled with `Gradient::color_at`.
    /// `amount` blends original→mapped (0 = original, 1 = full map).
    GradientMap {
        /// Shadow color at luma 0 (straight linear RGB).
        low: [f32; 3],
        /// Midtone color at luma 0.5 (straight linear RGB).
        mid: [f32; 3],
        /// Highlight color at luma 1 (straight linear RGB).
        high: [f32; 3],
        /// Original→mapped blend, `0..=1`.
        amount: f32,
    },
    /// Tritone / Tint-map (After Effects' "Tritone"): like Gradient Map but
    /// authored as three named tonal targets — `shadows`, `midtones`,
    /// `highlights` — mapped by luma through the same shared multi-stop
    /// [`prism_core::gradient::Gradient`] (`Gradient::color_at`). Distinct from
    /// Gradient Map only in intent / defaults (a classic three-tone duotone-style
    /// grade). `amount` blends original→toned.
    Tritone {
        /// Color mapped to the darkest pixels (luma 0).
        shadows: [f32; 3],
        /// Color mapped to mid-gray pixels (luma 0.5).
        midtones: [f32; 3],
        /// Color mapped to the brightest pixels (luma 1).
        highlights: [f32; 3],
        /// Original→toned blend, `0..=1`.
        amount: f32,
    },
}

impl Effect {
    /// A short, stable label for the UI and for the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            Effect::Tint { .. } => "Tint",
            Effect::BrightnessContrast { .. } => "Brightness & Contrast",
            Effect::Exposure { .. } => "Exposure",
            Effect::Levels { .. } => "Levels",
            Effect::HueSaturation { .. } => "Hue / Saturation",
            Effect::Curves { .. } => "Curves",
            Effect::ColorBalance { .. } => "Color Balance",
            Effect::ChannelMixer { .. } => "Channel Mixer",
            Effect::GradientMap { .. } => "Gradient Map",
            Effect::Tritone { .. } => "Tritone",
        }
    }

    /// The identity curve points (the straight `y = x` ramp): a no-op [`Effect::Curves`].
    pub const CURVE_IDENTITY: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

    /// A fresh, value-neutral (or sensibly-default) instance of each effect, for
    /// the "add effect" menu. Defaults are identity where possible so adding an
    /// effect never changes the look until a parameter is touched — except Tint,
    /// which seeds a recognizable black→white map at full strength.
    pub fn defaults() -> [Effect; 10] {
        [
            Effect::Tint {
                black: [0.0, 0.0, 0.0],
                white: [1.0, 1.0, 1.0],
                amount: 1.0,
            },
            Effect::BrightnessContrast {
                brightness: 0.0,
                contrast: 1.0,
            },
            Effect::Exposure {
                stops: 0.0,
                offset: 0.0,
                gamma: 1.0,
            },
            Effect::Levels {
                in_black: 0.0,
                in_white: 1.0,
                gamma: 1.0,
                out_black: 0.0,
                out_white: 1.0,
            },
            Effect::HueSaturation {
                hue: 0.0,
                saturation: 0.0,
                lightness: 0.0,
            },
            Effect::Curves {
                points: Effect::CURVE_IDENTITY,
            },
            Effect::ColorBalance {
                shadows: [0.0; 3],
                midtones: [0.0; 3],
                highlights: [0.0; 3],
            },
            Effect::ChannelMixer {
                // Identity: each output channel = its own input, no constant.
                red: [1.0, 0.0, 0.0, 0.0],
                green: [0.0, 1.0, 0.0, 0.0],
                blue: [0.0, 0.0, 1.0, 0.0],
                monochrome: false,
            },
            Effect::GradientMap {
                // Identity grayscale map (black→black, mid→gray, white→white):
                // adding it is a no-op until the stops are recolored.
                low: [0.0, 0.0, 0.0],
                mid: [0.5, 0.5, 0.5],
                high: [1.0, 1.0, 1.0],
                amount: 1.0,
            },
            Effect::Tritone {
                // A recognizable three-tone grade (deep blue shadows, warm
                // midtones, pale highlights) at full strength.
                shadows: [0.04, 0.02, 0.18],
                midtones: [0.55, 0.42, 0.30],
                highlights: [0.98, 0.95, 0.82],
                amount: 1.0,
            },
        ]
    }

    /// Apply this effect to a straight linear-light RGBA pixel, returning the
    /// processed pixel. Alpha is passed through untouched (these are color
    /// operations); RGB stays clamped to `[0,1]` on output.
    pub fn apply(&self, rgba: [f32; 4]) -> [f32; 4] {
        let [r, g, b, a] = rgba;
        let out = match *self {
            Effect::Tint {
                black,
                white,
                amount,
            } => {
                // Rec.709 luma in linear light, used as the tint mix parameter.
                let l = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0);
                let lerp = |lo: f32, hi: f32| lo + (hi - lo) * l;
                let tinted = [
                    lerp(black[0], white[0]),
                    lerp(black[1], white[1]),
                    lerp(black[2], white[2]),
                ];
                let m = amount.clamp(0.0, 1.0);
                [
                    r + (tinted[0] - r) * m,
                    g + (tinted[1] - g) * m,
                    b + (tinted[2] - b) * m,
                ]
            }
            Effect::BrightnessContrast {
                brightness,
                contrast,
            } => {
                let f = |v: f32| (v - 0.5) * contrast + 0.5 + brightness;
                [f(r), f(g), f(b)]
            }
            Effect::Exposure {
                stops,
                offset,
                gamma,
            } => {
                let mul = 2.0_f32.powf(stops);
                let g_inv = 1.0 / gamma.max(1e-3);
                let f = |v: f32| {
                    let lifted = (v * mul + offset).max(0.0);
                    lifted.powf(g_inv)
                };
                [f(r), f(g), f(b)]
            }
            Effect::Levels {
                in_black,
                in_white,
                gamma,
                out_black,
                out_white,
            } => {
                let span = (in_white - in_black).abs().max(1e-3);
                let g_inv = 1.0 / gamma.max(1e-3);
                let f = |v: f32| {
                    let normalized = ((v - in_black) / span).clamp(0.0, 1.0);
                    let curved = normalized.powf(g_inv);
                    out_black + (out_white - out_black) * curved
                };
                [f(r), f(g), f(b)]
            }
            Effect::HueSaturation {
                hue,
                saturation,
                lightness,
            } => {
                let (mut h, mut s, mut l) = rgb_to_hsl(r, g, b);
                h = (h + hue).rem_euclid(360.0);
                s = (s * (1.0 + saturation)).clamp(0.0, 1.0);
                // Lightness: positive pushes toward white, negative toward black,
                // scaling the headroom so ±1 reaches the extreme.
                l = if lightness >= 0.0 {
                    l + (1.0 - l) * lightness.min(1.0)
                } else {
                    l * (1.0 + lightness.max(-1.0))
                }
                .clamp(0.0, 1.0);
                hsl_to_rgb(h, s, l)
            }
            Effect::Curves { points } => [
                curve_eval(&points, r),
                curve_eval(&points, g),
                curve_eval(&points, b),
            ],
            Effect::ColorBalance {
                shadows,
                midtones,
                highlights,
            } => {
                let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0);
                // Smooth per-range weights from luma: shadows weight darks,
                // highlights weight brights, midtones peak at mid-gray. The shadow
                // and highlight weights are smoothstep ramps; the midtone weight is
                // a bell (1 - |2·luma - 1|) so all three sum to a sensible blend.
                let w_shadow = 1.0 - smoothstep(0.0, 0.5, luma);
                let w_high = smoothstep(0.5, 1.0, luma);
                let w_mid = 1.0 - (2.0 * luma - 1.0).abs();
                let push = |ch: usize| {
                    shadows[ch] * w_shadow + midtones[ch] * w_mid + highlights[ch] * w_high
                };
                // A push scales toward 1 (positive) or 0 (negative) by its weight,
                // capped at 0.5 magnitude per range so the blend stays tasteful.
                let apply_push = |v: f32, p: f32| {
                    let p = p * 0.5;
                    if p >= 0.0 {
                        v + (1.0 - v) * p
                    } else {
                        v * (1.0 + p)
                    }
                };
                [
                    apply_push(r, push(0)),
                    apply_push(g, push(1)),
                    apply_push(b, push(2)),
                ]
            }
            Effect::ChannelMixer {
                red,
                green,
                blue,
                monochrome,
            } => {
                // Reuse the shared prism-core mixer math — Pulse only wires it.
                let m = ChannelMixerMatrix {
                    r: red,
                    g: green,
                    b: blue,
                    monochrome,
                };
                m.apply([r, g, b])
            }
            Effect::GradientMap {
                low,
                mid,
                high,
                amount,
            } => gradient_map(low, mid, high, amount, r, g, b),
            Effect::Tritone {
                shadows,
                midtones,
                highlights,
                amount,
            } => gradient_map(shadows, midtones, highlights, amount, r, g, b),
        };
        [
            out[0].clamp(0.0, 1.0),
            out[1].clamp(0.0, 1.0),
            out[2].clamp(0.0, 1.0),
            a,
        ]
    }
}

/// Apply an ordered effect stack to a straight linear-light RGBA pixel.
pub fn apply_effects(effects: &[Effect], mut rgba: [f32; 4]) -> [f32; 4] {
    for e in effects {
        rgba = e.apply(rgba);
    }
    rgba
}

/// Map a pixel's Rec.709 luma through a three-stop color gradient (`low` at luma
/// 0, `mid` at 0.5, `high` at 1) and blend original→mapped by `amount`. The
/// gradient is the shared multi-stop [`prism_core::gradient::Gradient`], sampled
/// with [`Gradient::color_at`] — the reusable primitive behind both Gradient Map
/// and Tritone. Returns straight RGB (alpha handled by the caller).
fn gradient_map(
    low: [f32; 3],
    mid: [f32; 3],
    high: [f32; 3],
    amount: f32,
    r: f32,
    g: f32,
    b: f32,
) -> [f32; 3] {
    let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0);
    let grad = Gradient {
        color_stops: vec![
            ColorStop::new(0.0, low),
            ColorStop::new(0.5, mid),
            ColorStop::new(1.0, high),
        ],
        opacity_stops: Vec::new(),
        kind: GradientType::Linear,
        dither: false,
    };
    let mapped = grad.color_at(luma);
    let m = amount.clamp(0.0, 1.0);
    [
        r + (mapped[0] - r) * m,
        g + (mapped[1] - g) * m,
        b + (mapped[2] - b) * m,
    ]
}

/// The classic GLSL `smoothstep`: 0 below `e0`, 1 above `e1`, a smooth Hermite
/// ramp between. `e0 == e1` degenerates to a hard step at the edge.
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Convert RGB (each `0..=1`) to HSL — hue in degrees `0..360`, saturation and
/// lightness in `0..=1`.
pub fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= f32::EPSILON {
        return (0.0, 0.0, l); // achromatic
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h.rem_euclid(360.0), s.clamp(0.0, 1.0), l)
}

/// Convert HSL (hue in degrees, s/l in `0..=1`) back to RGB (each `0..=1`).
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    [
        (r1 + m).clamp(0.0, 1.0),
        (g1 + m).clamp(0.0, 1.0),
        (b1 + m).clamp(0.0, 1.0),
    ]
}

/// Evaluate a 5-point tone curve at input `x` (`0..=1`). The control points are
/// the outputs at inputs `0, ¼, ½, ¾, 1`; between them the curve is a
/// Catmull-Rom spline (so a smooth S-curve through hand-set points), with the
/// result clamped to `[0,1]`. Inputs outside `[0,1]` clamp to the end points.
pub fn curve_eval(points: &[f32; 5], x: f32) -> f32 {
    let n = points.len(); // 5
    let xc = x.clamp(0.0, 1.0);
    // Locate the segment: each spans 1/(n-1) in input.
    let seg_w = 1.0 / (n - 1) as f32;
    let seg = ((xc / seg_w).floor() as usize).min(n - 2);
    let local = (xc - seg as f32 * seg_w) / seg_w; // 0..1 within the segment
                                                   // Catmull-Rom needs the two neighbours; at the ends, *extrapolate* the
                                                   // missing point by reflecting (`2·edge − inner`) rather than clamping, so a
                                                   // collinear (e.g. identity) ramp stays exactly linear instead of bulging.
    let at = |i: usize| points[i];
    let p1 = at(seg);
    let p2 = at(seg + 1);
    let p0 = if seg == 0 { 2.0 * p1 - p2 } else { at(seg - 1) };
    let p3 = if seg + 2 >= n {
        2.0 * p2 - p1
    } else {
        at(seg + 2)
    };
    let t = local;
    let t2 = t * t;
    let t3 = t2 * t;
    // Standard Catmull-Rom basis (tension 0.5).
    let y = 0.5
        * ((2.0 * p1)
            + (-p0 + p2) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3);
    y.clamp(0.0, 1.0)
}
