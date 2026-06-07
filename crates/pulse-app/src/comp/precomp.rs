//! Precomps (nested compositions): a layer that references another
//! [`Comp`](super::Comp) by id and renders it recursively at comp time `t`.
//!
//! A [`PrecompLayer`] holds the **id** of the target comp plus a scalar
//! `time_offset` (in seconds) added to the host comp's time before the nested
//! comp is sampled — a deliberately minimal stand-in for After Effects' full
//! time-remapping (a single shift, not a remap curve). The nested comp is
//! rendered through the *same* render path as the top-level comp (so it honours
//! the nested comp's own layers, transforms, masks, mattes, effects, and motion
//! blur), then its rendered frame is composited into the precomp layer's quad
//! exactly like decoded footage.
//!
//! Reference **cycles** (A → B → A, or a comp nesting itself) can't be encoded
//! away at the model level, so the renderer carries a visited-set of comp ids
//! and refuses to re-enter a comp already on the stack — a cyclic precomp simply
//! renders nothing, so a corrupt or self-referential project can never
//! infinite-loop or overflow the stack.
//!
//! **Project model.** Precomps need more than one comp to point at, so the
//! document is a [`Project`]: an id-keyed set of [`Comp`](super::Comp)s with one
//! marked active for editing. Each comp carries a stable [`Comp::id`] (the
//! reference target). Old single-comp `.pulse` files (a bare `Comp`) still load
//! directly; the app wraps them into a one-comp project on import.

use serde::{Deserialize, Serialize};

use super::Comp;

/// A precomp layer's reference: which comp it nests, and a time shift.
///
/// `source` is the target [`Comp::id`] (or `None` for an unwired precomp, which
/// draws nothing). `time_offset` is added to the host comp time before the
/// nested comp is sampled, so a precomp can be slipped earlier/later on the
/// host timeline (a minimal time-remap: a shift, not a curve).
///
/// `serde`-defaulted in full so pre-precomp `.pulse` files load with no
/// reference (`source: None`, `time_offset: 0`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrecompLayer {
    /// The referenced comp's [`Comp::id`], or `None` if unwired.
    #[serde(default)]
    pub source: Option<u64>,
    /// Seconds added to the host comp time before sampling the nested comp.
    #[serde(default)]
    pub time_offset: f32,
}

impl PrecompLayer {
    /// A precomp referencing comp `id` with no time offset.
    pub fn to(id: u64) -> Self {
        Self {
            source: Some(id),
            time_offset: 0.0,
        }
    }

    /// Whether this precomp points at a comp (so the renderer should try to
    /// resolve and render it).
    pub fn is_set(&self) -> bool {
        self.source.is_some()
    }

    /// Map host comp time `t` to the nested comp's time (`t + time_offset`).
    pub fn nested_time(&self, t: f32) -> f32 {
        t + self.time_offset
    }
}

/// The whole motion document: a set of [`Comp`]s (so precomps have something to
/// reference) with one marked **active** for editing.
///
/// Comps are addressed by their stable [`Comp::id`]. A [`Project`] always holds
/// at least one comp (the active one); IDs are minted from a monotonic counter
/// so a freshly added comp never collides with an existing one — including ones
/// loaded from disk.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    /// The project's comps, in creation order. Always non-empty.
    pub comps: Vec<Comp>,
    /// Index into [`comps`](Self::comps) of the comp currently being edited.
    pub active: usize,
    /// Next comp id to mint (monotonic; never reused).
    #[serde(default)]
    pub next_id: u64,
}

// The project model's accessors are part of the document API but are exercised
// today mostly by tests and the (forthcoming) project-load path; the live app
// keeps the active comp inline (see `PulseApp`) and only constructs a `Project`
// for saving. Allow the not-yet-wired accessors in non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
impl Project {
    /// A new project with a single fresh demo comp (id 1) active.
    pub fn new() -> Self {
        let mut comp = Comp::new();
        comp.id = 1;
        Self {
            comps: vec![comp],
            active: 0,
            next_id: 2,
        }
    }

    /// Wrap a single loaded [`Comp`] (e.g. an old single-comp `.pulse` file) into
    /// a one-comp project, minting an id if the comp carries none (id 0).
    pub fn from_comp(mut comp: Comp) -> Self {
        if comp.id == 0 {
            comp.id = 1;
        }
        let next_id = comp.id + 1;
        Self {
            comps: vec![comp],
            active: 0,
            next_id,
        }
    }

    /// Mint a fresh, never-reused comp id.
    pub fn mint_id(&mut self) -> u64 {
        // Be defensive against deserialized projects whose `next_id` lags behind
        // the live comp ids (e.g. hand-edited files): never hand out a live id.
        let highest = self.comps.iter().map(|c| c.id).max().unwrap_or(0);
        let id = self.next_id.max(highest + 1).max(1);
        self.next_id = id + 1;
        id
    }

    /// The comp currently being edited.
    pub fn active(&self) -> &Comp {
        &self.comps[self.active.min(self.comps.len().saturating_sub(1))]
    }

    /// Find a comp by id.
    pub fn comp_by_id(&self, id: u64) -> Option<&Comp> {
        self.comps.iter().find(|c| c.id == id)
    }

    /// Add a comp to the project (minting and assigning it a fresh id) and return
    /// its new id.
    pub fn push_comp(&mut self, mut comp: Comp) -> u64 {
        let id = self.mint_id();
        comp.id = id;
        self.comps.push(comp);
        id
    }
}

impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}
