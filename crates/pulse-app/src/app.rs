//! The Pulse application: composition state, the transport (play/scrub), panels,
//! menus, and the per-frame loop tying the motion model to the preview and
//! timeline.

use crate::comp::{Comp, Ease, Effect, Interp, LayerKind, MatteMode, Prop, PulseLayer};
use crate::graph::GraphState;
use crate::{graph, icons, preview, render, theme, timeline};
use egui::{Color32, Sense};

/// Which editor occupies the bottom panel: the lane timeline or the value-curve
/// graph editor (After Effects' two timeline modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum EditorMode {
    #[default]
    Timeline,
    Graph,
}

pub struct PulseApp {
    comp: Comp,
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
    /// Last save/export status, surfaced briefly in the menu bar.
    status: Option<String>,
}

impl PulseApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        icons::install(&cc.egui_ctx);
        Self {
            comp: Comp::new(),
            time: 0.0,
            playing: false,
            selected: Some(0),
            rng: 0x1234_5678,
            mode: EditorMode::default(),
            graph: GraphState::default(),
            status: None,
        }
    }

    // --- Commands -----------------------------------------------------------

    fn new_comp(&mut self) {
        self.comp = Comp::new();
        self.time = 0.0;
        self.playing = false;
        self.selected = Some(0);
        self.graph = GraphState::default();
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
        };
        let mut layer = PulseLayer::of_kind(kind, name, color);
        if kind == LayerKind::Adjustment {
            layer.scale.set_key(0.0, 3.0); // cover the whole comp
        }
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

    fn save_dialog(&self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Pulse composition", &["pulse", "json"])
            .set_file_name("untitled.pulse")
            .save_file()
        {
            match serde_json::to_string_pretty(&self.comp) {
                Ok(json) => {
                    if let Err(e) = std::fs::write(&path, json) {
                        log::error!("save failed: {e}");
                    } else {
                        log::info!(
                            "saved comp ({} layers) to {}",
                            self.comp.layers.len(),
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
        match render::export_sequence(&self.comp, &dir, stem) {
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

        // Advance the playhead by real dt while playing; loop at duration.
        if self.playing {
            let dt = ctx.input(|i| i.stable_dt).min(0.1);
            self.time += dt;
            if self.time >= self.comp.duration {
                self.time %= self.comp.duration.max(0.001);
            }
            ctx.request_repaint();
        }

        // Spacebar toggles playback.
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            self.playing = !self.playing;
        }

        self.menu_bar(root);
        self.layers_panel(root);
        self.properties_panel(root);
        self.timeline_panel(root);
        self.preview_panel(root);
    }
}

impl PulseApp {
    fn menu_bar(&mut self, root: &mut egui::Ui) {
        egui::TopBottomPanel::top("menu_bar").show_inside(root, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button(format!("{}  New", icons::ADD_LAYER)).clicked() {
                        self.new_comp();
                        ui.close_menu();
                    }
                    if ui.button("Save .pulse…").clicked() {
                        self.save_dialog();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .button(format!("{}  Export PNG sequence…", icons::EXPORT))
                        .on_hover_text("Render every frame to a PNG image sequence")
                        .clicked()
                    {
                        self.export_dialog();
                        ui.close_menu();
                    }
                });
                ui.menu_button("Layer", |ui| {
                    ui.menu_button(format!("{}  New", icons::ADD_LAYER), |ui| {
                        for kind in LayerKind::ALL {
                            if ui.button(kind.label()).clicked() {
                                self.add_layer_of_kind(kind);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.add_enabled_ui(self.selected.is_some(), |ui| {
                        if ui
                            .button(format!("{}  Delete layer", icons::TRASH))
                            .clicked()
                        {
                            if let Some(i) = self.selected {
                                self.delete_layer(i);
                            }
                            ui.close_menu();
                        }
                    });
                });
                ui.menu_button("Comp", |ui| {
                    self.motion_blur_menu(ui);
                });
                ui.separator();
                ui.label(egui::RichText::new("Pulse").strong());
                ui.weak("motion · Prism");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!(
                        "{}×{}  ·  {:.0} fps  ·  {:.1}s",
                        self.comp.width, self.comp.height, self.comp.fps, self.comp.duration
                    ));
                    if let Some(status) = &self.status {
                        ui.separator();
                        ui.weak(status).on_hover_text(status);
                    }
                });
            });
        });
    }

    /// The comp **Motion Blur** controls: a master enable, plus the shutter
    /// angle / phase and sample count (After Effects' Advanced composition
    /// settings). The shutter sliders are disabled while motion blur is off.
    fn motion_blur_menu(&mut self, ui: &mut egui::Ui) {
        let mb = &mut self.comp.motion_blur;
        ui.checkbox(&mut mb.enabled, "Enable motion blur")
            .on_hover_text("Master switch — layers also need their own motion-blur flag");
        ui.add_enabled_ui(mb.enabled, |ui| {
            ui.add(
                egui::Slider::new(&mut mb.angle, 1.0..=720.0)
                    .text("Shutter angle")
                    .suffix("°"),
            )
            .on_hover_text("Fraction of a frame the shutter is open (180° = half)");
            ui.add(
                egui::Slider::new(&mut mb.phase, -360.0..=360.0)
                    .text("Shutter phase")
                    .suffix("°"),
            )
            .on_hover_text(
                "Where the open window sits relative to the frame (−angle/2 centers it)",
            );
            ui.add(egui::Slider::new(&mut mb.samples, 1..=64).text("Samples"))
                .on_hover_text("Sub-frame snapshots integrated across the shutter");
        });
    }

    fn layers_panel(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::left("layers")
            .default_width(210.0)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("Layers");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button(icons::ADD_LAYER)
                            .on_hover_text("Add layer")
                            .clicked()
                        {
                            self.add_layer();
                        }
                    });
                });
                ui.add_space(2.0);

                let mut to_delete: Option<usize> = None;
                let mut to_move: Option<(usize, bool)> = None;
                let n = self.comp.layers.len();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Top of the list = front of the stack (highest index).
                    for idx in (0..n).rev() {
                        let selected = self.selected == Some(idx);
                        ui.horizontal(|ui| {
                            let vis = self.comp.layers[idx].visible;
                            let eye = if vis { icons::EYE } else { icons::EYE_OFF };
                            if ui.button(eye).on_hover_text("Visibility").clicked() {
                                self.comp.layers[idx].visible = !vis;
                            }
                            if ui
                                .selectable_label(selected, &self.comp.layers[idx].name)
                                .clicked()
                            {
                                self.selected = Some(idx);
                            }
                            // Mark a layer that is being used as the track-matte
                            // source for the layer directly below it.
                            if self.comp.is_matte_source(idx) {
                                ui.weak(icons::MATTE)
                                    .on_hover_text("Used as a track matte for the layer below");
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button(icons::TRASH).on_hover_text("Delete").clicked() {
                                        to_delete = Some(idx);
                                    }
                                    if ui
                                        .add_enabled(
                                            idx + 1 < n,
                                            egui::Button::new(icons::ARROW_UP),
                                        )
                                        .on_hover_text("Move up")
                                        .clicked()
                                    {
                                        to_move = Some((idx, true));
                                    }
                                    if ui
                                        .add_enabled(idx > 0, egui::Button::new(icons::ARROW_DOWN))
                                        .on_hover_text("Move down")
                                        .clicked()
                                    {
                                        to_move = Some((idx, false));
                                    }
                                },
                            );
                        });
                    }
                    if n == 0 {
                        ui.weak("No layers. Click + to add one.");
                    }
                });

                if let Some(i) = to_delete {
                    self.delete_layer(i);
                }
                if let Some((i, up)) = to_move {
                    self.move_layer(i, up);
                }
            });
    }

    fn properties_panel(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::right("properties")
            .default_width(260.0)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                ui.heading("Properties");
                ui.add_space(2.0);

                let Some(idx) = self.selected else {
                    ui.weak("Select a layer to edit its properties.");
                    return;
                };
                if idx >= self.comp.layers.len() {
                    self.selected = None;
                    return;
                }

                // Layer name + kind.
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.comp.layers[idx].name);
                });
                ui.horizontal(|ui| {
                    ui.label("Kind");
                    let cur = self.comp.layers[idx].kind;
                    egui::ComboBox::from_id_salt(("kind", idx))
                        .selected_text(cur.label())
                        .show_ui(ui, |ui| {
                            for kind in LayerKind::ALL {
                                if ui.selectable_label(cur == kind, kind.label()).clicked() {
                                    self.comp.layers[idx].kind = kind;
                                }
                            }
                        });
                });

                // Color is only meaningful for layers that draw their own pixels.
                if self.comp.layers[idx].kind.draws_own_pixels() {
                    ui.horizontal(|ui| {
                        ui.label("Color");
                        let c = &mut self.comp.layers[idx].color;
                        let mut col = Color32::from_rgba_unmultiplied(
                            (c[0] * 255.0) as u8,
                            (c[1] * 255.0) as u8,
                            (c[2] * 255.0) as u8,
                            (c[3] * 255.0) as u8,
                        );
                        if ui.color_edit_button_srgba(&mut col).changed() {
                            c[0] = col.r() as f32 / 255.0;
                            c[1] = col.g() as f32 / 255.0;
                            c[2] = col.b() as f32 / 255.0;
                            c[3] = col.a() as f32 / 255.0;
                        }
                    });
                }

                // Parent pick-whip: a child inherits this layer's transform.
                self.parent_row(ui, idx);

                // Track matte: borrow the layer above as this layer's alpha/luma.
                self.matte_row(ui, idx);

                // Per-layer motion-blur switch (only meaningful for layers that
                // draw their own pixels — a null/adjustment has nothing to blur).
                if self.comp.layers[idx].kind.draws_own_pixels() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.comp.layers[idx].motion_blur, "Motion blur")
                            .on_hover_text("Blur this layer's motion across the comp shutter");
                        if self.comp.layers[idx].motion_blur && !self.comp.motion_blur.enabled {
                            ui.weak("(comp switch off)")
                                .on_hover_text("Enable Comp ▸ Motion blur to see it");
                        }
                    });
                }

                ui.separator();

                let t = self.time;
                for prop in Prop::ALL {
                    self.property_row(ui, idx, prop, t);
                }

                // Effect stack (color-correction passes). Nulls draw nothing, so
                // an effect stack on them would do nothing — hide the section.
                if self.comp.layers[idx].kind != LayerKind::Null {
                    ui.separator();
                    self.effects_section(ui, idx);
                }
            });
    }

    /// The layer's **effect stack** editor: an "Add effect" menu, then each
    /// effect with reorder / remove controls and per-parameter sliders. Effects
    /// process the layer's own color (solid) or the layers below (adjustment).
    fn effects_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        ui.horizontal(|ui| {
            ui.heading("Effects");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(format!("{}  Add", icons::ADD_KEY), |ui| {
                    for eff in Effect::defaults() {
                        if ui.button(eff.label()).clicked() {
                            self.comp.layers[idx].effects.push(eff);
                            ui.close_menu();
                        }
                    }
                });
            });
        });

        if self.comp.layers[idx].kind == LayerKind::Adjustment {
            ui.weak("Grades every layer below, within this layer's bounds.");
        }
        if self.comp.layers[idx].effects.is_empty() {
            ui.weak("No effects. Click Add to apply one.");
            return;
        }

        let mut to_remove: Option<usize> = None;
        let mut to_move: Option<(usize, bool)> = None;
        let n = self.comp.layers[idx].effects.len();
        for ei in 0..n {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(self.comp.layers[idx].effects[ei].label()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(icons::TRASH).on_hover_text("Remove").clicked() {
                        to_remove = Some(ei);
                    }
                    if ui
                        .add_enabled(ei > 0, egui::Button::new(icons::ARROW_UP))
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        to_move = Some((ei, true));
                    }
                    if ui
                        .add_enabled(ei + 1 < n, egui::Button::new(icons::ARROW_DOWN))
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        to_move = Some((ei, false));
                    }
                });
            });
            effect_params(ui, idx, ei, &mut self.comp.layers[idx].effects[ei]);
        }

        if let Some(ei) = to_remove {
            self.comp.layers[idx].effects.remove(ei);
        }
        if let Some((ei, up)) = to_move {
            let effects = &mut self.comp.layers[idx].effects;
            let other = if up { ei.wrapping_sub(1) } else { ei + 1 };
            if other < effects.len() {
                effects.swap(ei, other);
            }
        }
    }

    /// The Parent selector for layer `idx`: a combo of "None" plus every other
    /// layer that can legally be a parent (no self, no cycle). Choosing a parent
    /// makes `idx` inherit that layer's transform.
    fn parent_row(&mut self, ui: &mut egui::Ui, idx: usize) {
        let current = self.comp.layers[idx].parent;
        let current_label = match current {
            Some(p) if p < self.comp.layers.len() => self.comp.layers[p].name.clone(),
            _ => "None".to_owned(),
        };
        let mut chosen: Option<Option<usize>> = None;
        ui.horizontal(|ui| {
            ui.label("Parent");
            egui::ComboBox::from_id_salt(("parent", idx))
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "None").clicked() {
                        chosen = Some(None);
                    }
                    for other in 0..self.comp.layers.len() {
                        if other == idx || !self.comp.can_parent(idx, other) {
                            continue;
                        }
                        let sel = current == Some(other);
                        if ui
                            .selectable_label(sel, &self.comp.layers[other].name)
                            .clicked()
                        {
                            chosen = Some(Some(other));
                        }
                    }
                });
        });
        if let Some(next) = chosen {
            // Re-validate (the combo only lists safe options, but be defensive).
            self.comp.layers[idx].parent = match next {
                Some(p) if self.comp.can_parent(idx, p) => Some(p),
                Some(_) => current,
                None => None,
            };
        }
    }

    /// The **track-matte** selector for layer `idx`. When active, the layer
    /// directly above (`idx + 1`) becomes this layer's matte source (and stops
    /// compositing on its own). The picker is disabled — and shows a hint — when
    /// no layer sits above this one to borrow.
    fn matte_row(&mut self, ui: &mut egui::Ui, idx: usize) {
        // Matte applies to layers that draw their own pixels; a null/adjustment
        // has no coverage to mask, so hide the row for them.
        if !self.comp.layers[idx].kind.draws_own_pixels() {
            return;
        }
        let above = idx + 1; // the layer drawn directly above this one
        let source_name =
            (above < self.comp.layers.len()).then(|| self.comp.layers[above].name.clone());
        let current = self.comp.layers[idx].matte;
        let mut chosen: Option<MatteMode> = None;
        ui.horizontal(|ui| {
            ui.label("Track matte");
            ui.add_enabled_ui(source_name.is_some(), |ui| {
                egui::ComboBox::from_id_salt(("matte", idx))
                    .selected_text(current.label())
                    .show_ui(ui, |ui| {
                        for mode in MatteMode::ALL {
                            if ui.selectable_label(current == mode, mode.label()).clicked() {
                                chosen = Some(mode);
                            }
                        }
                    });
            });
        });
        match &source_name {
            Some(name) if current.is_active() => {
                ui.weak(format!("Matte from “{name}” (the layer above)"));
            }
            None => {
                ui.weak("Needs a layer above to use as its matte.");
            }
            _ => {}
        }
        if let Some(next) = chosen {
            // Only honor a matte when a source actually exists above.
            self.comp.layers[idx].matte = if source_name.is_some() {
                next
            } else {
                MatteMode::None
            };
        }
    }

    /// One property: live value slider + keyframe controls.
    fn property_row(&mut self, ui: &mut egui::Ui, idx: usize, prop: Prop, t: f32) {
        let layer = &mut self.comp.layers[idx];
        let (range, suffix) = prop.range();
        let key_count = layer.track(prop).keys.len();

        // The slider edits the *sampled value at the playhead*. When keyframes
        // exist, dragging the slider re-keys the current time so animation and
        // direct editing stay consistent.
        let mut value = layer.value(prop, t);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(prop.label()).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(format!("{key_count} {}", icons::KEYFRAME));
            });
        });

        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::Slider::new(&mut value, range)
                    .suffix(suffix)
                    .clamping(egui::SliderClamping::Never),
            );
            if resp.changed() {
                // Editing the slider keys the value at the current time: with no
                // prior keys this lays down a single constant key (the value
                // sticks); with keys present it re-keys this instant so direct
                // edits and animation stay consistent.
                layer.track_mut(prop).set_key(t, value);
            }

            if ui
                .button(icons::ADD_KEY)
                .on_hover_text("Add keyframe @ playhead")
                .clicked()
            {
                layer.track_mut(prop).set_key(t, value);
            }
        });

        // Interpolation selector — only meaningful when the playhead sits on a
        // keyframe of this property. The mode applies to the segment leaving the
        // key (After Effects' convention).
        if let Some(current) = layer.track(prop).interp_at(t) {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.weak(current.label());
                let chosen = interp_picker(ui, current);
                if let Some(next) = chosen {
                    if next != current {
                        layer.track_mut(prop).set_interp(t, next);
                    }
                }
            });
        }
        ui.add_space(2.0);
    }

    fn timeline_panel(&mut self, root: &mut egui::Ui) {
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(240.0)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                // Transport + editor-mode row.
                ui.horizontal(|ui| {
                    if ui
                        .button(icons::TO_START)
                        .on_hover_text("Go to start")
                        .clicked()
                    {
                        self.time = 0.0;
                    }
                    let play_icon = if self.playing {
                        icons::PAUSE
                    } else {
                        icons::PLAY
                    };
                    if ui
                        .button(egui::RichText::new(play_icon).size(16.0))
                        .on_hover_text("Play / Pause (Space)")
                        .clicked()
                    {
                        self.playing = !self.playing;
                    }
                    if ui
                        .button(icons::TO_END)
                        .on_hover_text("Go to end")
                        .clicked()
                    {
                        self.time = self.comp.duration;
                        self.playing = false;
                    }
                    ui.separator();
                    let frame = (self.time * self.comp.fps).round() as i32;
                    ui.monospace(format!("{:>6.2}s   frame {:>4}", self.time, frame));

                    // Right-aligned editor-mode toggle (timeline vs graph).
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .selectable_label(
                                self.mode == EditorMode::Graph,
                                format!("{}  Graph", icons::GRAPH),
                            )
                            .on_hover_text("Graph editor — drag keyframes & ease handles")
                            .clicked()
                        {
                            self.mode = EditorMode::Graph;
                        }
                        if ui
                            .selectable_label(
                                self.mode == EditorMode::Timeline,
                                format!("{}  Timeline", icons::TIMELINE),
                            )
                            .on_hover_text("Timeline — keyframe lanes")
                            .clicked()
                        {
                            self.mode = EditorMode::Timeline;
                        }
                    });
                });
                ui.add_space(2.0);

                match self.mode {
                    EditorMode::Timeline => {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let resp = timeline::show(ui, &self.comp, self.time, self.selected);
                            if let Some(t) = resp.scrub_time {
                                self.time = t.clamp(0.0, self.comp.duration);
                            }
                            if let Some(i) = resp.clicked_layer {
                                self.selected = Some(i);
                            }
                        });
                    }
                    EditorMode::Graph => {
                        self.graph_property_chips(ui);
                        let resp = graph::show(
                            ui,
                            &mut self.comp,
                            self.selected,
                            self.time,
                            &mut self.graph,
                        );
                        if let Some(t) = resp.scrub_time {
                            self.time = t.clamp(0.0, self.comp.duration);
                        }
                    }
                }
            });
    }

    /// A row of toggle chips choosing which properties the graph editor plots.
    /// With none selected the graph shows every keyframed property.
    fn graph_property_chips(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.weak("Show:");
            for prop in Prop::ALL {
                let on = self.graph.is_shown(prop);
                if ui
                    .selectable_label(on, prop.label())
                    .on_hover_text("Toggle in graph (none selected = all keyed)")
                    .clicked()
                {
                    self.graph.toggle(prop);
                }
            }
            if !self.graph.shown.is_empty() && ui.small_button("all").clicked() {
                self.graph.shown.clear();
            }
        });
        ui.add_space(2.0);
    }

    fn preview_panel(&mut self, root: &mut egui::Ui) {
        egui::CentralPanel::default().show_inside(root, |ui| {
            let (_resp, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
            preview::paint_comp(
                &painter,
                painter.clip_rect(),
                &self.comp,
                self.time,
                self.selected,
            );
        });
    }
}

/// A compact row of interpolation presets. Highlights the active mode and
/// returns the chosen [`Interp`] when the user picks one this frame.
///
/// `Ease` is treated as a single bucket (any custom handles count as "Ease");
/// the discrete buttons set the standard AE presets — Easy Ease (F9), Ease In,
/// Ease Out — without discarding a hand-tuned curve unless a button is clicked.
fn interp_picker(ui: &mut egui::Ui, current: Interp) -> Option<Interp> {
    let mut chosen = None;
    let is = |want: Interp| std::mem::discriminant(&current) == std::mem::discriminant(&want);

    // Linear / Hold are exact-match selections; the three ease presets all map
    // to the `Ease` discriminant, so we mark the group active and let the value
    // distinguish which preset is live.
    let presets: [(&str, &str, Interp); 5] = [
        (icons::INTERP_LINEAR, "Linear", Interp::Linear),
        (icons::INTERP_HOLD, "Hold", Interp::Hold),
        (icons::INTERP_EASE, "Easy Ease", Interp::Ease(Ease::EASY)),
        ("›", "Ease Out", Interp::Ease(Ease::OUT)),
        ("‹", "Ease In", Interp::Ease(Ease::IN)),
    ];

    for (glyph, tip, mode) in presets {
        let active = match (current, mode) {
            (Interp::Ease(a), Interp::Ease(b)) => a == b,
            _ => is(mode),
        };
        if ui
            .selectable_label(active, glyph)
            .on_hover_text(tip)
            .clicked()
        {
            chosen = Some(mode);
        }
    }
    chosen
}

/// Parameter sliders / color pickers for one [`Effect`], editing it in place.
/// `idx`/`ei` salt widget ids so multiple effects don't collide.
fn effect_params(ui: &mut egui::Ui, idx: usize, ei: usize, effect: &mut Effect) {
    let slider = |ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(label);
            ui.add(egui::Slider::new(v, lo..=hi));
        });
    };
    match effect {
        Effect::Tint {
            black,
            white,
            amount,
        } => {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label("Black");
                rgb_button(ui, (idx, ei, 0), black);
                ui.label("White");
                rgb_button(ui, (idx, ei, 1), white);
            });
            slider(ui, "Amount", amount, 0.0, 1.0);
        }
        Effect::BrightnessContrast {
            brightness,
            contrast,
        } => {
            slider(ui, "Brightness", brightness, -1.0, 1.0);
            slider(ui, "Contrast", contrast, 0.0, 3.0);
        }
        Effect::Exposure {
            stops,
            offset,
            gamma,
        } => {
            slider(ui, "Stops", stops, -5.0, 5.0);
            slider(ui, "Offset", offset, -0.5, 0.5);
            slider(ui, "Gamma", gamma, 0.1, 3.0);
        }
        Effect::Levels {
            in_black,
            in_white,
            gamma,
            out_black,
            out_white,
        } => {
            slider(ui, "In black", in_black, 0.0, 1.0);
            slider(ui, "In white", in_white, 0.0, 1.0);
            slider(ui, "Gamma", gamma, 0.1, 3.0);
            slider(ui, "Out black", out_black, 0.0, 1.0);
            slider(ui, "Out white", out_white, 0.0, 1.0);
        }
    }
}

/// An sRGB color-edit button bound to an `[f32; 3]` (0..1), salted by `id`.
fn rgb_button(ui: &mut egui::Ui, id: (usize, usize, u8), c: &mut [f32; 3]) {
    let mut col = Color32::from_rgb(
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
    );
    let resp = ui.push_id(id, |ui| ui.color_edit_button_srgba(&mut col));
    if resp.inner.changed() {
        c[0] = col.r() as f32 / 255.0;
        c[1] = col.g() as f32 / 255.0;
        c[2] = col.b() as f32 / 255.0;
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
