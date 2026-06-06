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

/// How a [`Mask`] combines with the masks above it on the same layer (After
/// Effects' mask-mode dropdown).
///
/// Each mask produces a per-pixel coverage in `[0, 1]` (1 = fully inside the
/// shape, 0 = fully outside, fractional on a feathered edge). The masks on a
/// layer are folded **top-down** into a single coverage that multiplies the
/// layer's own alpha: an [`MaskMode::Add`] unions its shape in, a
/// [`MaskMode::Subtract`] knocks it out, an [`MaskMode::Intersect`] keeps only
/// the overlap, and a [`MaskMode::Difference`] keeps the symmetric difference.
/// [`MaskMode::None`] disables the mask without deleting it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskMode {
    /// Disabled — the mask contributes nothing (kept for re-enabling/editing).
    None,
    /// Union: `out = acc + cov·(1 - acc)` (the default for a new mask).
    #[default]
    Add,
    /// Knockout: `out = acc·(1 - cov)`.
    Subtract,
    /// Keep the overlap: `out = acc·cov`.
    Intersect,
    /// Symmetric difference: `out = acc + cov - 2·acc·cov`.
    Difference,
}

impl MaskMode {
    /// All modes, in menu order.
    pub const ALL: [MaskMode; 5] = [
        MaskMode::Add,
        MaskMode::Subtract,
        MaskMode::Intersect,
        MaskMode::Difference,
        MaskMode::None,
    ];

    /// Short label for the mask-mode picker.
    pub fn label(self) -> &'static str {
        match self {
            MaskMode::None => "None",
            MaskMode::Add => "Add",
            MaskMode::Subtract => "Subtract",
            MaskMode::Intersect => "Intersect",
            MaskMode::Difference => "Difference",
        }
    }

    /// Fold this mask's coverage `cov` (already feathered/inverted, in `[0,1]`)
    /// into the running accumulated coverage `acc`, returning the new
    /// accumulator. The very first **enabled** mask on a layer is composited
    /// against a fully-transparent base, so an `Add` reveals exactly its shape
    /// and a `Subtract`/`Intersect` against nothing yields nothing — matching
    /// After Effects, where the topmost mask's mode acts on an empty layer mask.
    pub fn combine(self, acc: f32, cov: f32) -> f32 {
        let cov = cov.clamp(0.0, 1.0);
        let acc = acc.clamp(0.0, 1.0);
        let out = match self {
            MaskMode::None => acc,
            MaskMode::Add => acc + cov * (1.0 - acc),
            MaskMode::Subtract => acc * (1.0 - cov),
            MaskMode::Intersect => acc * cov,
            MaskMode::Difference => acc + cov - 2.0 * acc * cov,
        };
        out.clamp(0.0, 1.0)
    }
}

/// One vertex of a [`Mask`] path: a layer-local anchor point plus its two
/// Bézier tangent handles, stored as **offsets** from the anchor (After
/// Effects' in/out tangents).
///
/// Coordinates are in the layer's local frame — the same `±half_w/±half_h`
/// comp-pixel space the layer's quad lives in (origin at the layer center),
/// before the layer's world transform — so a mask rides the layer's
/// position/scale/rotation/parenting for free. A zero in/out handle makes the
/// adjoining segment a straight line (a corner point); non-zero handles make it
/// a cubic Bézier.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskVertex {
    /// Anchor position (layer-local comp px).
    pub x: f32,
    pub y: f32,
    /// Tangent handle leaving the *previous* segment / arriving at this anchor
    /// (offset from the anchor).
    pub in_x: f32,
    pub in_y: f32,
    /// Tangent handle leaving this anchor toward the *next* vertex (offset).
    pub out_x: f32,
    pub out_y: f32,
}

impl MaskVertex {
    /// A corner vertex at `(x, y)` with no tangent handles (straight segments).
    pub fn corner(x: f32, y: f32) -> Self {
        MaskVertex {
            x,
            y,
            in_x: 0.0,
            in_y: 0.0,
            out_x: 0.0,
            out_y: 0.0,
        }
    }

    /// The anchor as a tuple.
    pub fn pos(&self) -> (f32, f32) {
        (self.x, self.y)
    }
    /// The absolute (layer-local) position of the outgoing tangent control.
    pub fn out_handle(&self) -> (f32, f32) {
        (self.x + self.out_x, self.y + self.out_y)
    }
    /// The absolute (layer-local) position of the incoming tangent control.
    pub fn in_handle(&self) -> (f32, f32) {
        (self.x + self.in_x, self.y + self.in_y)
    }
}

/// A **mask** on a layer: a closed Bézier path defining a region of the layer
/// to keep or remove, in layer-local space (After Effects' layer masks).
///
/// The path is flattened to a polygon (sampling each cubic Bézier segment) and
/// rasterized by an even-odd point-in-polygon test, yielding a per-pixel
/// coverage that is then **expanded/contracted** (offset), **feathered**
/// (softened) and optionally **inverted**, scaled by `opacity`, and finally
/// folded into the layer's coverage by the mask's [`MaskMode`]. Mask shapes are
/// not yet keyframable (that arrives with the typed-`Property<Path>` rebuild),
/// so a mask is a fixed shape per layer for now; the geometry below is the pure,
/// time-agnostic core a future animated mask will sample into.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// Display name (for the masks list).
    pub name: String,
    /// Boolean combination with the masks above it (and the layer).
    pub mode: MaskMode,
    /// Invert the coverage (`1 - cov`) before combining — show the layer
    /// *outside* the shape.
    pub inverted: bool,
    /// Mask opacity in `[0, 1]`: scales the coverage the shape contributes.
    pub opacity: f32,
    /// Edge softness in comp px (per side). `0` is a hard edge; larger values
    /// ramp the coverage linearly across `feather` px straddling the boundary.
    pub feather: f32,
    /// Signed offset in comp px: positive **expands** the shape outward,
    /// negative **contracts** it (After Effects' mask expansion).
    pub expansion: f32,
    /// The closed path's vertices (layer-local comp px), in order.
    pub vertices: Vec<MaskVertex>,
}

impl Default for Mask {
    fn default() -> Self {
        Mask {
            name: "Mask".to_owned(),
            mode: MaskMode::Add,
            inverted: false,
            opacity: 1.0,
            feather: 0.0,
            expansion: 0.0,
            vertices: Vec::new(),
        }
    }
}

/// How finely each cubic Bézier mask segment is flattened into line segments.
/// A fixed subdivision is plenty for the small mask paths Pulse edits and keeps
/// the point-in-polygon test cheap and deterministic.
const MASK_BEZIER_STEPS: u32 = 16;

impl Mask {
    /// A rectangular mask covering `[-hw, hw] × [-hh, hh]` (layer-local px),
    /// four corner vertices — the default "new mask" shape, sized to the layer.
    pub fn rect(hw: f32, hh: f32) -> Self {
        Mask {
            vertices: vec![
                MaskVertex::corner(-hw, -hh),
                MaskVertex::corner(hw, -hh),
                MaskVertex::corner(hw, hh),
                MaskVertex::corner(-hw, hh),
            ],
            ..Mask::default()
        }
    }

    /// An elliptical mask inscribed in `[-hw, hw] × [-hh, hh]`, built from four
    /// Bézier vertices with the standard `k ≈ 0.5523` circle-approximation
    /// handles (a smooth oval — AE's elliptical mask tool).
    pub fn ellipse(hw: f32, hh: f32) -> Self {
        // Kappa: handle length as a fraction of the radius for a 90° arc.
        const K: f32 = 0.552_284_8;
        let (kx, ky) = (hw * K, hh * K);
        // Right, bottom, left, top anchors with tangents along the perimeter.
        let verts = vec![
            MaskVertex {
                x: hw,
                y: 0.0,
                in_x: 0.0,
                in_y: -ky,
                out_x: 0.0,
                out_y: ky,
            },
            MaskVertex {
                x: 0.0,
                y: hh,
                in_x: kx,
                in_y: 0.0,
                out_x: -kx,
                out_y: 0.0,
            },
            MaskVertex {
                x: -hw,
                y: 0.0,
                in_x: 0.0,
                in_y: ky,
                out_x: 0.0,
                out_y: -ky,
            },
            MaskVertex {
                x: 0.0,
                y: -hh,
                in_x: -kx,
                in_y: 0.0,
                out_x: kx,
                out_y: 0.0,
            },
        ];
        Mask {
            vertices: verts,
            ..Mask::default()
        }
    }

    /// Whether the mask actually contributes (mode isn't [`MaskMode::None`] and
    /// it has enough vertices to enclose an area).
    pub fn is_active(&self) -> bool {
        self.mode != MaskMode::None && self.vertices.len() >= 3
    }

    /// Flatten the closed Bézier path into a polygon of `(x, y)` points in
    /// layer-local space, subdividing each cubic segment into
    /// [`MASK_BEZIER_STEPS`] chords. The polygon is implicitly closed (the last
    /// point connects back to the first). Straight segments (zero handles)
    /// collapse to a single chord cheaply since their interior points are
    /// colinear.
    pub fn flatten(&self) -> Vec<(f32, f32)> {
        let n = self.vertices.len();
        if n < 2 {
            return self.vertices.iter().map(|v| v.pos()).collect();
        }
        let mut out = Vec::with_capacity(n * MASK_BEZIER_STEPS as usize);
        for i in 0..n {
            let a = &self.vertices[i];
            let b = &self.vertices[(i + 1) % n];
            let (p0x, p0y) = a.pos();
            let (p1x, p1y) = a.out_handle();
            let (p2x, p2y) = b.in_handle();
            let (p3x, p3y) = b.pos();
            // A straight segment (no handles either side) needs only its start.
            let straight = a.out_x == 0.0 && a.out_y == 0.0 && b.in_x == 0.0 && b.in_y == 0.0;
            if straight {
                out.push((p0x, p0y));
                continue;
            }
            let steps = MASK_BEZIER_STEPS;
            for s in 0..steps {
                let u = s as f32 / steps as f32;
                let mt = 1.0 - u;
                let w0 = mt * mt * mt;
                let w1 = 3.0 * mt * mt * u;
                let w2 = 3.0 * mt * u * u;
                let w3 = u * u * u;
                out.push((
                    w0 * p0x + w1 * p1x + w2 * p2x + w3 * p3x,
                    w0 * p0y + w1 * p1y + w2 * p2y + w3 * p3y,
                ));
            }
        }
        out
    }

    /// The signed distance-ish **coverage** of layer-local point `(px, py)`
    /// against this mask, in `[0, 1]`, *before* opacity scaling and mode
    /// folding.
    ///
    /// Computed from the flattened polygon: the point's signed distance to the
    /// nearest edge (negative = outside, positive = inside, via an even-odd
    /// inside test) is shifted by `expansion` and ramped across the `feather`
    /// width to a soft `[0,1]` coverage, then inverted if requested and scaled
    /// by `opacity`. A hard-edged mask (`feather == 0`) returns a crisp 0/1
    /// (then ×opacity).
    pub fn coverage_at(&self, poly: &[(f32, f32)], px: f32, py: f32) -> f32 {
        if poly.len() < 3 {
            return 0.0;
        }
        let inside = point_in_polygon(poly, px, py);
        let dist = dist_to_polygon(poly, px, py); // ≥ 0, distance to boundary
                                                  // Signed distance: positive inside, negative outside.
        let signed = if inside { dist } else { -dist };
        // Expansion shifts the boundary outward (+) / inward (−).
        let signed = signed + self.expansion;
        // Feather ramps coverage from 0 to 1 across ±feather/2 around the edge.
        let cov = if self.feather <= 0.0 {
            if signed >= 0.0 {
                1.0
            } else {
                0.0
            }
        } else {
            let half = self.feather * 0.5;
            ((signed + half) / self.feather).clamp(0.0, 1.0)
        };
        let cov = if self.inverted { 1.0 - cov } else { cov };
        (cov * self.opacity).clamp(0.0, 1.0)
    }
}

/// Even-odd point-in-polygon test (ray casting) for a closed polygon given as
/// an ordered list of `(x, y)` vertices (the closing edge is implicit).
pub fn point_in_polygon(poly: &[(f32, f32)], px: f32, py: f32) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        // Does a horizontal ray from (px, py) cross edge j→i?
        let crosses = (yi > py) != (yj > py)
            && px < (xj - xi) * (py - yi) / (yj - yi + f32::EPSILON.copysign(yj - yi)) + xi;
        if crosses {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The shortest Euclidean distance from `(px, py)` to the boundary of a closed
/// polygon (the minimum distance to any of its edges). Always `≥ 0`.
pub fn dist_to_polygon(poly: &[(f32, f32)], px: f32, py: f32) -> f32 {
    let n = poly.len();
    if n == 0 {
        return f32::INFINITY;
    }
    let mut best = f32::INFINITY;
    let mut j = n - 1;
    for i in 0..n {
        best = best.min(dist_to_segment((px, py), poly[j], poly[i]));
        j = i;
    }
    best
}

/// Euclidean distance from point `p` to the segment `a→b`.
fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Fold a layer's whole mask stack into a single coverage multiplier in
/// `[0, 1]` for the layer-local point `(px, py)`.
///
/// The masks are combined **top-down** (list order) via each mask's
/// [`MaskMode::combine`], each contributing its [`Mask::coverage_at`]. When the
/// layer has **no active masks** the layer is unmasked, so this returns `1.0`
/// (full coverage) — callers should special-case "no masks" rather than
/// multiplying by this. `polys` must be the pre-flattened polygon for each mask
/// in `masks` (same order), so the hot per-pixel loop doesn't re-flatten.
pub fn mask_stack_coverage(masks: &[Mask], polys: &[Vec<(f32, f32)>], px: f32, py: f32) -> f32 {
    let mut acc = 0.0;
    let mut any = false;
    for (mask, poly) in masks.iter().zip(polys.iter()) {
        if !mask.is_active() {
            continue;
        }
        any = true;
        let cov = mask.coverage_at(poly, px, py);
        acc = mask.mode.combine(acc, cov);
    }
    if any {
        acc
    } else {
        1.0
    }
}

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerKind {
    #[default]
    Solid,
    Null,
    Adjustment,
}

impl LayerKind {
    /// All kinds, in menu order.
    pub const ALL: [LayerKind; 3] = [LayerKind::Solid, LayerKind::Null, LayerKind::Adjustment];

    pub fn label(self) -> &'static str {
        match self {
            LayerKind::Solid => "Solid",
            LayerKind::Null => "Null",
            LayerKind::Adjustment => "Adjustment",
        }
    }

    /// Whether a layer of this kind draws its own pixels. A null draws nothing;
    /// an adjustment draws nothing of its own (it only re-processes the layers
    /// beneath it).
    pub fn draws_own_pixels(self) -> bool {
        matches!(self, LayerKind::Solid)
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

/// One animated layer: a solid color rect transformed by its tracks, optionally
/// **parented** to another layer (whose transform it inherits).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PulseLayer {
    pub name: String,
    /// What this layer *is* (solid / null / adjustment). `serde`-defaulted to
    /// `Solid` so pre-layer-kind `.pulse` files still load as solids.
    #[serde(default)]
    pub kind: LayerKind,
    /// **Per-layer motion-blur** switch (After Effects' layer MB toggle). A
    /// layer is motion-blurred only when both this and the comp's
    /// [`MotionBlur::enabled`] master switch are on. `serde`-defaulted to `false`
    /// so pre-motion-blur `.pulse` files still load.
    #[serde(default)]
    pub motion_blur: bool,
    /// Solid swatch color (straight sRGB RGBA, 0..=1) for the v0 preview.
    pub color: [f32; 4],
    pub visible: bool,
    /// Non-destructive, ordered **effect stack**. For a solid layer the stack
    /// processes the layer's own pixels; for an adjustment layer it processes
    /// the composite of everything below. `serde`-defaulted to empty for old
    /// projects.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Parent layer index, if this layer is parented. A child inherits its
    /// parent's full transform (position, scale, rotation, anchor) but **not**
    /// its opacity (matching After Effects). `serde`-defaulted so pre-parenting
    /// `.pulse` files still load as unparented.
    #[serde(default)]
    pub parent: Option<usize>,
    /// **Track matte** mode. When active, the layer directly *above* this one in
    /// the stack defines this layer's per-pixel transparency and is itself
    /// removed from normal compositing (matching After Effects). `serde`-defaulted
    /// to [`MatteMode::None`] so pre-matte `.pulse` files still load.
    #[serde(default)]
    pub matte: MatteMode,
    /// **Masks**: closed Bézier paths (layer-local) that carve the layer's
    /// coverage. Folded top-down into a single coverage multiplier on the
    /// layer's alpha (see [`mask_stack_coverage`]). `serde`-defaulted to empty
    /// so pre-mask `.pulse` files still load unmasked.
    #[serde(default)]
    pub masks: Vec<Mask>,
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
            kind: LayerKind::Solid,
            motion_blur: false,
            color,
            visible: true,
            effects: Vec::new(),
            parent: None,
            matte: MatteMode::None,
            masks: Vec::new(),
            anchor_x: Track::default(),
            anchor_y: Track::default(),
            x: Track::default(),
            y: Track::default(),
            scale: Track::default(),
            rotation: Track::default(),
            opacity: Track::default(),
        }
    }

    /// A new layer of the given kind, name, and color (empty tracks/effects).
    pub fn of_kind(kind: LayerKind, name: impl Into<String>, color: [f32; 4]) -> Self {
        Self {
            kind,
            ..Self::new(name, color)
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

    /// Whether this layer has at least one **active** mask (so the renderer must
    /// run the per-pixel mask-coverage pass for it).
    pub fn has_active_masks(&self) -> bool {
        self.masks.iter().any(Mask::is_active)
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
    /// Composition **motion-blur** settings (master switch + shutter
    /// angle/phase + sample count). `serde`-defaulted so pre-motion-blur
    /// `.pulse` files still load with motion blur off.
    #[serde(default)]
    pub motion_blur: MotionBlur,
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
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        // Enable comp motion blur so the demo's fast slide/spin reads with a
        // cinematic shutter out of the box (the sliding solid opts in below).
        c.motion_blur.enabled = true;
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
        demo.motion_blur = true; // opt this layer into the comp's shutter
                                 // A soft elliptical mask carves the solid into a feathered oval (sized to
                                 // the layer's base quad), so masks read out of the box.
        let mask_hw = 1280.0 * 0.22; // matches the renderer's LAYER_HALF_FRAC
        let mask_hh = 720.0 * 0.22;
        let mut oval = Mask::ellipse(mask_hw, mask_hh);
        oval.feather = 60.0;
        demo.masks.push(oval);
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

        // A full-frame adjustment layer on top: its effect stack regrades every
        // layer beneath it (here a punchy Levels contrast) without drawing any
        // pixels of its own — showcasing layer kinds + the effect stack on launch.
        let mut grade = PulseLayer::of_kind(LayerKind::Adjustment, "Grade", [1.0; 4]);
        grade.scale.set_key(0.0, 3.0); // cover the whole frame
        grade.effects.push(Effect::Levels {
            in_black: 0.05,
            in_white: 0.85,
            gamma: 1.1,
            out_black: 0.0,
            out_white: 1.0,
        });
        c.layers.push(grade); // index 2
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

    /// Whether layer `idx` is rendered with **motion blur**: the comp's master
    /// [`MotionBlur::enabled`] switch is on *and* the layer has its own
    /// per-layer `motion_blur` flag set. A missing index is `false`.
    pub fn layer_motion_blurred(&self, idx: usize) -> bool {
        self.motion_blur.enabled && self.layers.get(idx).is_some_and(|layer| layer.motion_blur)
    }

    /// The index of layer `idx`'s **matte source** — the layer directly above it
    /// in the stack (next-higher index) — when `idx` has an active [`MatteMode`]
    /// and such a layer exists. `None` if the layer has no matte or sits at the
    /// top of the stack (no layer above to borrow).
    pub fn matte_source(&self, idx: usize) -> Option<usize> {
        let layer = self.layers.get(idx)?;
        if !layer.matte.is_active() {
            return None;
        }
        let src = idx + 1;
        (src < self.layers.len()).then_some(src)
    }

    /// Whether layer `idx` is **consumed as a matte source** by the layer
    /// directly below it (so it must not composite on its own). True iff the
    /// layer below (`idx - 1`) has an active matte mode.
    pub fn is_matte_source(&self, idx: usize) -> bool {
        idx.checked_sub(1)
            .and_then(|below| self.layers.get(below))
            .is_some_and(|below| below.matte.is_active())
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
            motion_blur: MotionBlur::default(),
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
}
