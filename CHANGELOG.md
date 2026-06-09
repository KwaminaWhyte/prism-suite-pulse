# Changelog

All notable changes to **Pulse** (the After Effects analog, app #3 of the Prism
suite) are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Real outline fonts + font-family selection for text layers** (After Effects'
  *Character* panel font picker; PLAN Phase 3 *Text — real font shaping*) — text
  layers can now render with **real TrueType glyph outlines** from a chosen font
  family, not just the built-in stroke font. A new **Font** dropdown in the Text
  Properties section lists the bundled default ("Built-in stroke font") plus every
  installed system family (enumerated via the pure-Rust `fontdb` stack); picking a
  family lays the string out with that face's advance metrics + glyph contours
  (`ttf-parser`) and rasterizes **filled, antialiased** glyphs by **reusing the
  Shape layer's even-odd polygon fill** (`point_in_polygon` + nearest-edge AA), so
  glyph holes (`o`, `A`, `8`) are carved out correctly and text composes with the
  full pipeline (masks / mattes / effects / motion blur) exactly like the stroke
  font. Loaded faces are **cached** per family (a font file is read at most once,
  never per frame), and an unknown / unloadable family **falls back** to the
  bundled Ubuntu Light face so text never vanishes.
  - **Back-compat is preserved exactly:** a new `font_family: Option<String>`
    field (`#[serde(default)]` → `None`) keeps the **built-in stroke font** as the
    default. `None` — the default for new layers *and* every legacy `.pulse` file,
    which carries no `font_family` key — renders **identically** to before; only a
    selected `Some(family)` switches to outlines. Serde round-trips the new field,
    and a legacy text layer with no key deserializes to `None` → the stroke font.
  - Pure logic is unit-tested: family enumeration (default first, de-duplicated),
    resolution + fallback (unknown → bundled, cached/shared bytes), `Some` vs
    `None` selecting the outline vs stroke path, outline layout width monotonic +
    deterministic + size-proportional, even-odd hole carving, and the serde /
    legacy round-trips. *Still open:* variable-font axes, weight/style
    sub-selection, kerning / full OpenType shaping, and per-character animators.
- **RAM-cache fill indicator** (After Effects' *RAM Preview* green cache bar; PLAN
  Phase 6 *Caching*) — a subtle **"caching… N%"** readout now appears in the
  timeline transport row while the **RAM-preview cache** is filling, so the
  first-pass lag the playhead shows (the CPU compositor renders each work-area
  frame **once** off the UI thread, then the loop replays cached frames in real
  time) is legible rather than mysterious. The readout reads a new
  `PreviewRenderer::cache_progress()` accessor (`(cached, total)` work-area frames)
  and **disappears once the comp is fully cached** — from then on the loop plays
  back smoothly straight from RAM. This makes the *expected* "laggy first play,
  smooth on loop" behaviour self-explanatory in the UI; the underlying pacing
  (playback gated by the cache via `is_frame_ready` / `fully_cached`) is unchanged.
  `cache_progress` is unit-tested (0 frames before a comp is registered, climbing
  to `total` exactly as `fully_cached` turns true).

### Fixed

- **Guard: a text layer's position is preserved across a font change** (regression
  test). A sibling app surfaced a bug where editing a text's font family / size /
  alignment reset its position to the top-left corner. Pulse is **not** affected —
  a text layer's position / anchor / scale / rotation live on the layer's
  animatable transform tracks, wholly separate from the glyph buffer, and both
  font paths (built-in stroke `None` ↔ real outline `Some(family)`) lay the block
  out **centered on the layer-local origin**, so the text rides the same anchor
  regardless of font. Added a unit test
  (`font_family_change_preserves_text_layer_transform_and_anchor`) that places /
  moves a text layer, switches font family (and size / align), and asserts every
  transform component and the anchor's comp-space landing point are unchanged —
  and that both font paths stay centered on the origin — locking this in.

## [0.2.0] - 2026-06-09

### Added

- **Cell Pattern generator (Voronoi)** (After Effects' *Generate ▸ Cell Pattern*;
  PLAN *Generate* item) — a new **cellular / Voronoi** generate effect, added as a
  `CellPattern` variant on the established generate infrastructure (the
  `GenerateEffect` enum + `composite_generate` pass + Properties *Generate* section
  with its generator picker + the *Effects & Presets* browser's *Generate*
  category), mirroring the Fractal Noise / Ramp / Checkerboard / 4-Color / Grid
  family exactly. Seed-hashed feature points sit one per cell on a jittered grid
  (displaced by **Disorder**); each output pixel finds the nearest (**F1**) and
  second-nearest (**F2**) feature point and shapes them by a **cell type** —
  **Bubbles** (smooth rounded F1), **Crystals** (raw faceted F1), **Plates** and
  **Static Plates** (flat per-cell tones, the latter not evolving), and
  **Borders** (the `F2 − F1` cell web, bright along the boundaries). It exposes
  **Size** (cell scale), **Contrast / Brightness**, optional **Invert**, a
  **Seed**, and — like Fractal Noise — a keyframable **Evolution** axis reusing
  the same evolution track, so the cells flow over time. Output is
  **grayscale-linear** (like Fractal Noise), deterministic for a given
  `(seed, evolution, pixel)`. The `CellPattern` variant is an additive enum
  variant (and `generate` stays `serde`-defaulted), so older `.pulse` projects
  round-trip unchanged. Determinism, the F1-zero-at-a-feature-point / borders web /
  invert symmetry / per-type difference / output range math, the evolution track
  driving the field, and the browser entry are unit + render-path tested.

- **Null layers + transform parenting (pick-whip)** (After Effects' *null object*
  + *parent & link*; PLAN Phase 2 *Null / parenting*) — a layer can now be
  **parented** to another so it inherits that parent's full transform up the
  chain, driven from a per-layer **Parent** pick-whip in the Properties panel
  (a combo of *None* + every layer that can legally be a parent), with the
  transform-only **Null** layer kind (`LayerKind::Null`) available from *Layer ▸
  New ▸ Null* as a non-rendering pivot / rig handle. A child's **world** matrix
  folds its own local transform under every ancestor's (`Comp::world_matrix`,
  parent applied outermost, so translate / rotate / scale all inherit), and the
  pick-whip only offers safe targets via a **cycle guard** (`Comp::can_parent`
  rejects self-parenting, missing layers, and descendant links that would loop —
  `world_matrix` itself also breaks any corrupt cycle with a bounded walk).
  Parent references are kept consistent when layers are deleted or reordered
  (children of a removed layer unparent; indices shift to follow swaps). The
  `parent` field is `serde`-defaulted to `None` and `Null` is an additive
  `LayerKind` variant, so older `.pulse` projects round-trip unchanged.
  Parent-chain composition, the self / mutual / descendant cycle guards, Null
  creation, parent assign → clear, and serde back-compat are unit-tested.

## [0.1.0] - 2026-06-09

### Fixed

- **Playback no longer races the preview (real-time, cache-gated)** — advancing
  the playhead by wall-clock time while the slower CPU render lagged behind made
  the timeline bar run "way ahead" of a janky preview that dropped frames.
  Playback now advances at **real time but gated by the RAM-preview cache** (see
  *Added*): the playhead steps by the frame's `dt` only when the target frame is
  already cached (or the whole work area is), otherwise it **holds** on the
  current frame until that frame renders. So the playhead never outruns the cache
  (no racing ahead), and once the comp is fully cached the loop plays back in real
  time straight from RAM. The UI stays responsive throughout — the render is off
  the UI thread.

- **Overlays led the pixels during playback** — after moving the preview render
  off the UI thread, the displayed frame lags the live playhead (drop-frame), but
  the editor overlays (selection box, motion path, mask outlines, transform
  gizmo) were still drawn at the *live* time, so the outlines floated **ahead of**
  the shapes while playing. `PreviewRenderer` now reports the time the shown frame
  was rendered for (`shown_time`), and the overlays / onion-skin ghosts / gizmo
  are drawn at that time, so they stay locked to the pixels on screen.

- **Built-in font drew "U" as "V"** — the stroke-font `U` glyph converged both
  sides to a single centre-bottom point, giving a pointed bottom that read as a
  `V` (so "PULSE" rendered "PVLSE"). The glyph now has a **flat bottom** (the
  verticals drop to ~0.72 then meet a short bottom segment between 0.16–0.34),
  clearly distinct from `V`.

- **Playback no longer locks up the UI** — pressing **Play** (or scrubbing) made
  the whole app laggy: the preview composited the comp on the **UI thread** every
  repaint (a ~1 MP CPU render per frame), and playback's per-frame repaint ran it
  continuously, so input couldn't be serviced and the lag compounded the longer
  playback ran. Compositing now happens entirely on a **background worker pool**
  (see *RAM-preview cache* in *Added*); the UI thread never composites, so it
  stays fully responsive regardless of how heavy a frame is.

### Added

- **Stylize family — Find Edges / Mosaic** (After Effects' *Stylize ▸ Find Edges
  / Mosaic*; PLAN Phase 3 *Stylize*) — a new **stylize** (whole-buffer
  look-shaping) effect family, built to mirror the established distort / keying
  stacks exactly: a new `StylizeEffect` enum + its `apply` passes +
  `apply_stylize_effects` stack runner + a compositor bridge (`apply_stylize`) + a
  Properties *Stylize effects* editor + the *Effects & Presets* browser's
  *Stylize* folder. Each runs on the layer's **isolated premultiplied
  linear-light** buffer in `finish_layer` — **after** the spatial passes and
  **before** the distort passes (matching AE's top-down effect order, distort
  below stylize) — so they compose with opacity / blend / masks / track-mattes /
  keying / blur / distort / motion-blur in both the offline render and the live
  render-preview. Both passes are pure (no time / IO / RNG) and deterministic.
  - **Find Edges** (`StylizeEffect::FindEdges`) — a **Sobel** gradient-magnitude
    edge detector run per channel on the buffer's *straight* colour
    (un-premultiplied so coverage-faded edges don't read as colour gradients;
    off-buffer taps clamp to the edge pixel so the frame border isn't a hard
    edge), **inverted** like AE so flat regions go white and edges go dark (the
    "ink outline" look). An `amount` scales the edge response and an `invert` flag
    flips back to bright-edges-on-black. Alpha (the layer's matte) is preserved.
  - **Mosaic** (`StylizeEffect::Mosaic`) — pixelate the buffer into
    `horizontal × vertical` blocks, each filled with the **premultiplied** average
    colour of the pixels it covers (the correct pooling for transparency). Block
    bounds are floored so an uneven division spreads the remainder; counts clamp to
    `>= 1` (and to per-pixel when over-subdivided). `1 × 1` collapses the whole
    buffer to its average colour.
  - Posterize is left as the remaining *Stylize* TODO (it would overlap the
    existing Levels / Curves / Gradient-Map grade, so it's deferred until it can be
    shaped distinctly).
  - Tests (25 new): Find-Edges flat-region-stays-white / edge-produces-response /
    amount-strengthens-edges / invert-flips / alpha-preserved / determinism;
    Mosaic block-is-constant / block-is-the-average / 1×1-whole-average /
    per-pixel-identity / zero-counts-clamp / over-subdivide-clamps /
    premultiplied-transparency-pooling / determinism; stack-applies-in-order;
    labels/defaults + serde round-trip + empty-buffer no-op; **render-path** tests
    (per-pixel mosaic routes value-neutrally through the isolated buffer, Find
    Edges whitens a flat solid interior, a 1×1 mosaic pools the frame to a constant
    block, stylize composes with a mask); `stylize_effects` serde-defaults to empty
    so existing project files round-trip; browser findability + the *Stylize*
    folder now lists Glow + Find Edges + Mosaic.

- **Color-correction expansion — Channel Mixer / Gradient Map / Tritone** (After
  Effects' *Color Correction ▸ Channel Mixer / Photoshop "Gradient Map" /
  Tritone*; PLAN Phase 3 *Color correction*) — three new per-layer
  **color-correction** effects added to the existing CC stack (Tint,
  Brightness/Contrast, Exposure, Levels, Hue/Saturation, Curves, Color Balance),
  each a pure straight-RGBA linear-light pass wired exactly like the others
  (new `Effect` variant + `apply` arm + `defaults()` slot + a Properties editor
  + the *Effects & Presets* browser's *Color Correction* folder). All reuse
  shared `prism_core` math — Pulse only wires it; the shared crate is untouched.
  - **Channel Mixer** (`Effect::ChannelMixer`) — each output channel is a linear
    mix of the input R/G/B plus a constant (`[from_r, from_g, from_b, const]`
    per output row), with a **monochrome** toggle that collapses to the red row's
    weighted gray. The mix math is the shared
    `prism_core::adjust::ChannelMixerMatrix::apply` (asserted bit-identical in a
    test) — no reimplementation. Defaults to identity (a no-op until edited).
  - **Gradient Map** (`Effect::GradientMap`) — maps each pixel's Rec.709 luma
    through a **three-stop** color gradient (`low` at luma 0, `mid` at 0.5,
    `high` at 1) so black takes the first stop, mid-gray the middle, white the
    last. Built from the shared multi-stop `prism_core::gradient::Gradient` and
    sampled with `Gradient::color_at`; an `amount` knob blends original→mapped.
    Defaults to an identity grayscale map (a no-op until recolored).
  - **Tritone** (`Effect::Tritone`) — the same shared gradient primitive authored
    as three named tonal targets (shadows / midtones / highlights), a classic
    three-tone duotone-style grade; `amount` blends original→toned. Seeds a
    recognizable blue-shadow / warm-mid / pale-highlight look.
  - Tests: channel-mixer identity passthrough / **R←B swap** / constant + clamp /
    monochrome gray / **bit-identical to the shared `ChannelMixerMatrix`**;
    gradient-map black→first / white→last / mid→mid / interpolation / amount-0
    passthrough; tritone three-tone mapping; determinism + alpha preserved for all
    three; **render-path** tests compositing a Channel-Mixer swap and a
    Gradient-Map recolor into the frame buffer; browser findability + the
    Color-Correction folder now lists all ten effects.

- **Blur & Sharpen family — Box Blur / Directional Blur / Radial Blur** (After
  Effects' *Blur & Sharpen ▸ Box Blur / Directional Blur / Radial Blur*; PLAN
  Phase 3 *Blur & sharpen*) — three new **spatial** (whole-buffer) blurs added to
  the existing Gaussian-blur stack, built to mirror it exactly: new
  `SpatialEffect` variants + their `apply` passes + a Properties *Blur* editor +
  the *Effects & Presets* browser's *Blur & Sharpen* folder. Each runs on the
  layer's **isolated premultiplied linear-light** buffer in `finish_layer` (after
  masks / track matte / keying, with the other spatial passes), so they compose
  with opacity / blend / masks / track-mattes / keying / distort / motion-blur in
  both the offline render and the live render-preview. All pure (no time / IO /
  RNG) and deterministic.
  - **Box Blur** (`SpatialEffect::BoxBlur`) — a **separable** moving-average box
    convolution of half-`radius` (comp px) run `iterations` times (clamped
    `1..=8`); one pass is a flat uniform average, ~**three** passes approximate a
    Gaussian (central-limit) at a fraction of the cost. `repeat_edge` clamps to the
    edge pixel ("Repeat Edge Pixels"); otherwise off-buffer taps read transparent
    and are excluded from the divisor (a true average of the in-bounds taps,
    matching the Gaussian path).
  - **Directional Blur** (`SpatialEffect::DirectionalBlur`) — a 1-D box average of
    half-`length` (comp px) taken **along** `angle` (degrees, 0° = +x), the classic
    motion streak: it smears one axis and leaves the perpendicular crisp, sampling
    sub-pixel taps bilinearly so the streak stays smooth; off-buffer taps fade
    transparent at the frame border.
  - **Radial Blur** (`SpatialEffect::RadialBlur`) — blur **about** a `center`
    (normalized buffer space) in **Spin** mode (samples swept ±`amount/2` degrees
    around the centre — a rotational motion blur) or **Zoom** mode (samples swept
    ±`amount` fractional radius along the ray from the centre — a dolly-zoom
    streak). Symmetric taps keep the centre sharp; the blur grows with radius.
  - Wired through the model (`PulseLayer::spatial_effects` already routes these
    through `has_spatial_effects` / `apply_spatial`), the *Effects & Presets*
    registry (four blurs now live under the *Blur & Sharpen* folder), and the
    Properties spatial-effects section (per-variant sliders + a Spin/Zoom mode
    picker for Radial).
  - Tests: box-blur separability / normalization / flat-window average / iteration
    smoothing / mass conservation / no-colour-bleed; directional smear along the
    angle only (horizontal vs vertical); radial spin (tangential) vs zoom (radial)
    directionality; per-blur determinism and empty-buffer no-op; render-path tests
    that composite each into the buffer (box softens the edge, directional smears
    along the row, radial warps + is byte-deterministic). 473 passing (was 454).

- **Keying effect family — Color Key / Luma Key / Chroma Key / Spill Suppression
  / Matte Choke** (After-Effects *Keying ▸ Color Key / Luma Key / Keylight-style
  chroma key / Spill Suppressor / Matte Choker*; PLAN Phase 3 *Keying*) — the
  first **keying** stack: whole-buffer **matte-pull** passes that *carve a layer's
  alpha* from a per-pixel colour test (where the colour stack recolours, the
  spatial stack convolves/blooms, and the distort stack warps). Built to mirror
  the spatial / distort families exactly — a `KeyEffect` enum + an
  `apply_key_effects` pass + an `apply_key` compositor bridge + a Properties
  *Keying* section + the *Effects & Presets* browser's new *Keying* category. Each
  works on the layer's **isolated premultiplied linear-light** buffer
  (un-premultiplying per pixel to test the straight colour, re-premultiplying by
  the new coverage), run in `finish_layer` **after** masks and the track matte but
  **before** the spatial passes (AE's keyer-then-blur matte-refine order) so a key
  pulls the matte first and a later Gaussian Blur can soften the keyed edge — and
  so it composes with opacity / blend / masks / track-mattes / spatial-effects /
  distort / motion-blur like every other layer effect, in both the offline render
  and the live render-preview.
  - **Color Key** (`KeyEffect::ColorKey`) — key out a target colour within an RGB
    **tolerance** (linear-light Euclidean distance), with a **softness** band that
    feathers the matte edge back to opaque (AE's *Color Key*).
  - **Luma Key** (`KeyEffect::LumaKey`) — key on Rec.709 **luminance** above
    (*key highlights*) or below (*key shadows*) a **threshold**, with a softness
    band (AE's *Luma Key*).
  - **Chroma Key** (`KeyEffect::ChromaKey`) — key a chroma colour by **YCbCr**
    chroma-plane distance (luminance ignored, so shading on the backing doesn't
    pull holes), with matte **gain**, green↔blue **balance**, and edge **softness**
    (a Keylight-style chroma keyer).
  - **Spill Suppression** (`KeyEffect::SpillSuppression`) — neutralise the key
    colour bleeding onto retained edges by pulling the dominant key channel toward
    the other two by an **amount**, leaving alpha untouched (AE's *Spill
    Suppressor* / Keylight despill).
  - **Matte Choke** (`KeyEffect::MatteChoke`) — erode (negative) / dilate
    (positive) the alpha by a pixel **choke** radius (morphological min/max), plus
    **clip black** / **clip white** levels that crush the matte's soft tails
    (AE's *Matte Choker* / *Simple Choker* + Keylight clip controls).
  - Wired through the model (`PulseLayer::key_effects`, `serde`-defaulted +
    `has_key_effects`), the render path, the browser registry (findable by name +
    synonyms like *green screen*, *keylight*, *despill*, *erode*), and the
    Properties UI (add / reorder / remove + per-keyer params: key colour,
    tolerance / softness, threshold + direction, gain / balance, amount, choke +
    clip levels). Pure matte math (colour-key removes the target + keeps others +
    softness feather, luma-key threshold direction + softness, chroma distance
    keying independent of luminance, spill reduces the key channel, choke
    erode/dilate + clip-level crush, determinism, stacking order, serde
    round-trip) + render-path (keyed layer composites into the buffer, composes
    with masks, isolated-buffer routing, dilate recovers masked alpha,
    determinism) unit-tested. **Open** (Phase 3 *Keying*): Difference Matte,
    advanced matte refine (edge blur/grow with colour-aware feather), Inner/Outer
    edge keying.

- **Distort effect family — Corner Pin / Transform / Mirror / Polar Coordinates**
  (After-Effects *Distort ▸ Corner Pin / Transform / Mirror / Polar Coordinates*;
  PLAN Phase 3 *Distort*) — the first **distort** stack: whole-buffer
  **coordinate-remap** passes that *warp* a layer's rendered pixels (where the
  spatial stack convolves/blooms them and the colour stack recolours them). Built
  to mirror the spatial-effect family exactly — a `DistortEffect` enum + an
  `apply_distort_effects` pass + an `apply_distort` compositor bridge + a
  Properties *Distort effects* section + the *Effects & Presets* browser's new
  *Distort* category. Each is an inverse-warp resampler over the layer's
  **isolated premultiplied linear-light** buffer (bilinear sampling, off-buffer
  samples read transparent), run in `finish_layer` **after** the spatial passes
  (AE's distort-below-blur order) so it composes with opacity / blend / masks /
  track-mattes / spatial-effects / motion-blur like every other layer effect, in
  both the offline render and the live render-preview. Positions are in
  **normalized buffer space** `[0,1]²` so a distort reads the same at preview and
  export resolutions.
  - **Corner Pin** (`DistortEffect::CornerPin`) — pin the buffer's four corners to
    four targets; the interior is filled by **inverse-bilinear** four-point
    mapping (AE's Corner Pin), the staple screen-replacement / perspective-paste
    tool. Pixels outside the target quad are transparent.
  - **Transform** (`DistortEffect::Transform`) — an effect-level extra anchor /
    position / uniform-scale / rotation / **skew** / opacity applied *inside* the
    effect stack (AE's *Transform* effect), so a geometric move can sit between
    other effects rather than only on the layer. A zero scale collapses to empty;
    opacity fades the whole buffer.
  - **Mirror** (`DistortEffect::Mirror`) — reflect the buffer across a line
    (centre + angle); the near half passes through and the far half becomes its
    reflection.
  - **Polar Coordinates** (`DistortEffect::Polar`) — remap between rectangular and
    polar space about a centre (**Rect→Polar** / **Polar→Rect**), the
    tiny-planet / radial-streak transform, with an **Interpolation** blend back to
    the unaltered buffer.
  - Wired through the model (`PulseLayer::distort_effects`, `serde`-defaulted +
    `has_distort_effects`), the demo comp (a subtle in-stack Transform on the
    satellite, so the family reads on launch), the render path, the browser
    registry, and the Properties UI (add / reorder / remove + per-effect params).
    Pure remap math (corner-pin maps corners to targets, transform composition /
    translation / scale-collapse / opacity, mirror reflection symmetry, polar
    round-trip, bilinear sampling, determinism, stacking order) + render-path
    (composites into the buffer, composes with masks, isolated-buffer routing,
    determinism) unit-tested. **Open** (Phase 3 *Distort*): Mesh/Bezier warp,
    Displacement map, Turbulent/Wave, Optics-comp.

- **Generate effect family — Ramp / Checkerboard / 4-Color Gradient / Grid**
  (After-Effects *Generate ▸ Gradient Ramp / Checkerboard / 4-Color Gradient /
  Grid*; PLAN Phase 3 *Generate*) — four more **generate** fills joining Fractal
  Noise, all on the **same** generate infrastructure (the `GenerateEffect` enum +
  the `composite_generate` compositor pass + the Properties *Generate* section +
  the *Effects & Presets* browser's *Generate* category). Like Fractal Noise each
  *replaces* the layer's content with synthesised pixels rather than reading them,
  fills the layer's quad in the **isolated premultiplied linear-light** buffer,
  and honours opacity / blend / masks / track-mattes / spatial effects.
  - **Gradient / Ramp** (`GenerateEffect::Ramp`) — a colour ramp between
    `start_color` and `end_color`, either **Linear** (interpolating along the
    start→end axis, constant perpendicular to it) or **Radial** (interpolating
    with distance from `start` out to `radius`). The ramp parameter is clamped at
    the endpoints (no extrapolation past the ends), and an optional **ramp
    scatter** dithers it (deterministic per pixel — a stable SplitMix64 hash, never
    `rand`) to break up banding.
  - **Checkerboard** (`GenerateEffect::Checkerboard`) — a two-colour chequer grid;
    cell `(i + j)` parity (via `rem_euclid`, so the pattern stays continuous across
    the origin) picks `color1` (even) or `color2` (odd), with per-axis cell `size`
    and an `anchor` that shifts the grid origin.
  - **4-Color Gradient** (`GenerateEffect::FourColorGradient`) — four corner
    colours (top-left / top-right / bottom-left / bottom-right) **bilinearly
    blended** across the layer's quad. A **blend** knob biases the weights toward
    the corners (sharper) or centre (softer), and an optional **jitter** dithers
    them (deterministic per pixel) to soften banding.
  - **Grid** (`GenerateEffect::Grid`) — a line grid over a transparent or filled
    background: cells of `size_w × size_h` outlined with `border`-px lines in
    `color`, the cell interior the `background` colour at `background_opacity`
    (`0` = transparent cells, so only the lines show — the common grid-over-footage
    look); `anchor` shifts the grid origin.
  - **Colour-space-correct compositing** — Fractal Noise emits a grayscale value
    treated as *linear-light* (`[0,1]`, no decode); the four colour generators emit
    straight **sRGB** colour decoded **sRGB → linear** at the gamma boundary by the
    compositor, exactly like a solid's swatch — so their colours match the colour
    pickers. A new `GenerateEffect::produces_color()` drives that path split, and a
    new `GenerateEffect::rgba_at(lx, ly, half_w, half_h)` returns the straight RGBA
    each generator synthesises (the 4-color / radial ramp normalise across the
    quad's half-extents); `composite_generate` now samples `rgba_at` for every
    generator. The per-layer colour-correction stack still runs on the straight
    value before compositing.
  - **Deterministic + keyframable** — every generator is deterministic in
    `(params, pixel)` (the colour generators have no time axis, so they're bit-
    stable within a frame; they animate by keyframing their scalar params). The
    Fractal-Noise **evolution** track is a no-op for the colour generators (they
    have no evolution axis), so it's hidden in the UI for them.
  - **UI** — the Properties **Generate** section gained a **generator picker**
    (Fractal Noise / Gradient Ramp / Checkerboard / 4-Color Gradient / Grid;
    switching swaps to that generator's defaults) and per-generator parameter
    editors (shape / points / radius / colours / sizes / border / scatter / jitter
    / blend / opacity), with the evolution-animation row shown only for Fractal
    Noise. Each generator is registered in the **Effects & Presets** browser under
    the **Generate** category, found by name + synonyms (e.g. *gradient* / *radial*
    → Gradient Ramp, *checker* / *squares* → Checkerboard, *corner* / *mesh* →
    4-Color Gradient, *lines* / *graph* → Grid).
  - **Pure + tested** (+29 tests, 413 total) — each generator is unit-tested:
    linear-ramp endpoints / midpoint / endpoint-clamp / constant-perpendicular,
    radial-ramp centre / radius-edge / isotropy / midpoint, ramp scatter
    determinism + perturbation, degenerate (zero-length / zero-size) no-NaN
    guards; checkerboard cell parity (the four cells around the origin), negative-
    cell continuity, anchor shift; 4-color corner values + interior bilinear blend
    (centre = corner average, edge midpoints) + jitter determinism; grid line-vs-
    cell pixels, horizontal / intersection lines, filled-background opacity,
    thicker-border coverage; the `bias` curve; opacity clamp + serde round-trip for
    **every** generator + `produces_color`. The **render path** is integration-
    tested for each (ramp gradient fills the quad top→bottom, checkerboard
    alternates adjacent cells, 4-color blends the corners across quadrants, grid
    draws opaque lines over a transparent background), plus colour-generator render
    determinism and an sRGB-decode round-trip (a known sRGB colour comes back to
    ~the same 8-bit output). The browser registry test confirms all five
    generators are findable and grouped under Generate.

- **Fractal Noise generator** (After-Effects *Generate ▸ Fractal Noise* — the
  motion-design workhorse; PLAN Phase 3 *Generate*) — the first **generate**
  effect: a per-layer fill that *replaces* the layer's pixels with multi-octave
  gradient (Perlin-style) **fractal noise**, the field that drives smoke, clouds,
  energy, organic textures, mattes, and displacement maps. Unlike the colour /
  spatial stacks (which read the layer's pixels), a generate effect synthesises
  the layer's content from its parameters + the pixel position.
  - **Deterministic, hash-seeded noise** (`comp/generate.rs`, new
    [`GenerateEffect`]) — the field is `gradient_noise_3d`: at each integer lattice
    corner a pseudo-random gradient (a stable **SplitMix64** hash of the corner +
    seed — the same hash philosophy `wiggle` uses, never `rand` / system entropy /
    `Math.random`) is dotted with the offset and quintic-fade interpolated, summed
    over octaves as fBm. The same `(params, evolution, seed, pixel)` **always**
    yields the same value, so a frame renders identically on every pass (for the
    RAM-preview cache, the multi-frame render pool, and golden-frame tests).
  - **Full AE-style params** — **fractal type** (Basic = signed-octave sum,
    smooth/cloud-like; **Turbulent** = abs-octave sum, billowy/ridged smoke),
    **contrast** (about mid-grey) + **brightness**, uniform **scale** (feature
    size in comp px) + per-axis **Scale X / Scale Y**, **complexity** (octave
    count 1–10), **sub-influence** (persistence) + **sub-scaling** (lacunarity),
    a random **seed**, and an **overflow** mode (**Clip** / **Wrap** /
    **Allow HDR**). The field is evaluated in the layer's **local** frame, so it
    rides the layer's transform (scale zooms it, position/rotation move it).
  - **Evolution — the key motion knob — is keyframable** — the generate's static
    params are plain scalars (matching how the colour / spatial effect stacks
    expose params), but **evolution** (the phase/time input that *flows* the field)
    gets a full keyframable [`Track`] on the layer (`generate_evolution`,
    `serde`-defaulted empty). When keyed it overrides the static `evolution` at the
    sampled time (and is expression-able via the track); empty, the static field is
    used. `PulseLayer::generate_at(t)` resolves the effect with evolution sampled
    at `t`.
  - **Compositor wiring** (`render/`) — a new `composite_generate` pass fills the
    layer's quad with the grayscale field into the **isolated, premultiplied
    linear-light** buffer (RGB = value, A = value · generate-opacity ·
    layer-opacity), the layer's per-pixel colour-correction stack runs on the
    value, then the same **masks / track-matte / spatial** passes + blend a normal
    layer runs. It takes precedence over the kind-specific rasterizers (fills any
    solid / shape / text / footage / precomp quad). The field is static within a
    frame (no motion-blur snapshot averaging — the motion comes from keyframing
    evolution), so it shows in the **render-preview** (and exports) like any layer.
  - **UI** — a new **Generate** section in the Properties panel (a *Fractal Noise*
    enable toggle, then type / contrast / brightness / scale (+ X/Y) / complexity /
    sub-influence / sub-scaling / static-evolution / seed / overflow / opacity
    controls, plus an **Animate** button that seeds a keyframable Evolution track),
    shown for layers that draw their own pixels; and a **Fractal Noise** entry in
    the searchable **Effects & Presets** browser under a new **Generate** category
    (found by *fractal* / *noise* / *perlin* / *turbulent* / *clouds*). The launch
    demo ships a full-frame Turbulent Fractal Noise layer (Screen-blended, modest
    opacity) whose **evolution is keyframed 0 → 6** over the comp, so the flowing
    noise reads out of the box.
  - **Pure + tested** (+30 tests, 384 total) — the noise logic is unit-tested
    (determinism across calls, gradient-noise determinism + bound, unit-range under
    Clip, **evolution** changes the field, **seed** changes the field, **turbulent
    vs basic** differ, turbulent fbm is non-negative, **complexity** adds detail,
    one-octave ignores persistence/scaling, contrast pushes from mid-grey,
    brightness lifts, all three overflow modes, scale changes feature size, zero
    scale doesn't NaN, opacity clamp, serde round-trip); the comp model
    (`generate_at` static-vs-track evolution, none-without-fill); the **render
    path** (generate fills the quad, byte-identical across renders, evolution
    changes the frame, the evolution **track** flows the field over comp time,
    colour-correction applies to the field, a cleared generate is inert, the demo
    comp composites); the effect-browser registry (Fractal Noise findable, grouped
    under Generate, registry/defaults in lock-step); and serde back-compat (a
    pre-generate `.pulse` loads with no fill).

- **Export honors the work area** (After-Effects *Render: Work Area / Full Comp*,
  Phase 4 *Markers* follow-on) — the PNG-sequence export now renders **only the
  work-area range** (in → out) by default, instead of always rendering the whole
  `[0, duration]` timeline. The work area already bounded RAM-preview / playback;
  the render path now respects it too, matching After Effects, where the work
  area *is* the default render range.
  - **`RenderRange`** (`render/export.rs`, new enum) — `WorkArea` (render only the
    clamped work-area frames) vs `Full` (the whole comp). `RenderRange::default_for`
    is the AE-style default: the **work area** when it is a real sub-range, else the
    **full comp** (a full or degenerate / empty work area falls back to full so an
    export is never empty). New pure `frame_range` returns the inclusive
    `[first, last]` **comp-frame indices** the range covers (using the existing
    `clamped_work_area`), and `range_frame_count` its length.
  - **Range-aware exporters** — `export_sequence` / `export_sequence_in_project`
    take a `RenderRange` and iterate only `first..=last`. Files are still numbered
    by their **comp frame index**, so a work-area render's first file is the
    work-area start frame (e.g. `comp_0030.png`), not `_0000` — the frame numbering
    and the reported frame count reflect the chosen range correctly (first exported
    frame = work-area start, padding still over the full count).
  - **UI** (`app/menu.rs`, `app/mod.rs`) — *File ▸ Render range…* is a new submenu
    that picks **Work area** vs **Full comp** (each showing its frame count), the
    radio reflecting (and pinning) the effective choice; left unset, export follows
    the comp's work area automatically. The export status now names the rendered
    scope ("Exported 31 frames (work area) → …").
  - **Playback unchanged** — the transport's work-area loop behaviour is untouched;
    this only changes what the exporter renders.
  - **Tested** (+8 tests, 354 total) — `frame_range` for full (`0..=last`) and a
    trimmed work area (in/out frames, first = start frame), the full-/degenerate-/
    empty-serde-default work area falling back to a full render, `default_for`
    preferring a trimmed area (else full), and the exporter writing only the in→out
    frames numbered by comp index (frames outside the work area are not written)
    while a full render still writes every frame.

- **Markers + work area** (After-Effects *Composition/Layer markers* + *Work
  Area*, Phase 4 *Markers*) — the timeline can now carry labelled **markers** to
  call out beats, and a **work area** sub-range that bounds playback, with
  transport **time-navigation** to jump between markers. Pure timeline metadata —
  no pixels, no compositor changes.
  - **Model** (`comp/marker.rs`, new `Marker` + `WorkArea`) — a [`Marker`] is a
    `time` + optional `duration` (0 = a point marker) + `label` + sRGB `color`
    (default AE green); a [`WorkArea`] is a `[start, end]` range with `clamped` /
    `length` / `contains` / `is_full` helpers that keep it ordered and inside the
    comp. Pure `next_marker_time` / `prev_marker_time` find the nearest marker
    ahead/behind a time (order-independent). `Comp` gains `markers` + `work_area`
    fields and `PulseLayer` gains `markers`, all `serde`-defaulted (empty / full)
    so pre-marker `.pulse` files load unchanged; `clamped_work_area` **self-heals**
    the serde-default empty `[0,0]` range to the whole timeline so an old project
    loops its full length. Comp-level `next_marker` / `prev_marker` span the
    comp's own markers **and** the selected layer's (AE's jump-to-marker scope).
  - **Playback loops the work area** — the transport now advances within the
    clamped work area and wraps back to its start at the end (a full work area
    degrades to looping the whole `[0, duration]` timeline exactly as before); the
    RAM-preview cache (whole timeline) covers it, so the gate is unchanged.
  - **Timeline UI** (`timeline.rs`) — comp markers draw as tinted house-shaped
    tabs (with labels) just below the ruler ticks; layer markers ride the bottom
    edge of each lane; a durationed marker draws a faint band. A trimmed work area
    shows as an accent band + end caps on the ruler, and the playhead **dims** when
    it sits outside the work area (so "won't loop here" reads at a glance).
  - **Transport + menus + shortcuts** — the timeline transport gains
    **prev-marker / add-comp-marker / next-marker** buttons; a **Comp ▸ Work area**
    submenu (set start/end to playhead, reset to whole comp) and a **Comp ▸
    Markers** submenu (add comp/layer marker, jump prev/next, disabled when none
    in that direction); and AE-style keys **`B`** / **`N`** (trim work-area
    start/end) + **`M`** (drop a comp marker), suppressed while a text field has
    focus. A new **Markers** section in the Properties panel edits the selected
    layer's markers (time / label / duration / color, add at playhead, go-to,
    remove, kept time-sorted). The launch demo ships a comp marker at 2.5 s.
  - **Pure + tested** (+16 tests, 346 total) — marker model (point/durationed
    end, default color), work-area clamping / length / containment / is-full /
    full-vs-default, marker navigation (nearest ahead/behind, strict, unsorted,
    empty), comp-level navigation (spans comp + selected-layer markers, ignores
    other layers'), comp work-area clamp staying inside the timeline + fresh comp
    full, and serde round-trip + pre-marker back-compat (defaults to empty / full).
  - **Deferred** — marker-snapping while scrubbing, and dragging markers on the
    timeline (today they're edited in Properties). *(Rendering / exporting the
    **work-area range only** has since landed — see "Export honors the work area"
    above.)*

- **RAM-preview cache + parallel render pool** (After Effects' *RAM Preview* /
  green cache bar; PLAN Phase 6 *Caching* + *Multi-frame rendering*) — the preview
  is now a **frame cache**: each comp frame is rendered through the offline
  compositor **once**, its pixels stored by frame index, so **loop playback runs
  in real time straight from RAM** after the first pass (no re-compositing). This
  is the fix for "playback takes ~1 s per frame" — the cost is paid once per frame
  and then reused every loop.
  - **Parallel fill** (`preview.rs`, `worker_loop` pool) — a pool of worker
    threads (one per core minus two, clamped 1–8), each with its own footage
    `FrameCache`, fills the cache concurrently, so the first pass is ~N× faster
    than serial. Jobs round-robin across the pool and are tagged with a cache
    **epoch**; a worker **skips** any job whose epoch was superseded by an edit /
    resize, so a rapid scrub never wastes full renders on stale frames.
  - **Invalidation + memory budget** — a signature of `(comp-state hash, render
    dims)` gates the whole cache; any layer/keyframe/property edit or a resolution
    change bumps the epoch and re-fills. A **~1 GiB byte budget** bounds memory;
    when exceeded, frames **farthest from the playhead** are evicted first, so a
    short comp fits whole (real-time loop) while a long comp keeps a window around
    the playhead cached. A single reusable texture is re-pointed at the displayed
    frame (or its nearest cached neighbour while the exact frame still renders).
  - **Tests** — pool worker renders a job and **skips a stale-epoch** job;
    `is_frame_ready` / `fully_cached` readiness predicates over the cache; cache
    signature stability/invalidation; work-area `frame_count`.

- **Preview fidelity — render preview** (After Effects' *Composition viewer*) —
  the interactive preview now shows **real composited pixels** instead of flat
  placeholder quads. It renders the comp at the playhead through the **existing
  offline CPU compositor** (`render::render_preview_frame` → `render_comp`) at a
  capped preview resolution, uploads the result as an egui texture, and draws it
  as the preview image. Because it reuses the real compositor, **footage frames,
  precomps (recursive render), effects, color-correction, masks, track mattes,
  motion blur, time-remap, and expressions** all appear in the live preview with
  full fidelity — matching an exported frame, just smaller.
  - **Render path** (`render/mod.rs`) — new `render_preview_frame` renders the
    active comp project-aware (precomps resolve against sibling comps, cycle guard
    intact) at a resolution capped by new `preview_dims` to **1280 px on the long
    edge**, preserving the comp's aspect (small comps render native, never
    upscaled). `render_frame_in_project` is promoted to a normal `pub` entry (no
    longer dead-code-gated) as the shared project-aware renderer.
  - **Off-thread render + cache** — compositing runs off the UI thread and each
    rendered frame is cached for reuse (superseded and expanded by the *RAM-preview
    cache + parallel render pool* entry above — a worker **pool** filling a
    frame-indexed RAM cache, invalidated by a `(comp-state hash, render dims)`
    signature). Each worker keeps a persistent footage `FrameCache`, so a still /
    sequence frame is decoded **once** and reused across the frames it renders; the
    offline-export path keeps its own per-export cache, unchanged.
  - **Overlays on top** — the transform gizmo, selection box, mask paths, null
    pivots, adjustment-layer bounds, and onion-skin ghosts are drawn over the
    rendered image via the egui painter, pixel-aligned through the **same**
    aspect-fit `comp px → screen` mapping the image uses (so they track the
    rendered pixels through letterbox/pillarbox scaling). The placeholder-quad
    arms for footage / precomp / solid / shape / text — and their preview-only
    color/matte/motion-blur approximations — are removed (the texture now carries
    them for real).
  - **Tests** — cache-signature (comp-state hash) stability/invalidation,
    comp-space ↔ display-rect round-trip incl. letterboxing, the resolution cap,
    and persistent-`FrameCache` decode-once reuse across preview renders.

- **Time remapping** (After-Effects' *Enable Time Remap*, Phase 4 *Time
  remapping*) — a time-based layer (a **footage image-sequence** or a **precomp**)
  can now carry an optional, keyframable **time-remap** curve that drives the
  **source time** it is sampled at, instead of the comp time. This lets the user
  **freeze** (a constant remap holds one source frame), **reverse** (a decreasing
  remap plays the source backwards), and **slow / speed up** footage and precomp
  playback via keyframes — and, since the remap is a normal `Track`, via
  expressions too (`time * 0.5` for half-speed).
  - **Model** (`comp/time_remap.rs`, new `TimeRemap`) — an `enabled` switch plus a
    scalar `Track` of *source* times (seconds) keyed against comp time. A
    `TimeRemap` field is added to `PulseLayer`, `serde`-defaulted to **disabled
    with an empty track** so every pre-time-remap `.pulse` file (and layer kind)
    loads and samples its source unchanged — the remap is a pure no-op until
    switched on. `source_time` / `source_time_ctx` return the remapped source time
    when the remap is **active** (enabled *and* keyed) and the identity comp time
    otherwise, so an "on but unconfigured" remap can never collapse the source to
    time 0. A comp-level `layer_source_time` threads the expression context
    (fps/duration/index) so an expressioned remap drives source time too.
  - **Sampling integration** — the footage frame-index/sampling path and the
    precomp recursive-render time (both previously on comp time `t`) now route
    their *source* time through `layer_source_time` (transforms/opacity stay on
    `t` — only the sampled content is retimed). The footage **fps-override / loop /
    hold-last** behaviour is honoured at the remapped time, and the precomp's
    `time_offset` shift still applies on top of the remap. Motion-blur sub-frames
    and footage/precomp track-matte sources are remapped too, so a retimed source
    blurs and mattes at the remapped rate.
  - **Default keyframes** — enabling the remap seeds **AE-style default keys**: an
    identity ramp from source time `0` at comp time `0` to the source's natural
    duration at the comp's end (footage = `frames / fps`, precomp = the referenced
    comp's duration), eased like AE's time-remap default — so freshly enabling it
    plays the source 1:1, then the user reshapes the curve. A source with no usable
    duration (a still, an unwired reference) seeds a single identity key (a hold at
    source start). Disabling keeps the keys, so re-enabling never clobbers a
    hand-tuned curve.
  - **UI** (`app/properties.rs`) — time-based layers gain a **Time remap**
    section with an **"Enable Time Remap"** toggle and the remap value shown as a
    **keyframable property** (a new generic `track_row` reusing the same value
    slider + add-key + `fx` expression + interpolation UI as the transform rows).
  - **Pure + tested** (+16 tests, 320 total) — model unit tests (disabled /
    enabled-but-empty remap is the identity; an identity-seed matches no remap; a
    reverse remap maps `t → dur − t`; a constant remap freezes one source time; an
    eased remap samples monotonically; an expression drives the remap; seeding is
    idempotent on a non-empty track and falls back to a single identity key without
    a duration), comp-level samplers (`layer_source_time` is the identity when off
    and follows an active reversing remap), serde (an enabled, keyed remap layer
    round-trips; a missing `time_remap` field defaults to disabled), and the
    **render path** (an identity remap on a precomp renders byte-identically to no
    remap; a reverse remap samples the nested comp backwards; a freeze remap holds
    one source frame across host time).
- **Expressions on properties** (After-Effects *expression* parity, Phase 4
  *Expressions* — first slice) — any animatable scalar property (anchor X/Y,
  position X/Y, scale, rotation, opacity) can now carry an optional **expression**
  string. When set, the property's value at time `t` is computed by evaluating the
  expression instead of (or driven by) the keyframed value — the AE signature
  feature that makes things move procedurally.
  - **Engine** (`comp/expr.rs`, new dep **`rhai` 1.25**, pure-Rust / no system
    deps) — a sandboxed scripting evaluator with tightened limits (max operations
    / call depth) so a runaway script can't hang the render. Each evaluation binds
    a small `ExprCtx` into scope as plain variables: **`time`** (seconds),
    **`value`** (the property's keyframed sample at `t`, so an expression can
    *offset* the animation — `value + 10`), comp **`fps`** / **`duration`**, and
    the layer **`index`**. Helper functions are registered alongside rhai's math
    (`sin`/`cos`/`abs`/`floor`/…): **`wiggle(freq, amp)`** — smooth jitter that is
    **deterministic** per `(layer, time)` (seeded from a stable SplitMix64 hash,
    never `Math.random`, so a given frame always renders identically),
    **`linear(t, tmin, tmax, v1, v2)`** (endpoint-clamped remap), and
    **`clamp(v, lo, hi)`**. Integer literals coerce to floats so natural
    expressions like `wiggle(2, 30)` just work.
  - **Sampling integration** — `Track` gains a `serde`-defaulted
    `expression: Option<String>` (skipped on serialize when empty, so unexpressed
    tracks round-trip byte-identically to pre-expression `.pulse` files);
    `Track::sample_expr` samples the keyframes, exposes that as `value`, evaluates
    the expression, and **falls back to the keyframed value on any parse/eval
    error or non-finite result — never panicking**. Comp-level expression-aware
    samplers (`layer_value` / `layer_opacity` / an expression-aware `world_matrix`
    + `transform_ctx`) thread the context (fps/duration/index) so expressions
    drive **position / scale / rotation / anchor / opacity through the real
    compositor and preview** (including the parent chain, motion-blur sub-frames,
    and track-matte sources), not just the model.
  - **UI** — every transform property row in the Properties panel gains an **`fx`
    toggle** that reveals a monospace **expression text field** (seeded with
    `value` so enabling it is value-neutral). The field shows the live
    expression-resolved value, and **turns red with an "expression error" note**
    when the script fails to evaluate (the render transparently uses the keyframed
    value). The launch demo's satellite now spins via
    `value + time * 120 + wiggle(3, 15)` so the feature reads out of the box.
  - **Pure + tested** (+18 tests, 304 total) — engine unit tests (`time * 2` at
    several `t`; `value + 10` offsets the keyframed value; `wiggle` is
    deterministic for a fixed time and varies across time within amplitude;
    `linear`/`clamp`/math helpers; fps/duration/index in scope; a malformed
    expression returns `None` and flags an error) plus integration tests (an
    expression overrides the keyframed value; `value` sees the keyframed sample;
    a malformed expression falls back without panicking; serde round-trip of a
    property with an expression; the missing field defaults to `None`; and through
    the **render path** — a position expression moves coverage, an opacity
    expression fades the layer over time, and a broken expression doesn't crash
    the render).
  - **Deferred** (kept honest as gaps, not silently dropped): the broader AE
    expression library (`loopOut`/`loopIn`, `ease`, `random`/`seedRandom`,
    `valueAtTime`, `thisComp`/`thisLayer`), **pick-whip property links** (one
    property referencing another), expressions on **non-scalar** properties
    (2D/3D position, color, path), and on **effect/mask parameters** — these land
    with the typed-`Property<T>` rebuild and the property-link picker.
- **Precomps / nested compositions** (After-Effects *precomp* parity, Phase 2
  *Precomps*) — a new layer kind that **nests another composition**: a precomp
  layer references a sibling comp by id and, at comp time, that referenced comp is
  rendered **recursively** (through the same render path) into the layer's buffer
  and composited like any other layer — honouring its transform / anchor /
  parenting / opacity / blend mode / masks / track matte / effects / motion blur.
  This is the first layer whose pixels are *another whole comp*, so the document
  graduates from a single comp to a **project of comps**.
  - **`LayerKind::Precomp`** — joins Solid / Shape / Text / Footage / Null /
    Adjustment. A precomp layer carries a [`PrecompLayer`] (a `serde`-defaulted
    `precomp` field on every layer, so pre-precomp `.pulse` files still load with
    no reference) — the target [`Comp::id`] plus a scalar **time offset** (seconds
    added to the host time before the nested comp is sampled — a deliberately
    minimal stand-in for full time-remap: a shift, not a curve).
  - **Project model** (`comp/precomp.rs`) — precomps need more than one comp to
    point at, so the document becomes a [`Project`]: an id-keyed set of comps with
    one marked **active** for editing. Each [`Comp`] gains a stable `id` and a
    display `name` (both `serde`-defaulted, so an old single-comp `.pulse` — a bare
    `Comp` — still deserializes and wraps cleanly via `Project::from_comp`, which
    mints an id). IDs are minted monotonically and defensively (never reused, never
    colliding with a live id even if a hand-edited `next_id` lags).
  - **Recursive render + cycle guard** (`render/`) — the software compositor
    resolves a precomp's target against the project's comps and renders it
    recursively, sampling the rendered nested frame into the layer's quad (sRGB →
    linear at the gamma boundary, then the layer's effect stack) exactly like
    footage, before the same **masks / track matte / spatial-effect / motion-blur**
    passes a footage layer runs. A per-render **visited-set of comp ids** carries
    the recursion stack: rendering refuses to re-enter a comp already on the stack,
    so a reference **cycle** (A → B → A) or a self-reference simply renders nothing
    — a corrupt or self-referential project can never infinite-loop or overflow the
    stack. New project-aware entries `render_frame_in_project` /
    `export_sequence_in_project` resolve precomps; the single-comp
    `render_frame` / `export_sequence` keep working (a precomp there draws nothing,
    having no project to resolve against).
  - **UI** — a new **Precomp** section in the Properties panel (a **source-comp**
    picker over the project's other comps, the active comp flagged as a
    self-reference, plus a **time-offset** drag), shown for precomp layers; *Layer
    ▸ New ▸ Precomp* (auto-wires to an existing comp if any); and **Layer ▸
    Pre-compose**, the classic AE workflow — it wraps the selected layer into a
    new comp (sized to the host) and replaces it in place with a precomp layer
    referencing the new comp. The coarse vector preview shows a precomp as a
    placeholder quad (its nested comp renders in the offline render / export);
    save now serialises the whole **project** so precomp references round-trip.
  - **Pure + tested** (+14 tests, 288 total) — the precomp render path is
    integration-tested (a precomp renders its referenced comp's content; the time
    offset samples the nested comp at the shifted time; **the cycle guard
    terminates** for A → B → A and for a self-reference, rendering nothing; a
    missing target renders nothing; a precomp nests two levels deep; the
    single-comp entry ignores precomps), and the model is unit-tested (precomp
    layer + project serde round-trips, pre-precomp/old single-comp back-compat via
    serde defaults, unique-id minting, `push_comp`, and a model-level pre-compose
    that wraps + references correctly).
  - **Deferred** — multi-layer pre-compose (wrapping a *selection set* preserving
    inter-layer parenting), full **time-remapping** (a remap curve, time-stretch,
    reverse, freeze-frame), **collapse transformations**, a comp navigator / tab
    UI, and rendering the nested comp live in the coarse preview — all noted in
    PLAN.md Phase 2 / Phase 4.

- **Footage layers** (After-Effects footage-layer parity, Phase 2 *Footage
  layers*) — a new layer kind that draws **decoded image footage** — a single
  **still** or a numbered **image sequence** — sampled at comp time, the first
  layer type whose pixels come from a file on disk rather than authored geometry
  or a swatch. Scoped to stills + sequences via the suite's shared `prism-io`;
  **real FFmpeg video decode is deferred** to the shared `prism-media` crate (the
  next footage step — see PLAN.md Phase 2).
  - **`LayerKind::Footage`** — joins Solid / Shape / Text / Null / Adjustment. A
    footage layer carries a [`FootageLayer`] (a `serde`-defaulted `footage` field
    on every layer, so pre-footage `.pulse` files still load with no source) — an
    optional [`FootageSource`] plus interpretation settings: **alpha mode**, an
    optional **fps override**, and **loop** / **hold-last** end behaviour. The
    kind is added from *Layer ▸ New* and switchable per-layer in Properties like
    any other.
  - **`FootageSource`** (`comp/footage.rs`) — either a **Still** (constant over
    the whole timeline) or a numbered **Sequence** (a printf-style `{}` pattern +
    zero-pad width + start number + frame count, one file per frame). Time-indexed
    frame sampling — `frame_index(t, fps, looping, hold_last)` maps comp time to
    the 0-based source frame as `floor(t·fps)` (fps = the layer override or the
    comp fps), holding the first frame before `t = 0`, and past the end either
    **looping** (modulo wrap) or **holding the last frame** (the safe default when
    neither is set). `source_from_path` auto-detects a sequence from any one
    chosen frame: it splits the stem's trailing digit run into a pattern, infers
    pad/start, and probes the contiguous run of frames on disk (the picked frame
    need not be the first), falling back to a Still when there's no trailing
    number.
  - **Decode-once `FrameCache`** — a bounded **MRU decode cache** so a given
    (path) is decoded at most once per render pass and reused across the many comp
    frames (and motion-blur sub-frames) that reference the same source frame; a
    failed/missing decode is cached as a miss so it isn't retried (or re-logged)
    within a pass, and a least-recently-used eviction keeps memory bounded. Decode
    goes through `prism_io::load_image` (8-bit sRGB RGBA), converts each channel
    **sRGB → linear at the gamma boundary**, and resolves the **alpha mode**
    (Straight / Premultiplied un-premultiply / Ignore-as-opaque) to straight color
    + straight coverage — the exact representation the solid / shape / text
    rasterizers feed the compositor. `DecodedFrame::sample` bilinearly samples a
    decoded frame at normalized UV, with transparent out-of-range edges.
  - **Full compositor path** (`render/`) — the software compositor rasterizes a
    footage layer into an **isolated, premultiplied linear-light** buffer
    (inverse-mapping each comp pixel to footage UV through the layer's resolved
    world matrix and bilinearly sampling the cached frame), then runs the same
    **masks**, **track matte**, and **spatial-effect** passes a solid does before
    compositing through the layer's **transform / anchor / parenting / opacity /
    blend mode** — so footage composes with masks, mattes, blur/shadow/glow, track
    mattes, and **motion blur** (the shutter integrator samples the
    time-indexed frame per sub-frame, and a footage layer can serve as a
    track-matte source).
  - **UI** — a new **Footage** section in the Properties panel (a native `rfd`
    file picker for the source with a Still / Image-sequence kind label and
    display path, an **alpha** mode dropdown, an **fps override** toggle + value,
    and **loop** / **hold-last** checkboxes), shown for footage layers; *Layer ▸
    New ▸ Footage*; and **File ▸ Import footage…**, which pops a file picker and
    adds a footage layer with the auto-detected sequence (or still), named after
    the file.
  - **Pure + tested** (+17 tests, 274 total) — the time→frame mapping
    (still-is-constant, fps-driven index, fps override, hold-last clamp,
    loop-wrap, negative-time-holds-first), `path_for` zero-padding + start offset,
    `path_at` override-then-comp-fps, the unset-source no-op, the cache
    (decode-once, failure-caching, LRU eviction), and `DecodedFrame` bilerp + OOB
    are all unit-tested; the renderer's footage path is integration-tested
    (unset footage renders nothing).

- **Effects & Presets browser** (After-Effects *Effects & Presets* panel parity,
  previously a noted UI/UX gap) — a **searchable, categorised** effect panel that
  replaces the two flat "Add" menus with a type-to-filter surface: type part of
  an effect's name (or a synonym), pick from the matching, folder-grouped list,
  and it lands on the selected layer's matching stack.
  - **`effect_browser`** (`comp/effect_browser.rs`) — the pure registry + matcher
    behind the panel. A single [`REGISTRY`] of every addable effect across **both**
    stacks (the seven per-pixel colour effects and the three whole-buffer spatial
    effects), each [`BrowserEntry`] tagged with a display **name**, a [`Category`]
    folder (Color Correction / Blur & Sharpen / Perspective / Stylize), the
    [`Stack`] it appends to, and a set of search **keywords** (synonyms / AE names
    that aren't in the display name — e.g. *bloom* finds Glow, *hsl* finds
    Hue/Saturation). [`BrowserEntry::instantiate`] builds a fresh,
    sensibly-defaulted instance (a tagged [`NewEffect`]) by indexing the stack's
    existing `defaults()` array, so the browser and the per-stack editors never
    drift.
  - **Ranked, token-AND search** — `filter` / `filter_grouped` score each entry
    against a case-insensitive, whitespace-split query: every query token must
    match *somewhere* (name or a keyword) for an entry to appear (typing more
    narrows), and per token a whole-name exact match outranks a name prefix,
    which outranks a mid-string substring, which outranks any keyword hit. Results
    sort best-score-first (ties alphabetical for a stable order); an empty query
    lists the whole registry. `filter_grouped` buckets the ranked hits into the
    category folders the panel renders, dropping empty folders.
  - **Browser panel** (`app/effects.rs`) — a new left-docked **Effects & Presets**
    panel: a magnifying-glass search box (with a **Clear** button and a "→ <layer>"
    hint of where a click lands), then the filtered effects as collapsible
    **category folders** (auto-opened while searching, tidy/collapsed when idle).
    Clicking an effect appends it to the selected layer's `effects` (colour) or
    `spatial_effects` (spatial) stack and surfaces an "Added <name>" status; with
    no layer selected the panel prompts to select one. The flat per-stack "Add"
    menus in Properties stay as a quick inline alternative.
  - **A fourth dockable panel** — the browser joins the Window-menu show/hide set
    (`app/workspace.rs`) as `Panel::Effects`, **hidden by default** so the classic
    four-panel workspace is unchanged; *Window ▸ Effects & Presets* (or *Show all
    panels*) opens it. `PanelVisibility` gained an `all_shown` query, and
    `show_all` now shows **every** panel (including the browser) rather than
    resetting to the classic default.
  - **Pure + tested** — all the registry/search/grouping logic is unit-tested:
    the registry stays in lock-step with both `defaults()` arrays (indices map to
    real slots, names equal the effects' labels, every effect is reachable), the
    empty/whitespace query lists everything alphabetically, name-substring +
    case-insensitive + keyword-only matching, exact-name-beats-substring and
    prefix-beats-mid-substring ranking, multi-token AND (an unmatched token drops
    the entry), the no-match-is-empty case, descending-score order, and grouping
    that preserves category order + per-group ranking and drops empty folders —
    plus the new `Panel::Effects` default-hidden / toggle-to-open / show-all
    workspace behaviour.

- **Onion skinning** (a motion-tooling staple, previously a noted parity gap) —
  faint **ghost copies of the comp at the neighbouring frames** are drawn behind
  the live frame so hand-keyed timing reads at a glance: where the motion *came
  from* and where it's *going*. Toggled (with controls) from a new **View** menu;
  off by default.
  - **`OnionSkin`** (`onion.rs`) — the pure model behind the menu: a master
    `enabled`, how many ghost frames to show **before** / **after** the playhead
    (0…8 each side), the **frame step** between ghosts (1 = every frame), and the
    **opacity** of the nearest ghost. `OnionSkin::ghosts(time, fps, duration)`
    turns those into the ordered list of ghost frames to paint — each a comp
    `time`, a `Dir` (Before/After), a tint, and an opacity.
  - **Directional tint + distance falloff** — past ghosts get a cool blue tint,
    future ghosts a warm orange, so the two directions read apart; opacity falls
    off linearly from the nearest ghost (full `opacity`) to a 25%-of-base floor
    at the farthest, so the trail fades but stays visible. Ghosts whose frame
    falls off either end of the timeline (`< 0` or `> duration`) are dropped, and
    the list is ordered farthest → nearest so nearer (more opaque) ghosts paint
    last (on top).
  - **Preview integration** (`preview.rs`) — `paint_onion` draws each ghost
    frame *behind* the live comp as flat, tinted, faded quads of every visible
    pixel-drawing layer at the ghost's sampled world transform (no effects /
    masks / mattes — onion skinning is a *timing* aid, not a render preview),
    cheap enough to run every frame.
  - **View menu** (`app/menu.rs`) — an **Onion skinning** enable plus
    **Before** / **After** / **Frame step** / **Opacity** sliders (disabled while
    off), mirroring the way the **Comp ▸ Motion Blur** controls read.
  - **Pure + tested** — all the ghost-frame timing / falloff / range-clipping
    logic is unit-tested: disabled ⇒ no ghosts, count = before + after when in
    range, step-spaced times, out-of-range ghosts dropped at the timeline ends,
    nearest ghost most opaque and painted last, single-ghost full opacity, the
    far-ghost opacity floor, distinct before/after tints, `step`/count clamping,
    and non-positive `fps`/`duration` ⇒ empty.

- **Three new color-correction effects** — **Hue / Saturation**, **Curves**, and
  **Color Balance** (Phase-3 *Color correction* surface) — joining Tint /
  Brightness & Contrast / Exposure / Levels in every layer's effect stack, so
  the per-layer grade now covers the After-Effects color staples (the "Add"
  menu in Properties now lists seven effects).
  - **Hue / Saturation** (`Effect::HueSaturation`) — rotate **hue** (degrees),
    scale **saturation** (`-1` grayscale … `+1`), and lift/crush **lightness**
    (`-1` black … `+1` white). The pixel round-trips through HSL (new pure
    `rgb_to_hsl` / `hsl_to_rgb`); alpha is untouched. Zeroed params are an exact
    no-op.
  - **Curves** (`Effect::Curves`) — a master tone curve set by five control
    points at inputs `0, ¼, ½, ¾, 1` (the AE Curves grid), evaluated as a
    Catmull-Rom spline (`curve_eval`) through the points and applied to every
    RGB channel. The straight identity ramp (`Effect::CURVE_IDENTITY`) is a
    no-op; the editor exposes the five output sliders plus a **Reset** button
    (a draggable curve canvas lands with the typed-`Property` graph-editor
    rebuild).
  - **Color Balance** (`Effect::ColorBalance`) — independent red/green/blue
    pushes for **shadows**, **midtones**, and **highlights**, each weighted by a
    smooth function of the pixel's luma (`smoothstep` ramps for darks/brights,
    a bell for midtones) so the three ranges blend — matching AE's three-range
    color balance.
  - **Pure + tested** — all three are straight-`[f32;4]` linear-light passes
    that preserve alpha, slot into the existing ordered effect stack, and are
    unit-tested (HSL round-trip, `hue+120°` cycling red→green, full desaturate
    to gray, curve hitting its control points + staying identity on the ramp +
    lifting midtones, `smoothstep` endpoints/midpoint, color-balance no-op +
    range-targeted push, and alpha preservation across every default effect).

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

[Unreleased]: https://github.com/prism-suite/pulse/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/prism-suite/pulse/releases/tag/v0.2.0
[0.1.0]: https://github.com/prism-suite/pulse/releases/tag/v0.1.0
[0.0.1]: https://github.com/prism-suite/pulse/releases/tag/v0.0.1
