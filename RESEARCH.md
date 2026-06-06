# Pulse — Research Findings (June 2026)

Cited findings backing [PLAN.md](./PLAN.md). Verify all crate versions against crates.io at build time
— third-party version metadata is sometimes stale. Pulse is **app #3 of the Prism suite**; shared
infrastructure decisions live in [../SUITE.md](https://github.com/KwaminaWhyte/prism-suite-prism/blob/main/SUITE.md) and reuse [Pigment's engine research](https://github.com/KwaminaWhyte/prism-suite-pigment/blob/main/RESEARCH.md)
(compositor, blend math, color, AI runtime).

---

## 1. The render graph is the same one Pigment already runs

After Effects is, structurally, **a keyframed layer compositor with an effect stack**. The compositor —
a DAG of layer/blend/effect/group nodes, evaluated per output **tile**, caching intermediate results and
recomputing only what's dirty — is exactly what `prism-core`/`prism-gpu` already implement for Pigment
(see [../pigment/RESEARCH.md §2](https://github.com/KwaminaWhyte/prism-suite-pigment/blob/main/RESEARCH.md)). Pulse's addition is a **time axis**: every node
is sampled at a frame time `t`, and the cache key becomes (node, **frame**, tile). The **18 blend modes**,
linear-light premultiplied math, and float (Rgba16Float) working buffers transfer unchanged. The
adjustment shaders (`prism-core::adjust`: levels/curves/hue-sat/exposure…) become color-correction
*effects*. **Reuse, don't reimplement** — keep these crates time-agnostic; time is Pulse's layer on top.

Sources: ../pigment/RESEARCH.md §2 (tile compositing, blend math, render graph) · w3.org/TR/compositing-1

## 2. Keyframes, interpolation, easing, graph editor

- Today's `Track` (in `comp.rs`) is **linear-only `f32`** with constant hold outside the range. AE parity
  needs **typed properties** (scalar/2D/3D/color/path) and per-keyframe **interpolation**: linear, **hold**,
  **Bézier** (manual ease), **auto-Bézier**, and **Easy Ease** (the F9 default). Temporal interpolation is a
  cubic Bézier on the (time, value) curve with editable in/out handles — exactly what a **Graph Editor**
  manipulates (value graph + speed graph), plus **roving keyframes** (auto-time to keep constant speed).
- Rust building blocks: **`kurbo`** cubic Béziers give the ease curve and the **spatial motion path**
  (position keyframes trace an editable Bézier the layer travels along, with auto-orient); **`bezier_easing`**
  (port of the standard CSS `cubic-bezier`) and the **`keyframe`** crate provide ready easing/animation
  sequencing to model or validate against. Separable dimensions (X/Y/Z as independent properties) and an
  **anchor point** are required transform additions.

Sources: docs.rs/kurbo · lib.rs/crates/bezier_easing · github.com/hannesmann/keyframe · helpx.adobe.com/after-effects (keyframe interpolation, graph editor)

## 3. Expressions

- AE's expression language is JavaScript over a property/layer/comp object model (`wiggle`, `time`,
  `value`, `loopOut/In`, `linear/ease`, `random/seedRandom`, `valueAtTime`, `thisComp/thisLayer`,
  pick-whip links). Rust embeddable engines: **`rhai`** (pure-Rust, sandboxed, trivial to bind a custom
  API, deterministic — the default choice) and **`rune`** (stack-VM, closer JS-ish ergonomics). Bind a
  `Property`/`Layer`/`Comp` API surface, **compile-and-cache** each expression, evaluate per frame with
  cycle/error guards, and surface errors without aborting the render. A `boa`/`deno_core` JS engine is a
  later option for literal AE-script familiarity.

Sources: rhai.rs · github.com/rune-rs/rune · github.com/boa-dev/boa · helpx.adobe.com (expression basics, expression language reference)

## 4. Effects — one OpenFX-style host for the whole suite

- AE ships hundreds of effects; the durable approach is a **`prism-fx`** OpenFX-style GPU pass registry
  (suite-shared — the same host Pigment's filter galleries and Contour's live-effects use): each effect
  declares params (full `Property<T>`, so keyframable/expressable) + a GPU pass, and stacks
  non-destructively per layer. **OpenFX (OFX)** is the established cross-host image-effect plugin standard
  (Natron/Nuke/Resolve), so adopting its shape buys an ecosystem and authoring-once reuse.
- Priorities by motion-design usage: **color correction** (reuse `prism-core::adjust`), **Gaussian/Camera-
  lens/Directional/Radial blur**, **Fractal/Turbulent Noise** (the single most-used generator), **Gradient/
  Ramp**, **keying** (chroma/luma + spill suppression, Keylight-class), **Glow**, **Echo/Time-Displacement**,
  **Corner Pin / Displacement / Warp**, and a **particle** simulation. Keyers and depth/alpha-aware blurs
  are the quality-sensitive ones; noise/particles need seeded determinism for cache correctness.

Sources: openfx.readthedocs.io · helpx.adobe.com/after-effects/using/effect-list.html · ../pigment/RESEARCH.md §8 (prism-fx, GPU filter passes)

## 5. Color management & HDR (OCIO / OpenEXR)

- Compositing must be **linear-light, float, color-managed**. `prism-color` already does linear/ICC; Pulse
  adds **OpenColorIO** (config-driven input/working/display/output transforms and creative looks — the VFX
  standard). The **Academy Software Foundation Rust Working Group** has shipped an initial **OpenEXR Rust
  binding** (crates.io, Windows/Linux) and targets **OpenColorIO** bindings next; until OCIO bindings
  mature, the pure-Rust **`exr`** crate covers EXR f16/f32 sequences (multi-layer/deep), and a minimal OCIO
  config interpreter can drive the transforms. Work in 32-bpc where it matters; manage display vs render.

Sources: aswf.io (Rust Working Group, OpenEXR Rust binding) · github.com/johannesvollmer/exrs · opencolorio.org · ../pigment/RESEARCH.md §3 (exr, color)

## 6. Media — FFmpeg engine, shared with Reel

- Import/export of video, image sequences, and audio runs through a suite **`prism-media`** crate wrapping
  FFmpeg, **shared with the future Reel** (so the NLE and the compositor decode/encode identically).
  Options: **`ffmpeg-next`** (mature safe wrapper, broad FFmpeg API), **`rsmpeg`** (exposes more of FFmpeg's
  power), **`video-rs`** (higher-level read/write/encode), or the newer pure-Rust **`oximedia`** (FFmpeg+
  OpenCV reconstruction, DPX/EXR/TIFF IO + color science + stabilization, v0.1.7 2026-05 — promising but
  young). Decode to scene-linear/float, honor footage interpretation (alpha, frame rate, color space),
  support proxies and relink-missing. Export targets: PNG/EXR/DPX/TIFF sequences, MP4/MOV (H.264/H.265/
  **ProRes**/VP9/AV1), animated GIF/APNG/WebP — straight vs premultiplied alpha handled explicitly.

Sources: crates.io/crates/ffmpeg-next · github.com/larksuite/rsmpeg · crates.io/crates/video-rs · github.com/cool-japan/oximedia

## 7. Masks, roto, shape layers, text animators

- **Masks / shape layers** reuse the suite's vector primitives (**`kurbo`** Béziers, **`lyon`** tessellation,
  **`i_overlay`** booleans) — ideally the same shared **`prism-vector`** crate Contour promotes, so a mask
  path, a shape-layer path, and a Contour path are one model. Mask modes (add/subtract/intersect/difference)
  are boolean compositions; **mask feather** (incl. variable-width) and expansion are offset operations;
  mask shapes are keyframable `Property<Path>`. **Shape-layer operators** (trim paths, repeater, merge,
  offset, wiggle) are graph operations on the path.
- **Roto Brush** = video object segmentation: reuse the suite **`pigment-ai`/`ort`** runtime with **SAM2/SAM3**
  (temporal/promptable segmentation) + a matting model (BiRefNet) to produce an animated mask, propagated
  across frames; feature-gated, models fetched on demand, graceful no-model fallback (manual masks).
- **Text animators** = per-character layout (from `cosmic-text`) driven by **range selectors** (a window
  over characters) feeding transform/opacity/color offsets — animatable like any `Property`.

Sources: ../contour/RESEARCH.md §1 (kurbo/lyon/i_overlay) · github.com/pykeio/ort · github.com/ZhengPeng7/BiRefNet · github.com/pop-os/cosmic-text · helpx.adobe.com (masks, shape layers, text animators)

## 8. Motion blur, time, 3D

- **Motion blur**: integrate sub-frame samples across the shutter angle/phase (per-layer + comp-level);
  applies to transform and effect motion; AE 2025 added 32-bpc GPU blend modes under motion blur — match by
  accumulating samples in the float compositor. **Frame blending**: frame-mix (cross-dissolve) now,
  pixel-motion (optical flow) later. **Time remapping** = sample a layer's source at a remapped time
  `Property` (enables reverse/freeze/speed-ramp); **time stretch** and **Echo/Time-Displacement** read
  multiple source times.
- **3D**: per-layer Z + 3D orientation, a **camera** (focal length/FOV, depth-of-field via focus
  distance/aperture), **lights** (point/spot/parallel/ambient) and **shadows** (shadow catcher). A
  "classic-3D" compositing renderer (depth-sorted layers with shadows) is the pragmatic first target;
  extruded/ray-traced 3D is a later, larger surface. AE's recent releases lean heavily into 3D (gizmos,
  parametric meshes, Substance materials) — those are explicitly *later/optional* for parity.

Sources: helpx.adobe.com/after-effects (motion blur, time remapping, 3D, what's-new 2025/2026) · cgchannel.com (AE 25.6 3D)

## 9. Caching, multi-frame rendering, reliability

- **RAM preview** is AE's realtime story: render the work-area into a frame cache (RAM + disk spill), then
  play it back at frame rate; purge intelligently on edit (only downstream of the change). **Multi-frame
  rendering** parallelizes frames/tiles across cores (`rayon`) to fill the cache fast and speed final
  render. GPU-memory budget + LRU eviction (same machinery as Pigment's tile cache). Plus the universal
  reliability set: **autosave + crash recovery**, a versioned `.pulse` **project file** with
  **relink-missing-footage**, and a background **render queue** with output modules. These are what make a
  compositor trustworthy, not any single effect.

Sources: github.com/rayon-rs/rayon · helpx.adobe.com/after-effects (multi-frame rendering, RAM preview, render queue) · ../pigment/RESEARCH.md §2 (tile cache LRU / streaming)

## 10. Interop — Pulse is the Dynamic-Link producer

Pulse closes the suite loop: its comps drop **live** into a Reel timeline (Dynamic Link), and it **consumes**
Contour artboards and Pigment documents as layers that re-render on source change — all via the suite's
shared render-graph node model and the `prism-doc` interchange container (see [../SUITE.md](https://github.com/KwaminaWhyte/prism-suite-prism/blob/main/SUITE.md)).
A precomp, a placed `.contour`, and a placed `.pigment` are the *same* mechanism: a graph node that
evaluates a linked document at the requested time/resolution and caches its tiles. Build the node model
suite-aware from the start; define the container with the suite, not unilaterally.

Sources: ../SUITE.md (Dynamic Link, smart objects, prism-doc, shared color/assets)
