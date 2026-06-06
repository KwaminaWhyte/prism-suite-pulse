//! The Pulse application: composition state, the transport (play/scrub), panels,
//! menus, and the per-frame loop tying the motion model to the preview and
//! timeline.

use crate::comp::{Comp, Ease, Interp, Prop, PulseLayer};
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
        let color = self.next_color();
        let n = self.comp.layers.len() + 1;
        self.comp
            .layers
            .push(PulseLayer::new(format!("Solid {n}"), color));
        self.selected = Some(self.comp.layers.len() - 1);
    }

    fn delete_layer(&mut self, idx: usize) {
        if idx < self.comp.layers.len() {
            self.comp.layers.remove(idx);
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
                    if ui
                        .button(format!("{}  Add layer", icons::ADD_LAYER))
                        .clicked()
                    {
                        self.add_layer();
                        ui.close_menu();
                    }
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

                // Layer name + color swatch.
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut self.comp.layers[idx].name);
                });
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

                ui.separator();

                let t = self.time;
                for prop in Prop::ALL {
                    self.property_row(ui, idx, prop, t);
                }
            });
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
