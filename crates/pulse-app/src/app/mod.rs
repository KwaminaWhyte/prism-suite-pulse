//! The Pulse application: composition state, the transport (play/scrub), panels,
//! menus, and the per-frame loop tying the motion model to the preview and
//! timeline.

use crate::comp::{
    source_from_path, Comp, LayerKind, PrecompLayer, Project, PulseLayer, ShapeItem, ShapePrimitive,
};
use crate::graph::GraphState;
use crate::{icons, render, theme};

mod effects;
mod layers;
mod menu;
mod panels;
mod properties;
mod workspace;

pub(crate) use workspace::{Panel, PanelVisibility};

/// Which editor occupies the bottom panel: the lane timeline or the value-curve
/// graph editor (After Effects' two timeline modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum EditorMode {
    #[default]
    Timeline,
    Graph,
}

pub struct PulseApp {
    /// The comp currently being edited (the active project comp, kept inline so
    /// every panel edits it directly). The rest of the project lives in
    /// [`others`](Self::others); [`comps`](Self::comps) merges them for rendering
    /// and saving.
    comp: Comp,
    /// The project's **other** comps (everything except the active
    /// [`comp`](Self::comp)) — the pool a [`LayerKind::Precomp`] layer references
    /// by id, and the comps a precomp render resolves recursively against.
    others: Vec<Comp>,
    /// Next comp id to mint (monotonic; never reused), so a new precompose target
    /// can't collide with an existing comp.
    next_id: u64,
    /// Current playhead position in seconds.
    time: f32,
    playing: bool,
    selected: Option<usize>,
    /// Tiny LCG state for picking fresh layer colors.
    rng: u32,
    /// Bottom-panel editor mode (timeline vs graph).
    mode: EditorMode,
    /// Graph-editor state (shown properties + active drag).
    graph: GraphState,
    /// Active on-canvas transform-gizmo drag (preview), if any.
    gizmo_drag: Option<GizmoDrag>,
    /// Which dockable panels (Layers / Properties / Timeline) are shown — driven
    /// by the **Window** menu. The central Preview viewport is always present.
    panels: PanelVisibility,
    /// Onion-skinning: ghost neighbouring frames behind the playhead for
    /// hand-keyed timing. Driven by the **View** menu; off by default.
    onion: crate::onion::OnionSkin,
    /// The Effects & Presets browser's live search query (type-to-filter).
    effect_query: String,
    /// Last save/export status, surfaced briefly in the menu bar.
    status: Option<String>,
    /// The interactive **render preview**: caches the comp rendered (through the
    /// real offline compositor) to a capped-res egui texture, re-rendering only
    /// when the frame's fingerprint changes, and holds a persistent footage
    /// [`FrameCache`](crate::comp::FrameCache) so scrubbing doesn't re-decode.
    preview: crate::preview::PreviewRenderer,
}

/// An in-progress drag of the preview's transform gizmo: which handle is held,
/// the layer + the time and transform captured when the grab started, and the
/// pointer position (comp space) at grab time. The drag math is recomputed each
/// frame against the live pointer, so the result is always relative to the grab.
#[derive(Clone, Copy, Debug)]
struct GizmoDrag {
    layer: usize,
    handle: crate::gizmo::Handle,
    /// Playhead time when the grab started (where the edits are keyed).
    time: f32,
    /// The layer's sampled transform at grab time.
    start_tf: crate::comp::Transform,
    /// The layer's parent matrix at grab time (local-space conversion).
    parent: crate::comp::Affine2,
    /// Pointer position (comp space) when the grab started.
    start_comp: (f32, f32),
}

impl PulseApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        icons::install(&cc.egui_ctx);
        let mut comp = Comp::new();
        comp.id = 1;
        Self {
            comp,
            others: Vec::new(),
            next_id: 2,
            time: 0.0,
            playing: false,
            selected: Some(0),
            rng: 0x1234_5678,
            mode: EditorMode::default(),
            graph: GraphState::default(),
            gizmo_drag: None,
            panels: PanelVisibility::default(),
            onion: crate::onion::OnionSkin::default(),
            effect_query: String::new(),
            status: None,
            preview: crate::preview::PreviewRenderer::default(),
        }
    }

    // --- Commands -----------------------------------------------------------

    fn new_comp(&mut self) {
        let mut comp = Comp::new();
        comp.id = 1;
        self.comp = comp;
        self.others = Vec::new();
        self.next_id = 2;
        self.time = 0.0;
        self.playing = false;
        self.selected = Some(0);
        self.graph = GraphState::default();
    }

    /// Mint a fresh, never-reused comp id (defensive against any live id, so a
    /// loaded project whose `next_id` lags can't hand out a colliding id).
    fn mint_id(&mut self) -> u64 {
        let highest = std::iter::once(self.comp.id)
            .chain(self.others.iter().map(|c| c.id))
            .max()
            .unwrap_or(0);
        let id = self.next_id.max(highest + 1).max(1);
        self.next_id = id + 1;
        id
    }

    /// The whole project's comps (the active [`comp`](Self::comp) plus the
    /// [`others`](Self::others)), for project-aware rendering / export / save.
    fn project_comps(&self) -> Vec<Comp> {
        let mut comps = Vec::with_capacity(self.others.len() + 1);
        comps.push(self.comp.clone());
        comps.extend(self.others.iter().cloned());
        comps
    }

    /// **Pre-compose** the selected layer into a new comp and replace it with a
    /// [`LayerKind::Precomp`] layer referencing it (the classic After Effects
    /// workflow, single-layer slice). The new comp inherits the host comp's size
    /// / duration / fps; the wrapped layer keeps its content but its transform is
    /// reset on the precomp layer (the wrapped layer is re-centered inside the new
    /// comp, "leave all attributes" style). No-op if nothing is selected.
    ///
    /// Multi-layer pre-compose (wrapping a selection set, preserving inter-layer
    /// parenting) is a documented gap — see `PLAN.md`.
    fn precompose_selected(&mut self) {
        let Some(idx) = self.selected else {
            return;
        };
        if idx >= self.comp.layers.len() {
            return;
        }
        // Build the nested comp from the host's canvas/timeline and move the
        // selected layer into it (re-centered: its transform tracks are dropped so
        // it sits at the new comp's center, the common "move all attributes into
        // the new comp" result for a single layer).
        let id = self.mint_id();
        let inner_name = self.comp.layers[idx].name.clone();
        let mut nested = Comp::empty_like(format!("{inner_name} Comp"), &self.comp);
        nested.id = id;
        let mut wrapped = self.comp.layers[idx].clone();
        // The wrapped layer is now top-level inside the nested comp: it has no
        // parent there, and its transform resets to identity (centered).
        wrapped.parent = None;
        wrapped.anchor_x = Default::default();
        wrapped.anchor_y = Default::default();
        wrapped.x = Default::default();
        wrapped.y = Default::default();
        wrapped.scale = Default::default();
        wrapped.rotation = Default::default();
        wrapped.opacity = Default::default();
        nested.layers.push(wrapped);
        self.others.push(nested);

        // Replace the selected layer in place with a precomp referencing the new
        // comp (so it keeps its stacking position and any children parented to it
        // still point at the same index).
        let precomp = {
            let mut l = PulseLayer::of_kind(LayerKind::Precomp, inner_name, [0.5, 0.5, 0.5, 1.0]);
            l.precomp = PrecompLayer::to(id);
            l
        };
        self.comp.layers[idx] = precomp;
        self.selected = Some(idx);
    }

    /// Drop a **comp marker** at the playhead (After Effects' add-marker key),
    /// avoiding a duplicate at the same time. Newly added markers are kept sorted
    /// by time so the timeline and navigation read in order.
    fn add_comp_marker(&mut self) {
        let t = self.time.clamp(0.0, self.comp.duration);
        if self.comp.markers.iter().any(|m| (m.time - t).abs() < 1e-3) {
            return;
        }
        self.comp.markers.push(crate::comp::Marker::at(t));
        self.comp
            .markers
            .sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Add a **layer marker** at the playhead on the selected layer (avoiding a
    /// duplicate), kept sorted by time. No-op if nothing is selected.
    fn add_layer_marker(&mut self) {
        let Some(idx) = self.selected else { return };
        let t = self.time.clamp(0.0, self.comp.duration);
        let Some(layer) = self.comp.layers.get_mut(idx) else {
            return;
        };
        if layer.markers.iter().any(|m| (m.time - t).abs() < 1e-3) {
            return;
        }
        layer.markers.push(crate::comp::Marker::at(t));
        layer
            .markers
            .sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Set the work-area **start** to the playhead (clamped below the end).
    fn set_work_area_start(&mut self) {
        let dur = self.comp.duration;
        let t = self.time.clamp(0.0, dur);
        self.comp.work_area.start = t.min(self.comp.work_area.end);
        self.comp.work_area = self.comp.work_area.clamped(dur);
    }

    /// Set the work-area **end** to the playhead (clamped above the start).
    fn set_work_area_end(&mut self) {
        let dur = self.comp.duration;
        let t = self.time.clamp(0.0, dur);
        self.comp.work_area.end = t.max(self.comp.work_area.start);
        self.comp.work_area = self.comp.work_area.clamped(dur);
    }

    /// Reset the work area to span the whole `[0, duration]` timeline.
    fn reset_work_area(&mut self) {
        self.comp.work_area = crate::comp::WorkArea::full(self.comp.duration);
    }

    /// A pseudo-random vivid color for a new layer.
    fn next_color(&mut self) -> [f32; 4] {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        let h = (x % 360) as f32;
        let (r, g, b) = hsv_to_rgb(h, 0.65, 0.9);
        [r, g, b, 1.0]
    }

    fn add_layer(&mut self) {
        self.add_layer_of_kind(LayerKind::Solid);
    }

    /// Add a new layer of the given kind, named and colored to suit. Null layers
    /// default to a neutral swatch (they don't draw); adjustment layers cover
    /// the frame (scale 3x) so their grade affects everything below out of the box.
    fn add_layer_of_kind(&mut self, kind: LayerKind) {
        let n = self.comp.layers.len() + 1;
        let (name, color) = match kind {
            LayerKind::Solid => (format!("Solid {n}"), self.next_color()),
            LayerKind::Null => (format!("Null {n}"), [0.6, 0.6, 0.6, 1.0]),
            LayerKind::Adjustment => (format!("Adjustment {n}"), [1.0, 1.0, 1.0, 1.0]),
            LayerKind::Shape => (format!("Shape {n}"), self.next_color()),
            LayerKind::Text => (format!("Text {n}"), self.next_color()),
            LayerKind::Footage => (format!("Footage {n}"), [0.5, 0.5, 0.5, 1.0]),
            LayerKind::Precomp => (format!("Precomp {n}"), [0.5, 0.5, 0.5, 1.0]),
        };
        let mut layer = PulseLayer::of_kind(kind, name, color);
        match kind {
            LayerKind::Adjustment => {
                layer.scale.set_key(0.0, 3.0); // cover the whole comp
            }
            LayerKind::Text => {
                // Tint the default text fill with the layer's color so a new text
                // layer reads in its own swatch out of the box.
                if let Some(fill) = layer.text.fill.as_mut() {
                    fill.color = [color[0], color[1], color[2]];
                }
            }
            LayerKind::Shape => {
                // Seed a new shape layer with a filled rectangle in the layer's
                // own color so it draws something out of the box.
                let half = self.comp.width as f32 * render::LAYER_HALF_FRAC * 0.5;
                let mut item = ShapeItem::new(ShapePrimitive::Rectangle {
                    half_w: half,
                    half_h: half,
                    radius: 0.0,
                });
                if let Some(fill) = item.fill.as_mut() {
                    fill.color = [color[0], color[1], color[2]];
                }
                layer.shape.items.push(item);
            }
            LayerKind::Precomp => {
                // Wire a new precomp to an existing other comp out of the box, if
                // any (else leave it unwired for the user to pick in Properties).
                if let Some(first) = self.others.first() {
                    layer.precomp = PrecompLayer::to(first.id);
                }
            }
            _ => {}
        }
        self.comp.layers.push(layer);
        self.selected = Some(self.comp.layers.len() - 1);
    }

    /// Import footage as a new layer: pop a file picker, then add a
    /// [`LayerKind::Footage`] layer whose source is a single still or (when the
    /// picked file is a numbered frame) the detected image sequence on disk. The
    /// new layer is named after the file and selected. No-op if cancelled.
    fn import_footage(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import footage (still or image-sequence frame)")
            .add_filter("Images", prism_io::SUPPORTED_EXTENSIONS)
            .pick_file()
        else {
            return;
        };
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Footage".to_string());
        // Prefer a detected sequence; fall back to a single still.
        let source = source_from_path(&path);
        let mut layer = PulseLayer::of_kind(LayerKind::Footage, name, [0.5, 0.5, 0.5, 1.0]);
        layer.footage.source = Some(source);
        self.comp.layers.push(layer);
        self.selected = Some(self.comp.layers.len() - 1);
    }

    fn delete_layer(&mut self, idx: usize) {
        if idx < self.comp.layers.len() {
            self.comp.layers.remove(idx);
            // Fix up parent references: children of the removed layer become
            // unparented; indices above `idx` shift down by one.
            for layer in &mut self.comp.layers {
                layer.parent = match layer.parent {
                    Some(p) if p == idx => None,
                    Some(p) if p > idx => Some(p - 1),
                    other => other,
                };
            }
            self.selected = match self.selected {
                Some(s) if s == idx => None,
                Some(s) if s > idx => Some(s - 1),
                other => other,
            };
        }
    }

    /// Move a layer up (toward the top / front of the stack).
    fn move_layer(&mut self, idx: usize, up: bool) {
        let n = self.comp.layers.len();
        // "Up" in the list = toward the end of the vec (front of paint order).
        let target = if up {
            if idx + 1 >= n {
                return;
            }
            idx + 1
        } else {
            if idx == 0 {
                return;
            }
            idx - 1
        };
        self.comp.layers.swap(idx, target);
        // Swapping two layers swaps their positional indices, so any parent
        // reference pointing at one must now point at the other.
        for layer in &mut self.comp.layers {
            layer.parent = match layer.parent {
                Some(p) if p == idx => Some(target),
                Some(p) if p == target => Some(idx),
                other => other,
            };
        }
        if self.selected == Some(idx) {
            self.selected = Some(target);
        } else if self.selected == Some(target) {
            self.selected = Some(idx);
        }
    }

    /// Assemble the whole project (active comp + others) into a [`Project`] for
    /// saving, with the active comp at its current index.
    fn to_project(&self) -> Project {
        // `project_comps` puts the active comp first, so the active index is 0.
        Project {
            comps: self.project_comps(),
            active: 0,
            next_id: self.next_id,
        }
    }

    fn save_dialog(&self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Pulse project", &["pulse", "json"])
            .set_file_name("untitled.pulse")
            .save_file()
        {
            // Save the whole **project** (every comp + precomp references), so a
            // project with precomps round-trips. Old single-comp `.pulse` files
            // remain loadable via the back-compat loader.
            let project = self.to_project();
            match serde_json::to_string_pretty(&project) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&path, json) {
                        log::error!("save failed: {e}");
                    } else {
                        log::info!(
                            "saved project ({} comps) to {}",
                            project.comps.len(),
                            path.display()
                        );
                    }
                }
                Err(e) => log::error!("serialize failed: {e}"),
            }
        }
    }

    /// Render the whole comp to a PNG image sequence in a chosen folder.
    ///
    /// Pauses playback (a render is a discrete action), pops a folder picker,
    /// then writes `<stem>_0000.png`, … one file per frame across the comp's
    /// `[0, duration]` timeline at its fps. Status (frames written / errors) is
    /// logged and shown in the menu bar.
    fn export_dialog(&mut self) {
        self.playing = false;
        let Some(dir) = rfd::FileDialog::new()
            .set_title("Export PNG sequence to folder…")
            .pick_folder()
        else {
            return;
        };
        let stem = "comp";
        // Project-aware export so any precomp layers in the active comp render
        // their nested comps recursively.
        let comps = self.project_comps();
        let id = self.comp.id;
        match render::export_sequence_in_project(&comps, id, &dir, stem) {
            Ok(summary) => {
                let msg = format!(
                    "Exported {} frames → {}",
                    summary.frames,
                    summary.dir.display()
                );
                log::info!("{msg}");
                self.status = Some(msg);
            }
            Err(e) => {
                let msg = format!("Export failed: {e}");
                log::error!("{msg}");
                self.status = Some(msg);
            }
        }
    }
}

impl eframe::App for PulseApp {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();

        // Playback runs at **real time** (wall clock) but is **gated by the
        // RAM-preview cache**: advance the playhead by the frame's `dt`, but only
        // commit the step when the target frame is already cached (or the whole
        // work area is). While the cache is still filling, an uncached target
        // makes the playhead **hold** on the current frame until that frame
        // renders — so playback never outruns the cache (no racing ahead) and,
        // once the comp is fully cached, the loop plays back smoothly in real time
        // straight from RAM. The UI stays responsive throughout (render is off the
        // UI thread); the worker pool renders ahead so the playhead rarely waits.
        if self.playing {
            // Playback loops within the **work area** (After Effects' RAM-preview
            // range), not the whole timeline: advance by `dt`, and when the
            // playhead reaches the work-area end (or the playhead sits outside the
            // area), wrap back to the work-area start. A full work area degrades to
            // looping the whole `[0, duration]` timeline exactly as before.
            let dur = self.comp.duration.max(0.001);
            let wa = self.comp.clamped_work_area();
            let (lo, hi) = (wa.start, wa.end.max(wa.start + 1e-3).min(dur));
            let mut next = self.time + ctx.input(|i| i.stable_dt).min(0.1);
            if next >= hi || self.time < lo {
                next = lo;
            }
            if self.preview.fully_cached() || self.preview.is_frame_ready(next) {
                self.time = next;
            }
            ctx.request_repaint();
        }

        // Spacebar toggles playback.
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.playing = !self.playing;
        }

        // Marker / work-area keys (After Effects muscle memory), suppressed while
        // a text field has focus so typing into a label / name doesn't trigger
        // them. `B` / `N` trim the work-area start / end to the playhead; `M`
        // drops a comp marker.
        if !ctx.wants_keyboard_input() {
            let (b, n, m) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::B),
                    i.key_pressed(egui::Key::N),
                    i.key_pressed(egui::Key::M),
                )
            });
            if b {
                self.set_work_area_start();
            }
            if n {
                self.set_work_area_end();
            }
            if m {
                self.add_comp_marker();
            }
        }

        self.menu_bar(root);
        // Dockable panels are shown only when their Window-menu toggle is on; the
        // central Preview viewport is always present (it fills whatever space the
        // side/bottom panels leave).
        if self.panels.is_shown(Panel::Layers) {
            self.layers_panel(root);
        }
        // The Effects & Presets browser docks left, beside the Layers panel.
        if self.panels.is_shown(Panel::Effects) {
            self.effects_panel(root);
        }
        if self.panels.is_shown(Panel::Properties) {
            self.properties_panel(root);
        }
        if self.panels.is_shown(Panel::Timeline) {
            self.timeline_panel(root);
        }
        self.preview_panel(root);
    }
}

/// Convert HSV (h in degrees, s/v in 0..1) to RGB in 0..1.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let hp = (h / 60.0) % 6.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

