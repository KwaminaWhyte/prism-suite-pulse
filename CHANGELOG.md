# Changelog

All notable changes to **Pulse** (the After Effects analog, app #3 of the Prism
suite) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Window-menu panel show/hide** (After-Effects *Window* / Affinity *View ▸
  Studio* parity) — the shell's dockable panels can now be **hidden and shown**
  from a new **Window** menu, so the user can reclaim screen for the panels they
  need instead of living with the fixed four-panel layout.
  - **`PanelVisibility`** (`app/workspace.rs`) — the pure state behind the menu:
    which of the three dockable panels (**Layers** / **Properties** /
    **Timeline**) are shown. The central **Preview** viewport is deliberately
    *not* toggleable — it is the comp canvas and always fills whatever space the
    side/bottom panels leave, so hiding it would be meaningless. Defaults to all
    panels shown (the classic workspace), and the app gates each panel's render
    on its flag this frame.
  - **Window menu** — a checkbox per dockable panel (live with its current
    state), plus **Show all panels** (restore the default workspace, disabled
    when nothing is hidden) and **Hide all panels** (leave only the preview
    viewport — a quick "maximize the canvas", disabled when already preview-only).
  - **Pure logic** — `is_shown` / `set` / `toggle` / `shown_count` / `all_hidden`
    / `show_all` / `hide_all`, plus a `Panel` enum (`ALL` in menu order, with
    distinct labels) — all unit-tested: the all-shown default, single-panel
    toggle isolation + round-trip, idempotent `set`, `hide_all` ⇒ `all_hidden`,
    `show_all` restoring the default, and the `ALL`-list/label invariants.

- **On-canvas transform gizmo** (After-Effects / Affinity selection-handle
  parity) — the selected layer can now be **moved, scaled, rotated, and
  re-anchored directly in the preview**, instead of only nudging the Properties
  sliders.
  - **Handles** — a bounding box around the layer's base quad with four **corner
    scale handles**, a **rotation knob** on a connector above the top edge, an
    **anchor-point cross** at the scale/rotation pivot, and the box **body** for
    move. Each handle highlights on hover and while held, with a matching cursor
    (move / grab / crosshair / resize).
  - **Drag → keyframe** — a drag edits the layer's local transform properties
    (`X`/`Y`, uniform `Scale`, `Rotation`, `Anchor X`/`Anchor Y`) and **keys the
    new value at the playhead** via the same `set_key` re-key convention the
    sliders use, so direct manipulation and animation stay consistent. Dragging
    the anchor moves the pivot while compensating position so the layer doesn't
    visually jump (matching AE).
  - **Parent-aware math** (`gizmo.rs`) — because position/scale/rotation are
    applied in the layer's **parent space** (`world = parent_world · local`),
    every drag maps the pointer screen → comp → **parent-local** before taking
    the delta, so a parented layer's handles drag correctly under the parent's
    rotation/scale. Scale is the distance ratio about the anchor; rotation is the
    signed angle swept about the anchor (normalized across the `atan2` branch cut
    so a small drag never jumps ~360°).
  - **Pure logic** — `GizmoGeom::build` (the box/anchor/knob geometry from the
    resolved world matrix), `screen_to_comp`, `parent_matrix`, `hit_test`
    (knob → corners → anchor → body precedence, even-odd box interior), and
    `drag` (handle + grab transform + pointer delta → the property values to key)
    are all unit-tested: screen↔comp round-trip, move adding the parent-local
    delta (incl. under a rotated parent), scale distance ratio + non-negative
    clamp + degenerate grab-on-pivot, rotation sweep + branch-cut normalization,
    anchor move + position compensation + scale-undo, the key-list skipping
    untouched props, handle-precedence hit-testing, and the demo-layer geometry
    overlay.

- **Per-layer blend modes** (After-Effects blending-mode parity) — every layer
  now carries a **blend mode** that decides how its pixels combine with the
  composite beneath it, the same 18-mode set the suite already shares.
  - **Shared mode set** — reuses `prism-core`'s [`BlendMode`] (Normal, Multiply,
    Screen, Overlay, Darken, Lighten, Color Dodge, Color Burn, Hard Light, Soft
    Light, Difference, Exclusion, Linear Dodge (Add), Linear Burn, and the four
    non-separable HSL modes Hue / Saturation / Color / Luminosity), so a Pulse
    layer's blend mode round-trips by the same stable numeric id Pigment writes.
    A new [`LayerBlend`] wrapper field on every layer is `serde`-defaulted to
    **Normal**, so pre-blend-mode `.pulse` files still load and render
    byte-identically.
  - **CPU blend math** (`comp/blend.rs`) — a self-contained, pure-Rust
    `blend_over(mode, src, dst)` implementing the W3C blend+composite model on
    the compositor's **straight, linear-light** RGBA: the separable per-channel
    formulas plus `set_lum` / `set_sat` / `clip_color` for the HSL modes mirror
    Pigment's `composite.wgsl` `blend_fn` (same cases, same `lum` weights) so the
    suite shares one definition of each mode. Blending happens in **linear light**
    (AE's linearized/32-bpc model) and is weighted by the backdrop's alpha, so it
    only takes hold where there is something beneath; Normal reduces exactly to
    source-over.
  - **Compositor wiring** — the software renderer composites each finished layer
    buffer onto the accumulator through the layer's blend mode; a solid with a
    non-Normal mode now routes through the isolated-buffer path (alongside
    masks / track mattes / spatial effects) so the blend is applied buffer-to-
    backdrop. Blend modes compose with masks, mattes, spatial effects, and motion
    blur, and apply to solid / shape / text layers (a null draws nothing; an
    adjustment grades in place).
  - **UI** — a **Blend** dropdown in the Properties panel (separable group, then
    a divider, then the HSL group) for any layer that draws its own pixels, plus
    a compact blend badge in the **Layers panel** that appears on any layer with
    a non-Normal mode (hover shows the mode name). The launch demo's star uses
    **Screen** so the feature reads out of the box.
  - **Tests** — the blend math is unit-tested (Normal == source-over, Multiply
    darkens, Screen lightens, Add clamps, Difference, the HSL hue/luma
    constructions, and the "no backdrop / transparent source" identities); the
    renderer's blend path is integration-tested (Normal byte-identical to the
    default, Multiply/Screen shift the overlap, blend is a no-op over an empty
    backdrop, and old projects load as Normal).
- **Text layers** (After-Effects text-layer parity) — a new layer kind that
  draws a **string** with a self-contained, dependency-free **stroke vector
  font**, the second layer type whose pixels come from authored geometry rather
  than a swatch (after shape layers).
  - **`LayerKind::Text`** — joins Solid / Shape / Null / Adjustment. A text layer
    carries a [`TextLayer`] (a `serde`-defaulted `text` field on every layer, so
    pre-text `.pulse` files still load) — a string plus type settings (**font
    size**, **tracking**, **leading**, **alignment**) and an optional **fill** /
    **stroke** (reused from the shape system). New text layers are added from
    *Layer ▸ New* (seeded with the word "TEXT" in the layer's color), and the
    kind is switchable per-layer in the Properties panel like any other.
  - **Built-in stroke font** — every printable character (A–Z, 0–9, space, and
    common punctuation `. , ! ? - _ : / + =`) maps to a small set of **polyline
    strokes** authored once on a unit em grid; letters are uppercased and laid
    out in a monospace cell, unknown printables fall back to a box, and
    control/space characters draw nothing. There is **no font dependency** — the
    font is intentionally simple so the feature is self-contained and
    deterministic. Per-character animators and real OpenType/variable fonts are a
    later step.
  - **Layout + coverage** — `TextLayer::segments` lays the string out into
    layer-local stroke segments (multi-line, vertically centered, per-line
    left/center/right aligned, with each glyph ink centered in its advance cell);
    `TextLayer::coverage_at` rasterizes a glyph as a **thickened pen band** around
    the nearest stroke (the fill body), with an optional **outline stroke** band
    straddling the body edge composited over it, antialiased over a ~1 px ramp via
    the mask system's segment-distance geometry (`dist_to_segment`, now public).
    `TextLayer::local_bounds` (pen/stroke-padded AABB) bounds the rasterizer. All
    pure logic is unit-tested: layout centering/alignment/tracking/leading, the
    space-advances-without-strokes case, unknown-char fallback, case-insensitive
    glyphs, on-stroke vs far coverage, fill opacity, stroke-over-fill outline,
    size-scaled bounds, and the serde round-trip.
  - The software compositor (`render.rs`) rasterizes a text layer into an
    **isolated, premultiplied linear-light** buffer (`composite_text`, the mirror
    of `composite_shape`), then runs the same **masks**, **track matte**, and
    **spatial-effect** passes a solid/shape does before compositing source-over —
    so text composes with masks, mattes, blur/shadow/glow, and **motion blur**
    (the shutter integrator dispatches to the text rasterizer per sub-frame, and a
    text layer can serve as a track-matte source). The per-layer isolated-buffer
    finish (mask → matte → spatial → over) was factored into a shared
    `finish_layer` helper used by the solid / shape / text paths. The egui preview
    paints each glyph stroke as a thick line through the layer's world matrix
    (fill body, with the outline stroke drawn thicker underneath). New compositor
    tests cover glyph coverage + fill color, the blank-text no-op, opacity, mask
    composition, the stroke outline band, motion-blur footprint widening, a text
    **luma matte** driving a solid, and the pre-text serde default.
  - UI: a new **Text** section in the Properties panel (a multi-line text editor,
    size / tracking / leading sliders, an alignment picker, and fill / stroke
    toggles with color + opacity/width), shown for text layers. The launch demo
    now ships a centered **PULSE** title text layer that fades up over the first
    second with a blue outline, so text layers read out of the box.
- **Shape layers** (After-Effects shape-layer parity) — a new layer kind that
  draws **parametric vector shapes** instead of a fixed solid quad, the first
  layer type whose pixels come from authored geometry rather than a swatch.
  - **`LayerKind::Shape`** — joins Solid / Null / Adjustment. A shape layer
    carries a [`ShapeLayer`] (a `serde`-defaulted `shape` field on every layer,
    so pre-shape `.pulse` files still load) — an ordered stack of [`ShapeItem`]s
    composited bottom-up. New shape layers are added from *Layer ▸ New* (seeded
    with a filled rectangle in the layer's color), and the kind is switchable
    per-layer in the Properties panel like any other.
  - **`ShapePrimitive`** — four parametric primitives, each centered at its
    local origin and flattened to a closed layer-local polygon: **Rectangle**
    (with an optional corner **radius** → rounded rect, each corner a
    quarter-arc), **Ellipse** (sampled smooth), **Polygon** (a regular
    `points`-gon, first vertex straight up), and **Star** (`points`-pointed,
    alternating **outer**/**inner** circumradii). Degenerate parameters
    (non-positive size, <3 polygon/star points) flatten to nothing.
  - **`Fill` + `Stroke`** — each item has an optional solid **fill** (straight
    sRGB color + opacity) and an optional **stroke** (color + width centered on
    the path + opacity). Coverage is antialiased over a ~1 px ramp using the
    nearest-edge signed distance (reusing the mask system's even-odd
    point-in-polygon + segment-distance geometry); the stroke is a band of
    `width` straddling the boundary, composited over the fill, so an item reads
    as a filled-then-outlined shape.
  - **Pure geometry** — `ShapePrimitive::flatten` (per-primitive polygon),
    `ShapeItem::polygon` (offset into layer-local space), `item_coverage`
    (fill + stroke straight-RGBA at a point), `ShapeLayer::coverage_at` (the
    bottom-up item stack), and `ShapeLayer::local_bounds` (stroke-padded AABB to
    bound the rasterizer) — all unit-tested: corner extents, rounded-corner
    clipping, ellipse/polygon/star inside-outside + vertex radii, degenerate
    emptiness, fill coverage/opacity/edge-AA, stroke-only band + stroke-over-fill,
    bottom-up stacking, stroke-padded bounds, and the serde round-trip.
  - The software compositor (`render.rs`) rasterizes a shape layer into an
    **isolated, premultiplied linear-light** buffer (bounding the pixel loop to
    the shape's transformed extent), then runs the same **masks**, **track
    matte**, and **spatial-effect** passes a solid uses before compositing
    source-over — so a shape composes with masks, mattes, blur/shadow/glow, and
    **motion blur** (the shutter integrator dispatches to the shape rasterizer
    per sub-frame). The egui preview paints each item's flattened polygon through
    the layer's world matrix (fill via the tessellator, stroke as a closed
    outline). New compositor tests cover fill coverage + color, the empty-shape
    no-op, opacity, ellipse corner-clipping, mask composition, the unfilled
    stroke outline, motion-blur footprint widening, and the pre-shape serde
    default.
  - UI: a new **Shape** section in the Properties panel (an "Add" menu for the
    four primitives, then per-item primitive sliders, local offset, and fill /
    stroke toggles with color + opacity/width), shown for shape layers. The
    launch demo now ships a stroked five-point **Star** shape layer sliding and
    rotating across the frame (graded by the adjustment layer above it) so shape
    layers read out of the box.
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
