//! Onion-skinning — ghosting neighbouring frames behind the live playhead.
//!
//! A staple of hand-keyed motion tooling (and 2D animation packages): faint
//! "ghost" copies of the comp at the frames *before* and *after* the current one
//! are drawn behind the live frame, so an animator can see where the motion came
//! from and where it's going while setting timing by hand. The ghosts fade with
//! distance from the playhead and are tinted (cool for past, warm for future) so
//! before/after read apart at a glance.
//!
//! This module is the **pure** core: [`OnionSkin`] holds the user-facing settings
//! (enabled, how many frames each side, the frame step between ghosts, the
//! nearest ghost's opacity), and [`OnionSkin::ghosts`] turns those settings + the
//! current playhead into the ordered list of [`Ghost`]s to paint — each a comp
//! time, a tint, and an opacity. The preview consumes that list; all the timing /
//! falloff math lives (and is unit-tested) here, free of egui.

/// Whether a ghost lies before the playhead (the past) or after it (the future).
/// Drives the tint so the two directions read apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// A frame earlier than the playhead — "where the motion came from".
    Before,
    /// A frame later than the playhead — "where the motion is going".
    After,
}

/// One ghost frame to paint behind the live comp: the comp `time` to render it
/// at, an sRGB `tint` color the ghost is multiplied toward, and the `opacity`
/// (0..1) it's drawn at. The list is ordered farthest → nearest so nearer (more
/// opaque) ghosts paint over farther (fainter) ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ghost {
    /// Comp time (seconds) to sample the comp at for this ghost.
    pub time: f32,
    /// Which side of the playhead this ghost is on.
    pub dir: Dir,
    /// sRGB tint the ghost's colors are pulled toward (cool past / warm future).
    pub tint: [f32; 3],
    /// Opacity (0..1) the whole ghost frame is drawn at.
    pub opacity: f32,
}

/// Onion-skin settings: the user-facing knobs behind the View-menu controls.
///
/// `before` / `after` are how many ghost frames to show on each side of the
/// playhead; `step` is the frame stride between ghosts (1 = every frame, 2 =
/// every other, …) so sparse timing checks don't need dozens of ghosts;
/// `opacity` is the nearest ghost's opacity, from which farther ghosts fade off
/// linearly. Defaults to *disabled* with a sensible 2-each-side / every-frame /
/// 35%-nearest setup, so flipping `enabled` on is immediately useful.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionSkin {
    /// Master switch — when off, [`ghosts`](OnionSkin::ghosts) is empty.
    pub enabled: bool,
    /// Number of ghost frames shown *before* the playhead.
    pub before: u32,
    /// Number of ghost frames shown *after* the playhead.
    pub after: u32,
    /// Frame stride between successive ghosts (clamped to ≥ 1).
    pub step: u32,
    /// Opacity (0..1) of the ghost nearest the playhead; farther ghosts fade.
    pub opacity: f32,
}

impl Default for OnionSkin {
    fn default() -> Self {
        Self {
            enabled: false,
            before: 2,
            after: 2,
            step: 1,
            opacity: 0.35,
        }
    }
}

impl OnionSkin {
    /// The cool tint applied to *past* ghosts (a desaturated blue).
    pub const TINT_BEFORE: [f32; 3] = [0.45, 0.60, 0.95];
    /// The warm tint applied to *future* ghosts (a desaturated orange).
    pub const TINT_AFTER: [f32; 3] = [0.95, 0.65, 0.40];

    /// The largest count allowed per side (keeps the ghost stack — and the
    /// per-frame paint cost — bounded; also the UI slider's ceiling).
    pub const MAX_PER_SIDE: u32 = 8;

    /// Compute the ordered ghost frames to paint behind the live comp at playhead
    /// `time`, given the comp's `fps` and `duration`.
    ///
    /// For each side, walks out `before`/`after` steps of `step` frames, emitting
    /// a [`Ghost`] per frame whose time lands inside `[0, duration]` (ghosts that
    /// would fall off the ends of the timeline are skipped — there's nothing to
    /// show there). Opacity falls off linearly with distance: the nearest ghost
    /// gets the full `opacity`, the farthest a small floor, so the trail reads as
    /// a fade. The returned list is ordered **farthest → nearest within each
    /// side** (past block then future block) so nearer ghosts paint last (on top).
    ///
    /// Returns empty when disabled, when `fps`/`duration` are non-positive, or
    /// when nothing falls in range.
    pub fn ghosts(&self, time: f32, fps: f32, duration: f32) -> Vec<Ghost> {
        if !self.enabled || fps <= 0.0 || duration <= 0.0 {
            return Vec::new();
        }
        let step = self.step.max(1);
        let before = self.before.min(Self::MAX_PER_SIDE);
        let after = self.after.min(Self::MAX_PER_SIDE);
        let base_op = self.opacity.clamp(0.0, 1.0);
        let dt = step as f32 / fps;

        let mut out = Vec::new();
        // A side's ghosts, farthest-first so the nearest (most opaque) is last.
        let mut push_side = |dir: Dir, count: u32| {
            if count == 0 {
                return;
            }
            let sign = match dir {
                Dir::Before => -1.0,
                Dir::After => 1.0,
            };
            let tint = match dir {
                Dir::Before => Self::TINT_BEFORE,
                Dir::After => Self::TINT_AFTER,
            };
            // k = count (farthest) down to 1 (nearest).
            for k in (1..=count).rev() {
                let t = time + sign * dt * k as f32;
                if t < 0.0 || t > duration {
                    continue;
                }
                out.push(Ghost {
                    time: t,
                    dir,
                    tint,
                    opacity: opacity_at(base_op, k, count),
                });
            }
        };
        push_side(Dir::Before, before);
        push_side(Dir::After, after);
        out
    }
}

/// Opacity for the `k`-th ghost out of `count` on one side (k = 1 is nearest the
/// playhead, k = count is farthest). The nearest ghost gets `base`; farther ones
/// fade linearly to a floor of `base * MIN_FRACTION`, so even the farthest ghost
/// stays faintly visible rather than vanishing.
fn opacity_at(base: f32, k: u32, count: u32) -> f32 {
    /// The farthest ghost keeps this fraction of the nearest's opacity.
    const MIN_FRACTION: f32 = 0.25;
    if count <= 1 {
        return base;
    }
    // distance 0 (nearest) → 1.0 weight; distance (count-1) (farthest) → MIN_FRACTION.
    let dist = (k - 1) as f32 / (count - 1) as f32; // 0..1
    let weight = 1.0 - dist * (1.0 - MIN_FRACTION);
    base * weight
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f32 = 24.0;
    const DUR: f32 = 10.0;

    #[test]
    fn disabled_yields_no_ghosts() {
        let mut o = OnionSkin::default();
        assert!(!o.enabled);
        assert!(o.ghosts(5.0, FPS, DUR).is_empty());
        o.enabled = true;
        assert!(!o.ghosts(5.0, FPS, DUR).is_empty());
    }

    #[test]
    fn count_matches_before_plus_after_when_in_range() {
        let o = OnionSkin {
            enabled: true,
            before: 3,
            after: 2,
            step: 1,
            opacity: 0.4,
        };
        // Mid-timeline: all 5 ghosts land in range.
        let g = o.ghosts(5.0, FPS, DUR);
        assert_eq!(g.len(), 5);
        assert_eq!(g.iter().filter(|x| x.dir == Dir::Before).count(), 3);
        assert_eq!(g.iter().filter(|x| x.dir == Dir::After).count(), 2);
    }

    #[test]
    fn ghosts_spaced_by_step_over_fps() {
        let o = OnionSkin {
            enabled: true,
            before: 0,
            after: 2,
            step: 3,
            opacity: 0.5,
        };
        let g = o.ghosts(1.0, FPS, DUR);
        assert_eq!(g.len(), 2);
        let dt = 3.0 / FPS;
        // Ordered farthest → nearest: +2 step then +1 step.
        assert!((g[0].time - (1.0 + 2.0 * dt)).abs() < 1e-6);
        assert!((g[1].time - (1.0 + dt)).abs() < 1e-6);
    }

    #[test]
    fn out_of_range_ghosts_are_dropped() {
        let o = OnionSkin {
            enabled: true,
            before: 4,
            after: 4,
            step: 1,
            opacity: 0.4,
        };
        // At t=0 the "before" ghosts are all negative → dropped; only after survive.
        let g = o.ghosts(0.0, FPS, DUR);
        assert!(g.iter().all(|x| x.dir == Dir::After));
        assert_eq!(g.len(), 4);
        // At t=duration the "after" ghosts overflow → dropped; only before survive.
        let g = o.ghosts(DUR, FPS, DUR);
        assert!(g.iter().all(|x| x.dir == Dir::Before));
        assert_eq!(g.len(), 4);
    }

    #[test]
    fn nearest_is_most_opaque_and_last() {
        let o = OnionSkin {
            enabled: true,
            before: 0,
            after: 4,
            step: 1,
            opacity: 0.6,
        };
        let g = o.ghosts(2.0, FPS, DUR);
        assert_eq!(g.len(), 4);
        // Ordered farthest → nearest, so opacity strictly increases through the list.
        for w in g.windows(2) {
            assert!(
                w[1].opacity > w[0].opacity,
                "opacity should rise toward the playhead"
            );
        }
        // The nearest ghost (last) carries the full base opacity.
        assert!((g.last().unwrap().opacity - 0.6).abs() < 1e-6);
    }

    #[test]
    fn single_ghost_uses_full_opacity() {
        let o = OnionSkin {
            enabled: true,
            before: 1,
            after: 0,
            step: 1,
            opacity: 0.5,
        };
        let g = o.ghosts(5.0, FPS, DUR);
        assert_eq!(g.len(), 1);
        assert!((g[0].opacity - 0.5).abs() < 1e-6);
    }

    #[test]
    fn opacity_floor_keeps_farthest_visible() {
        let o = OnionSkin {
            enabled: true,
            before: 0,
            after: 8,
            step: 1,
            opacity: 0.8,
        };
        let g = o.ghosts(0.0, FPS, DUR);
        // Every ghost stays above zero (a floor fraction of base), and below base.
        for gh in &g {
            assert!(gh.opacity > 0.0);
            assert!(gh.opacity <= 0.8 + 1e-6);
        }
        // Farthest (first) is exactly base * MIN_FRACTION.
        assert!((g[0].opacity - 0.8 * 0.25).abs() < 1e-6);
    }

    #[test]
    fn directions_carry_distinct_tints() {
        let o = OnionSkin {
            enabled: true,
            before: 1,
            after: 1,
            step: 1,
            opacity: 0.4,
        };
        let g = o.ghosts(5.0, FPS, DUR);
        let before = g.iter().find(|x| x.dir == Dir::Before).unwrap();
        let after = g.iter().find(|x| x.dir == Dir::After).unwrap();
        assert_eq!(before.tint, OnionSkin::TINT_BEFORE);
        assert_eq!(after.tint, OnionSkin::TINT_AFTER);
        assert_ne!(before.tint, after.tint);
    }

    #[test]
    fn step_and_count_are_clamped() {
        // step 0 behaves as 1; counts above MAX_PER_SIDE are capped.
        let o = OnionSkin {
            enabled: true,
            before: 100,
            after: 0,
            step: 0,
            opacity: 0.4,
        };
        let g = o.ghosts(5.0, FPS, DUR);
        assert_eq!(g.len() as u32, OnionSkin::MAX_PER_SIDE);
        // With step treated as 1, the nearest before-ghost is one frame back.
        let nearest = g.last().unwrap();
        assert!((nearest.time - (5.0 - 1.0 / FPS)).abs() < 1e-6);
    }

    #[test]
    fn non_positive_fps_or_duration_is_empty() {
        let o = OnionSkin {
            enabled: true,
            ..Default::default()
        };
        assert!(o.ghosts(5.0, 0.0, DUR).is_empty());
        assert!(o.ghosts(5.0, FPS, 0.0).is_empty());
    }
}
