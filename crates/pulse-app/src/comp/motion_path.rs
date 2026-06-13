//! Spatial **motion paths**: the editable curve a layer's animated position
//! traces, plus **auto-orient along path** (After Effects' *Orient Along Path*).
//!
//! A layer's `(X, Y)` position tracks each interpolate independently in time
//! (linear / hold / Bézier-eased per channel — see [`Track`](super::Track)). Read
//! together over time those two scalar tracks describe a **spatial curve** in
//! comp space: the path the layer's anchor sweeps along as the playhead moves.
//! This module exposes that curve as a **pure** function — [`sample_path`] yields
//! the position *and* the unit tangent (direction of travel) at any time `t` — so
//! it can drive both headless rendering (auto-orient folds the tangent into the
//! layer's effective rotation) and, later, an editable on-canvas path overlay.
//!
//! The tangent is the velocity of the parametric curve `p(t) = (x(t), y(t))`,
//! estimated by a centered finite difference of the position tracks. Because it
//! samples the *same* per-channel interpolation the renderer uses, the tangent is
//! exactly the direction the layer is actually moving — eased segments, hold
//! steps and all — rather than a separate spatial spline that could disagree with
//! the rendered motion. Everything here is pure (no time source, no IO) and
//! unit-testable headlessly.

use super::keyframe::Track;

/// A sample of a layer's spatial motion path at one instant: the position in
/// comp space and the (approximate) unit tangent — the direction of travel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathSample {
    /// Position `(x, y)` in comp space (comp px), the layer's anchor location.
    pub pos: [f32; 2],
    /// Unit tangent `(dx, dy)` — the normalized direction of travel at `t`.
    /// `None` when the layer is momentarily stationary (zero velocity: a single
    /// position key, a hold step, or the apex of an ease) so there is no
    /// well-defined heading.
    pub tangent: Option<[f32; 2]>,
}

impl PathSample {
    /// The travel **heading** in degrees (`0°` = +x / right, clockwise with `+y`
    /// down — matching [`Affine2::rotate_deg`](super::Affine2::rotate_deg) and the
    /// preview's screen convention). `None` when the path is stationary at `t`.
    pub fn heading_deg(&self) -> Option<f32> {
        self.tangent
            .map(|[dx, dy]| dy.atan2(dx).to_degrees())
    }
}

/// The finite-difference half-step (seconds) used to estimate the path tangent.
/// Small enough to read the local heading of an eased segment, large enough to
/// stay clear of `f32` cancellation noise.
const TANGENT_DT: f32 = 1.0 / 240.0;

/// Sample the spatial motion path described by a layer's `x` / `y` position
/// tracks at time `t`: the position and the unit tangent (direction of travel).
///
/// A **pure** function over the two position tracks (keyframes only — no
/// expressions, no parent transform): it samples each track with the same
/// per-channel interpolation the renderer uses, so the returned position matches
/// `layer.transform(t)`'s `(x, y)` and the tangent is the true direction of the
/// rendered motion. The tangent is a centered finite difference of `p(t)`; a
/// (near-)zero velocity — a single key, a hold step, or an ease apex — yields
/// `tangent: None` (no defined heading).
///
/// `default_x` / `default_y` are the resting values used when a track is empty
/// (the layer's [`Prop::default_value`](super::Prop::default_value)), so an
/// unanimated layer samples a static point with no tangent.
pub fn sample_path(x: &Track, y: &Track, t: f32, default_x: f32, default_y: f32) -> PathSample {
    let pos = [x.sample(t, default_x), y.sample(t, default_y)];
    let dt = TANGENT_DT;
    // Centered difference: velocity ≈ (p(t+dt) - p(t-dt)) / (2·dt). Sampling the
    // tracks themselves keeps the heading consistent with the rendered motion.
    let ahead = [x.sample(t + dt, default_x), y.sample(t + dt, default_y)];
    let behind = [x.sample(t - dt, default_x), y.sample(t - dt, default_y)];
    let vx = ahead[0] - behind[0];
    let vy = ahead[1] - behind[1];
    let len = (vx * vx + vy * vy).sqrt();
    // Below ~a hundredth of a comp px across the 2·dt window the heading is noise;
    // treat it as stationary so a static / held / apex point has no orientation.
    let tangent = if len > 1e-4 {
        Some([vx / len, vy / len])
    } else {
        None
    };
    PathSample { pos, tangent }
}

/// The **auto-orient** rotation (degrees) contributed by the layer's motion path
/// at time `t`: the path's travel heading, or `0.0` where the path is stationary
/// (so a held / static layer keeps its keyed rotation unchanged).
///
/// This is *added* to the layer's keyframed rotation by the renderer when
/// auto-orient is on — After Effects' *Orient Along Path* likewise composes the
/// path heading with the layer's own Rotation property, so a spinning layer still
/// spins *relative to* its direction of travel.
pub fn auto_orient_deg(x: &Track, y: &Track, t: f32, default_x: f32, default_y: f32) -> f32 {
    sample_path(x, y, t, default_x, default_y)
        .heading_deg()
        .unwrap_or(0.0)
}
