# Pulse

Motion-graphics / compositing app — the After Effects analog and **app #3 of the
Prism creative suite** (sibling to [Pigment](https://github.com/KwaminaWhyte/prism-suite-pigment), the raster editor, and
[Contour](https://github.com/KwaminaWhyte/prism-suite-contour), the vector editor).

Built in Rust with [`eframe`](https://github.com/emilk/egui)/`egui` 0.34 (glow
backend). The preview is drawn through egui's `Painter` — no custom GPU pass
needed. The composition serializes with `serde`.

## Status — v0 scaffold

Real but scoped. It builds, launches, and lets you animate solid layers over a
timeline: keyframe transforms, scrub/play the playhead, and watch the preview
update live.

**Implemented**

- **Motion model** — a `Comp` (width, height, duration, fps) holding an ordered
  stack of `PulseLayer`s. Each layer is a solid color plus five animatable
  properties (`x`, `y`, `scale`, `rotation`, `opacity`), each stored as a
  `Track` of `Keyframe`s.
  - Sampling: per-keyframe interpolation — **linear**, **hold** (stepped), or a
    temporal cubic-**Bézier ease** (After-Effects style: Easy Ease / Ease In /
    Ease Out, with a Newton-solved CSS-`cubic-bezier` curve) — applied to the
    segment leaving each key; constant hold before the first / after the last
    key; empty track → the property default
    (`x=0, y=0, scale=1, rotation=0, opacity=1`). Covered by unit tests.
- **Preview** (central panel): the comp as a centered, aspect-fit frame; every
  visible layer drawn as a rotated/scaled solid quad at its interpolated `(x, y)`,
  faded by opacity, for the current playhead time.
- **Timeline** (bottom panel): a per-second time ruler, one lane per layer with
  keyframe diamonds (union of all five tracks), a draggable accent playhead, and
  transport (go-to-start, play/pause, go-to-end). Play advances by real `dt` each
  frame and loops at `duration` (drives `ctx.request_repaint()`); click/drag the
  ruler to scrub. **Space** toggles playback.
- **Properties** (right panel): the selected layer's name + color, then each of
  the five properties with a value slider (edits re-key the value at the
  playhead) and an "add keyframe @ playhead" button, plus a per-property keyframe
  count. When the playhead sits on a keyframe, an **interpolation picker**
  (Linear / Hold / Easy Ease / Ease In / Ease Out) sets that key's outgoing
  easing. Timeline markers reflect the mode (diamond = linear, square = hold,
  circle = ease).
- **Graph editor** (bottom panel, toggled against the timeline): an
  After-Effects-style **value-curve** view of the selected layer. Plots each
  animated property as a value-over-time curve on a shared, auto-framed value
  axis, with **draggable keyframes** (retime + revalue, with live re-sorting) and
  **draggable Bézier ease handles** (drag a handle to shape a segment's easing;
  dragging on a Linear/Hold segment promotes it to an editable ease). Per-property
  show/hide chips choose which curves are plotted.
- **Layers** (left panel): add (random vivid color), select, delete, reorder
  (up/down), and per-layer visibility toggle.
- **Menus**: File (New, Save `.pulse` → JSON via `serde` + `rfd` save dialog,
  Export stub), Layer (add / delete).

**Out of scope for v0** (noted): undo/redo, per-layer source media (layers are
solids), masks, effects, real frame rendering/export, and multi-select. The graph
editor covers draggable keyframes and per-key Bézier ease handles; auto-Bézier and
separate value/speed-graph modes are still to come.

## Shared foundation

Pulse depends on the suite's shared crate **`prism-core`** by path
(`../crates/prism-core`) to demonstrate the shared-foundation model:

- `prism_core::Size` — available for logical comp dimensions.
- `prism_core::color::{srgb_to_linear, linear_to_srgb}` — used at the color
  encode boundary when painting layer swatches.

`prism-core` declares `[lints] workspace = true`, so Pulse's workspace mirrors
Pigment's `[workspace.lints]` block (and adds `[lints] workspace = true` to the
app crate); otherwise building it here errors on an undefined `workspace.lints`.

## Build & run

```sh
# from prism/pulse
cargo run        # launches the Pulse window
cargo build      # debug build
cargo test       # track-sampling unit tests
cargo fmt        # formatting (clean)
cargo clippy     # lints (clean)
```

Binary name: `pulse` (crate `pulse-app`).

## Layout

```
pulse/
├── Cargo.toml                  # workspace + shared lint config + prism-core path dep
└── crates/pulse-app/
    └── src/
        ├── main.rs             # eframe entry point
        ├── app.rs              # comp state, transport, panels, menus, per-frame loop
        ├── comp.rs             # Keyframe/Track/PulseLayer/Comp model + sampling
        ├── preview.rs          # composition + layer painting via egui Painter
        ├── timeline.rs         # ruler, lanes, keyframe diamonds, playhead, scrub
        ├── graph.rs            # value-curve graph editor: drag keys + ease handles
        ├── theme.rs            # Prism dark theme
        └── icons.rs            # egui-phosphor install + action glyphs
```
