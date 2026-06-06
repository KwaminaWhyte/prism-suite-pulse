//! Pulse's motion document model.
//!
//! A [`Comp`] is a composition: a fixed-size canvas with a duration and frame
//! rate, holding an ordered stack of [`PulseLayer`]s. Each layer carries five
//! animatable properties — x, y, scale, rotation, opacity — stored as
//! [`Track`]s of [`Keyframe`]s.
//!
//! Sampling: a track linearly interpolates between bracketing keyframes; before
//! the first key it holds the first value, after the last it holds the last
//! value (constant extrapolation). An empty track returns the property's
//! sensible default.
//!
//! Layer paint order is bottom-up: index 0 is drawn first (back), the last
//! index on top. Colors are straight sRGB RGBA in `[f32; 4]` so they round-trip
//! cleanly through egui's color picker and JSON.

use serde::{Deserialize, Serialize};

/// A single animation keyframe: a property `value` at time `t` (seconds).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Keyframe {
    pub t: f32,
    pub value: f32,
}

/// One animated property: a time-ordered list of keyframes.
///
/// Invariant: `keys` is kept sorted ascending by `t` (see [`Track::set_key`]).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Track {
    pub keys: Vec<Keyframe>,
}

impl Track {
    /// Sample the track at time `t`, falling back to `default` when empty.
    ///
    /// Linear interpolation between bracketing keys; constant hold outside the
    /// [first, last] range.
    pub fn sample(&self, t: f32, default: f32) -> f32 {
        match self.keys.as_slice() {
            [] => default,
            [only] => only.value,
            keys => {
                let first = keys[0];
                let last = keys[keys.len() - 1];
                if t <= first.t {
                    return first.value;
                }
                if t >= last.t {
                    return last.value;
                }
                // Find the segment [a, b] with a.t <= t < b.t.
                let i = keys.partition_point(|k| k.t <= t);
                let a = keys[i - 1];
                let b = keys[i];
                let span = b.t - a.t;
                if span <= f32::EPSILON {
                    return b.value;
                }
                let f = (t - a.t) / span;
                a.value + (b.value - a.value) * f
            }
        }
    }

    /// Insert (or overwrite) a keyframe at time `t`, keeping `keys` sorted.
    ///
    /// If an existing key sits within `EPS` of `t`, its value is replaced rather
    /// than adding a near-duplicate.
    pub fn set_key(&mut self, t: f32, value: f32) {
        const EPS: f32 = 1e-3;
        if let Some(k) = self.keys.iter_mut().find(|k| (k.t - t).abs() < EPS) {
            k.value = value;
            return;
        }
        let idx = self.keys.partition_point(|k| k.t < t);
        self.keys.insert(idx, Keyframe { t, value });
    }
}

/// Which of a layer's five tracks; used to drive generic property UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
}

impl Prop {
    /// All properties, in display order.
    pub const ALL: [Prop; 5] = [Prop::X, Prop::Y, Prop::Scale, Prop::Rotation, Prop::Opacity];

    pub fn label(self) -> &'static str {
        match self {
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
            Prop::X | Prop::Y | Prop::Rotation => 0.0,
            Prop::Scale | Prop::Opacity => 1.0,
        }
    }

    /// Editing range and unit suffix for the value slider.
    pub fn range(self) -> (std::ops::RangeInclusive<f32>, &'static str) {
        match self {
            Prop::X => (-2000.0..=2000.0, " px"),
            Prop::Y => (-2000.0..=2000.0, " px"),
            Prop::Scale => (0.0..=5.0, "x"),
            Prop::Rotation => (-360.0..=360.0, "°"),
            Prop::Opacity => (0.0..=1.0, ""),
        }
    }
}

/// One animated layer: a solid color rect transformed by its five tracks.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PulseLayer {
    pub name: String,
    /// Solid swatch color (straight sRGB RGBA, 0..=1) for the v0 preview.
    pub color: [f32; 4],
    pub visible: bool,
    // Animated properties. An empty track means "use the default constant".
    pub x: Track,
    pub y: Track,
    pub scale: Track,
    pub rotation: Track,
    pub opacity: Track,
}

impl PulseLayer {
    /// A new layer with the given name and color and all-empty tracks.
    pub fn new(name: impl Into<String>, color: [f32; 4]) -> Self {
        Self {
            name: name.into(),
            color,
            visible: true,
            x: Track::default(),
            y: Track::default(),
            scale: Track::default(),
            rotation: Track::default(),
            opacity: Track::default(),
        }
    }

    /// Borrow the track for `prop`.
    pub fn track(&self, prop: Prop) -> &Track {
        match prop {
            Prop::X => &self.x,
            Prop::Y => &self.y,
            Prop::Scale => &self.scale,
            Prop::Rotation => &self.rotation,
            Prop::Opacity => &self.opacity,
        }
    }

    /// Mutably borrow the track for `prop`.
    pub fn track_mut(&mut self, prop: Prop) -> &mut Track {
        match prop {
            Prop::X => &mut self.x,
            Prop::Y => &mut self.y,
            Prop::Scale => &mut self.scale,
            Prop::Rotation => &mut self.rotation,
            Prop::Opacity => &mut self.opacity,
        }
    }

    /// Sample one property at time `t`.
    pub fn value(&self, prop: Prop, t: f32) -> f32 {
        self.track(prop).sample(t, prop.default_value())
    }

    /// Sample all five properties at time `t` into a [`Transform`].
    pub fn transform(&self, t: f32) -> Transform {
        Transform {
            x: self.value(Prop::X, t),
            y: self.value(Prop::Y, t),
            scale: self.value(Prop::Scale, t),
            rotation_deg: self.value(Prop::Rotation, t),
            opacity: self.value(Prop::Opacity, t).clamp(0.0, 1.0),
        }
    }
}

/// A sampled layer transform at one instant.
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
}

/// The whole motion document: a sized, timed canvas and its layer stack.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comp {
    pub width: u32,
    pub height: u32,
    pub duration: f32,
    pub fps: f32,
    pub layers: Vec<PulseLayer>,
}

impl Comp {
    /// A fresh 1280x720, 5-second, 30fps composition with one demo layer.
    pub fn new() -> Self {
        let mut c = Self {
            width: 1280,
            height: 720,
            duration: 5.0,
            fps: 30.0,
            layers: Vec::new(),
        };
        // Seed one animated layer so the preview/timeline aren't empty on launch.
        let mut demo = PulseLayer::new("Solid 1", [0.27, 0.55, 0.85, 1.0]);
        demo.x.set_key(0.0, -300.0);
        demo.x.set_key(5.0, 300.0);
        demo.rotation.set_key(0.0, 0.0);
        demo.rotation.set_key(5.0, 180.0);
        c.layers.push(demo);
        c
    }
}

impl Default for Comp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
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
}
