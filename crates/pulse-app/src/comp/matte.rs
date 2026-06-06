//! Track mattes: how a layer borrows the layer above it to define transparency.

use serde::{Deserialize, Serialize};

/// A **track matte**: how a layer borrows the layer directly above it to define
/// its own transparency (After Effects' track-matte feature).
///
/// When a layer's matte is anything other than [`MatteMode::None`], the layer
/// immediately **above** it in the stack (the next-higher index) becomes its
/// *matte source*: that source is removed from normal compositing and instead
/// multiplies this layer's per-pixel alpha. An **alpha** matte uses the source's
/// alpha; a **luma** matte uses the source's perceptual brightness. Either can be
/// **inverted** (`1 - factor`) — so a layer shows only where its matte source is
/// transparent / dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatteMode {
    /// No matte: the layer composites normally and the layer above is unaffected.
    #[default]
    None,
    /// Alpha matte: this layer is visible where the matte source is opaque.
    Alpha,
    /// Inverted alpha matte: visible where the source is *transparent*.
    AlphaInverted,
    /// Luma matte: this layer is visible where the matte source is *bright*.
    Luma,
    /// Inverted luma matte: visible where the source is *dark*.
    LumaInverted,
}

impl MatteMode {
    /// All modes, in menu order.
    pub const ALL: [MatteMode; 5] = [
        MatteMode::None,
        MatteMode::Alpha,
        MatteMode::AlphaInverted,
        MatteMode::Luma,
        MatteMode::LumaInverted,
    ];

    /// Short label for the matte picker.
    pub fn label(self) -> &'static str {
        match self {
            MatteMode::None => "No matte",
            MatteMode::Alpha => "Alpha",
            MatteMode::AlphaInverted => "Alpha inverted",
            MatteMode::Luma => "Luma",
            MatteMode::LumaInverted => "Luma inverted",
        }
    }

    /// Whether this mode actually consumes a matte source (everything but
    /// [`MatteMode::None`]).
    pub fn is_active(self) -> bool {
        !matches!(self, MatteMode::None)
    }

    /// The matte multiplier in `[0, 1]` for a matte-source pixel given as a
    /// **straight, linear-light** RGBA. Alpha modes read the source's alpha; luma
    /// modes read its Rec.709 luma (weighted by alpha, so a transparent bright
    /// pixel still mattes to ~0); the `*Inverted` variants return `1 - factor`.
    /// [`MatteMode::None`] is a passthrough (factor `1`).
    pub fn factor(self, src: [f32; 4]) -> f32 {
        let [r, g, b, a] = src;
        let alpha = a.clamp(0.0, 1.0);
        let f = match self {
            MatteMode::None => return 1.0,
            MatteMode::Alpha => alpha,
            MatteMode::AlphaInverted => 1.0 - alpha,
            MatteMode::Luma => (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0) * alpha,
            MatteMode::LumaInverted => {
                1.0 - (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 1.0) * alpha
            }
        };
        f.clamp(0.0, 1.0)
    }
}
