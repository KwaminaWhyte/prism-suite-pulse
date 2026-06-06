//! Animatable-property identifiers and composition motion-blur settings.

use serde::{Deserialize, Serialize};

/// Which of a layer's animatable tracks; used to drive generic property UI.
///
/// [`Prop::AnchorX`] / [`Prop::AnchorY`] are the layer's **anchor point**: the
/// pivot that scale and rotation happen about, and the layer-local point that is
/// aligned to `(X, Y)` position. The anchor is expressed as an offset (comp px)
/// from the layer's geometric center, so the default `(0, 0)` keeps a layer
/// pivoting about its center exactly as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    AnchorX,
    AnchorY,
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
}

impl Prop {
    /// All properties, in display order (anchor first, matching After Effects'
    /// Transform group ordering).
    pub const ALL: [Prop; 7] = [
        Prop::AnchorX,
        Prop::AnchorY,
        Prop::X,
        Prop::Y,
        Prop::Scale,
        Prop::Rotation,
        Prop::Opacity,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Prop::AnchorX => "Anchor X",
            Prop::AnchorY => "Anchor Y",
            Prop::X => "X position",
            Prop::Y => "Y position",
            Prop::Scale => "Scale",
            Prop::Rotation => "Rotation",
            Prop::Opacity => "Opacity",
        }
    }

    /// The property's resting value when no keyframes exist.
    pub fn default_value(self) -> f32 {
        match self {
            Prop::AnchorX | Prop::AnchorY | Prop::X | Prop::Y | Prop::Rotation => 0.0,
            Prop::Scale | Prop::Opacity => 1.0,
        }
    }

    /// Editing range and unit suffix for the value slider.
    pub fn range(self) -> (std::ops::RangeInclusive<f32>, &'static str) {
        match self {
            Prop::AnchorX | Prop::AnchorY => (-2000.0..=2000.0, " px"),
            Prop::X => (-2000.0..=2000.0, " px"),
            Prop::Y => (-2000.0..=2000.0, " px"),
            Prop::Scale => (0.0..=5.0, "x"),
            Prop::Rotation => (-360.0..=360.0, "°"),
            Prop::Opacity => (0.0..=1.0, ""),
        }
    }
}

/// **Motion blur** settings for a composition (After Effects' comp shutter
/// model).
///
/// When `enabled` (the comp's master motion-blur switch), every layer that has
/// its own per-layer motion-blur flag set is rendered by integrating
/// `samples` sub-frame snapshots of its transform across the time the virtual
/// shutter is open. The open window is a fraction of the frame interval set by
/// `angle` (degrees: 360° = the whole frame, 180° = half — the cinematic
/// default), positioned by `phase` (degrees, relative to the frame): the
/// shutter opens at `phase/360` of a frame before the frame time and stays open
/// for `angle/360` of a frame. Accumulating the snapshots in linear light is the
/// float-compositor motion-blur recipe.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionBlur {
    /// Comp-level master switch. With it off, no layer is motion-blurred.
    pub enabled: bool,
    /// Shutter angle in degrees: the fraction of a frame the shutter is open
    /// (`angle/360`). 360° blurs across a whole frame; 180° (the default) across
    /// half. Clamped to `(0, 720]` when sampled.
    pub angle: f32,
    /// Shutter phase in degrees: where the open window sits relative to the
    /// frame time (`phase/360` of a frame). The After-Effects default `0` opens
    /// the shutter slightly before the frame and closes after; `-angle/2`
    /// centers it on the frame time.
    pub phase: f32,
    /// Number of sub-frame samples integrated across the shutter. More samples =
    /// smoother blur at higher cost. Clamped to `[1, 64]` when sampled.
    pub samples: u32,
}

impl Default for MotionBlur {
    fn default() -> Self {
        // After Effects' defaults: a 180° shutter at phase 0, sampled enough to
        // look smooth offline. Disabled by default so a fresh comp renders crisp
        // until the user opts in.
        MotionBlur {
            enabled: false,
            angle: 180.0,
            phase: 0.0,
            samples: 16,
        }
    }
}

impl MotionBlur {
    /// The shutter-open time window `[open, close]` (seconds) for a frame
    /// presented at `t`, given the comp's `fps`. The window width is
    /// `(angle/360)` of a frame; it is shifted by `(phase/360)` of a frame so
    /// `phase = 0` opens it at `t` and `phase = -angle/2` centers it on `t`.
    ///
    /// `angle` is clamped to `(0, 720]` and `fps` floored at 1 so the window is
    /// always a finite, non-empty interval.
    pub fn shutter_window(self, t: f32, fps: f32) -> (f32, f32) {
        let fps = fps.max(1.0);
        let frame = 1.0 / fps;
        let angle = self.angle.clamp(1e-3, 720.0);
        let width = (angle / 360.0) * frame;
        let offset = (self.phase / 360.0) * frame;
        let open = t + offset;
        (open, open + width)
    }

    /// The presentation times of each motion-blur sample for a frame at `t`:
    /// `samples` points spread evenly across the shutter window, taken at the
    /// *center* of each sub-interval (midpoint sampling, so the set is symmetric
    /// and has no endpoint bias). `samples` is clamped to `[1, 64]`.
    ///
    /// A single sample lands at the window center (degrading gracefully to a
    /// crisp snapshot at the shutter's midpoint).
    pub fn sample_times(self, t: f32, fps: f32) -> Vec<f32> {
        let n = self.samples.clamp(1, 64);
        let (open, close) = self.shutter_window(t, fps);
        let span = close - open;
        (0..n)
            .map(|i| open + span * ((i as f32 + 0.5) / n as f32))
            .collect()
    }
}
