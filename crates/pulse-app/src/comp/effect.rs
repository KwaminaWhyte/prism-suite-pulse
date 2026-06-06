//! Layer kinds and the per-pixel color-correction effect stack.

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    #[default]
    Solid,
    Null,
    Adjustment,
    Shape,
    Text,
}

impl LayerKind {
    /// All kinds, in menu order.
    pub const ALL: [LayerKind; 5] = [
        LayerKind::Solid,
        LayerKind::Text,
        LayerKind::Shape,
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
        }
    }

    /// Whether a layer of this kind draws its own pixels. A null draws nothing;
    /// an adjustment draws nothing of its own (it only re-processes the layers
    /// beneath it). A solid, a shape, and text all draw their own pixels.
    pub fn draws_own_pixels(self) -> bool {
        matches!(self, LayerKind::Solid | LayerKind::Shape | LayerKind::Text)
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
/// & Contrast**, **Exposure**, and **Levels** (input/output black & white +
/// gamma). All parameters are plain scalars (not yet animatable `Property`s —
/// that arrives with the typed-property rebuild), so the stack is a fixed look
/// per layer for now.
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
}

impl Effect {
    /// A short, stable label for the UI and for the "add effect" menu.
    pub fn label(&self) -> &'static str {
        match self {
            Effect::Tint { .. } => "Tint",
            Effect::BrightnessContrast { .. } => "Brightness & Contrast",
            Effect::Exposure { .. } => "Exposure",
            Effect::Levels { .. } => "Levels",
        }
    }

    /// A fresh, value-neutral (or sensibly-default) instance of each effect, for
    /// the "add effect" menu. Defaults are identity where possible so adding an
    /// effect never changes the look until a parameter is touched — except Tint,
    /// which seeds a recognizable black→white map at full strength.
    pub fn defaults() -> [Effect; 4] {
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
