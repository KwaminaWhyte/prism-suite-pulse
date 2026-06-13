//! **Animation presets**: a named, serializable snapshot of a layer's
//! *animatable state* — its full effect stack and its transform/property
//! keyframe tracks — that can be **captured** from one layer and **applied** to
//! another, re-creating the same effects and keyframes (same values, times, and
//! easing) on the target.
//!
//! This is After Effects' *Save Animation Preset* / *Apply Animation Preset*
//! reduced to Pulse's model. A preset is **pure data**: capturing reads a
//! [`PulseLayer`], applying mutates one, and neither touches the GPU, time, or
//! IO — so both directions are headlessly unit-testable and deterministic.
//!
//! ## What a preset captures
//!
//! - the layer's six effect stacks: the per-pixel **color** [`Effect`] stack,
//!   the **spatial** / **distort** / **key** / **stylize** whole-buffer stacks,
//!   and the optional **generate** fill (plus its evolution track);
//! - the seven transform/property **tracks** (anchor x/y, position x/y, scale,
//!   rotation, opacity), each carrying its keyframes *and* any expression.
//!
//! It deliberately does **not** capture identity-defining or wiring fields
//! (name, color, kind, parent index, masks, matte, footage/precomp source,
//! markers) — a preset is the layer's *animation*, not the layer.
//!
//! ## Apply semantics (documented rule)
//!
//! Applying a preset **replaces** the target's animatable state: the captured
//! effect stacks overwrite the target's stacks wholesale, and each captured
//! transform track overwrites the target's track for that property. A property
//! that was *not* captured (its track was empty at capture time) is **left
//! untouched** on the target — so a preset that only carries, say, a Position
//! animation won't wipe the target's existing Scale keyframes. This "replace the
//! captured, leave the rest" rule is the predictable middle ground between a full
//! reset and a blind merge.
//!
//! ## Persistence
//!
//! [`AnimationPreset`]s live in the [`Project`](super::Project) as a named list
//! (`presets`), `#[serde(default)]` so legacy `.pulse` files (which carry no
//! presets) load with an empty list and round-trip unchanged.

use serde::{Deserialize, Serialize};

use super::{
    DistortEffect, Effect, EffectMask, GenerateEffect, KeyEffect, Prop, PulseLayer, SpatialEffect,
    StylizeEffect, Track,
};

/// A named snapshot of a layer's effect stacks + transform keyframe tracks.
///
/// Captured from a layer with [`AnimationPreset::capture`] and re-applied with
/// [`AnimationPreset::apply`]. Fully serializable and `#[serde(default)]` on
/// every field so a preset written by a newer build (with more captured state)
/// still loads in an older one, and vice versa.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimationPreset {
    /// User-facing name (shown in the Apply-preset picker).
    #[serde(default)]
    pub name: String,

    // --- Effect stacks (mirroring the layer's six stacks) -------------------
    /// Per-pixel color-correction [`Effect`] stack.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// **Effect mask** gating the color-correction stack (region + feather /
    /// invert / opacity). `serde`-defaulted to disabled so older presets apply
    /// the effect everywhere.
    #[serde(default)]
    pub effect_mask: EffectMask,
    /// Whole-buffer **spatial** effect stack (blur / shadow / glow).
    #[serde(default)]
    pub spatial_effects: Vec<SpatialEffect>,
    /// Whole-buffer **distort** effect stack (corner pin / transform / …).
    #[serde(default)]
    pub distort_effects: Vec<DistortEffect>,
    /// Whole-buffer **key** effect stack (color / luma / chroma key, …).
    #[serde(default)]
    pub key_effects: Vec<KeyEffect>,
    /// Whole-buffer **stylize** effect stack (find edges / mosaic).
    #[serde(default)]
    pub stylize_effects: Vec<StylizeEffect>,
    /// Optional **generate** fill (e.g. Fractal Noise).
    #[serde(default)]
    pub generate: Option<GenerateEffect>,
    /// The generate fill's keyframable **evolution** track.
    #[serde(default)]
    pub generate_evolution: Track,

    // --- Transform / property tracks ----------------------------------------
    /// The captured transform tracks, one entry per property that carried
    /// keyframes at capture time. A property absent here was empty when captured
    /// and is **left untouched** on apply (see the module-level apply rule).
    #[serde(default)]
    pub tracks: Vec<PresetTrack>,
}

/// One captured transform track: which [`Prop`] it animates, and the [`Track`]
/// (keyframes + expression) itself.
///
/// [`Prop`] isn't itself `serde`-able (it's a UI/dispatch enum), so it is stored
/// by a stable string tag via [`PropTag`]; an unrecognised tag (e.g. a property a
/// newer build added) is simply skipped on apply rather than failing the load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresetTrack {
    /// Which transform property this track drives.
    pub prop: PropTag,
    /// The captured keyframes + expression for that property.
    pub track: Track,
}

/// A serializable tag for a transform [`Prop`].
///
/// `Prop` is a lightweight `Copy` dispatch enum that intentionally doesn't
/// derive `Serialize`; this mirror carries the same variants and round-trips
/// through serde, so a preset records *which* property each captured track
/// belongs to in a stable, forward-compatible way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropTag {
    AnchorX,
    AnchorY,
    X,
    Y,
    Scale,
    Rotation,
    Opacity,
}

impl PropTag {
    /// The [`Prop`] this tag denotes.
    pub fn prop(self) -> Prop {
        match self {
            PropTag::AnchorX => Prop::AnchorX,
            PropTag::AnchorY => Prop::AnchorY,
            PropTag::X => Prop::X,
            PropTag::Y => Prop::Y,
            PropTag::Scale => Prop::Scale,
            PropTag::Rotation => Prop::Rotation,
            PropTag::Opacity => Prop::Opacity,
        }
    }

    /// The tag for a [`Prop`].
    pub fn of(prop: Prop) -> Self {
        match prop {
            Prop::AnchorX => PropTag::AnchorX,
            Prop::AnchorY => PropTag::AnchorY,
            Prop::X => PropTag::X,
            Prop::Y => PropTag::Y,
            Prop::Scale => PropTag::Scale,
            Prop::Rotation => PropTag::Rotation,
            Prop::Opacity => PropTag::Opacity,
        }
    }
}

impl AnimationPreset {
    /// **Capture** a layer's animatable state into a named preset.
    ///
    /// Clones the layer's six effect stacks and every transform track that has
    /// keyframes *or* an expression (an entirely empty, expression-less track is
    /// skipped — there's nothing to reproduce, and skipping it makes apply leave
    /// the target's matching property untouched). Pure: it only reads `layer`.
    pub fn capture(name: impl Into<String>, layer: &PulseLayer) -> Self {
        let tracks = Prop::ALL
            .into_iter()
            .filter_map(|prop| {
                let track = layer.track(prop);
                // Capture a track only when it carries something to reproduce:
                // at least one keyframe, or a non-empty expression.
                if track.keys.is_empty() && !track.has_expression() {
                    None
                } else {
                    Some(PresetTrack {
                        prop: PropTag::of(prop),
                        track: track.clone(),
                    })
                }
            })
            .collect();

        Self {
            name: name.into(),
            effects: layer.effects.clone(),
            effect_mask: layer.effect_mask.clone(),
            spatial_effects: layer.spatial_effects.clone(),
            distort_effects: layer.distort_effects.clone(),
            key_effects: layer.key_effects.clone(),
            stylize_effects: layer.stylize_effects.clone(),
            generate: layer.generate,
            generate_evolution: layer.generate_evolution.clone(),
            tracks,
        }
    }

    /// **Apply** this preset to a layer, re-creating its effects + keyframes.
    ///
    /// Per the module-level rule: the effect stacks (and generate fill) **replace**
    /// the target's wholesale, and each captured transform track **overwrites**
    /// the target's track for that property. Properties not captured here are
    /// left untouched. Pure: it only mutates `layer` from this preset's data.
    pub fn apply(&self, layer: &mut PulseLayer) {
        layer.effects = self.effects.clone();
        layer.effect_mask = self.effect_mask.clone();
        layer.spatial_effects = self.spatial_effects.clone();
        layer.distort_effects = self.distort_effects.clone();
        layer.key_effects = self.key_effects.clone();
        layer.stylize_effects = self.stylize_effects.clone();
        layer.generate = self.generate;
        layer.generate_evolution = self.generate_evolution.clone();

        for pt in &self.tracks {
            *layer.track_mut(pt.prop.prop()) = pt.track.clone();
        }
    }
}
