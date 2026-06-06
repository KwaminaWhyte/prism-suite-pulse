//! Pulse's motion document model.
//!
//! A [`Comp`] is a composition: a fixed-size canvas with a duration and frame
//! rate, holding an ordered stack of [`PulseLayer`]s. Each layer carries seven
//! animatable properties — anchor x/y, position x/y, scale, rotation, opacity —
//! stored as [`Track`]s of [`Keyframe`]s, and may be **parented** to another
//! layer (inheriting its transform). Scale and rotation pivot about the layer's
//! **anchor point**; a layer's resolved [`Affine2`] world matrix folds its own
//! transform under its parent chain.
//!
//! Sampling: between two bracketing keyframes the value is interpolated
//! according to the *outgoing* keyframe's [`Interp`] mode — linear, hold
//! (stepped), or a temporal cubic-Bézier **ease** (After-Effects style, with
//! editable in/out handles). Before the first key it holds the first value,
//! after the last it holds the last value (constant extrapolation). An empty
//! track returns the property's sensible default.
//!
//! Layer paint order is bottom-up: index 0 is drawn first (back), the last
//! index on top. Colors are straight sRGB RGBA in `[f32; 4]` so they round-trip
//! cleanly through egui's color picker and JSON.

use serde::{Deserialize, Serialize};

/// Temporal interpolation between a keyframe and the next one.
///
/// The mode lives on the *outgoing* keyframe (the earlier of a pair), matching
/// how After Effects attaches a segment's behaviour to the left key. An
/// [`Interp::Ease`] carries a normalized cubic-Bézier easing curve whose two
/// control points are `(out_x, out_y)` leaving this key and `(in_x, in_y)`
/// arriving at the next — exactly the CSS `cubic-bezier(x1,y1,x2,y2)` shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Interp {
    /// Straight line: constant velocity across the segment.
    #[default]
    Linear,
    /// Stepped: hold the outgoing value until the next key (no interpolation).
    Hold,
    /// Cubic-Bézier temporal ease with editable handles.
    Ease(Ease),
}

impl Interp {
    /// Short label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Interp::Linear => "Linear",
            Interp::Hold => "Hold",
            Interp::Ease(_) => "Ease",
        }
    }
}

/// A normalized cubic-Bézier easing curve mapping a segment's elapsed-time
/// fraction `x ∈ [0,1]` to an eased value fraction `y ∈ [0,1]`.
///
/// Control points are `P0 = (0,0)`, `P1 = (out_x, out_y)`, `P2 = (in_x, in_y)`,
/// `P3 = (1,1)`. `out_*` is the handle leaving the earlier key; `in_*` is the
/// handle arriving at the later key. `out_x` / `in_x` are clamped to `[0,1]`
/// (CSS rules) so the curve is always a function of `x`; the `y` components may
/// over/undershoot for anticipation/overshoot, matching AE.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ease {
    pub out_x: f32,
    pub out_y: f32,
    pub in_x: f32,
    pub in_y: f32,
}

impl Ease {
    /// After Effects' "Easy Ease" default (F9): symmetric ease in and out with
    /// ~33% influence — equivalent to CSS `cubic-bezier(0.33, 0, 0.67, 1)`.
    pub const EASY: Ease = Ease {
        out_x: 0.33,
        out_y: 0.0,
        in_x: 0.67,
        in_y: 1.0,
    };

    /// "Ease Out" only: accelerates away from the key, arrives linearly.
    pub const OUT: Ease = Ease {
        out_x: 0.33,
        out_y: 0.0,
        in_x: 1.0,
        in_y: 1.0,
    };

    /// "Ease In" only: leaves linearly, decelerates into the next key.
    pub const IN: Ease = Ease {
        out_x: 0.0,
        out_y: 0.0,
        in_x: 0.67,
        in_y: 1.0,
    };

    /// A "custom" linear-looking ease (the straight diagonal). Used as the seed
    /// when the graph editor converts a Linear/Hold segment into an editable
    /// eased one: the curve starts as `y = x` so converting is value-neutral,
    /// then the handles can be dragged away from the diagonal.
    pub const LINEAR: Ease = Ease {
        out_x: 1.0 / 3.0,
        out_y: 1.0 / 3.0,
        in_x: 2.0 / 3.0,
        in_y: 2.0 / 3.0,
    };

    /// Evaluate the eased `y` for an elapsed-time fraction `x ∈ [0,1]`.
    ///
    /// Solves `bezier_x(s) = x` for the curve parameter `s` (Newton's method
    /// with a bisection fallback), then returns `bezier_y(s)`.
    pub fn eval(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        // Endpoints are exact; skip the solve.
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let x1 = self.out_x.clamp(0.0, 1.0);
        let x2 = self.in_x.clamp(0.0, 1.0);
        let s = solve_bezier_x(x, x1, x2);
        cubic_bezier(s, self.out_y, self.in_y)
    }

    /// Replace the *outgoing* handle (the control leaving the earlier key),
    /// keeping `x` inside `[0,1]` (CSS rule — the curve must stay a function of
    /// `x`). `y` is free to over/undershoot for anticipation. Used by the graph
    /// editor when a handle is dragged.
    #[must_use]
    pub fn with_out(mut self, x: f32, y: f32) -> Self {
        self.out_x = x.clamp(0.0, 1.0);
        self.out_y = y;
        self
    }

    /// Replace the *incoming* handle (the control arriving at the later key),
    /// keeping `x` inside `[0,1]`. See [`Ease::with_out`].
    #[must_use]
    pub fn with_in(mut self, x: f32, y: f32) -> Self {
        self.in_x = x.clamp(0.0, 1.0);
        self.in_y = y;
        self
    }
}

/// Which Bézier handle of an eased segment a drag targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    /// The control leaving the earlier (outgoing) key.
    Out,
    /// The control arriving at the later (incoming) key.
    In,
}

/// A cubic Bézier with endpoints fixed at 0 and 1 and interior controls
/// `p1, p2`, evaluated at parameter `s ∈ [0,1]`.
fn cubic_bezier(s: f32, p1: f32, p2: f32) -> f32 {
    let mt = 1.0 - s;
    // 3·(1-s)²·s·p1 + 3·(1-s)·s²·p2 + s³  (the P0=0, P3=1 cubic).
    3.0 * mt * mt * s * p1 + 3.0 * mt * s * s * p2 + s * s * s
}

/// Derivative w.r.t. `s` of [`cubic_bezier`].
fn cubic_bezier_deriv(s: f32, p1: f32, p2: f32) -> f32 {
    let mt = 1.0 - s;
    3.0 * mt * mt * p1 + 6.0 * mt * s * (p2 - p1) + 3.0 * s * s * (1.0 - p2)
}

/// Invert the x-component of a normalized cubic Bézier: find `s` such that
/// `cubic_bezier(s, x1, x2) == x`. Newton-Raphson seeded at `s = x`, with a
/// bisection fallback when the derivative is too flat to make progress.
fn solve_bezier_x(x: f32, x1: f32, x2: f32) -> f32 {
    let mut s = x;
    for _ in 0..8 {
        let err = cubic_bezier(s, x1, x2) - x;
        if err.abs() < 1e-6 {
            return s;
        }
        let d = cubic_bezier_deriv(s, x1, x2);
        if d.abs() < 1e-6 {
            break;
        }
        s -= err / d;
    }
    // Bisection fallback (guaranteed to converge: x(s) is monotonic in s
    // because x1,x2 ∈ [0,1]).
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    s = x;
    for _ in 0..32 {
        let xs = cubic_bezier(s, x1, x2);
        if (xs - x).abs() < 1e-6 {
            break;
        }
        if xs < x {
            lo = s;
        } else {
            hi = s;
        }
        s = 0.5 * (lo + hi);
    }
    s
}

/// A single animation keyframe: a property `value` at time `t` (seconds), plus
/// the [`Interp`] mode driving the segment to the *next* keyframe.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Keyframe {
    pub t: f32,
    pub value: f32,
    /// Interpolation for the segment leaving this key. Defaults to `Linear`
    /// (and is `serde`-defaulted so pre-easing `.pulse` files still load).
    #[serde(default)]
    pub interp: Interp,
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
    /// Between bracketing keys the value follows the *outgoing* key's
    /// [`Interp`] mode (linear / hold / Bézier ease); outside the
    /// `[first, last]` range it holds the nearest key (constant extrapolation).
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
                let f = (t - a.t) / span; // elapsed fraction across the segment
                let eased = match a.interp {
                    Interp::Hold => return a.value,
                    Interp::Linear => f,
                    Interp::Ease(e) => e.eval(f),
                };
                a.value + (b.value - a.value) * eased
            }
        }
    }

    /// Borrow the keyframe nearest in time to `t` (within `EPS`), if any.
    pub fn key_at(&self, t: f32) -> Option<&Keyframe> {
        const EPS: f32 = 1e-3;
        self.keys.iter().find(|k| (k.t - t).abs() < EPS)
    }

    /// The interpolation mode of the key at (or just before) time `t`, used to
    /// drive the per-keyframe interpolation UI.
    pub fn interp_at(&self, t: f32) -> Option<Interp> {
        self.key_at(t).map(|k| k.interp)
    }

    /// Insert (or overwrite) a keyframe at time `t`, keeping `keys` sorted.
    ///
    /// If an existing key sits within `EPS` of `t`, its value is replaced
    /// (its interpolation mode is preserved); otherwise a new key is added,
    /// inheriting the interpolation of the key it follows so re-keying inside an
    /// eased segment doesn't silently snap back to linear.
    pub fn set_key(&mut self, t: f32, value: f32) {
        const EPS: f32 = 1e-3;
        if let Some(k) = self.keys.iter_mut().find(|k| (k.t - t).abs() < EPS) {
            k.value = value;
            return;
        }
        let idx = self.keys.partition_point(|k| k.t < t);
        let interp = idx
            .checked_sub(1)
            .map(|prev| self.keys[prev].interp)
            .unwrap_or_default();
        self.keys.insert(idx, Keyframe { t, value, interp });
    }

    /// Set the outgoing interpolation mode for the key nearest `t`, if any.
    /// Returns `true` when a key was found and updated.
    pub fn set_interp(&mut self, t: f32, interp: Interp) -> bool {
        const EPS: f32 = 1e-3;
        if let Some(k) = self.keys.iter_mut().find(|k| (k.t - t).abs() < EPS) {
            k.interp = interp;
            true
        } else {
            false
        }
    }

    /// The min/max sampled value across the track's keyframes, used by the graph
    /// editor to frame the value axis. Returns `None` for an empty track. Because
    /// eased segments can over/undershoot, this samples the curve densely rather
    /// than just taking the keyframe extremes.
    pub fn value_bounds(&self) -> Option<(f32, f32)> {
        let first = self.keys.first()?;
        let last = self.keys.last()?;
        let mut lo = first.value;
        let mut hi = first.value;
        let mut consider = |v: f32| {
            lo = lo.min(v);
            hi = hi.max(v);
        };
        for k in &self.keys {
            consider(k.value);
        }
        // Sample interior of each segment to catch ease overshoot.
        let span = last.t - first.t;
        if span > f32::EPSILON {
            let steps = 64;
            for i in 1..steps {
                let t = first.t + span * (i as f32 / steps as f32);
                consider(self.sample(t, first.value));
            }
        }
        Some((lo, hi))
    }

    /// Move the keyframe at index `i` to a new time and value, re-sorting if the
    /// move crosses a neighbour. Returns the index the key ended up at (so a
    /// drag can keep tracking it). Out-of-range `i` is a no-op returning `i`.
    pub fn move_key(&mut self, i: usize, new_t: f32, new_value: f32) -> usize {
        if i >= self.keys.len() {
            return i;
        }
        self.keys[i].t = new_t;
        self.keys[i].value = new_value;
        // Bubble the key into sorted position, carrying its identity.
        let mut j = i;
        while j > 0 && self.keys[j - 1].t > self.keys[j].t {
            self.keys.swap(j - 1, j);
            j -= 1;
        }
        while j + 1 < self.keys.len() && self.keys[j + 1].t < self.keys[j].t {
            self.keys.swap(j + 1, j);
            j += 1;
        }
        j
    }

    /// Mutably borrow the keyframe at index `i`, if in range.
    pub fn key_mut(&mut self, i: usize) -> Option<&mut Keyframe> {
        self.keys.get_mut(i)
    }
}

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

/// One animated layer: a solid color rect transformed by its tracks, optionally
/// **parented** to another layer (whose transform it inherits).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PulseLayer {
    pub name: String,
    /// Solid swatch color (straight sRGB RGBA, 0..=1) for the v0 preview.
    pub color: [f32; 4],
    pub visible: bool,
    /// Parent layer index, if this layer is parented. A child inherits its
    /// parent's full transform (position, scale, rotation, anchor) but **not**
    /// its opacity (matching After Effects). `serde`-defaulted so pre-parenting
    /// `.pulse` files still load as unparented.
    #[serde(default)]
    pub parent: Option<usize>,
    // Animated properties. An empty track means "use the default constant".
    /// Anchor-point offset from the layer's geometric center (comp px). The
    /// pivot for scale/rotation and the local point aligned to `(x, y)`.
    #[serde(default)]
    pub anchor_x: Track,
    #[serde(default)]
    pub anchor_y: Track,
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
            parent: None,
            anchor_x: Track::default(),
            anchor_y: Track::default(),
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
            Prop::AnchorX => &self.anchor_x,
            Prop::AnchorY => &self.anchor_y,
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
            Prop::AnchorX => &mut self.anchor_x,
            Prop::AnchorY => &mut self.anchor_y,
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

    /// Sample the transform properties at time `t` into a [`Transform`].
    pub fn transform(&self, t: f32) -> Transform {
        Transform {
            anchor_x: self.value(Prop::AnchorX, t),
            anchor_y: self.value(Prop::AnchorY, t),
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
    /// Anchor-point offset from the layer center (comp px), the pivot.
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
}

impl Transform {
    /// The layer's **local** affine matrix (comp space, origin at comp center),
    /// mapping layer-local points into the layer's own comp-space frame —
    /// *before* any parent transform.
    ///
    /// Built as `Translate(position) · Rotate · Scale · Translate(-anchor)`:
    /// the anchor point maps to `position`, and scale/rotation pivot about the
    /// anchor — the standard After-Effects transform order.
    pub fn local_matrix(self) -> Affine2 {
        let s = self.scale.max(0.0);
        Affine2::translate(self.x, self.y)
            .then(Affine2::rotate_deg(self.rotation_deg))
            .then(Affine2::scale(s))
            .then(Affine2::translate(-self.anchor_x, -self.anchor_y))
    }
}

/// A 2-D affine transform `[[a, c, tx], [b, d, ty]]` mapping a point
/// `(x, y)` to `(a·x + c·y + tx, b·x + d·y + ty)`. Comp space; origin at the
/// comp center with `+y` downward (screen convention), matching the preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine2 {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine2 {
    /// The identity transform.
    pub const IDENTITY: Affine2 = Affine2 {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    /// A pure translation.
    pub fn translate(tx: f32, ty: f32) -> Self {
        Affine2 {
            tx,
            ty,
            ..Affine2::IDENTITY
        }
    }

    /// A uniform scale about the origin.
    pub fn scale(s: f32) -> Self {
        Affine2 {
            a: s,
            d: s,
            ..Affine2::IDENTITY
        }
    }

    /// A rotation (degrees) about the origin. `+y` is downward, so a positive
    /// angle rotates clockwise on screen, matching the preview.
    pub fn rotate_deg(deg: f32) -> Self {
        let (sin, cos) = deg.to_radians().sin_cos();
        Affine2 {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Compose: `self.then(rhs)` applies `rhs` first, then `self` — i.e. the
    /// matrix product `self * rhs`. Reads left-to-right as outermost-first.
    #[must_use]
    pub fn then(self, rhs: Affine2) -> Self {
        Affine2 {
            a: self.a * rhs.a + self.c * rhs.b,
            b: self.b * rhs.a + self.d * rhs.b,
            c: self.a * rhs.c + self.c * rhs.d,
            d: self.b * rhs.c + self.d * rhs.d,
            tx: self.a * rhs.tx + self.c * rhs.ty + self.tx,
            ty: self.b * rhs.tx + self.d * rhs.ty + self.ty,
        }
    }

    /// Apply the transform to a point.
    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// The inverse transform, or `None` if the matrix is singular (e.g. a
    /// zero-scale collapse). Used by the rasterizer to map a comp-space pixel
    /// back into the layer's local frame for coverage testing.
    pub fn inverse(self) -> Option<Affine2> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let a = self.d * inv;
        let b = -self.b * inv;
        let c = -self.c * inv;
        let d = self.a * inv;
        Some(Affine2 {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }
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
    /// A fresh 1280x720, 5-second, 30fps composition with a parented demo pair.
    pub fn new() -> Self {
        let mut c = Self {
            width: 1280,
            height: 720,
            duration: 5.0,
            fps: 30.0,
            layers: Vec::new(),
        };
        // Seed an animated layer so the preview/timeline aren't empty on launch.
        // The X slide uses Easy Ease so the easing is visible immediately (it
        // eases in and out of the travel rather than gliding linearly), while
        // rotation stays linear for contrast.
        let mut demo = PulseLayer::new("Solid 1", [0.27, 0.55, 0.85, 1.0]);
        demo.x.set_key(0.0, -300.0);
        demo.x.set_key(5.0, 300.0);
        demo.x.set_interp(0.0, Interp::Ease(Ease::EASY));
        demo.rotation.set_key(0.0, 0.0);
        demo.rotation.set_key(5.0, 180.0);
        c.layers.push(demo); // index 0

        // A smaller satellite parented to Solid 1: it rides the parent's slide
        // and spin while orbiting via its own position offset — showcasing
        // parenting and the anchor-based pivot out of the box.
        let mut satellite = PulseLayer::new("Satellite", [0.95, 0.72, 0.25, 1.0]);
        satellite.parent = Some(0);
        satellite.scale.set_key(0.0, 0.4);
        satellite.x.set_key(0.0, 360.0);
        satellite.y.set_key(0.0, -180.0);
        c.layers.push(satellite); // index 1
        c
    }
}

impl Comp {
    /// The **world** affine matrix of layer `idx` at time `t`: its own local
    /// transform composed under every ancestor's transform (parent applied
    /// outermost), mapping the layer's local-space points into final comp space.
    ///
    /// Walks the parent chain defensively: out-of-range or self-referential
    /// parents are ignored, and a `visited` set breaks any cycle (a corrupt
    /// project can't hang the renderer), so the worst case is a finite, bounded
    /// walk producing the longest acyclic prefix.
    pub fn world_matrix(&self, idx: usize, t: f32) -> Affine2 {
        let mut visited = vec![false; self.layers.len()];
        let mut cur = idx;
        let mut m = Affine2::IDENTITY;
        loop {
            let Some(layer) = self.layers.get(cur) else {
                break;
            };
            if visited[cur] {
                break; // cycle guard
            }
            visited[cur] = true;
            // Parent applies outermost: world = parent_world · ... · local.
            m = layer.transform(t).local_matrix().then(m);
            match layer.parent {
                Some(p) if p != cur && p < self.layers.len() => cur = p,
                _ => break,
            }
        }
        m
    }

    /// Whether making `child` a parent of `parent` is legal: a layer can't
    /// parent to itself, to a missing layer, or to one of its own descendants
    /// (which would create a cycle). Returns `true` when the link is safe.
    pub fn can_parent(&self, child: usize, parent: usize) -> bool {
        if child == parent || parent >= self.layers.len() || child >= self.layers.len() {
            return false;
        }
        // Walk up from `parent`; if we reach `child`, linking would cycle.
        let mut visited = vec![false; self.layers.len()];
        let mut cur = parent;
        loop {
            if cur == child {
                return false;
            }
            if visited[cur] {
                return true; // pre-existing cycle elsewhere; this link is fine
            }
            visited[cur] = true;
            match self.layers[cur].parent {
                Some(p) if p < self.layers.len() => cur = p,
                _ => return true,
            }
        }
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
            layers: Vec::new(),
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
}
