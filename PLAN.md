# Pulse — Open Source After Effects Alternative

Professional motion-graphics, VFX & compositing app in Rust, and **app #3 of the Prism suite** (sibling
to [Pigment](https://github.com/KwaminaWhyte/prism-suite-pigment) raster, [Contour](https://github.com/KwaminaWhyte/prism-suite-contour) vector). **Goal: reach ≥85% of Adobe After
Effects' real-world capability** — features, reliability, and ease-of-use — in staged milestones, on the
suite's shared engine: `prism-core` (layers, blend math, render graph), `prism-color` (linear/OCIO),
and a shared `prism-fx` (OpenFX-style effects) and `prism-media` (FFmpeg) layer.

> Companion docs: [RESEARCH.md](./RESEARCH.md) (cited findings + crate matrix), [SUITE.md](https://github.com/KwaminaWhyte/prism-suite-prism/blob/main/SUITE.md) (four-app vision + interop). The repo README (when added) tracks what runs *today*; this PLAN is the parity roadmap.

---

## 0. Why this can work

- The hard parts are solved and free: a tile/render-graph compositor (shared with Pigment), blend math
  and color science (`prism-core`/`prism-color`), keyframe interpolation + Bézier easing, OpenColorIO/
  OpenEXR (ASWF Rust bindings), FFmpeg decode/encode, an embeddable expression engine (`rhai`/`rune`).
- After Effects is, at its core, **a keyframed, effect-driven layer compositor**. The compositor is the
  *same* render graph Pigment already runs — Pulse adds **time** (keyframes, expressions), **effects at
  scale** (`prism-fx`), and **media** (`prism-media`). That shared engine is the suite's whole bet.
- AE's real moat is interop + the effect/expression ecosystem + reliability (caching, multi-frame
  render). We target those deliberately.

**Non-negotiable principle:** a **time-addressable render graph** — every layer/effect/property is a
function of time `t`, composited in **linear light** (`prism-color`), cached per (node, frame, tile).
Float (16/32-bit) working buffers from day one; OCIO-managed color; never bake until render.

---

## 0a. Suite boundaries — what belongs in Pulse vs Pigment / Contour / Reel

Pulse shares `prism-core` (already a dependency) and will share `prism-color` / `prism-fx` /
`prism-media`. Every feature is filed against three rules so we never overwrite a sibling app's work:

- **Pulse-owned (motion / VFX / compositing):** comps & precomps, the timeline, keyframes & graph
  editor, expressions, layer types (solid/text/shape/footage/adjustment/null/camera/light), masks &
  roto, track mattes, motion blur, time remapping, 2D/3D compositing, the render queue. Lives in
  `pulse-app` or motion-only modules.
- **Shared-crate, app-agnostic:** the compositor/render-graph, tile model, **blend modes** (reuse
  `prism-core`'s 18), **adjustments** (reuse `prism-core::adjust` for color-correction effects), color
  transforms (`prism-color` + OCIO), the **`prism-fx`** OpenFX host, and **`prism-media`** (FFmpeg
  decode/encode + audio, shared with the future Reel). These **must not** assume Pulse — additions are
  additive and time-agnostic (time is Pulse's layer on top).
- **Out of scope — a sibling app's domain (do not build here):**
  - *A clip-based, multi-track non-linear **video editor*** (trim/ripple/insert clips, transitions
    between clips, full audio mixer) → **Reel** (the Premiere analog). Pulse's timeline is *layer +
    keyframe* based, not clip-based. Pulse renders comps that Reel places via Dynamic Link.
  - *Primary raster painting / photo retouch* → **Pigment**; *deep vector authoring* → **Contour**.
    Pulse has shape layers and text animators (motion-native), and it *consumes* Pigment docs and
    Contour artboards as footage/smart layers via suite interop — it does not re-implement them.
  - *Cross-app interop glue* (Dynamic Link host, `prism-doc` container, shared clipboard/asset library)
    is **suite-level**. Pulse is the canonical Dynamic-Link *producer* (its comps drop live into Reel),
    but the mechanism is defined with the suite, not unilaterally.

---

## 1. Current state (what runs today)

Grounded in `pulse/crates/pulse-app/src/`. An early but real motion scaffold:

- **Comp model** (`comp.rs`) — `Comp { width, height, duration, fps, layers }`; `PulseLayer { name,
  color, visible, + 5 Tracks }`. Five animatable properties: **X, Y, Scale, Rotation, Opacity**, each a
  `Track` of `Keyframe { t, value }`. Sampling = **linear interpolation** between bracketing keys,
  constant hold outside the range; `set_key` inserts/overwrites keeping keys sorted. serde JSON.
- **Transport** (`app.rs`) — playhead `time`, play/pause (Spacebar), real-`dt` advance, loop at
  duration; add/delete/move/recolor layers; save `.pulse` (serde + `rfd`); Export is a **stub**.
- **Timeline** (`timeline.rs`) — time ruler, one lane per layer with **keyframe diamonds**, draggable
  **playhead**, click/drag scrub.
- **Preview** (`preview.rs`) — CPU egui-`Painter` render: each visible layer is a **solid color rect**,
  transformed by sampled (x, y), uniform scale, rotation about center, faded by opacity; fitted to the
  panel.
- **Shell** — `theme.rs` Prism dark theme, `icons.rs` phosphor glyphs; depends on `prism-core`.

**Reality check:** layers are solid swatches; interpolation is linear-only; preview is CPU/egui (no GPU
compositor, no effects, no media, no blend modes yet). Everything below builds from this seed.

---

## 2. Validated tech stack (verify with `cargo add` at build)

| Concern | Crate | Notes |
|---|---|---|
| Compositor / render graph | `prism-core` + wgpu (shared w/ Pigment) | Tile model, dirty eval, **18 blend modes**, adjustments. Add the time axis. |
| Color / linear / OCIO | `prism-color` + OpenColorIO (ASWF Rust binding) | Linear-light compositing; OCIO config-driven looks; 8/16/32-bpc. |
| HDR / EXR frames | `exr` 1.74 (+ ASWF OpenEXR Rust binding) | Read/write EXR sequences (f16/f32, multi-layer, deep). |
| Effects host | `prism-fx` (OpenFX-style, suite-shared) | Blur/color/distort/generate/keying authored once, run suite-wide. |
| Media (decode/encode/audio) | `prism-media` → `ffmpeg-next`/`rsmpeg` (or `oximedia`) | Import video/audio/image-seq; export MP4/MOV/ProRes; shared with Reel. |
| Keyframe easing | `kurbo` (Bézier) + `keyframe`/`bezier_easing` | Temporal Bézier ease, hold/linear/auto-Bézier; spatial motion paths via `kurbo`. |
| Expressions | `rhai` (default) / `rune` | Sandboxed per-property expression engine; `wiggle/time/loopOut`-style API. |
| Text / type | `cosmic-text` + `swash`/`harfrust` | Text layers, per-char layout for animators, OpenType + variable fonts. |
| Vector (shape layers / masks) | `kurbo` + `lyon` + `i_overlay` (→ shared `prism-vector`) | Mask paths, shape-layer paths, trim/repeater; same primitives as Contour. |
| AI (roto / matting) | `ort` (shared `pigment-ai`) | Roto Brush via SAM2/3 + matting; feature-gated, models on demand. |
| Undo / project | `undo`/custom + `serde` | Command stack over the comp/graph; `.pulse` project IO. |
| Util | `glam`, `bytemuck`, `rayon` | Math, GPU casts, multi-frame parallel render. |

---

## 3. Architecture (target)

```
┌──────────────────────────────────────────────────────────┐
│  pulse-app  (eframe + egui)                              │
│  panels: project · comp/preview · timeline · effects ·   │
│          graph editor · properties · render queue        │
├──────────────────────────────────────────────────────────┤
│  motion model                                            │
│  Comp · Layer{solid,text,shape,footage,adj,null,cam,light}│
│  Property<T>(time→value): keyframes + easing + expression │
│  Mask · TrackMatte · CommandStack(undo)                  │
├──────────────────────────────────────────────────────────┤
│  prism-core   time-addressable render graph (compositor, │
│               tiles, 18 blend modes, adjustments)        │
│  prism-color  linear-light + OpenColorIO + EXR           │
│  prism-fx     OpenFX-style effects (suite-shared)        │
│  prism-media  FFmpeg decode/encode + audio (w/ Reel)     │
│  prism-vector kurbo/lyon/i_overlay (masks, shape layers) │
├──────────────────────────────────────────────────────────┤
│  Dynamic Link: place a .contour artboard / .pigment doc /│
│  nested comp as a live layer, re-rendered on demand      │
└──────────────────────────────────────────────────────────┘
```

### Core data model (target)
- **Property<T>** — the atom: a value that is a function of time, defined by **keyframes** (with temporal
  + spatial interpolation) **or** an **expression** (evaluated per frame). Generalize today's `Track`
  (linear-only `f32`) into typed properties (scalar/2D/3D/color/path) with selectable interpolation.
- **Layer** — a typed source (solid/text/shape/footage/adjustment/null/camera/light/**precomp**) + a
  Transform group (anchor, position [2D/3D, separable], scale, rotation/orientation, opacity) + an
  ordered **effect stack** + **masks** + a **track-matte** ref + blend mode + motion-blur/3D flags.
- **Comp** — sized/timed canvas + layer stack + camera/lights; nestable (a comp is a valid layer
  source = **precomp**). The render graph evaluates `comp(t)` → tiles, cached per (node, frame, tile).

---

## 4. Phased backlog (toward ≥85% parity)

Effort tags **S/M/L**. "shared?" = touches/promotes a `prism-*` crate → keep app-agnostic & time-free.
Phase 0 is **done** (see §1); the rest is the road to parity.

### Phase 0 — Skeleton: comp, timeline, transport  *(DONE)*
- [x] Comp model (size/duration/fps + layers), 5 transform tracks, linear keyframes + hold
- [x] Timeline (ruler, per-layer lanes, keyframe diamonds, draggable playhead, scrub)
- [x] Transport (play/pause/loop), CPU preview of solid layers, `.pulse` save

### Phase 1 — Real property system + GPU compositor  *(the foundation rebuild)*
- [ ] **Typed `Property<T>`** (L): scalar/2D/3D/color/path; generalize `Track`; **anchor point** + **separable XYZ position**
- [~] **Keyframe interpolation** (M): linear / **hold** / **Bézier ease** + **Easy Ease**/In/Out **done** (per-key `Interp` on the segment, Newton-solved CSS-`cubic-bezier`, unit-tested; UI picker + timeline markers); auto-Bézier and draggable per-key in/out handles pending (land with the Graph Editor)
- [~] **Graph Editor** (M): value-curve editor **done** (per-layer value-over-time curves on a shared auto-framed axis, draggable keyframes with live re-sort, draggable per-key Bézier in/out ease handles incl. promoting Linear/Hold segments, per-property show/hide, scrub); still TODO: a dedicated **speed graph**, **roving keyframes**, and **auto-Bézier**
- [x] **Preview fidelity / render-preview** (M) — **done**: the interactive preview shows **real composited pixels** (footage frames, precomps, effects, color-correction, masks, track mattes, motion blur, time-remap, expressions), not placeholder quads. It renders the comp at the playhead through the **existing CPU offline compositor** (`render::render_preview_frame` → `render_comp`, project-aware, cycle-guarded) at a **capped preview resolution** (1280 px long edge, aspect-preserved; `preview_dims`), uploads it as an egui texture, and draws it as the preview image; the gizmo / selection / mask / null / adjustment overlays + onion-skin ghosts draw on top via the painter, pixel-aligned through the same aspect-fit mapping. Rendered frames are held in a **RAM-preview cache** (frame-indexed, `PreviewRenderer`) filled by a **pool of background worker threads** off the UI thread, so **playback/scrubbing never block input** and a fully-cached comp **loops in real time** straight from RAM; each worker keeps a persistent footage `FrameCache` (export keeps its own per-run cache). The cache is invalidated by a `(comp-state hash, render dims)` signature — any edit/resize re-fills, and stale in-flight jobs are skipped via a cache **epoch** — and bounded by a **~1 GiB budget** (frames farthest from the playhead evicted first, so short comps fit whole and long comps keep a window cached). Playback is real-time but **cache-gated**: the playhead holds on a frame until it's rendered, so it never outruns the cache. State-hash invalidation + pool render / stale-skip + cache readiness (`is_frame_ready` / `fully_cached`) + mapping round-trip + cap + decode-once reuse unit-tested. Still TODO: this is still the **CPU** compositor — the **GPU compositor** below would give true realtime on the *first* pass; **disk cache** + smart purge, **preview-resolution controls** (full/half/quarter), and cutting per-frame cost (e.g. fewer preview motion-blur samples) are the remaining levers (Phase 6 *Caching* / *Preview controls*).
- [ ] **GPU compositor** (L, shared `prism-core`): move preview onto the suite's wgpu render graph; **18 blend modes**; float (16/32-bit) buffers; linear-light
- [ ] **Layer types v1** (M): Solid, **Adjustment**, **Null**; precomp stub
- [ ] Tests: interpolation parity, blend-mode pixels, time-sampling determinism

### Phase 2 — Layers, masks, mattes, precomps  *(compositing core)*
- [~] **Footage layers** (L, shared `prism-io` now / `prism-media` next): **stills + numbered image sequences done** (`LayerKind::Footage` drawing a `FootageSource` — a single still or a printf-pattern image sequence — decoded through the shared `prism-io` `load_image`; **time-indexed frame sampling** with an optional **fps override** and **loop** / **hold-last** end behaviour; auto-sequence detection from any one picked frame; an MRU **decode-once `FrameCache`**; sRGB→linear at the gamma boundary; **alpha interpretation** straight/premultiplied/ignore; full premultiplied-linear-light compositor path so footage composes with transform/anchor/parent/opacity/blend/masks/track-mattes/spatial-effects/motion-blur; Properties section + *Layer ▸ New ▸ Footage* + *File ▸ Import footage…*; pure + compositor unit-tested); still TODO: **real video decode (FFmpeg)** + **audio** via the shared **`prism-media`** crate (the next footage step), footage color/OCIO interpretation, and proxies/placeholders
- [~] **Text layers** (M): a self-contained **stroke vector font** **done** (`LayerKind::Text` drawing a string with a dependency-free built-in font — A–Z, 0–9, space, common punctuation — with **font size / tracking / leading / alignment** and a **fill** + **stroke**; multi-line layout flattened to layer-local stroke segments, rasterized as an antialiased thickened pen band into the isolated premultiplied buffer so text composes with masks/mattes/spatial effects/motion blur; Properties editor + preview + launch-demo title; pure layout + coverage unit-tested); still TODO: real `cosmic-text` shaping, **per-character layout** (animator-ready), and variable fonts
- [~] **Shape layers** (M, shared `prism-vector`): parametric primitives **done** (`LayerKind::Shape` drawing a bottom-up stack of `ShapeItem`s — **Rectangle** (rounded), **Ellipse**, **Polygon**, **Star** — each with an antialiased **fill** and **stroke**; rasterized in the layer's local frame into the isolated premultiplied buffer so shapes compose with masks/mattes/spatial effects/motion blur; Properties editor + preview + launch-demo star; pure geometry + compositor unit-tested); still TODO: **arbitrary Bézier paths**, **trim paths**, **repeater**, merge, offset, wiggle-path, and path keyframing (land with the typed-`Property<Path>` rebuild + `prism-vector`)
- [~] **Masks** (L): Bézier mask paths per layer **done** (closed `Mask` of `MaskVertex` in layer-local space, rect/ellipse seeds, flatten → even-odd coverage; modes **add/subtract/intersect/difference** + none; **feather**, **expansion**, **opacity**, **invert**; folded per-pixel into the layer's alpha in the software compositor, composing with motion blur + track mattes; preview outlines + Properties editor — all pure logic unit-tested); still TODO: **animated mask shapes** (keyframable `Property<Path>`), variable-width feather, and on-canvas vertex editing
- [ ] **Track mattes** (M): alpha / luma (inverted) mattes; preserve-underlying-transparency; stencil/silhouette
- [~] **Precomps** (M): nesting **done** (`LayerKind::Precomp` referencing a sibling comp by id + a scalar **time-offset** shift; the document is now a **`Project`** of id-keyed comps with one active; the software compositor renders the referenced comp **recursively** through the same render path — sampling the rendered nested frame into the layer's quad like footage — so a precomp composes with transform/anchor/parent/opacity/blend/masks/track-mattes/spatial-effects/motion-blur; a per-render **visited-set cycle guard** breaks reference cycles A→B→A and self-references — they render nothing rather than infinite-loop/overflow; **pre-compose** (single layer → new comp + precomp reference) + a Properties source-comp/time-offset section + *Layer ▸ New ▸ Precomp*; project save round-trips precomp refs; serde-defaulted for back-compat; render + model unit-tested); still TODO: **multi-layer pre-compose** (selection set, preserving inter-layer parenting), **collapse transformations**, and a **comp navigator**/tab UI (live nested render now shows in the preview — the **render-preview** renders precomps recursively into real pixels, see Phase 1 *Preview fidelity*; full **time-remapping** — a keyframable remap curve, not just the `time_offset` shift — landed; see Phase 4 *Time remapping*)
- [ ] **Null / parenting** (S): parent transforms, pick-whip parenting
- [ ] Tests: matte compositing, mask boolean modes, precomp re-eval, footage decode round-trip

### Phase 3 — Effects at scale (`prism-fx`)  *(the AE effect surface)*
Build effects on a unified **`prism-fx`** OpenFX-style GPU pass registry (suite-shared) so each is
authored once and stacks non-destructively per layer.
- [ ] **Effect engine** (M): per-layer ordered effect stack; effect params are full `Property<T>` (keyframable/expressable); effect masks
- [~] **Color correction** (M, reuse `prism-core::adjust`): **done** — Tint, Brightness/Contrast, Exposure, Levels, **Hue/Saturation** (HSL rotate/saturate/lighten), **Curves** (5-point Catmull-Rom master curve), **Color Balance** (per-range shadow/midtone/highlight pushes), **Channel Mixer** (per-output-channel RGB + constant mix, monochrome collapse — reuses the shared `prism_core::adjust::ChannelMixerMatrix::apply`), **Gradient Map** (luma → three-stop color gradient via the shared multi-stop `prism_core::gradient::Gradient::color_at`; black→first stop, mid→mid, white→last, original→mapped `amount` blend), **Tritone** (the same shared gradient primitive authored as shadows/midtones/highlights three-tone grade), all pure straight-RGBA linear-light passes in the per-layer stack, unit + render-path tested; still TODO: **Lumetri-style** grade
- [~] **Blur & sharpen** (M): **Gaussian blur** done (separable, per-axis sigma, repeat-edge, premultiplied so soft edges don't bleed — unit-tested; runs as a whole-buffer pass after color-correction/masks/matte, composes with motion blur); **Box Blur** done (separable moving-average, radius + 1..=8 **iterations** — ~3 ≈ Gaussian via central-limit — repeat-edge, premultiplied); **Directional Blur** done (1-D box average along an angle, the motion streak — perpendicular axis stays crisp, bilinear sub-pixel taps); **Radial Blur** done (**Spin** rotational + **Zoom** dolly streak about a centre, amount, symmetric taps so the centre stays sharp) — all three on the same `SpatialEffect` infrastructure (enum + `apply` pass + Properties *Blur* editor [Spin/Zoom picker] + Effects-browser *Blur & Sharpen* folder), pure/deterministic, unit + render-path tested, compose with masks/matte/keying/distort/motion-blur. **Camera-Lens / Fast Box** (bokeh), **Smart Sharpen**, **CC**-style still TODO
- [~] **Distort** (M): **Corner Pin / Transform / Mirror / Polar Coordinates done** — the first **distort** stack (whole-buffer **coordinate-remap** passes that *warp* the layer's rendered pixels, mirroring the spatial-effect family exactly: a `DistortEffect` enum + `apply_distort_effects` pass + `apply_distort` compositor bridge + Properties *Distort effects* section + Effects-browser *Distort* category). Each is an inverse-warp resampler over the layer's **isolated premultiplied linear-light** buffer (bilinear, off-buffer→transparent), run **after** the spatial passes (AE's distort-below-blur order) so it composes with opacity / blend / masks / track-mattes / spatial-effects / motion-blur; positions in normalized buffer space `[0,1]²` (preview = export). **Corner Pin** (inverse-bilinear four-point pin), **Transform** (effect-level anchor/position/scale/rotation/**skew**/opacity), **Mirror** (reflect across a line), **Polar Coordinates** (Rect↔Polar + interpolation blend). Pure remap + render-path unit-tested. Still TODO: **Mesh/Bezier warp**, **Displacement map**, **Turbulent/Wave**, **Optics-comp** (and Warp).
- [~] **Generate** (M): **Fractal/Turbulent Noise** done (the motion-design workhorse — deterministic multi-octave hash-seeded gradient noise, **Basic/Turbulent** type, contrast/brightness, uniform + X/Y **scale**, **complexity** octaves, **sub-influence/sub-scaling** persistence/lacunarity, **keyframable evolution** track, **seed**, **overflow** Clip/Wrap/HDR); **Gradient/Ramp** done (Linear + Radial colour ramp, start/end points + radius, endpoint-clamped, optional deterministic ramp **scatter**); **Checkerboard** done (two colours, per-axis cell size + anchor, `rem_euclid` parity); **4-Color Gradient** done (four corner colours bilinearly blended, **blend** sharpness + deterministic **jitter**); **Grid** done (line grid — per-axis cell size, border width, line + background colours, transparent or filled background, anchor) — all on the same generate infrastructure (`GenerateEffect` enum + `composite_generate` pass + Properties *Generate* section w/ a generator picker + Effects-browser *Generate* category), each a per-layer fill that replaces the layer's pixels (colour generators sRGB→linear-decoded, Fractal Noise grayscale-linear), runs in the compositor + render-preview, unit + render-path tested. **Cell Pattern, Lightning/Beam, Lens Flare, Audio-Spectrum/Waveform** still TODO
- [~] **Keying** (L): **Color Key / Luma Key / Chroma Key / Spill Suppression / Matte Choke done** — the first **keying** stack (whole-buffer **matte-pull** passes that *carve the layer's alpha* from a per-pixel colour test, mirroring the spatial/distort families exactly: a `KeyEffect` enum + `apply_key_effects` pass + `apply_key` compositor bridge + Properties *Keying* section + Effects-browser *Keying* category). Each operates on the layer's **isolated premultiplied linear-light** buffer (un-premultiply → test straight colour → re-premultiply by the new coverage), run **after** masks + track-matte but **before** the spatial passes (AE's keyer-then-blur matte-refine order) so a key pulls the matte first and a later blur softens the edge; composes with opacity / blend / masks / track-mattes / spatial / distort / motion-blur (offline + preview). **Color Key** (RGB-distance tolerance + softness feather), **Luma Key** (Rec.709 luminance threshold, key high/low + softness), **Chroma Key** (Keylight-style YCbCr chroma-plane distance, luminance-independent, gain/balance/softness), **Spill Suppression** (pull the dominant key channel toward the others, alpha untouched), **Matte Choke** (erode/dilate morphology + clip-black/clip-white). Pure matte math + render-path unit-tested. Still TODO: **Difference Matte**, **advanced matte refine** (colour-aware edge feather / grow), **Inner/Outer edge keying**, Linear-colour key.
- [~] **Stylize / Channel / Matte / Time** (M): **Glow** done (threshold→blur→screen bloom, unit-tested); Find-Edges, Mosaic, Posterize; channel combiner/shift/invert; matte choke/simple-choker; **Echo**, Posterize-Time, **Time Displacement** still TODO
- [~] **Perspective / Simulation** (M/L): **Drop Shadow** done (angled/offset blurred tint behind the layer, shadow-only, unit-tested); Bevel, **Particle** system (CC-particle-style), Shatter/Card-dance still TODO
- [ ] **Presets / animation presets** (S): save an effect+keyframe stack as a named preset
- [ ] Tests: golden-frame per effect; keyer matte quality; noise determinism (seeded)

### Phase 4 — Motion, time & expressions  *(makes it move like AE)*
- [ ] **Motion blur** (M): per-layer + comp shutter angle/phase, samples; on-transform and on-effect
- [ ] **Frame blending** (S): frame-mix + pixel-motion (optical-flow later)
- [~] **Time remapping** (M): **done** — a time-based layer (footage image-sequence / precomp) carries an optional, keyframable **time-remap** `Track` (source times in seconds, `serde`-defaulted **disabled** → back-compat) that, when enabled, drives the **source time** it is sampled at instead of the comp time; wired through the footage frame-index/sampling path *and* the precomp recursive-render time (transforms/opacity stay on comp time; fps-override/loop/hold + the precomp `time_offset` honoured at the remapped time; motion-blur sub-frames + footage/precomp matte sources remapped too), so users can **freeze** (constant remap), **reverse** (decreasing remap), and **slow/speed** playback — and via expressions, since the track carries them; enabling seeds AE-style default keys (identity ramp 0→source-duration, eased; single identity key when the duration is unknown); an "Enable Time Remap" toggle + the remap value as a keyframable property in Properties; pure + render-path unit-tested. Still TODO: **time stretch** (a layer-level speed/duration multiplier), and **frame blending** (frame-mix / pixel-motion interpolation) so a slowed/retimed source interpolates between frames rather than stepping — see the Frame-blending item above (the natural follow-on)
- [ ] **Spatial motion paths** (M): position keys draw an editable Bézier path; auto-orient along path; roving in time
- [~] **Expressions** (L): per-property `rhai` engine **done** (first slice) — any animatable **scalar** property (anchor/position/scale/rotation/opacity) carries an optional `serde`-defaulted `expression: Option<String>`; at sample time it's evaluated against a context exposing **`time` / `value` / `fps` / `duration` / `index`** plus helpers **`wiggle`** (deterministic per (layer, time) — stable-hash seeded, *not* `Math.random`), **`linear`**, **`clamp`** (and rhai's `sin/cos/abs/floor/…`); the keyframed value is exposed as `value` so expressions offset/drive it; compiled ASTs are cached per string; a parse/eval error **falls back to the keyframed value without panicking** and is surfaced in the UI; wired through the **real compositor + preview** (position/scale/rotation/anchor/opacity, parent chain, motion-blur sub-frames, matte sources); `fx` toggle + per-property expression field with a red error state in the Properties panel; engine + integration + render-path unit-tested. Still TODO: the broader AE library (**`loopOut/In`**, **`ease`**, **`random/seedRandom`**, **`valueAtTime`**, **`thisComp/thisLayer`**), **pick-whip property links**, and expressions on **non-scalar** properties (2D/3D/color/path) + effect/mask params (land with the typed-`Property<T>` rebuild)
- [x] **Markers** (S): comp + layer markers, work-area, time navigation — **done**:
  pure `Marker` (time / duration / label / color) + `WorkArea` (`[start,end]` with
  clamp / length / contains / is-full) in `comp/marker.rs`; `Comp` carries `markers`
  + `work_area` and `PulseLayer` carries `markers` (all `serde`-defaulted +
  self-healing back-compat). **Playback loops the work area**; transport
  prev/add/next-marker buttons + AE keys `B`/`N`/`M`; Comp ▸ Work area / Markers
  menus; a Properties **Markers** section; timeline draws comp markers on the ruler,
  layer markers per lane, the work-area band + a dimmed-outside playhead. Pure +
  comp-level navigation/clamp/serde unit-tested. **Export now renders the
  work-area range only** (After Effects' default render range — `RenderRange`
  WorkArea/Full + auto-default, files numbered by comp frame index so the first
  exported frame is the work-area start; *File ▸ Render range…* picker; unit-tested).
  Still open: marker-snapping, on-timeline marker dragging
- [~] Tests: **time-remap sampling done** (identity == no-remap, reverse `t→dur−t`, freeze hold, easing, expression-driven, default-key seeding, serde, render path); expression evaluation parity (scalar slice) done; **markers + work-area done** (marker model, work-area clamp/length/contains/is-full, marker navigation incl. comp+selected-layer scope, serde + back-compat); motion-blur sample count **Planned**

### Phase 5 — 3D compositing  *(depth, camera, light)*
- [ ] **3D layers** (L): per-layer Z, 3D position/orientation/rotation, anchor; 2D/3D toggle
- [ ] **Camera** (M): one-/two-node camera, focal length / FOV, depth of field (focus distance/aperture/blur)
- [ ] **Lights** (M): point/spot/parallel/ambient; intensity/color/cone; **shadows** (shadow catcher)
- [ ] **3D renderer** (L): a classic-3D compositing renderer (sorted, with intersection/shadows where feasible); optional extruded text/shapes later
- [ ] **Environment / material** (S): per-layer material options (accepts/casts shadows/lights, specular)
- [ ] Tests: camera projection, light/shadow correctness, z-sort

### Phase 6 — Media IO, render queue, audio  *(opens/exports everything)*
- [ ] **Import** (M, shared `prism-media`): video (H.264/H.265/ProRes/VP9/AV1), image sequences (PNG/JPEG/**EXR**/DPX/TIFF), audio (WAV/AAC/MP3), still images, SVG (→ shape layers), `.pigment`/`.contour` (via Dynamic Link)
- [ ] **Render queue** (L): multiple render items, **output modules** (codec/format/color/range), render settings (quality/resolution/proxy), background + **multi-frame rendering** (parallel via `rayon`)
- [ ] **Export formats** (M): PNG/EXR/DPX/TIFF sequences; MP4/MOV (H.264/H.265/**ProRes**/VP9/AV1) via `prism-media`; animated GIF/APNG/WebP; alpha/straight-vs-premul; **Media-Encoder-style** queue
- [ ] **Audio** (M): waveform display, level keyframes, basic mixing, audio-reactive (drive params from amplitude), audio preview synced to playhead
- [ ] **Color-managed output** (M, shared `prism-color`): OCIO display/output transforms, EXR scene-linear, broadcast-safe
- [ ] Tests: encode round-trip, EXR sequence fidelity, OCIO transform ΔE bound

### Phase 7 — AI, automation, interop  *(modern + pro + suite glue)*
- [ ] **AI (feature-gated, shared `pigment-ai`/`ort`):** **Roto Brush** (SAM2/3 + matting → animated mask), content-aware fill for video (temporal inpaint), scene-edit detect, AI denoise/upscale, AI motion-track assist — models on demand, graceful no-model path
- [ ] **Motion tracking / stabilization** (L): point/planar tracker, 2D stabilize (`prism-media`/optical-flow); apply track to transform/effect via expressions
- [ ] **Dynamic Link (producer)** (M, suite): expose Pulse comps to Reel; consume Contour artboards / Pigment docs as live layers; re-render on source change
- [ ] **Automation** (M): scripting via `rhai` (project/comp/layer API); render-queue automation; templates / essential-graphics-style parameterized comps
- [ ] **Plugins** (L, shared `prism-fx`): OpenFX effect plugins load across the suite

### Phase 8 — Reliability, performance & ease-of-use  *(production-grade)*
- [~] **Caching** (L): **done** — a **RAM frame cache** (frame-indexed, `PreviewRenderer`) holds each rendered preview frame so the **work-area loops back in real time** once filled (the *RAM-preview* model); invalidated by a `(comp-state hash, render dims)` signature with a cache **epoch** that skips superseded in-flight renders, and bounded by a ~1 GiB **byte budget** with farthest-from-playhead eviction (smart-ish purge). Still TODO: **disk cache** + persistence across sessions, finer purge heuristics, and cache-on-render (share the export render into the preview cache)
- [~] **Multi-frame rendering** (M): **done (frame-level)** — a **worker pool** (cores−2, clamped 1–8) renders preview frames **concurrently across cores** to fill the RAM cache ~N× faster, each worker with its own footage cache, jobs round-robined and epoch-gated. Still TODO: **tile-level** parallelism, a global **GPU-memory budget**, and multi-frame determinism for the render queue / export path
- [ ] **Autosave + crash recovery** (M); **project file** robustness (versioned `.pulse`, relink missing footage)
- [ ] **Preferences / shortcuts / workspaces** (M): full remappable shortcut map (AE muscle-memory), dockable panels, saved workspaces, command palette
- [ ] **Timeline UX** (M): trim handles, layer-bar drag, snapping, dual playheads, in/out points, solo/lock/shy, label colors, search/filter layers
- [ ] **Preview controls** (S): resolution (full/half/quarter), region of interest, channel/alpha view, transparency grid, info/pixel readout, guides/grid/rulers/title-safe
- [ ] Tests: cache-hit correctness, RAM-preview realtime gate, autosave/relink round-trip, multi-frame determinism

---

## 4b. Parity coverage matrix (vs After Effects surface)

| Category | After Effects surface | Status | Phase |
|---|---|---|---|
| Comp / timeline / transport | comps, timeline, play/scrub | **Done** basic + **markers / work-area / time-navigation** (work-area-looped playback **and** work-area-range export); precomp **done** (see Layer types) | 0,2,4 |
| Properties / transform | anchor + 2D/3D position/scale/rot/opacity | **Partial** (5 linear tracks) → typed `Property<T>` **Planned** | 1 |
| Keyframe interpolation / graph editor | linear/hold/Bézier/auto, graph editor | **Partial** (linear/hold/Bézier ease + value-curve graph editor w/ draggable keys & handles; auto-Bézier/speed-graph/roving **Planned**) | 1 |
| Compositor / blend modes | GPU, 18+ modes, 32-bpc, linear | **Partial** (CPU software compositor; **per-layer blend modes** — all 18, reusing `prism-core` — done; GPU/32-bpc **Planned**) | 1 |
| Layer types | solid/text/shape/footage/adj/null/cam/light/precomp | **Partial** (solid, null, adjustment, **shape** [rect/ellipse/polygon/star + fill/stroke], **text** [built-in stroke font + fill/stroke/align/tracking/leading], **footage** [stills + numbered image sequences via `prism-io`, fps override / loop / hold-last / alpha interp], **precomp** [nest a sibling comp + time-offset, recursive render, cycle guard]) → footage **video** (FFmpeg/`prism-media`) / cam / light **Planned**; text shaping/animators **Planned** | 1,2,5 |
| Masks / roto | Bézier masks, modes, feather, roto brush | **Partial** (Bézier masks: add/subtract/intersect/difference, feather, expansion, opacity, invert — done; animated shapes / on-canvas editing / roto brush **Planned**) | 2,7 |
| Track mattes | alpha/luma | **Planned** | 2 |
| Precomps / parenting | nesting, collapse, pick-whip | **Partial** (precomp **nesting** [recursive render + cycle guard + time-offset], **pre-compose** [single layer], pick-whip **parenting**, **time-remap curve** done; **collapse transformations**, multi-layer pre-compose, comp navigator **Planned**) | 2 |
| Effects | ~hundreds; color/blur/distort/generate/keying/stylize/time | **Partial** (color-correction stack — Tint/Bright-Contrast/Exposure/Levels/**Hue-Sat**/**Curves**/**Color Balance** — + spatial **Gaussian Blur / Box Blur / Directional Blur / Radial Blur [Spin+Zoom] / Drop Shadow / Glow** + generate **Fractal Noise** [keyframable evolution] / **Gradient Ramp** [linear+radial] / **Checkerboard** / **4-Color Gradient** / **Grid** + distort **Corner Pin / Transform / Mirror / Polar Coordinates** + keying **Color Key / Luma Key / Chroma Key / Spill Suppression / Matte Choke**; Mesh/Bezier warp · Displacement · Turbulent/Wave · Optics-comp · Cell Pattern / Lightning / Lens Flare / Audio-Spectrum · Difference Matte + the rest **Planned** via `prism-fx`) | 3 |
| Motion blur / frame blend | full | **Planned** | 4 |
| Time remap / stretch | full | **Partial** (keyframable **time-remap curve** on footage-sequence / precomp — freeze/reverse/retime via keys + expressions, default-key seeding, UI toggle; **time stretch** + **frame blending** for retimed sources **Planned**) | 4 |
| Expressions | full JS expression language | **Partial** (scalar props via `rhai`: `time`/`value`/`fps`/`duration`/`index` + `wiggle`/`linear`/`clamp` + math, AST-cached, error fallback + UI `fx` toggle; `loopOut`/`ease`/`random`/`valueAtTime`/`thisComp` + **pick-whip links** + non-scalar props **Planned**) | 4 |
| 3D layers / camera / lights / shadows | classic + advanced 3D | **Planned** | 5 |
| Media import (video/seq/audio/EXR) | full | **Partial** (still images + numbered **image sequences** as footage layers via `prism-io`) → **video** / audio / EXR/DPX **Planned** (`prism-media`) | 2,6 |
| Render queue / output modules / MFR | full | **Planned** | 6 |
| Audio | waveform, levels, reactive | **Planned** | 6 |
| Color management (OCIO/linear/32-bpc) | full | **Planned** (`prism-color`) | 1,6 |
| Motion tracking / stabilize | point/planar/3D camera track | **Planned** | 7 |
| AI (roto/CAF/upscale) | Sensei | **Planned** (feature-gated) | 7 |
| Dynamic Link / interop | live to Premiere, comps | **Planned** (suite) | 7 |
| Automation / scripting / plugins | ExtendScript/UXP, C SDK | **Planned** | 7 |
| Caching / RAM-preview / MFR / autosave / prefs / workspaces | full | **Planned** | 8 |
| Clip-based NLE editing / audio mixer | — | **Won't** (Reel) | — |
| Primary raster paint / deep vector authoring | — | **Won't** (Pigment / Contour) | — |

---

## UI/UX & workspace

What a pro motion app's shell needs that we lack today (fixed four-panel layout, dark
theme, no docking). Phase-8 "Preferences / shortcuts / workspaces" tracks the deep cut;
this section is the concrete, near-term shell work. (Scroll + collapsible Properties
sections landed first — the rest below is unchecked.)

- [ ] **Scrollable panel bodies** — every long panel (Properties, Layers, Timeline lanes) wraps its body in a `ScrollArea` so content never overflows the window. *(done: Properties / Layers / Timeline)*
- [ ] **Collapsible Properties sections** — Transform / Effects / Spatial / Masks / Matte / Text / Shape each its own `CollapsingHeader` so users hide what they don't need. *(done — open by default, per-layer state)*
- [ ] **Dockable panels** — drag panels to re-dock / float / resize (egui `Tile`/`dock` or `egui_dock`); replace the hard-coded `SidePanel`/`TopBottomPanel` layout.
- [ ] **Saveable workspaces** — named layouts (Animation / Effects / Paint-style) persisted to prefs; quick-switch; reset-to-default.
- [x] **Panel show/hide via a Window menu** — **done**: a **Window** menu toggles each dockable panel's visibility (Layers / Properties / Timeline; the central Preview is always present), with *Show all* / *Hide all* shortcuts, backed by a pure unit-tested `PanelVisibility` (`app/workspace.rs`). Per-workspace persistence lands with saveable workspaces.
- [ ] **Tabbed panel groups** — stack panels as tabs in one dock slot (Affinity "Studio" / AE panel groups); drag a tab out to float.
- [ ] **Contextual options strip** — an Affinity-style toolbar row whose controls change with the active tool / selected layer kind.
- [ ] **Timeline panel ergonomics pass** — resizable label/lane columns, horizontal time zoom + scroll, sticky ruler/header, collapse-to-mini, layer search/filter (overlaps Phase-8 Timeline UX but is a focused shell job).
- [ ] **Keyboard shortcuts** — a remappable shortcut map for transport, layer ops, panel toggles (AE muscle-memory: `Space`, `[`/`]`, `T`/`P`/`S`/`R`, `U`); a shortcut-help overlay.
- [ ] **Theme toggle** — light / dark (and high-contrast) switch; today `theme.rs` hard-codes one dark theme. Persist the choice.

## UI/UX & workspace — parity gaps (vs AE + Affinity)

Important features still missing after skimming the plan against After Effects and Affinity
(Photo/Designer). One-line note each; not implemented this turn.

- [x] **Per-layer blend mode picker** — **done**: every layer carries a `serde`-defaulted [`BlendMode`] (reusing `prism-core`'s shared 18-mode set); the CPU compositor blends each layer onto the accumulator via a pure-Rust `blend_over` (W3C blend+composite in linear light, mirroring Pigment's `composite.wgsl`). A **Blend** dropdown in Properties (separable + HSL groups) and a non-Normal blend badge in the Layers panel; unit- + integration-tested. *(Blend modes now show in the live preview — the **render-preview** composites through the same CPU compositor; see Phase 1 *Preview fidelity*.)*
- [x] **Effect search / browser** — **done**: a searchable **Effects & Presets** panel (left-docked, Window-menu toggle, hidden by default) with a type-to-filter search box and **categorised** collapsing folders (Color Correction / Blur & Sharpen / Perspective / Stylize); clicking an effect adds it to the selected layer's matching stack. Backed by a pure, unit-tested registry + ranked token-AND matcher (`comp/effect_browser.rs`, name/keyword search, exact > prefix > substring > keyword scoring, category grouping) that stays in lock-step with both effect stacks' `defaults()`. Drag-onto-layer is the remaining nicety.
- [ ] **Onion-skinning** — ghost neighboring frames behind the playhead (frames before/after, count + opacity falloff) for hand-keyed timing — standard in motion tooling, absent from the plan.
- [x] **On-canvas transform gizmo** — **done**: drag the selected layer directly in the preview to **move / scale / rotate / re-anchor** it (bounding box, four corner scale handles, a rotation knob, and an anchor-point cross), keying the edited local transform at the playhead. The drag math (`gizmo.rs`) maps the pointer screen→comp→**parent-local** so parented layers drag correctly under a rotated/scaled parent; pure (`GizmoGeom::build` / `hit_test` / `drag`) and unit-tested.
- [ ] **Snapping & smart guides in the preview** — snap to layer edges/centers, comp center, and user guides while dragging (plan only lists *timeline* snapping + static guides).
- [ ] **Color / swatch picker reuse** — a shared eyedropper + recent-swatches across fill/stroke/effect colors (Affinity-style), instead of isolated `color_edit_button`s.
- [ ] **Numeric drag-edit on every value field** — click-drag a label to scrub, double-click to type (AE/Affinity convention) for all sliders, not just the slider track.
- [ ] **Undo / redo** — no command history exists yet; a core requirement for any editor (likely a shared `prism-core` edit-stack), gating real editing ergonomics.

---

## 5. Milestones

| Milestone | Phases | Capability | Approx parity |
|---|---|---|---|
| **Scaffold** | 0 | Comp + timeline + linear keyframes + solid preview + save | ~10% *(here today)* |
| **Foundation** | 1–2 | Typed props + Bézier ease + graph editor + GPU compositor + footage/text/shape/masks/mattes/precomps | ~40% |
| **Effects + motion** | 3–4 | + effect stack (`prism-fx`), motion blur, time remap, **expressions** | ~65% |
| **3D + media** | 5–6 | + 3D/camera/lights, media import, render queue/output, audio | **~85%** |
| **Parity+** | 7–8 | + AI/roto, tracking, Dynamic Link, caching/RAM-preview/MFR, autosave/prefs/workspaces | **≥90%** |

**The ≥85% line lands at the end of Phase 6.** Highest felt-parity-per-effort first: **the Phase-1
foundation rebuild (typed properties + Bézier easing + GPU compositor) gates everything** — do it before
breadth. Then footage/masks/precomps (Ph2), then effects + expressions (Ph3–4).

---

## 6. Hard problems (mitigations)

1. **Foundation debt** → today's linear-only `f32` `Track` and CPU solid-rect preview must become typed `Property<T>` + Bézier easing + the shared GPU render graph **before** piling on features; retrofitting interpolation/compositing later = rewrite. This is Phase 1, deliberately first.
2. **Realtime preview** → AE's RAM-preview model: cache rendered frames (RAM + disk), play the work-area back at frame rate; smart purge on edit; multi-frame rendering across cores fills the cache fast.
3. **Effect sprawl** → one `prism-fx` OpenFX-style pass registry (author once, suite-wide, stack non-destructively) instead of bespoke pipelines; effect params are full `Property<T>`.
4. **Color correctness** → linear-light float compositing through `prism-color`; OpenColorIO config-driven display/output transforms; EXR scene-linear; never bake until render.
5. **Media reliability** → `prism-media` (FFmpeg) shared with Reel; robust footage interpretation (alpha/frame-rate/color), proxies, and relink-missing-footage.
6. **Expressions safety/perf** → sandboxed `rhai`/`rune`; cache compiled expressions; evaluate per frame with cycle/error guards; surface errors without crashing the render.
7. **Shared-crate discipline** → the compositor/blend/adjust/color/fx/media crates stay **time-agnostic**; time (keyframes/expressions) is Pulse's layer on top, so Pigment/Contour/Reel reuse them unchanged.
8. **Scope vs Reel** → Pulse is a *layer+keyframe compositor*, not a clip-based NLE; resist building a multi-track audio mixer / clip-trim timeline — that's Reel, fed by Pulse via Dynamic Link.

---

## 7. Immediate next steps

1. [~] **Phase 1 foundation** — **Bézier easing** + hold landed (self-contained Newton-solved cubic, no `kurbo` dep yet); still TODO: generalize `Track`→ typed `Property<T>`; auto-Bézier; anchor-point + separable position.
2. [ ] **GPU compositor** on the shared `prism-core` render graph; 18 blend modes; float buffers; linear-light. Retire the CPU solid-rect preview.
3. [~] **Graph Editor** — value-curve editor with draggable keys + per-key Bézier ease handles landed; **speed graph**, **roving keys**, and **auto-Bézier** still TODO.
4. [~] **Layer types**: Adjustment + Null + **footage** (stills + image sequences via `prism-io`) + **precomps** (nest a sibling comp, recursive render + cycle guard + time-offset, single-layer pre-compose) + **time remapping** (keyframable source-time curve on footage-sequence / precomp — freeze/reverse/retime) done; **footage video** (FFmpeg / `prism-media`), **frame blending** (for retimed sources), **precomp collapse-transformations / multi-layer pre-compose** (Ph2/Ph4) still TODO.
5. [ ] Coordinate **`prism-fx`** (effects host), **`prism-media`** (FFmpeg, shared w/ Reel — the next footage step: real video + audio decode on top of today's stills/sequences), and **`prism-vector`** (masks/shape layers, shared w/ Contour) promotions with the suite before building on them.

*Foundations are free. The product is the polish — and the glue between apps.*
