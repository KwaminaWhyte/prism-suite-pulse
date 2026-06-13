//! **Roving keyframes** in time (After Effects' *Rove Across Time*).
//!
//! A spatial position keyframe marked **roving** is freed from its authored
//! time and *re-timed* so the layer travels at **constant velocity** along the
//! spatial motion path through the surrounding **anchored** (non-roving) keys.
//! Between two anchored keys, the elapsed time is distributed across the
//! interior roving keys in proportion to **arc length** along the path — so a
//! roving key that sits where the path is long in space gets a proportionally
//! long slice of time, equalizing speed.
//!
//! Only **interior** keys can rove; the first and last keys of the track are
//! always anchored (they pin the time range), matching After Effects. A roving
//! flag on an endpoint is ignored.
//!
//! The core re-timing is a **pure** function — [`roved_times`] maps each key's
//! `(time, position, roving)` to a new effective time — with no time source, no
//! IO, and no track mutation, so it is unit-testable headlessly. Position
//! sampling applies it by remapping the position tracks' key times before the
//! ordinary per-channel interpolation runs (see [`super::PulseLayer`]).

/// One position keyframe seen by the re-timer: its authored time, its position
/// `(x, y)` in comp space, and whether it is marked **roving**.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoveKey {
    /// Authored keyframe time (seconds).
    pub t: f32,
    /// Position `(x, y)` in comp space at this key.
    pub pos: [f32; 2],
    /// Whether this key roves across time (constant-velocity re-timing).
    pub roving: bool,
}

use super::keyframe::Track;

/// Whether either position track has at least one **interior** roving key — the
/// cheap gate the sampler uses to skip the re-timing clone entirely (the common
/// case: no roving keys, so position sampling is byte-for-byte unchanged).
///
/// Endpoints never rove, so a roving flag on the first/last key of a track does
/// not count.
pub fn has_roving(x: &Track, y: &Track) -> bool {
    let interior_rove = |t: &Track| {
        let n = t.keys.len();
        n >= 3 && t.keys[1..n - 1].iter().any(|k| k.roving)
    };
    interior_rove(x) || interior_rove(y)
}

/// Produce **roved copies** of a layer's `x` / `y` position tracks: clones whose
/// roving keys have been re-timed to constant velocity along the motion path
/// (see [`roved_times`]), ready to sample with the ordinary per-channel
/// interpolation. Anchored keys, values, interpolation modes, and expressions
/// are preserved untouched; only roving keys' **times** move.
///
/// Because X and Y are stored as two independent scalar tracks but roving is a
/// property of the 2D *position*, the two tracks are re-timed on a shared
/// timeline: the **union** of their key times. At each union time the position
/// is sampled from both tracks, a key roves when *either* track's key there is
/// roving, and the shared effective times are then written back onto whichever
/// track actually has a key at that time. Tracks whose key times are aligned
/// (the UI keys X and Y together) take the straightforward path; mismatched
/// tracks still re-time consistently.
///
/// Pure over the two input tracks (no time source, no IO); returns owned clones
/// so the caller samples them exactly like the originals.
pub fn roved_tracks(x: &Track, y: &Track) -> (Track, Track) {
    // Shared, de-duplicated, ascending timeline of every position key time.
    const EPS: f32 = 1e-4;
    let mut union: Vec<f32> = x.keys.iter().chain(y.keys.iter()).map(|k| k.t).collect();
    union.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    union.dedup_by(|a, b| (*a - *b).abs() <= EPS);

    // Whether `track` has a key at union time `t` that is flagged roving.
    let key_roves = |track: &Track, t: f32| {
        track
            .keys
            .iter()
            .any(|k| (k.t - t).abs() <= EPS && k.roving)
    };

    // Build the shared 2D key list. Position is sampled from both tracks (so a
    // key present in only one track still has a defined (x, y)); a union time
    // roves when either track's key there is flagged roving.
    let rove_keys: Vec<RoveKey> = union
        .iter()
        .map(|&t| RoveKey {
            t,
            pos: [x.sample(t, 0.0), y.sample(t, 0.0)],
            roving: key_roves(x, t) || key_roves(y, t),
        })
        .collect();

    let eff = roved_times(&rove_keys);

    // Write the shared effective times back onto each track's keys. Each track
    // key matches exactly one union time; map it to that union slot's new time.
    let remap = |track: &Track| {
        let mut out = track.clone();
        for k in &mut out.keys {
            if let Some(slot) = union.iter().position(|&u| (u - k.t).abs() <= EPS) {
                k.t = eff[slot];
            }
        }
        out
    };
    (remap(x), remap(y))
}

/// Euclidean distance between two comp-space points.
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

/// Re-time the **roving** keys so velocity along the spatial path is constant
/// between anchored keys, returning the **effective time** of every key (same
/// length and order as `keys`).
///
/// The keys must be time-ordered ascending. Anchored keys (and the always-pinned
/// first/last) keep their authored time. Each maximal run of interior roving
/// keys between two anchored keys is redistributed across that anchored
/// **segment**'s time span in proportion to the **arc length** of the polyline
/// through the run's positions: a key's effective time is the anchored start
/// time plus the segment's duration scaled by the cumulative path length up to
/// that key over the total path length of the segment.
///
/// Degenerate cases are handled gracefully: fewer than three keys (no interior
/// to rove) returns the authored times unchanged; a zero-length path segment
/// (all positions coincident) falls back to **even** time spacing so the keys
/// never collapse onto one instant; effective times are clamped to stay strictly
/// inside their anchored segment and non-decreasing, so the result is always a
/// valid sortable timeline.
///
/// Pure: depends only on its inputs, allocates one output `Vec`, and never reads
/// time or mutates the source — so it is deterministic and headlessly testable.
pub fn roved_times(keys: &[RoveKey]) -> Vec<f32> {
    let mut times: Vec<f32> = keys.iter().map(|k| k.t).collect();
    // Need at least one interior key (3 total) for roving to mean anything.
    if keys.len() < 3 {
        return times;
    }
    let last = keys.len() - 1;

    // Walk anchored "fence posts": the first key, every interior anchored key,
    // and the last key. Between consecutive posts, redistribute any interior
    // roving keys by arc length. An interior key only roves when its flag is set;
    // endpoints are always anchored regardless of their flag.
    let is_anchor = |i: usize| i == 0 || i == last || !keys[i].roving;

    let mut start = 0; // index of the current segment's anchored start
    for end in 1..=last {
        if !is_anchor(end) {
            continue;
        }
        // [start, end] is an anchored segment; indices start+1..end are the
        // interior roving keys to redistribute (empty when end == start+1).
        let interior = end - start - 1;
        if interior > 0 {
            redistribute(keys, &mut times, start, end);
        }
        start = end;
    }
    times
}

/// Redistribute the interior keys `start+1..end` across the anchored segment
/// `[start, end]` by arc length, writing their effective times into `times`.
fn redistribute(keys: &[RoveKey], times: &mut [f32], start: usize, end: usize) {
    let t0 = keys[start].t;
    let t1 = keys[end].t;
    let span = t1 - t0;

    // Cumulative path length from the segment start to each key in start..=end.
    let mut cum = vec![0.0_f32; end - start + 1];
    let mut total = 0.0_f32;
    for (k, c) in (start..end).zip(0..) {
        total += dist(keys[k].pos, keys[k + 1].pos);
        cum[c + 1] = total;
    }

    let count = end - start; // number of sub-intervals (>= 2 here)
    for (offset, i) in (1..count).zip(start + 1..end) {
        let frac = if total > f32::EPSILON {
            // Arc-length fraction along the segment's polyline.
            cum[offset] / total
        } else {
            // Coincident positions: fall back to even spacing so the roving keys
            // don't pile onto one instant.
            offset as f32 / count as f32
        };
        times[i] = t0 + span * frac;
    }

    // Guard the invariant the rest of the pipeline relies on: strictly ordered,
    // in-range times. Arc-length fractions are monotonic by construction, but
    // clamp defensively against f32 noise so sampling never sees an out-of-order
    // or out-of-range key.
    let mut prev = t0;
    for i in start + 1..end {
        times[i] = times[i].clamp(prev, t1);
        prev = times[i];
    }
}
