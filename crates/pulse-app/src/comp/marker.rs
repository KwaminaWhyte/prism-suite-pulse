//! Markers and the work area — comp/layer time annotations + a render range.
//!
//! After Effects pins **markers** to the composition's timeline (and to
//! individual layers) as labelled, optionally durationed points to call out beats
//! in the animation, plus a **work area** — a `[start, end]` sub-range of the
//! timeline that bounds RAM-preview / playback / render. Both are pure timeline
//! metadata: they carry no pixels and never touch the compositor, only the
//! transport and the timeline UI.
//!
//! This module is the **pure** core: [`Marker`] (a time, an optional duration, a
//! label, and a tint), [`WorkArea`] (the `[start, end]` range, with clamping and
//! containment helpers), and the navigation helpers that turn a list of markers +
//! the playhead into the next/previous marker time (so the transport can jump
//! between beats). All the timing math lives — and is unit-tested — here, free of
//! egui.

use serde::{Deserialize, Serialize};

/// One timeline marker: a labelled point (or span) on the composition or a layer.
///
/// `time` is the marker's start in seconds (comp time for a comp marker;
/// layer-local comp time for a layer marker — Pulse layers start at comp `0`, so
/// the two coincide today). `duration` is an optional span length in seconds
/// (`0` = an instantaneous point marker, the common case); a positive duration
/// draws the marker as a band. `label` is the user comment shown on the marker,
/// and `color` an sRGB tint so different beats read apart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Marker start time in seconds.
    pub time: f32,
    /// Span length in seconds; `0` = an instantaneous point marker.
    #[serde(default)]
    pub duration: f32,
    /// User comment / label shown on the marker.
    #[serde(default)]
    pub label: String,
    /// sRGB tint (0..1) the marker is drawn in.
    #[serde(default = "default_marker_color")]
    pub color: [f32; 3],
}

/// The default marker tint (After Effects' green marker), used when a
/// `serde`-loaded marker omits the color.
fn default_marker_color() -> [f32; 3] {
    Marker::DEFAULT_COLOR
}

impl Marker {
    /// The default marker tint — a saturated green, matching After Effects'
    /// default comp/layer marker color.
    pub const DEFAULT_COLOR: [f32; 3] = [0.30, 0.78, 0.45];

    /// A fresh instantaneous (point) marker at `time` with the default tint and an
    /// empty label.
    pub fn at(time: f32) -> Self {
        Self {
            time,
            duration: 0.0,
            label: String::new(),
            color: Self::DEFAULT_COLOR,
        }
    }

    /// This marker's end time (`time + duration`); equal to `time` for a point
    /// marker.
    pub fn end(&self) -> f32 {
        self.time + self.duration.max(0.0)
    }
}

/// The **work area**: a `[start, end]` sub-range of the comp timeline (seconds)
/// that bounds RAM-preview / playback / render. A fresh comp's work area spans the
/// whole `[0, duration]`.
///
/// `serde`-defaulted (and back-compat) to the full timeline via
/// [`WorkArea::default`] / [`WorkArea::full`] so pre-work-area `.pulse` files load
/// with the whole comp as the work area.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkArea {
    /// Work-area start in seconds (clamped to `[0, end]` when sampled).
    pub start: f32,
    /// Work-area end in seconds (clamped to `[start, duration]` when sampled).
    pub end: f32,
}

impl Default for WorkArea {
    fn default() -> Self {
        // The full default timeline; a comp resets this to its own duration on
        // creation (see `Comp::new`) / load.
        Self::full(0.0)
    }
}

impl WorkArea {
    /// A work area spanning the whole `[0, duration]` timeline.
    pub fn full(duration: f32) -> Self {
        Self {
            start: 0.0,
            end: duration.max(0.0),
        }
    }

    /// The work area clamped to a valid range inside `[0, duration]`: `start` is
    /// kept in `[0, duration]`, `end` in `[start, duration]`, so the returned
    /// range is always ordered and inside the timeline. Used by the transport /
    /// renderer so a hand-edited (or stale) range can never invert or escape the
    /// comp.
    pub fn clamped(self, duration: f32) -> Self {
        let duration = duration.max(0.0);
        let start = self.start.clamp(0.0, duration);
        let end = self.end.clamp(start, duration);
        Self { start, end }
    }

    /// The work area's length in seconds (clamped, so never negative).
    pub fn length(self, duration: f32) -> f32 {
        let c = self.clamped(duration);
        c.end - c.start
    }

    /// Whether `t` lies inside the clamped work area (inclusive of both ends).
    pub fn contains(self, t: f32, duration: f32) -> bool {
        let c = self.clamped(duration);
        t >= c.start && t <= c.end
    }

    /// Whether this work area is effectively the whole timeline (`[0, duration]`),
    /// within a small epsilon — used to gate "trimmed work area" UI / behaviour.
    pub fn is_full(self, duration: f32) -> bool {
        let c = self.clamped(duration);
        c.start <= 1e-4 && c.end >= duration.max(0.0) - 1e-4
    }
}

/// The first marker time **strictly after** `time` among `markers`, or `None`
/// when none lies ahead. Markers need not be sorted (the minimum qualifying time
/// is returned), so this is robust to the user adding markers out of order.
pub fn next_marker_time(markers: &[Marker], time: f32) -> Option<f32> {
    markers
        .iter()
        .map(|m| m.time)
        .filter(|&t| t > time + 1e-4)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

/// The last marker time **strictly before** `time` among `markers`, or `None`
/// when none lies behind. Markers need not be sorted (the maximum qualifying time
/// is returned).
pub fn prev_marker_time(markers: &[Marker], time: f32) -> Option<f32> {
    markers
        .iter()
        .map(|m| m.time)
        .filter(|&t| t < time - 1e-4)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(times: &[f32]) -> Vec<Marker> {
        times.iter().map(|&t| Marker::at(t)).collect()
    }

    #[test]
    fn point_marker_defaults() {
        let m = Marker::at(2.5);
        assert_eq!(m.time, 2.5);
        assert_eq!(m.duration, 0.0);
        assert!(m.label.is_empty());
        assert_eq!(m.color, Marker::DEFAULT_COLOR);
        assert_eq!(m.end(), 2.5); // a point marker ends where it starts
    }

    #[test]
    fn durationed_marker_end() {
        let mut m = Marker::at(1.0);
        m.duration = 2.0;
        assert_eq!(m.end(), 3.0);
        // A negative duration is treated as zero (never ends before it starts).
        m.duration = -5.0;
        assert_eq!(m.end(), 1.0);
    }

    #[test]
    fn work_area_default_and_full() {
        // The serde default is the empty full range; `full` spans [0, duration].
        assert_eq!(WorkArea::default(), WorkArea { start: 0.0, end: 0.0 });
        let w = WorkArea::full(5.0);
        assert_eq!(w, WorkArea { start: 0.0, end: 5.0 });
        assert!(w.is_full(5.0));
    }

    #[test]
    fn work_area_clamps_into_range() {
        // Out-of-range and inverted ranges are clamped to an ordered [0, dur] range.
        let dur = 10.0;
        let w = WorkArea { start: -2.0, end: 4.0 }.clamped(dur);
        assert_eq!(w, WorkArea { start: 0.0, end: 4.0 });
        let w = WorkArea { start: 3.0, end: 99.0 }.clamped(dur);
        assert_eq!(w, WorkArea { start: 3.0, end: 10.0 });
        // Inverted end < start: end snaps up to start (a zero-length area).
        let w = WorkArea { start: 6.0, end: 2.0 }.clamped(dur);
        assert_eq!(w, WorkArea { start: 6.0, end: 6.0 });
        assert_eq!(w.length(dur), 0.0);
    }

    #[test]
    fn work_area_length_and_contains() {
        let dur = 8.0;
        let w = WorkArea { start: 2.0, end: 5.0 };
        assert!((w.length(dur) - 3.0).abs() < 1e-6);
        assert!(w.contains(2.0, dur)); // inclusive start
        assert!(w.contains(5.0, dur)); // inclusive end
        assert!(w.contains(3.5, dur));
        assert!(!w.contains(1.0, dur));
        assert!(!w.contains(6.0, dur));
    }

    #[test]
    fn is_full_detects_trimmed_area() {
        let dur = 5.0;
        assert!(WorkArea::full(dur).is_full(dur));
        assert!(!WorkArea { start: 1.0, end: 5.0 }.is_full(dur));
        assert!(!WorkArea { start: 0.0, end: 4.0 }.is_full(dur));
    }

    #[test]
    fn next_marker_finds_nearest_ahead() {
        let m = marks(&[1.0, 3.0, 5.0]);
        assert_eq!(next_marker_time(&m, 0.0), Some(1.0));
        assert_eq!(next_marker_time(&m, 1.0), Some(3.0)); // strictly after
        assert_eq!(next_marker_time(&m, 2.5), Some(3.0));
        assert_eq!(next_marker_time(&m, 5.0), None); // none ahead of the last
    }

    #[test]
    fn prev_marker_finds_nearest_behind() {
        let m = marks(&[1.0, 3.0, 5.0]);
        assert_eq!(prev_marker_time(&m, 6.0), Some(5.0));
        assert_eq!(prev_marker_time(&m, 5.0), Some(3.0)); // strictly before
        assert_eq!(prev_marker_time(&m, 3.5), Some(3.0));
        assert_eq!(prev_marker_time(&m, 1.0), None); // none behind the first
    }

    #[test]
    fn navigation_handles_unsorted_markers() {
        // The helpers return the min/max qualifying time, so order doesn't matter.
        let m = marks(&[5.0, 1.0, 3.0]);
        assert_eq!(next_marker_time(&m, 0.0), Some(1.0));
        assert_eq!(prev_marker_time(&m, 6.0), Some(5.0));
    }

    #[test]
    fn navigation_empty_is_none() {
        let m: Vec<Marker> = Vec::new();
        assert_eq!(next_marker_time(&m, 1.0), None);
        assert_eq!(prev_marker_time(&m, 1.0), None);
    }
}
