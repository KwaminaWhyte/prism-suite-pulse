//! **Time remapping** (After Effects' *Enable Time Remap*): a keyframable scalar
//! property that remaps the **source time** used when sampling a time-based
//! layer.
//!
//! A [`Footage`](super::LayerKind::Footage) image-sequence and a
//! [`Precomp`](super::LayerKind::Precomp) are both *time-based* sources: at comp
//! time `t` they normally sample their source at that same `t` (footage derives a
//! frame index from it; a precomp renders its nested comp at `t + time_offset`).
//! When **time remap is enabled** on such a layer, the source is instead sampled
//! at a **remapped time** `r(t)` — a value driven by a keyframable [`Track`] (in
//! seconds) and, since tracks already carry expressions, by an expression too.
//!
//! This lets a user **freeze** (a constant remap holds one source frame),
//! **reverse** (a decreasing remap plays the source backwards), or **slow / speed
//! up** (a remap whose slope is less / greater than 1) footage and precomp
//! playback — exactly After Effects' Time Remap curve.
//!
//! Back-compat: a [`TimeRemap`] is `serde`-defaulted to **disabled** with an
//! empty track, so a layer (or whole `.pulse` file) that predates time remapping
//! loads with `enabled == false` and samples its source at the comp time exactly
//! as before — the remap is a pure no-op until switched on.

use serde::{Deserialize, Serialize};

use super::expr::ExprCtx;
use super::keyframe::{Ease, Interp, Track};

/// A layer's optional **time-remap** property: an enable switch plus the
/// keyframable scalar [`Track`] (seconds) that, when enabled, supplies the
/// **source time** a time-based layer is sampled at instead of the comp time `t`.
///
/// `serde`-defaulted in full (disabled, empty track) so pre-time-remap `.pulse`
/// files load with the remap off and behave identically.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimeRemap {
    /// Whether time remapping drives this layer's source time. When `false` the
    /// layer samples its source at the comp time (legacy behaviour); when `true`
    /// it samples at the remap track's value at `t`.
    #[serde(default)]
    pub enabled: bool,
    /// The remap curve: a scalar track of *source* times (seconds) keyed against
    /// comp time. Sampled like any other property (keyframes + easing +
    /// expression). Empty until [`seed_default`](Self::seed_default) lays down
    /// the identity keys (or the UI keys it).
    #[serde(default)]
    pub track: Track,
}

impl TimeRemap {
    /// Whether the remap is on **and** has something to sample (a non-empty
    /// track). A remap with no keys would collapse the source to source-time `0`
    /// (the track's empty default), so the renderer treats an empty-track remap
    /// as "off" and falls back to the comp time — a safe identity.
    pub fn is_active(&self) -> bool {
        self.enabled && !self.track.keys.is_empty()
    }

    /// The **source time** to sample at comp time `t`: the remap track's value
    /// when [`is_active`](Self::is_active), else `t` unchanged (identity). Pure
    /// keyframe sampling (no expression); see [`source_time_ctx`](Self::source_time_ctx)
    /// for the expression-aware form the renderer uses.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn source_time(&self, t: f32) -> f32 {
        if self.is_active() {
            // `default` is `t` so a still-empty active track is still identity.
            self.track.sample(t, t)
        } else {
            t
        }
    }

    /// The **source time** at comp time `ctx.time`, **evaluating the remap
    /// track's expression** if one is set (the keyframed remap value is exposed
    /// to the expression as `value`). Falls back to identity (`ctx.time`) when the
    /// remap is inactive. The renderer uses this so an expressioned remap drives
    /// source time too.
    pub fn source_time_ctx(&self, ctx: ExprCtx) -> f32 {
        if self.is_active() {
            self.track.sample_expr(ctx.time, ctx.time, ctx)
        } else {
            ctx.time
        }
    }

    /// Seed After-Effects-style **default keys** when the user enables time
    /// remap: an identity ramp from source time `0` at comp time `0` to
    /// `source_duration` at comp time `comp_duration` — so freshly enabling the
    /// remap plays the source 1:1 over the layer's span (then the user reshapes
    /// the curve to freeze / reverse / retime).
    ///
    /// If the source has no meaningful duration (a still, or an unknown length),
    /// a single identity key at `t = 0, value = 0` is laid down instead (a
    /// constant hold at the source start — a clean, value-neutral default). The
    /// last segment eases (After Effects gives time-remap keys an Easy Ease feel)
    /// only when there are two keys.
    ///
    /// Idempotent-ish: only seeds when the track is currently empty, so toggling
    /// the switch off and on again doesn't clobber a hand-keyed curve (the UI
    /// keeps the keys when disabling).
    pub fn seed_default(&mut self, comp_duration: f32, source_duration: Option<f32>) {
        if !self.track.keys.is_empty() {
            return;
        }
        match source_duration {
            Some(dur) if dur > 0.0 && comp_duration > 0.0 => {
                self.track.set_key(0.0, 0.0);
                self.track.set_key(comp_duration, dur);
                // Match AE's eased time-remap default on the single segment.
                self.track.set_interp(0.0, Interp::Ease(Ease::EASY));
            }
            _ => {
                // No usable source duration: a single identity key (hold at 0).
                self.track.set_key(0.0, 0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(t: f32) -> ExprCtx {
        ExprCtx {
            time: t,
            value: 0.0,
            fps: 30.0,
            duration: 5.0,
            index: 0,
        }
    }

    #[test]
    fn disabled_is_identity() {
        let tr = TimeRemap::default();
        assert!(!tr.enabled);
        for &t in &[0.0, 1.0, 2.5, 5.0] {
            assert_eq!(tr.source_time(t), t);
            assert_eq!(tr.source_time_ctx(ctx(t)), t);
        }
    }

    #[test]
    fn enabled_but_empty_track_is_identity() {
        // Enabled with no keys must NOT collapse to source-time 0 — it falls back
        // to the comp time so an "on but unconfigured" remap is harmless.
        let mut tr = TimeRemap::default();
        tr.enabled = true;
        assert!(!tr.is_active());
        for &t in &[0.0, 1.0, 3.3] {
            assert_eq!(tr.source_time(t), t);
        }
    }

    #[test]
    fn identity_seed_matches_no_remap() {
        // An identity ramp 0->dur over 0->comp_dur (with comp_dur == dur) samples
        // back to t itself, so an identity-seeded remap == no remap.
        let mut tr = TimeRemap::default();
        tr.enabled = true;
        tr.seed_default(5.0, Some(5.0));
        assert!(tr.is_active());
        // The eased segment passes through both endpoints exactly; the midpoint
        // is eased but still equals t at the symmetric Easy-Ease center.
        assert!((tr.source_time(0.0) - 0.0).abs() < 1e-4);
        assert!((tr.source_time(5.0) - 5.0).abs() < 1e-4);
        assert!((tr.source_time(2.5) - 2.5).abs() < 1e-3);
    }

    #[test]
    fn reverse_remap_maps_t_to_dur_minus_t() {
        // A linear remap from dur at t=0 down to 0 at t=dur plays the source
        // backwards: r(t) = dur - t.
        let dur = 4.0_f32;
        let mut tr = TimeRemap::default();
        tr.enabled = true;
        tr.track.set_key(0.0, dur);
        tr.track.set_key(dur, 0.0);
        for &t in &[0.0, 1.0, 2.0, 3.0, 4.0] {
            assert!((tr.source_time(t) - (dur - t)).abs() < 1e-4, "t={t}");
        }
    }

    #[test]
    fn freeze_remap_holds_one_source_time() {
        // A constant remap (single key, or two equal-valued keys) holds one
        // source time regardless of comp time — a freeze-frame.
        let mut tr = TimeRemap::default();
        tr.enabled = true;
        tr.track.set_key(0.0, 1.5);
        for &t in &[0.0, 1.0, 2.0, 9.0] {
            assert!((tr.source_time(t) - 1.5).abs() < 1e-4);
        }
    }

    #[test]
    fn remap_samples_with_easing() {
        // An eased segment over [0, 2] from 0 to 2: with Easy Ease the midpoint
        // value is pulled toward the center but the endpoints are exact, and the
        // curve is monotonic — distinct from the linear midpoint of 1.0 only if
        // we compare a non-symmetric sample. Verify endpoints + monotonicity.
        let mut tr = TimeRemap::default();
        tr.enabled = true;
        tr.track.set_key(0.0, 0.0);
        tr.track.set_key(2.0, 2.0);
        tr.track.set_interp(0.0, Interp::Ease(Ease::OUT)); // accelerates away
        let a = tr.source_time(0.5);
        let b = tr.source_time(1.0);
        assert!(a < b, "eased remap is monotonic increasing");
        // Ease Out leaves the first key slowly, so early source time lags linear.
        assert!(a < 0.5 + 1e-3);
        assert!((tr.source_time(0.0)).abs() < 1e-4);
        assert!((tr.source_time(2.0) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn expression_drives_remap() {
        // An expression on the remap track drives source time (here: half speed).
        let mut tr = TimeRemap::default();
        tr.enabled = true;
        tr.track.set_key(0.0, 0.0); // make it active
        tr.track.expression = Some("time * 0.5".to_string());
        assert!((tr.source_time_ctx(ctx(4.0)) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn seed_is_idempotent_on_nonempty_track() {
        let mut tr = TimeRemap::default();
        tr.track.set_key(0.0, 1.0);
        tr.seed_default(5.0, Some(5.0)); // should not clobber the existing key
        assert_eq!(tr.track.keys.len(), 1);
        assert_eq!(tr.track.keys[0].value, 1.0);
    }

    #[test]
    fn seed_without_duration_lays_single_identity_key() {
        let mut tr = TimeRemap::default();
        tr.seed_default(5.0, None);
        assert_eq!(tr.track.keys.len(), 1);
        assert_eq!(tr.track.keys[0].value, 0.0);
    }
}
