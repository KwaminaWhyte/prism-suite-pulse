# Pulse — Open Source After Effects Alternative

Professional motion-graphics, VFX & compositing app in Rust, and **app #3 of the Prism suite** (sibling
to [Pigment](../pigment) raster, [Contour](../contour) vector). **Goal: reach ≥85% of Adobe After
Effects' real-world capability** — features, reliability, and ease-of-use — in staged milestones, on the
suite's shared engine: `prism-core` (layers, blend math, render graph), `prism-color` (linear/OCIO),
and a shared `prism-fx` (OpenFX-style effects) and `prism-media` (FFmpeg) layer.

> Companion docs: [RESEARCH.md](./RESEARCH.md) (cited findings + crate matrix), [../SUITE.md](../SUITE.md) (four-app vision + interop). The repo README (when added) tracks what runs *today*; this PLAN is the parity roadmap.

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
- [ ] **GPU compositor** (L, shared `prism-core`): move preview onto the suite's wgpu render graph; **18 blend modes**; float (16/32-bit) buffers; linear-light
- [ ] **Layer types v1** (M): Solid, **Adjustment**, **Null**; precomp stub
- [ ] Tests: interpolation parity, blend-mode pixels, time-sampling determinism

### Phase 2 — Layers, masks, mattes, precomps  *(compositing core)*
- [ ] **Footage layers** (L, shared `prism-media`): import image / **image sequence** / **video** / audio; footage interpretation (alpha, frame rate, color/OCIO, looping); proxies/placeholders
- [ ] **Text layers** (M): `cosmic-text`; fill/stroke; per-character layout (animator-ready); variable fonts
- [ ] **Shape layers** (M, shared `prism-vector`): paths, fills/strokes, **trim paths**, **repeater**, merge, offset, wiggle-path; path keyframing
- [ ] **Masks** (L): Bézier mask paths per layer; modes (add/subtract/intersect/difference); **mask feather** (incl. variable-width), expansion, opacity; animated mask shapes
- [ ] **Track mattes** (M): alpha / luma (inverted) mattes; preserve-underlying-transparency; stencil/silhouette
- [ ] **Precomps** (M): nest a comp as a layer; pre-compose selection; collapse transformations; comp navigator
- [ ] **Null / parenting** (S): parent transforms, pick-whip parenting
- [ ] Tests: matte compositing, mask boolean modes, precomp re-eval, footage decode round-trip

### Phase 3 — Effects at scale (`prism-fx`)  *(the AE effect surface)*
Build effects on a unified **`prism-fx`** OpenFX-style GPU pass registry (suite-shared) so each is
authored once and stacks non-destructively per layer.
- [ ] **Effect engine** (M): per-layer ordered effect stack; effect params are full `Property<T>` (keyframable/expressable); effect masks
- [ ] **Color correction** (M, reuse `prism-core::adjust`): Levels, Curves, Hue/Sat, Exposure, Brightness/Contrast, Color Balance, Channel Mixer, Tint, Tritone, **Lumetri-style** grade, **gradient map**
- [ ] **Blur & sharpen** (M): Gaussian/Box/**Camera Lens** blur, Directional/Radial, Smart Sharpen, **CC**-style; depth/alpha-aware
- [ ] **Distort** (M): Transform, Warp, Mesh/Bezier warp, Displacement map, Turbulent/Wave, Optics-comp, Corner Pin, Mirror, Polar
- [ ] **Generate** (M): **Fractal/Turbulent Noise** (the motion-design workhorse), Gradient/Ramp, Cell Pattern, 4-Color/Grid, Checkerboard, Lightning/Beam, Lens Flare, Audio-Spectrum/Waveform
- [ ] **Keying** (L): Linear/Color/Luma key, **Keylight-style** chroma key, spill suppression, matte choke/refine, Difference Matte
- [ ] **Stylize / Channel / Matte / Time** (M): Glow, Find-Edges, Mosaic, Posterize; channel combiner/shift/invert; matte choke/simple-choker; **Echo**, Posterize-Time, **Time Displacement**
- [ ] **Perspective / Simulation** (M/L): 3D-ish Drop Shadow/Bevel, **Particle** system (CC-particle-style), Shatter/Card-dance (lower priority)
- [ ] **Presets / animation presets** (S): save an effect+keyframe stack as a named preset
- [ ] Tests: golden-frame per effect; keyer matte quality; noise determinism (seeded)

### Phase 4 — Motion, time & expressions  *(makes it move like AE)*
- [ ] **Motion blur** (M): per-layer + comp shutter angle/phase, samples; on-transform and on-effect
- [ ] **Frame blending** (S): frame-mix + pixel-motion (optical-flow later)
- [ ] **Time remapping** (M): remap a layer's time via a `Property`; **time stretch**, reverse, freeze-frame
- [ ] **Spatial motion paths** (M): position keys draw an editable Bézier path; auto-orient along path; roving in time
- [ ] **Expressions** (L): per-property `rhai`/`rune` engine; the AE staples — `wiggle`, `time`, `value`, `loopOut/In`, `linear/ease`, `random/seedRandom`, `valueAtTime`, `thisComp/thisLayer`, **pick-whip property links**; expression error surfacing + enable/disable
- [ ] **Markers** (S): comp + layer markers, work-area, time navigation
- [ ] Tests: time-remap sampling, expression evaluation parity, motion-blur sample count

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
- [ ] **Caching** (L): RAM **frame cache** + disk cache; smart purge; **RAM-preview** (cache work-area, play back realtime); cache-on-render
- [ ] **Multi-frame rendering** (M): parallelize frames/tiles across cores (`rayon`); GPU-memory budget + eviction
- [ ] **Autosave + crash recovery** (M); **project file** robustness (versioned `.pulse`, relink missing footage)
- [ ] **Preferences / shortcuts / workspaces** (M): full remappable shortcut map (AE muscle-memory), dockable panels, saved workspaces, command palette
- [ ] **Timeline UX** (M): trim handles, layer-bar drag, snapping, dual playheads, in/out points, solo/lock/shy, label colors, search/filter layers
- [ ] **Preview controls** (S): resolution (full/half/quarter), region of interest, channel/alpha view, transparency grid, info/pixel readout, guides/grid/rulers/title-safe
- [ ] Tests: cache-hit correctness, RAM-preview realtime gate, autosave/relink round-trip, multi-frame determinism

---

## 4b. Parity coverage matrix (vs After Effects surface)

| Category | After Effects surface | Status | Phase |
|---|---|---|---|
| Comp / timeline / transport | comps, timeline, play/scrub | **Done** basic; precomp/markers/work-area **Planned** | 0,2,4 |
| Properties / transform | anchor + 2D/3D position/scale/rot/opacity | **Partial** (5 linear tracks) → typed `Property<T>` **Planned** | 1 |
| Keyframe interpolation / graph editor | linear/hold/Bézier/auto, graph editor | **Partial** (linear/hold/Bézier ease + value-curve graph editor w/ draggable keys & handles; auto-Bézier/speed-graph/roving **Planned**) | 1 |
| Compositor / blend modes | GPU, 18+ modes, 32-bpc, linear | **Planned** (reuse `prism-core`) | 1 |
| Layer types | solid/text/shape/footage/adj/null/cam/light/precomp | **Partial** (solid) → **Planned** | 1,2,5 |
| Masks / roto | Bézier masks, modes, feather, roto brush | **Planned** | 2,7 |
| Track mattes | alpha/luma | **Planned** | 2 |
| Precomps / parenting | nesting, collapse, pick-whip | **Planned** | 2 |
| Effects | ~hundreds; color/blur/distort/generate/keying/stylize/time | **Planned** (via `prism-fx`) | 3 |
| Motion blur / frame blend | full | **Planned** | 4 |
| Time remap / stretch | full | **Planned** | 4 |
| Expressions | full JS expression language | **Planned** (`rhai`/`rune`) | 4 |
| 3D layers / camera / lights / shadows | classic + advanced 3D | **Planned** | 5 |
| Media import (video/seq/audio/EXR) | full | **Planned** (`prism-media`) | 2,6 |
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
4. [ ] **Layer types**: Adjustment + Null, then **footage** (`prism-media`) and **precomps** (Ph2).
5. [ ] Coordinate **`prism-fx`** (effects host), **`prism-media`** (FFmpeg, shared w/ Reel), and **`prism-vector`** (masks/shape layers, shared w/ Contour) promotions with the suite before building on them.

*Foundations are free. The product is the polish — and the glue between apps.*
