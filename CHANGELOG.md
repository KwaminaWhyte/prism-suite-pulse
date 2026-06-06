# Changelog

All notable changes to **Pulse** (the After Effects analog, app #3 of the Prism
suite) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Spatial effects** (After-Effects blur / stylize / perspective staples) — the
  first **whole-buffer** (multi-pixel) effects, beyond the per-pixel
  color-correction stack: **Gaussian Blur**, **Drop Shadow**, and **Glow**.
  - **`SpatialEffect`** — a per-layer `Vec<SpatialEffect>` of pure passes that
    read *neighbouring* pixels (convolve / offset / bloom) and so operate on the
    layer's **isolated, premultiplied, linear-light** RGBA buffer rather than one
    pixel at a time. `serde`-defaulted to empty so pre-spatial-effect `.pulse`
    files still load.
    - **Gaussian Blur** — a separable Gaussian with independent X/Y blurriness
      (sigma, comp px) and a **Repeat Edge Pixels** toggle (clamp the kernel to
      the edge vs. fade to transparent at the frame border).
    - **Drop Shadow** — a blurred, tinted copy of the layer's alpha offset by a
      **distance** at an **angle**, composited behind the layer at **opacity**,
      with a **softness** (blur) and a **Shadow Only** mode.
    - **Glow** — extracts the layer's bright areas above a **threshold**, blurs
      them by a **radius**, and screens the bloom back over the layer at an
      **intensity**, blooming the highlights and extending the glow past the edge.
  - **Pure convolution core** — `gaussian_kernel` (normalized, symmetric,
    `3·sigma` half-width; identity at sigma ≤ 0), a separable `gaussian_blur`
    (horizontal-then-vertical, premultiplied so soft edges don't bleed the quad
    color, with optional edge clamp), and the drop-shadow / glow builders, plus
    `apply_spatial_effects` (in-order stack) — all unit-tested: kernel
    normalization/symmetry/identity, alpha-mass conservation, no-color-bleed,
    zero-sigma no-op, shadow offset + shadow-only, glow brightening vs. an inert
    below-threshold glow, stack ordering, degenerate-buffer safety, and the serde
    default.
  - The software compositor (`render.rs`) runs the spatial stack on the layer's
    isolated buffer **after** its color-correction effects, masks, and track
    matte (a zero-conversion bridge — the compositor's accumulator is already
    premultiplied linear-light), then composites the filtered buffer over the
    frame. Crisp solids gain the isolated-buffer path only when they carry
    spatial effects, so un-effected layers stay byte-identical; motion-blurred
    layers route through the same path so blur/shadow/glow compose with the
    shutter, masks, and mattes. New compositor tests cover edge-softening,
    drop-shadow coverage + darkness in the composite, glow brightening, and the
    identity-blur byte-for-byte equivalence.
  - UI: a new **Spatial effects** section in the Properties panel (add via menu,
    reorder, remove, per-parameter sliders / color picker), shown for layers that
    draw their own pixels. The launch demo's satellite now ships a soft drop
    shadow plus a glow so the buffer passes read out of the box.
- **Masks** (After-Effects layer-mask parity) — a layer can now be carved by one
  or more closed Bézier mask paths instead of always compositing as a full quad.
  - **`Mask`** — a closed path of **`MaskVertex`** (anchor + in/out Bézier
    tangent handles, layer-local comp px) with a [`MaskMode`], an **invert**
    toggle, **opacity**, **feather** (soft edge, px), and **expansion** (signed
    offset that grows/shrinks the shape). Masks live in the layer's local frame
    (the same space the layer's quad lives in), so a mask rides the layer's
    position / scale / rotation / parenting for free. `Mask::rect` and
    `Mask::ellipse` (with the standard `k ≈ 0.5523` circle handles) seed the two
    default shapes. `serde`-defaulted to an empty list so pre-mask `.pulse` files
    still load unmasked.
  - **`MaskMode`** — the After-Effects boolean modes **Add** (union), **Subtract**
    (knockout), **Intersect** (overlap), **Difference** (symmetric difference),
    and **None** (disabled). `MaskMode::combine` folds each mask's coverage into a
    running accumulator top-down; the topmost mask composites against an empty
    base (so an Add reveals exactly its shape), matching AE.
  - **Pure geometry** — `Mask::flatten` subdivides each cubic segment into a
    polygon; `point_in_polygon` (even-odd ray cast), `dist_to_polygon`
    (nearest-edge distance), `Mask::coverage_at` (signed-distance → expansion →
    feather ramp → invert → opacity), and `mask_stack_coverage` (the folded
    per-pixel multiplier, returning full coverage when no mask is active) are all
    unit-tested — square/concave inside tests, edge distances, hard vs feathered
    coverage, inversion, expansion grow/shrink, opacity scaling, the smooth
    ellipse, every mode's algebra, and an Add-then-Subtract stack punching a hole.
  - The software compositor (`render.rs`) renders a masked solid into an isolated
    linear-light buffer, then `apply_masks` inverse-maps each comp pixel back into
    layer-local space and multiplies its alpha by the folded mask coverage — color
    is never touched, only coverage — before any track matte and the source-over.
    Motion-blurred and track-matted layers both route through the same masked
    path, so masks compose with blur and mattes. New compositor tests cover
    shape-clipping, inversion keeping the outside, a disabled mask being
    byte-identical to unmasked, color preservation, feather edge-softening, the
    Add/Subtract hole, masks following the layer transform, and mask + track
    matte together.
  - The egui preview draws the selected layer's mask outlines (flattened through
    its world matrix), dimming subtractive/inverted masks so the carve reads at a
    glance. A new **Masks** section in the Properties panel adds rectangle /
    ellipse masks and edits each one's name / mode / invert / opacity / feather /
    expansion, with reorder + remove. The launch demo's solid now ships a soft
    elliptical mask so masks read out of the box.
- **Motion blur** (After-Effects shutter parity) — fast-moving layers can now be
  rendered with a cinematic shutter instead of a crisp per-frame snapshot, the
  Phase-4 motion feature.
  - **`MotionBlur`** — a per-composition shutter model: a master `enabled`
    switch, a **shutter angle** (degrees: 360° = a whole frame of blur, 180° =
    half — the default), a **shutter phase** (degrees: where the open window sits
    relative to the frame; `-angle/2` centers it), and a **sample** count (1–64,
    default 16). The pure `MotionBlur::shutter_window` (the open `[open, close]`
    time interval for a frame) and `MotionBlur::sample_times` (the evenly-spread,
    midpoint-sampled, symmetric sub-frame times across that window) are
    unit-tested, including angle→width, phase centering, count clamping, and the
    single-sample-at-center degenerate case. `serde`-defaulted to **off** so
    pre-motion-blur `.pulse` files still load crisp.
  - **Per-layer switch** — each layer carries a `motion_blur` flag (After
    Effects' layer MB toggle); a layer is blurred only when both it and the
    comp's master switch are on (`Comp::layer_motion_blurred`, unit-tested).
    `serde`-defaulted to `false`.
  - The software compositor (`render.rs`) renders a motion-blurred solid as the
    **average of its sub-frame snapshots**: each shutter sample rasterizes the
    layer's resolved world transform at that instant into a scratch buffer, and
    the snapshots are integrated component-wise in the compositor's premultiplied
    linear-light space (so partly-covered edges average their coverage without
    bleeding the quad color into the transparent samples) before being matte-
    clipped and composited over the accumulator. Crisp (un-blurred) layers keep
    the exact prior direct-composite path, so existing frames are byte-identical;
    track mattes still clip the integrated coverage. New compositor tests cover
    edge-softening vs. the crisp render, no-color-bleed, the master-switch and
    per-layer gates (both must be on; output is byte-identical to crisp when
    either is off), and matte-clipping under blur.
  - The egui preview hints at the motion with faint **ghost quads** drawn at the
    shutter sample times (capped to ~8, each at `1/n` opacity so they sum to
    roughly one solid's coverage) behind the layer — a cheap, legible
    approximation of the per-pixel integral the offline render does.
  - UI: a new **Comp ▸ Motion blur** menu (master enable + angle / phase /
    samples sliders, the shutter controls disabled while off) and a per-layer
    **Motion blur** checkbox in the Properties panel (with a "(comp switch off)"
    hint when the master is disabled). The launch demo enables comp motion blur
    and opts its sliding/spinning solid in, so the shutter reads out of the box.
- **Track mattes** (After-Effects compositing parity) — a layer can now borrow
  the layer **directly above it** in the stack to define its own per-pixel
  transparency, instead of every layer compositing in isolation.
  - **`MatteMode`** — a per-layer matte selector with the After-Effects modes:
    **Alpha** (visible where the source is opaque), **Alpha inverted** (visible
    where it's transparent), **Luma** (visible where the source is bright,
    weighted by Rec.709 luma in linear light), and **Luma inverted**. The pure
    `MatteMode::factor` (straight-linear RGBA → a clamped `[0,1]` multiplier) and
    the stack-relationship helpers `Comp::matte_source` / `Comp::is_matte_source`
    are all unit-tested. `serde`-defaulted to `None` so pre-matte `.pulse` files
    still load.
  - When a layer's matte is active, the layer above becomes its **matte source**:
    that source is removed from normal compositing (matching AE) and instead
    multiplies the matted layer's alpha — color is never touched, only coverage.
    The software compositor (`render.rs`) renders the matted layer and its source
    into isolated linear-light buffers, applies the matte factor per pixel, then
    composites the result source-over; the egui preview honors mattes coarsely
    (the matte source is hidden and the matted layer's opacity is scaled by the
    source's flat-color factor — the preview's constant quads can't do per-pixel
    mattes). A new **Track matte** picker in the Properties panel (disabled, with
    a hint, when no layer sits above) drives it, and the Layers panel marks a
    layer that is being consumed as a matte source. New compositor tests cover
    matte-source suppression, alpha clipping, inversion as the complement, luma
    alpha-scaling, and base-color preservation.
- **Layer types + effect stack** (After-Effects layer-kind & color-correction
  parity) — layers are no longer all solids: each carries a **kind** and an
  ordered, non-destructive **effect stack**.
  - **Layer kinds** (`LayerKind`) — **Solid** (draws its colored quad, as
    before), **Null** (an invisible reference layer that renders nothing but
    whose transform is real — a controllable parent/pivot handle, shown as a
    small pivot cross in the preview), and **Adjustment** (draws nothing of its
    own; its effect stack regrades the composite of every layer *below* it,
    within the layer's transformed bounds, blended by its opacity — AE's
    adjustment layer). New layers of any kind are added from *Layer ▸ New*; the
    kind is switchable per-layer in the Properties panel. `serde`-defaulted to
    `Solid` so pre-kind `.pulse` files still load.
  - **Effect stack** (`Effect`) — a per-layer `Vec<Effect>` of pure
    color-correction passes evaluated in **linear light** (alpha preserved),
    stacking in order. Ships the After-Effects staples: **Tint** (luminance map
    between two colors with an amount mix), **Brightness & Contrast** (offset +
    pivot about 0.5), **Exposure** (stops + offset + gamma), and **Levels**
    (input/output black & white + midtone gamma). Edited in a new **Effects**
    section of the Properties panel (add via menu, reorder, remove, per-parameter
    sliders / color pickers). For a solid the stack processes the layer's own
    color; for an adjustment it reprocesses the layers beneath.
  - The software compositor (`render.rs`) and the egui preview both honor kinds
    and effects: nulls are skipped, solids composite their effect-processed
    color, and adjustments inverse-map their quad to regrade covered (non-
    transparent) pixels in place — so exported frames match the preview. The
    launch demo now ships a full-frame **Adjustment** layer applying a punchy
    Levels grade over the parented solid pair. All new pure logic — each effect's
    transfer function, alpha preservation, stack ordering, kind-dispatch, and
    adjustment quad-bounds / transparency handling — is unit-tested.
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
