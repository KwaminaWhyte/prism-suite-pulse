//! Pulse's motion document model.
//!
//! A [`Comp`] is a composition: a fixed-size canvas with a duration and frame
//! rate, holding an ordered stack of [`PulseLayer`]s. Each layer carries seven
//! animatable properties — anchor x/y, position x/y, scale, rotation, opacity —
//! stored as [`Track`]s of [`Keyframe`](keyframe::Keyframe)s, and may be **parented** to another
//! layer (inheriting its transform). Scale and rotation pivot about the layer's
//! **anchor point**; a layer's resolved [`Affine2`] world matrix folds its own
//! transform under its parent chain.
//!
//! Sampling: between two bracketing keyframes the value is interpolated
//! according to the *outgoing* keyframe's [`Interp`] mode — linear, hold
//! (stepped), or a temporal cubic-Bézier **ease** (After-Effects style, with
//! editable in/out handles). Before the first key it holds the first value,
//! after the last it holds the last value (constant extrapolation). An empty
//! track returns the property's sensible default.
//!
//! Layer paint order is bottom-up: index 0 is drawn first (back), the last
//! index on top. Colors are straight sRGB RGBA in `[f32; 4]` so they round-trip
//! cleanly through egui's color picker and JSON.

use serde::{Deserialize, Serialize};

mod blend;
mod effect;
mod effect_browser;
mod expr;
mod footage;
mod keyframe;
mod mask;
mod matte;
mod motion_blur;
mod precomp;
mod shape;
mod spatial;
mod text;
mod time_remap;
mod transform;

pub use blend::{blend_label, blend_over, BlendMode, BlendRgba, LayerBlend};
pub use effect::{apply_effects, Effect, LayerKind};
pub use effect_browser::{filter_grouped, BrowserEntry, NewEffect, Stack};
pub use expr::{last_error as expr_last_error, ExprCtx};
pub use footage::{
    source_from_path, AlphaMode, DecodedFrame, FootageLayer, FootageSource, FrameCache,
};
pub use keyframe::{Ease, Handle, Interp, Track};
pub use mask::{mask_stack_coverage, Mask, MaskMode};
pub use matte::MatteMode;
pub use motion_blur::{MotionBlur, Prop};
pub use precomp::{PrecompLayer, Project};
pub use shape::{Fill, ShapeItem, ShapeLayer, ShapePrimitive, Stroke};
pub use spatial::{apply_spatial_effects, SpatialEffect};
pub use text::{TextAlign, TextLayer};
pub use time_remap::TimeRemap;
pub use transform::{Affine2, Transform};

/// One animated layer: a solid color rect transformed by its tracks, optionally
/// **parented** to another layer (whose transform it inherits).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PulseLayer {
    pub name: String,
    /// What this layer *is* (solid / null / adjustment). `serde`-defaulted to
    /// `Solid` so pre-layer-kind `.pulse` files still load as solids.
    #[serde(default)]
    pub kind: LayerKind,
    /// **Per-layer blend mode** (After Effects' layer blending-mode dropdown):
    /// how this layer's pixels combine with the composite beneath it. Reuses the
    /// suite's shared 18-mode [`BlendMode`] set (`prism-core`), evaluated by the
    /// CPU compositor's [`blend_over`]. Wrapped in [`LayerBlend`] so a missing
    /// field `serde`-defaults to [`BlendMode::Normal`] — pre-blend-mode `.pulse`
    /// files still load and render byte-identically (Normal == source-over).
    #[serde(default)]
    pub blend: LayerBlend,
    /// **Per-layer motion-blur** switch (After Effects' layer MB toggle). A
    /// layer is motion-blurred only when both this and the comp's
    /// [`MotionBlur::enabled`] master switch are on. `serde`-defaulted to `false`
    /// so pre-motion-blur `.pulse` files still load.
    #[serde(default)]
    pub motion_blur: bool,
    /// Solid swatch color (straight sRGB RGBA, 0..=1) for the v0 preview.
    pub color: [f32; 4],
    pub visible: bool,
    /// Non-destructive, ordered **effect stack**. For a solid layer the stack
    /// processes the layer's own pixels; for an adjustment layer it processes
    /// the composite of everything below. `serde`-defaulted to empty for old
    /// projects.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Non-destructive, ordered **spatial effect stack** (whole-buffer passes:
    /// Gaussian Blur, Drop Shadow, Glow). Applied to the layer's isolated
    /// rendered buffer *after* its per-pixel color-correction stack, masks, and
    /// track matte. `serde`-defaulted to empty so pre-spatial-effect `.pulse`
    /// files still load.
    #[serde(default)]
    pub spatial_effects: Vec<SpatialEffect>,
    /// Parent layer index, if this layer is parented. A child inherits its
    /// parent's full transform (position, scale, rotation, anchor) but **not**
    /// its opacity (matching After Effects). `serde`-defaulted so pre-parenting
    /// `.pulse` files still load as unparented.
    #[serde(default)]
    pub parent: Option<usize>,
    /// **Track matte** mode. When active, the layer directly *above* this one in
    /// the stack defines this layer's per-pixel transparency and is itself
    /// removed from normal compositing (matching After Effects). `serde`-defaulted
    /// to [`MatteMode::None`] so pre-matte `.pulse` files still load.
    #[serde(default)]
    pub matte: MatteMode,
    /// **Masks**: closed Bézier paths (layer-local) that carve the layer's
    /// coverage. Folded top-down into a single coverage multiplier on the
    /// layer's alpha (see [`mask_stack_coverage`]). `serde`-defaulted to empty
    /// so pre-mask `.pulse` files still load unmasked.
    #[serde(default)]
    pub masks: Vec<Mask>,
    /// **Shape** content (rectangles / ellipses / polygons / stars with fills
    /// and strokes), drawn only when [`kind`](Self::kind) is
    /// [`LayerKind::Shape`]. `serde`-defaulted to empty so pre-shape `.pulse`
    /// files still load.
    #[serde(default)]
    pub shape: ShapeLayer,
    /// **Text** content (a string drawn with the built-in stroke font), drawn
    /// only when [`kind`](Self::kind) is [`LayerKind::Text`]. `serde`-defaulted
    /// so pre-text `.pulse` files still load.
    #[serde(default)]
    pub text: TextLayer,
    /// **Footage** content (a still image or numbered image sequence on disk),
    /// drawn only when [`kind`](Self::kind) is [`LayerKind::Footage`].
    /// `serde`-defaulted so pre-footage `.pulse` files still load with no source.
    #[serde(default)]
    pub footage: FootageLayer,
    /// **Precomp** reference (target comp id + a time-offset shift), drawn only
    /// when [`kind`](Self::kind) is [`LayerKind::Precomp`]: the referenced comp is
    /// rendered recursively at the mapped time and composited into this layer's
    /// quad. `serde`-defaulted so pre-precomp `.pulse` files still load with no
    /// reference.
    #[serde(default)]
    pub precomp: PrecompLayer,
    /// **Time remap** (After Effects' *Enable Time Remap*): an optional enable
    /// switch + a keyframable scalar track of *source* times. When enabled on a
    /// time-based layer (footage image-sequence / precomp), the source is sampled
    /// at the remapped time instead of the comp time — letting the user freeze /
    /// reverse / retime playback. `serde`-defaulted to disabled (empty track) so
    /// pre-time-remap `.pulse` files load and sample their source unchanged.
    #[serde(default)]
    pub time_remap: TimeRemap,
    // Animated properties. An empty track means "use the default constant".
    /// Anchor-point offset from the layer's geometric center (comp px). The
    /// pivot for scale/rotation and the local point aligned to `(x, y)`.
    #[serde(default)]
    pub anchor_x: Track,
    #[serde(default)]
    pub anchor_y: Track,
    pub x: Track,
    pub y: Track,
    pub scale: Track,
    pub rotation: Track,
    pub opacity: Track,
}

impl PulseLayer {
    /// A new layer with the given name and color and all-empty tracks.
    pub fn new(name: impl Into<String>, color: [f32; 4]) -> Self {
        Self {
            name: name.into(),
            kind: LayerKind::Solid,
            blend: LayerBlend::default(),
            motion_blur: false,
            color,
            visible: true,
            effects: Vec::new(),
            spatial_effects: Vec::new(),
            parent: None,
            matte: MatteMode::None,
            masks: Vec::new(),
            shape: ShapeLayer::default(),
            text: TextLayer::default(),
            footage: FootageLayer::default(),
            precomp: PrecompLayer::default(),
            time_remap: TimeRemap::default(),
            anchor_x: Track::default(),
            anchor_y: Track::default(),
            x: Track::default(),
            y: Track::default(),
            scale: Track::default(),
            rotation: Track::default(),
            opacity: Track::default(),
        }
    }

    /// A new layer of the given kind, name, and color (empty tracks/effects).
    pub fn of_kind(kind: LayerKind, name: impl Into<String>, color: [f32; 4]) -> Self {
        Self {
            kind,
            ..Self::new(name, color)
        }
    }

    /// Borrow the track for `prop`.
    pub fn track(&self, prop: Prop) -> &Track {
        match prop {
            Prop::AnchorX => &self.anchor_x,
            Prop::AnchorY => &self.anchor_y,
            Prop::X => &self.x,
            Prop::Y => &self.y,
            Prop::Scale => &self.scale,
            Prop::Rotation => &self.rotation,
            Prop::Opacity => &self.opacity,
        }
    }

    /// Mutably borrow the track for `prop`.
    pub fn track_mut(&mut self, prop: Prop) -> &mut Track {
        match prop {
            Prop::AnchorX => &mut self.anchor_x,
            Prop::AnchorY => &mut self.anchor_y,
            Prop::X => &mut self.x,
            Prop::Y => &mut self.y,
            Prop::Scale => &mut self.scale,
            Prop::Rotation => &mut self.rotation,
            Prop::Opacity => &mut self.opacity,
        }
    }

    /// Sample one property at time `t`, ignoring any expression (keyframes only).
    pub fn value(&self, prop: Prop, t: f32) -> f32 {
        self.track(prop).sample(t, prop.default_value())
    }

    /// Sample one property at time `t`, **evaluating its expression** if one is
    /// set. `ctx` carries the comp/layer context (fps, duration, layer index);
    /// `ctx.time` should be `t`. The keyframed value is exposed to the expression
    /// as `value`; a parse/eval error falls back to the keyframed value.
    pub fn value_ctx(&self, prop: Prop, ctx: ExprCtx) -> f32 {
        self.track(prop)
            .sample_expr(ctx.time, prop.default_value(), ctx)
    }

    /// Sample the transform properties at time `t` into a [`Transform`],
    /// ignoring expressions. Kept for callers without comp context (the gizmo's
    /// drag-start snapshot, tests). Expression-aware rendering uses
    /// [`Comp::layer_transform`].
    pub fn transform(&self, t: f32) -> Transform {
        Transform {
            anchor_x: self.value(Prop::AnchorX, t),
            anchor_y: self.value(Prop::AnchorY, t),
            x: self.value(Prop::X, t),
            y: self.value(Prop::Y, t),
            scale: self.value(Prop::Scale, t),
            rotation_deg: self.value(Prop::Rotation, t),
            opacity: self.value(Prop::Opacity, t).clamp(0.0, 1.0),
        }
    }

    /// Sample the transform properties at time `t` into a [`Transform`],
    /// **evaluating each property's expression** against `ctx` (one per
    /// property — each sees its own keyframed value as `value`).
    pub fn transform_ctx(&self, ctx: ExprCtx) -> Transform {
        Transform {
            anchor_x: self.value_ctx(Prop::AnchorX, ctx),
            anchor_y: self.value_ctx(Prop::AnchorY, ctx),
            x: self.value_ctx(Prop::X, ctx),
            y: self.value_ctx(Prop::Y, ctx),
            scale: self.value_ctx(Prop::Scale, ctx),
            rotation_deg: self.value_ctx(Prop::Rotation, ctx),
            opacity: self.value_ctx(Prop::Opacity, ctx).clamp(0.0, 1.0),
        }
    }

    /// This layer's resolved [`BlendMode`] (how it composites over the layers
    /// beneath it). [`BlendMode::Normal`] means plain source-over.
    pub fn blend_mode(&self) -> BlendMode {
        self.blend.0
    }

    /// Whether this layer has at least one **active** mask (so the renderer must
    /// run the per-pixel mask-coverage pass for it).
    pub fn has_active_masks(&self) -> bool {
        self.masks.iter().any(Mask::is_active)
    }

    /// Whether this layer has any **spatial effects** (Gaussian Blur / Drop
    /// Shadow / Glow), so the renderer must route it through an isolated buffer
    /// to run the whole-buffer passes.
    pub fn has_spatial_effects(&self) -> bool {
        !self.spatial_effects.is_empty()
    }

    /// Whether this layer is a [`LayerKind::Shape`] with at least one shape
    /// item to draw.
    pub fn has_shape(&self) -> bool {
        self.kind == LayerKind::Shape && !self.shape.is_empty()
    }

    /// Whether this layer is a [`LayerKind::Text`] with text to draw.
    pub fn has_text(&self) -> bool {
        self.kind == LayerKind::Text && !self.text.is_empty()
    }

    /// Whether this layer is a [`LayerKind::Footage`] with a source set.
    pub fn has_footage(&self) -> bool {
        self.kind == LayerKind::Footage && self.footage.is_set()
    }

    /// Whether this layer is a [`LayerKind::Precomp`] with a comp referenced.
    pub fn has_precomp(&self) -> bool {
        self.kind == LayerKind::Precomp && self.precomp.is_set()
    }
}

/// One composition: a sized, timed canvas and its layer stack. A document is a
/// [`Project`] of these; a [`LayerKind::Precomp`] layer references another comp
/// in the same project by [`id`](Self::id).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comp {
    /// Stable identifier within the project — the target a
    /// [`PrecompLayer`](precomp::PrecompLayer) references. `serde`-defaulted to
    /// `0` so old single-comp `.pulse` files (no id) load; the project assigns a
    /// real id on import (see [`Project::from_comp`]).
    #[serde(default)]
    pub id: u64,
    /// A short display name for the comp (shown in the precomp picker / comp
    /// list). `serde`-defaulted so old `.pulse` files load with an empty name
    /// (the UI falls back to a generated label).
    #[serde(default)]
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub duration: f32,
    pub fps: f32,
    /// Composition **motion-blur** settings (master switch + shutter
    /// angle/phase + sample count). `serde`-defaulted so pre-motion-blur
    /// `.pulse` files still load with motion blur off.
    #[serde(default)]
    pub motion_blur: MotionBlur,
    pub layers: Vec<PulseLayer>,
}

impl Comp {
    /// A fresh 1280x720, 5-second, 30fps composition with a parented demo pair.
    pub fn new() -> Self {
        let mut c = Self {
            id: 0,
            name: "Comp 1".to_string(),
            width: 1280,
            height: 720,
            duration: 5.0,
            fps: 30.0,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        };
        // Enable comp motion blur so the demo's fast slide/spin reads with a
        // cinematic shutter out of the box (the sliding solid opts in below).
        c.motion_blur.enabled = true;
        // Seed an animated layer so the preview/timeline aren't empty on launch.
        // The X slide uses Easy Ease so the easing is visible immediately (it
        // eases in and out of the travel rather than gliding linearly), while
        // rotation stays linear for contrast.
        let mut demo = PulseLayer::new("Solid 1", [0.27, 0.55, 0.85, 1.0]);
        demo.x.set_key(0.0, -300.0);
        demo.x.set_key(5.0, 300.0);
        demo.x.set_interp(0.0, Interp::Ease(Ease::EASY));
        demo.rotation.set_key(0.0, 0.0);
        demo.rotation.set_key(5.0, 180.0);
        demo.motion_blur = true; // opt this layer into the comp's shutter
                                 // A soft elliptical mask carves the solid into a feathered oval (sized to
                                 // the layer's base quad), so masks read out of the box.
        let mask_hw = 1280.0 * 0.22; // matches the renderer's LAYER_HALF_FRAC
        let mask_hh = 720.0 * 0.22;
        let mut oval = Mask::ellipse(mask_hw, mask_hh);
        oval.feather = 60.0;
        demo.masks.push(oval);
        c.layers.push(demo); // index 0

        // A smaller satellite parented to Solid 1: it rides the parent's slide
        // and spin while orbiting via its own position offset — showcasing
        // parenting and the anchor-based pivot out of the box.
        let mut satellite = PulseLayer::new("Satellite", [0.95, 0.72, 0.25, 1.0]);
        satellite.parent = Some(0);
        satellite.scale.set_key(0.0, 0.4);
        satellite.x.set_key(0.0, 360.0);
        satellite.y.set_key(0.0, -180.0);
        // An **expression** drives the satellite's rotation: it spins steadily
        // with time and jitters with a deterministic wiggle — so the AE-style
        // per-property expression engine reads on launch (and demonstrates
        // `time` + `wiggle` + offsetting the keyframed `value`).
        satellite.rotation.expression = Some("value + time * 120 + wiggle(3, 15)".to_string());
        // A soft drop shadow + glow on the satellite so the spatial-effect stack
        // (whole-buffer blur/shadow/bloom passes) reads out of the box.
        satellite.spatial_effects.push(SpatialEffect::DropShadow {
            color: [0.0, 0.0, 0.0],
            opacity: 0.55,
            angle: 135.0,
            distance: 16.0,
            softness: 10.0,
            shadow_only: false,
        });
        satellite.spatial_effects.push(SpatialEffect::Glow {
            threshold: 0.5,
            radius: 18.0,
            intensity: 0.9,
        });
        c.layers.push(satellite); // index 1

        // A shape layer: a stroked five-point star drifting up the frame, so the
        // vector shape rasterizer (fill + stroke, parametric primitive) reads out
        // of the box. It slides on its own X position with an Easy Ease.
        let mut star = PulseLayer::of_kind(LayerKind::Shape, "Star", [0.9, 0.3, 0.45, 1.0]);
        let mut star_item = ShapeItem::new(ShapePrimitive::Star {
            points: 5,
            outer: 130.0,
            inner: 56.0,
        });
        star_item.fill = Some(Fill {
            color: [0.95, 0.35, 0.5],
            opacity: 1.0,
        });
        star_item.stroke = Some(Stroke {
            color: [1.0, 1.0, 1.0],
            width: 8.0,
            opacity: 1.0,
        });
        star.shape.items.push(star_item);
        // Screen blend so the star brightens (rather than covers) wherever it
        // crosses the layers beneath it — the per-layer blend mode reads on launch.
        star.blend = LayerBlend(BlendMode::Screen);
        star.x.set_key(0.0, -260.0);
        star.x.set_key(5.0, 260.0);
        star.x.set_interp(0.0, Interp::Ease(Ease::EASY));
        star.y.set_key(0.0, 180.0);
        star.rotation.set_key(0.0, 0.0);
        star.rotation.set_key(5.0, 90.0);
        c.layers.push(star); // index 2

        // A title text layer near the top, drawn with the built-in stroke font:
        // it fades up over the first second (an opacity key) and carries a soft
        // outline, so text layers read out of the box.
        let mut title = PulseLayer::of_kind(LayerKind::Text, "Title", [1.0; 4]);
        title.text = TextLayer {
            text: "PULSE".to_string(),
            size: 150.0,
            tracking: 12.0,
            leading: 0.0,
            align: TextAlign::Center,
            fill: Some(Fill {
                color: [0.96, 0.97, 1.0],
                opacity: 1.0,
            }),
            stroke: Some(Stroke {
                color: [0.27, 0.55, 0.85],
                width: 6.0,
                opacity: 1.0,
            }),
        };
        title.y.set_key(0.0, -230.0);
        title.opacity.set_key(0.0, 0.0);
        title.opacity.set_key(1.0, 1.0);
        title.opacity.set_interp(0.0, Interp::Ease(Ease::EASY));
        c.layers.push(title); // index 3

        // A full-frame adjustment layer on top: its effect stack regrades every
        // layer beneath it (here a punchy Levels contrast) without drawing any
        // pixels of its own — showcasing layer kinds + the effect stack on launch.
        let mut grade = PulseLayer::of_kind(LayerKind::Adjustment, "Grade", [1.0; 4]);
        grade.scale.set_key(0.0, 3.0); // cover the whole frame
        grade.effects.push(Effect::Levels {
            in_black: 0.05,
            in_white: 0.85,
            gamma: 1.1,
            out_black: 0.0,
            out_white: 1.0,
        });
        c.layers.push(grade); // index 4
        c
    }

    /// An empty comp with the given name and canvas/timeline matching `like`
    /// (size, duration, fps) but no layers and no demo content — the container a
    /// **pre-compose** drops the wrapped layers into. Its `id` is `0` until the
    /// project assigns one on [`Project::push_comp`].
    pub fn empty_like(name: impl Into<String>, like: &Comp) -> Self {
        Self {
            id: 0,
            name: name.into(),
            width: like.width,
            height: like.height,
            duration: like.duration,
            fps: like.fps,
            motion_blur: MotionBlur::default(),
            layers: Vec::new(),
        }
    }

    /// A short label for the comp: its `name`, or a generated `Comp <id>` when
    /// unnamed (old files / freshly minted comps).
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            format!("Comp {}", self.id)
        } else {
            self.name.clone()
        }
    }
}

impl Comp {
    /// The **world** affine matrix of layer `idx` at time `t`: its own local
    /// transform composed under every ancestor's transform (parent applied
    /// outermost), mapping the layer's local-space points into final comp space.
    ///
    /// Walks the parent chain defensively: out-of-range or self-referential
    /// parents are ignored, and a `visited` set breaks any cycle (a corrupt
    /// project can't hang the renderer), so the worst case is a finite, bounded
    /// walk producing the longest acyclic prefix.
    pub fn world_matrix(&self, idx: usize, t: f32) -> Affine2 {
        let mut visited = vec![false; self.layers.len()];
        let mut cur = idx;
        let mut m = Affine2::IDENTITY;
        loop {
            let Some(layer) = self.layers.get(cur) else {
                break;
            };
            if visited[cur] {
                break; // cycle guard
            }
            visited[cur] = true;
            // Parent applies outermost: world = parent_world · ... · local. Each
            // layer in the chain samples its own transform with **its own**
            // expression context (its index), so an expression on a parent drives
            // the child through the chain exactly as in After Effects.
            m = layer.transform_ctx(self.expr_ctx(cur, t)).local_matrix().then(m);
            match layer.parent {
                Some(p) if p != cur && p < self.layers.len() => cur = p,
                _ => break,
            }
        }
        m
    }

    /// The expression-evaluation context for layer `idx` at time `t`: the comp's
    /// `fps` / `duration` and the layer's stack index. `value` is filled in per
    /// property by the track sampler (overridden to the keyframed sample).
    pub fn expr_ctx(&self, idx: usize, t: f32) -> ExprCtx {
        ExprCtx {
            time: t,
            value: 0.0,
            fps: self.fps,
            duration: self.duration,
            index: idx,
        }
    }

    /// Layer `idx`'s sampled [`Transform`] at time `t`, **expression-aware**
    /// (each transform property evaluates its expression against the layer's
    /// context). The renderer/preview use this instead of [`PulseLayer::transform`]
    /// so expressions drive position / scale / rotation / anchor / opacity.
    pub fn layer_transform(&self, idx: usize, t: f32) -> Transform {
        match self.layers.get(idx) {
            Some(layer) => layer.transform_ctx(self.expr_ctx(idx, t)),
            None => Transform {
                anchor_x: 0.0,
                anchor_y: 0.0,
                x: 0.0,
                y: 0.0,
                scale: 1.0,
                rotation_deg: 0.0,
                opacity: 1.0,
            },
        }
    }

    /// Layer `idx`'s sampled (and clamped) **opacity** at time `t`, expression-
    /// aware — the value the rasterizers scale coverage by. `0.0` for a missing
    /// layer. Reads it off the resolved [`Transform`] so it always matches
    /// [`layer_transform`](Self::layer_transform).
    pub fn layer_opacity(&self, idx: usize, t: f32) -> f32 {
        if self.layers.get(idx).is_none() {
            return 0.0;
        }
        self.layer_transform(idx, t).opacity
    }

    /// The **source time** layer `idx` should sample its time-based source at,
    /// given comp time `t`. When the layer's [`TimeRemap`] is active this is the
    /// remap track's (expression-aware) value at `t`; otherwise it is `t`
    /// unchanged (identity — every non-remapped layer behaves exactly as before).
    ///
    /// The renderer routes footage frame-indexing and precomp recursion through
    /// this so an enabled remap freezes / reverses / retimes the source. A missing
    /// layer returns `t` (identity).
    pub fn layer_source_time(&self, idx: usize, t: f32) -> f32 {
        match self.layers.get(idx) {
            Some(layer) => layer.time_remap.source_time_ctx(self.expr_ctx(idx, t)),
            None => t,
        }
    }

    /// Sample layer `idx`'s property `prop` at time `t`, expression-aware. Used by
    /// the UI to show the live (expression-resolved) value. `default_value` for a
    /// missing layer.
    pub fn layer_value(&self, idx: usize, prop: Prop, t: f32) -> f32 {
        self.layers
            .get(idx)
            .map(|l| l.value_ctx(prop, self.expr_ctx(idx, t)))
            .unwrap_or_else(|| prop.default_value())
    }

    /// Whether layer `idx` is rendered with **motion blur**: the comp's master
    /// [`MotionBlur::enabled`] switch is on *and* the layer has its own
    /// per-layer `motion_blur` flag set. A missing index is `false`.
    pub fn layer_motion_blurred(&self, idx: usize) -> bool {
        self.motion_blur.enabled && self.layers.get(idx).is_some_and(|layer| layer.motion_blur)
    }

    /// The index of layer `idx`'s **matte source** — the layer directly above it
    /// in the stack (next-higher index) — when `idx` has an active [`MatteMode`]
    /// and such a layer exists. `None` if the layer has no matte or sits at the
    /// top of the stack (no layer above to borrow).
    pub fn matte_source(&self, idx: usize) -> Option<usize> {
        let layer = self.layers.get(idx)?;
        if !layer.matte.is_active() {
            return None;
        }
        let src = idx + 1;
        (src < self.layers.len()).then_some(src)
    }

    /// Whether layer `idx` is **consumed as a matte source** by the layer
    /// directly below it (so it must not composite on its own). True iff the
    /// layer below (`idx - 1`) has an active matte mode.
    pub fn is_matte_source(&self, idx: usize) -> bool {
        idx.checked_sub(1)
            .and_then(|below| self.layers.get(below))
            .is_some_and(|below| below.matte.is_active())
    }

    /// Whether making `child` a parent of `parent` is legal: a layer can't
    /// parent to itself, to a missing layer, or to one of its own descendants
    /// (which would create a cycle). Returns `true` when the link is safe.
    pub fn can_parent(&self, child: usize, parent: usize) -> bool {
        if child == parent || parent >= self.layers.len() || child >= self.layers.len() {
            return false;
        }
        // Walk up from `parent`; if we reach `child`, linking would cycle.
        let mut visited = vec![false; self.layers.len()];
        let mut cur = parent;
        loop {
            if cur == child {
                return false;
            }
            if visited[cur] {
                return true; // pre-existing cycle elsewhere; this link is fine
            }
            visited[cur] = true;
            match self.layers[cur].parent {
                Some(p) if p < self.layers.len() => cur = p,
                _ => return true,
            }
        }
    }
}

impl Default for Comp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
