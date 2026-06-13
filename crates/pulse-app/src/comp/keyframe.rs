//! Temporal interpolation, easing curves, and per-property keyframe tracks.
//!
//! The [`Interp`] mode lives on the *outgoing* keyframe and drives how a
//! [`Track`] interpolates between two [`Keyframe`]s — linear, hold, or a
//! normalized cubic-Bézier [`Ease`].

use super::expr::{self, ExprCtx};
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
pub(super) fn cubic_bezier(s: f32, p1: f32, p2: f32) -> f32 {
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
pub(super) fn solve_bezier_x(x: f32, x1: f32, x2: f32) -> f32 {
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub t: f32,
    pub value: f32,
    /// Interpolation for the segment leaving this key. Defaults to `Linear`
    /// (and is `serde`-defaulted so pre-easing `.pulse` files still load).
    #[serde(default)]
    pub interp: Interp,
    /// **Roving across time** (After Effects' *Rove Across Time*): when set on
    /// an *interior* spatial-position key, the key is freed from its authored
    /// time and re-timed so the layer moves at constant velocity along the
    /// motion path between the surrounding anchored keys (see
    /// [`roving`](super::roving)). Only meaningful on the `x` / `y` position
    /// tracks; ignored on the first/last key (endpoints always anchor) and on
    /// non-spatial tracks. `serde`-defaulted to `false` and skipped when unset,
    /// so pre-roving `.pulse` files load and round-trip byte-identically.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub roving: bool,
}

/// One animated property: a time-ordered list of keyframes, plus an optional
/// **expression**.
///
/// Invariant: `keys` is kept sorted ascending by `t` (see [`Track::set_key`]).
///
/// When [`expression`](Self::expression) is `Some` and non-empty, the property's
/// value at time `t` is the result of evaluating that script (see [`expr`]) with
/// the keyframed sample exposed as `value` — so an expression can offset or
/// drive the animation. A parse/eval error transparently falls back to the
/// keyframed value (see [`Track::sample_expr`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub keys: Vec<Keyframe>,
    /// Optional per-property expression. `serde`-defaulted to `None` so pre-
    /// expression `.pulse` files still load (and skipped on serialize when empty
    /// so unexpressed tracks round-trip byte-identically to old files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
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

    /// Sample the track at time `t`, evaluating its **expression** if one is set.
    ///
    /// First samples the keyframes exactly like [`Track::sample`] (so the
    /// expression sees the keyframed value as `value`). If [`expression`] is
    /// `Some` and non-empty, that script is evaluated against `ctx` (with `value`
    /// overridden to the keyframed sample); its finite result replaces the value.
    /// A parse or runtime error — or a non-finite result — falls back to the
    /// keyframed value (never panics; the error is recorded for the UI via
    /// [`expr::last_error`]).
    ///
    /// [`expression`]: Self::expression
    pub fn sample_expr(&self, t: f32, default: f32, ctx: ExprCtx) -> f32 {
        let keyed = self.sample(t, default);
        match self.expression.as_deref() {
            Some(src) if !src.trim().is_empty() => {
                let mut ctx = ctx;
                ctx.value = keyed; // expose the keyframed value to the script
                expr::eval(src, &ctx).unwrap_or(keyed)
            }
            _ => keyed,
        }
    }

    /// Whether this track carries a non-empty expression.
    pub fn has_expression(&self) -> bool {
        self.expression
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
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
        self.keys.insert(
            idx,
            Keyframe {
                t,
                value,
                interp,
                roving: false,
            },
        );
    }

    /// Whether the key nearest `t` is an **interior** key (not the first or last)
    /// — the only keys that may rove (endpoints always anchor the time range).
    /// `false` when there is no key near `t`, or it is an endpoint, or the track
    /// has fewer than three keys.
    pub fn is_interior_key(&self, t: f32) -> bool {
        const EPS: f32 = 1e-3;
        if self.keys.len() < 3 {
            return false;
        }
        let last = self.keys.len() - 1;
        self.keys
            .iter()
            .position(|k| (k.t - t).abs() < EPS)
            .is_some_and(|i| i != 0 && i != last)
    }

    /// Set the **roving** flag on the key nearest `t` (After Effects' *Rove
    /// Across Time*). No-op returning `false` when no key is near `t` or it is an
    /// endpoint (endpoints can never rove). Returns `true` when a key was updated.
    pub fn set_roving(&mut self, t: f32, roving: bool) -> bool {
        if !self.is_interior_key(t) {
            return false;
        }
        const EPS: f32 = 1e-3;
        if let Some(k) = self.keys.iter_mut().find(|k| (k.t - t).abs() < EPS) {
            k.roving = roving;
            true
        } else {
            false
        }
    }

    /// Whether the key nearest `t` is flagged roving (and is interior). Drives
    /// the *Rove Across Time* toggle's checked state.
    pub fn is_roving_at(&self, t: f32) -> bool {
        self.is_interior_key(t) && self.key_at(t).is_some_and(|k| k.roving)
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
