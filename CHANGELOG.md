# Changelog

All notable changes to **Pulse** (the After Effects analog, app #3 of the Prism
suite) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Graph editor** — an After-Effects-style value-curve view in the bottom panel,
  toggled against the lane timeline via a Timeline / Graph switch. Plots each
  animated property of the selected layer as a curve of value over time on a
  shared, auto-framed value axis (with per-second time gridlines, value-axis
  labels, and a playhead guide).
  - **Draggable keyframes** — drag a keyframe to retime (x) and revalue (y) it;
    keys re-sort live when a drag crosses a neighbour without losing the grab.
  - **Draggable Bézier ease handles** — drag a segment's outgoing/incoming handle
    to shape its easing directly. Dragging a handle on a Linear or Hold segment
    promotes it to an editable ease (seeded at the straight diagonal, so the
    conversion is value-neutral) — the per-key handle editing previously deferred
    from the interpolation work.
  - **Property chips** — per-property show/hide toggles (X, Y, Scale, Rotation,
    Opacity), each with a distinct curve color; with none selected the graph shows
    every keyframed property.
- Motion-model support for the graph editor: `Track::value_bounds` (value range
  including ease overshoot), `Track::move_key` (retime/revalue with live
  re-sorting), `Ease::with_out` / `Ease::with_in` (clamped-x handle edits), an
  `Ease::LINEAR` straight-diagonal seed, and a `Handle` enum — all unit-tested.

## [0.0.1] - 2026-06-06

### Added

- **Motion model** — a `Comp` (width, height, duration, fps) holding an ordered
  stack of `PulseLayer`s. Each layer is a solid color with five animatable
  properties (X, Y, Scale, Rotation, Opacity), each a `Track` of `Keyframe`s.
  Samples by linear interpolation between bracketing keys with constant hold
  outside the range; `set_key` inserts/overwrites keeping keys sorted. Serializes
  to JSON via serde.
- **Keyframe interpolation** — per-keyframe interpolation modes on the outgoing
  segment: **linear**, **hold** (stepped), and a temporal cubic-**Bézier ease**.
  Ships the After-Effects easing presets — **Easy Ease** (F9), **Ease In**, and
  **Ease Out** — as a Newton-solved CSS-`cubic-bezier` curve, with an
  interpolation picker in the properties panel and timeline markers that encode
  the mode (diamond = linear, square = hold, circle = ease). Unit-tested.
- **Timeline** — a per-second time ruler, one lane per layer with keyframe
  markers (union of all five tracks), and a draggable playhead with click/drag
  scrubbing.
- **Transport** — play/pause (Spacebar), real-`dt` playhead advance, loop at
  duration, and go-to-start / go-to-end controls.
- **Preview** — a CPU egui-`Painter` render: the comp as a centered, aspect-fit
  frame with each visible layer drawn as a rotated/scaled solid quad at its
  interpolated transform, faded by opacity, for the current playhead time.
- **Layers panel** — add (random vivid color), select, delete, reorder
  (up/down), and per-layer visibility toggle.
- **Properties panel** — the selected layer's name and color, plus a value slider
  (edits re-key at the playhead) and add-keyframe button per property.
- **Shell** — Prism dark theme, Phosphor icon glyphs, `.pulse` save (serde JSON
  via a native file dialog), and a `prism-core` dependency for the shared color
  boundary.

[Unreleased]: https://github.com/prism-suite/pulse/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/prism-suite/pulse/releases/tag/v0.0.1
