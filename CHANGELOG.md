# Changelog

All notable changes to **Pulse** (the After Effects analog, app #3 of the Prism
suite) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Anchor point + layer parenting** (After-Effects transform parity) — the
  transform model is now a composed 2-D affine chain instead of an ad-hoc inline
  rotate/scale, bringing two AE staples online.
  - **Anchor point** — two new animatable properties (`Anchor X` / `Anchor Y`,
    comp-px offset from the layer's geometric center). The anchor is the pivot
    that scale and rotation happen about, and the layer-local point aligned to
    `(X, Y)` position — built as `Translate(pos)·Rotate·Scale·Translate(-anchor)`,
    the standard AE transform order. The default `(0, 0)` keeps a layer pivoting
    about its center, so existing comps render identically.
  - **Parenting** — a layer can be **parented** to another (`Parent` pick-whip in
    the Properties panel); the child inherits the parent's full transform
    (position, scale, rotation, anchor) but **not** opacity, matching AE. Parent
    references survive layer delete / reorder (indices are fixed up; orphaned
    children are unparented), and the picker only offers cycle-free targets.
  - **`Affine2`** — a 2-D affine matrix (translate / scale / rotate / compose /
    apply / invert) plus `Transform::local_matrix`, `Comp::world_matrix`
    (parent-chain composition with a cycle guard), and `Comp::can_parent`
    (self/missing/descendant rejection) — all unit-tested. The preview and the
    software compositor (`render.rs`) both rasterize through the resolved world
    matrix (inverse-mapping each pixel for coverage), so offline frames match the
    on-screen preview. The launch demo now ships a satellite layer parented to
    the sliding solid to showcase it.
- **PNG image-sequence export** — File ▸ *Export PNG sequence…* renders the whole
  composition to a folder of numbered PNGs (`comp_0000.png`, `comp_0001.png`, …),
  one file per frame across the comp's `[0, duration]` timeline at its fps
  (replacing the old export stub). Frame count / errors are surfaced in the menu
  bar and logged; the folder is picked via a native dialog (`rfd`).
- **Software compositor** (`render.rs`) — a pure, headless CPU rasterizer that is
  the offline twin of the egui preview: `render_frame(comp, t)` produces a
  native-resolution 8-bit sRGB RGBA `Frame` by inverse-transform sampling each
  visible layer's solid quad (position, uniform scale, rotation about center,
  opacity — the same transform model the preview uses) and compositing
  **source-over in linear light** through `prism-core`'s color boundary
  (`srgb_to_linear`/`linear_to_srgb`), so exported frames match the preview. Ships
  with `frame_count` / `frame_time` / `frame_path` sequence math (frame-inclusive
  duration, zero-padded names) and `export_sequence` to drive it — all
  unit-tested (rasterization, source-over blend, opacity/position/scale/rotation
  coverage, sequence counts, and a round-trip that writes real PNGs).
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
