//! The top menu bar (file/layer/playback commands + status) and the comp's
//! motion-blur settings popup.

use super::{Panel, PulseApp};
use crate::comp::LayerKind;
use crate::icons;

impl PulseApp {
    pub(super) fn menu_bar(&mut self, root: &mut egui::Ui) {
        egui::TopBottomPanel::top("menu_bar").show_inside(root, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button(format!("{}  New", icons::ADD_LAYER)).clicked() {
                        self.new_comp();
                        ui.close_menu();
                    }
                    if ui
                        .button("Open .pulse…")
                        .on_hover_text("Open a saved Pulse project (comps, layers, presets)")
                        .clicked()
                    {
                        self.open_dialog();
                        ui.close_menu();
                    }
                    if ui.button("Save .pulse…").clicked() {
                        self.save_dialog();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui
                        .button(format!("{}  Import footage…", icons::ADD_LAYER))
                        .on_hover_text("Add a still image or image sequence as a footage layer")
                        .clicked()
                    {
                        self.import_footage();
                        ui.close_menu();
                    }
                    ui.separator();
                    self.render_range_menu(ui);
                    if ui
                        .button(format!("{}  Export PNG sequence…", icons::EXPORT))
                        .on_hover_text(
                            "Render the chosen range (work area by default) to a PNG image sequence",
                        )
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
                    ui.separator();
                    ui.add_enabled_ui(self.selected.is_some(), |ui| {
                        if ui
                            .button(format!("{}  Pre-compose", icons::ADD_LAYER))
                            .on_hover_text(
                                "Wrap the selected layer into a new comp and replace it \
                                 with a precomp layer referencing it",
                            )
                            .clicked()
                        {
                            self.precompose_selected();
                            ui.close_menu();
                        }
                    });
                });
                ui.menu_button("Comp", |ui| {
                    self.motion_blur_menu(ui);
                    ui.separator();
                    self.camera_menu(ui);
                    ui.separator();
                    self.lights_menu(ui);
                    ui.separator();
                    self.work_area_menu(ui);
                    ui.separator();
                    self.marker_menu(ui);
                });
                ui.menu_button("View", |ui| {
                    self.onion_menu(ui);
                });
                ui.menu_button("Window", |ui| {
                    self.window_menu(ui);
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

    /// The composition **Camera** controls: position (X/Y/Z), the point of
    /// interest it looks at, and the lens (vertical FOV + the derived focal
    /// length). 3-D layers are projected through this camera; with the default
    /// camera and no 3-D layers the comp renders exactly as a flat 2-D comp.
    /// A *Reset* restores the comp-height-sized default placement.
    fn camera_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Camera").strong());
        let (comp_w, comp_h) = (self.comp.width as f32, self.comp.height as f32);
        let cam = &mut self.comp.camera;
        ui.horizontal(|ui| {
            ui.label("Position");
            ui.add(egui::DragValue::new(&mut cam.position[0]).speed(1.0).prefix("X "));
            ui.add(egui::DragValue::new(&mut cam.position[1]).speed(1.0).prefix("Y "));
            ui.add(egui::DragValue::new(&mut cam.position[2]).speed(1.0).prefix("Z "));
        });
        ui.horizontal(|ui| {
            ui.label("Look at  ");
            ui.add(egui::DragValue::new(&mut cam.poi[0]).speed(1.0).prefix("X "));
            ui.add(egui::DragValue::new(&mut cam.poi[1]).speed(1.0).prefix("Y "));
            ui.add(egui::DragValue::new(&mut cam.poi[2]).speed(1.0).prefix("Z "));
        });
        // FOV is read/edited live from the camera geometry (the projection's
        // focal length is the camera-to-poi distance); editing it dollies the
        // camera so the lens matches.
        let mut fov = cam.vertical_fov(comp_h);
        if ui
            .add(
                egui::Slider::new(&mut fov, 1.0..=179.0)
                    .text("Field of view")
                    .suffix("°"),
            )
            .on_hover_text("Vertical angle of view — smaller = longer (more telephoto) lens")
            .changed()
        {
            cam.set_vertical_fov(fov, comp_h);
        }
        // Focal length is the same lens expressed in mm (36 mm horizontal back);
        // editing it dollies the camera to that focal length.
        let mut focal = cam.focal_length(comp_w, comp_h);
        if ui
            .add(
                egui::Slider::new(&mut focal, 10.0..=300.0)
                    .text("Focal length")
                    .suffix(" mm"),
            )
            .on_hover_text("Lens focal length (drives the field of view)")
            .changed()
        {
            cam.set_focal_length(focal, comp_w, comp_h);
        }
        if ui
            .button("Reset camera")
            .on_hover_text("Restore the default camera (flat 2-D look)")
            .clicked()
        {
            *cam = crate::comp::Camera::default();
            cam.position = [0.0, 0.0, -crate::comp::Camera::default_distance(comp_h)];
            ui.close_menu();
        }
    }

    /// The composition **Lights** controls: add an Ambient or Point light, and
    /// edit each light's color / intensity (and position for a point). Lights
    /// shade only **3-D layers** that opt in via the per-layer *Accepts lights*
    /// toggle; with no lights (the default) the comp renders exactly as before.
    fn lights_menu(&mut self, ui: &mut egui::Ui) {
        use crate::comp::{Light, LightKind};
        ui.label(egui::RichText::new("Lights").strong());
        ui.horizontal(|ui| {
            if ui
                .button("Add ambient")
                .on_hover_text("A flat diffuse floor that lights every 3-D surface equally")
                .clicked()
            {
                self.comp.lights.push(Light::ambient([1.0, 1.0, 1.0], 0.3));
            }
            if ui
                .button("Add point")
                .on_hover_text("An omnidirectional point light (Lambert diffuse) in comp space")
                .clicked()
            {
                self.comp.lights.push(Light::point([0.0, 0.0, -500.0], [1.0, 1.0, 1.0], 1.0));
            }
        });
        if self.comp.lights.is_empty() {
            ui.weak("No lights — 3-D layers render unlit (flat).");
            return;
        }
        let mut remove: Option<usize> = None;
        for (i, light) in self.comp.lights.iter_mut().enumerate() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(format!("{}: {}", i + 1, light.kind.label()));
                if ui.button("✕").on_hover_text("Remove this light").clicked() {
                    remove = Some(i);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Color");
                ui.color_edit_button_rgb(&mut light.color);
                ui.add(
                    egui::Slider::new(&mut light.intensity, 0.0..=4.0).text("Intensity"),
                );
            });
            if light.kind == LightKind::Point {
                ui.horizontal(|ui| {
                    ui.label("Position");
                    ui.add(egui::DragValue::new(&mut light.position[0]).speed(1.0).prefix("X "));
                    ui.add(egui::DragValue::new(&mut light.position[1]).speed(1.0).prefix("Y "));
                    ui.add(egui::DragValue::new(&mut light.position[2]).speed(1.0).prefix("Z "));
                });
            }
        }
        if let Some(i) = remove {
            self.comp.lights.remove(i);
        }
    }

    /// The export **render range** picker (After Effects' *Render: Work Area /
    /// Full Comp*): choose whether *Export* renders only the work-area frames
    /// (in → out) or the whole `[0, duration]` comp. Defaults to **work area**
    /// when it's a real sub-range, else **full comp**; the radio reflects (and
    /// pins) the effective choice, and the entry shows the resulting frame count.
    fn render_range_menu(&mut self, ui: &mut egui::Ui) {
        use crate::render::{range_frame_count, RenderRange};
        let effective = self.effective_export_range();
        ui.menu_button(format!("{}  Render range…", icons::EXPORT), |ui| {
            ui.label(egui::RichText::new("Export renders").strong());
            let wa_frames = range_frame_count(&self.comp, RenderRange::WorkArea);
            let full_frames = range_frame_count(&self.comp, RenderRange::Full);
            let wa = ui
                .radio(
                    effective == RenderRange::WorkArea,
                    format!("Work area  ({wa_frames} frames)"),
                )
                .on_hover_text("Render only the work-area range (in → out), like After Effects");
            if wa.clicked() {
                self.export_range = Some(RenderRange::WorkArea);
                ui.close_menu();
            }
            let full = ui
                .radio(
                    effective == RenderRange::Full,
                    format!("Full comp  ({full_frames} frames)"),
                )
                .on_hover_text("Render the whole [0, duration] timeline");
            if full.clicked() {
                self.export_range = Some(RenderRange::Full);
                ui.close_menu();
            }
        });
    }

    /// The comp **Work area** controls (After Effects' `B` / `N` work-area keys):
    /// set the work-area start / end to the playhead, or reset to the whole
    /// timeline. The work area bounds playback (the loop range) and shows on the
    /// timeline ruler when trimmed. The reset is disabled when already full.
    fn work_area_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Work area").strong());
        let dur = self.comp.duration;
        let wa = self.comp.clamped_work_area();
        ui.label(format!(
            "[{:.2}s – {:.2}s]  ({:.2}s)",
            wa.start,
            wa.end,
            wa.length(dur)
        ));
        if ui
            .button("Set start to playhead (B)")
            .on_hover_text("Trim the work-area start to the current time")
            .clicked()
        {
            self.set_work_area_start();
            ui.close_menu();
        }
        if ui
            .button("Set end to playhead (N)")
            .on_hover_text("Trim the work-area end to the current time")
            .clicked()
        {
            self.set_work_area_end();
            ui.close_menu();
        }
        if ui
            .add_enabled(
                !wa.is_full(dur),
                egui::Button::new("Reset to whole comp"),
            )
            .on_hover_text("Span the work area across the whole timeline")
            .clicked()
        {
            self.reset_work_area();
            ui.close_menu();
        }
    }

    /// The comp **Markers** controls: add a comp marker at the playhead and jump
    /// to the previous / next marker (comp + selected layer). Mirrors the timeline
    /// transport's marker buttons; the navigation entries are disabled when there's
    /// no marker in that direction.
    fn marker_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Markers").strong());
        if ui
            .button(format!("{}  Add comp marker", icons::MARKER))
            .on_hover_text("Drop a marker at the playhead")
            .clicked()
        {
            self.add_comp_marker();
            ui.close_menu();
        }
        ui.add_enabled_ui(self.selected.is_some(), |ui| {
            if ui
                .button(format!("{}  Add layer marker", icons::MARKER))
                .on_hover_text("Drop a marker on the selected layer at the playhead")
                .clicked()
            {
                self.add_layer_marker();
                ui.close_menu();
            }
        });
        let prev = self.comp.prev_marker(self.time, self.selected);
        let next = self.comp.next_marker(self.time, self.selected);
        if ui
            .add_enabled(
                prev.is_some(),
                egui::Button::new(format!("{}  Previous marker", icons::MARKER_PREV)),
            )
            .clicked()
        {
            if let Some(t) = prev {
                self.time = t;
                self.playing = false;
            }
            ui.close_menu();
        }
        if ui
            .add_enabled(
                next.is_some(),
                egui::Button::new(format!("{}  Next marker", icons::MARKER_NEXT)),
            )
            .clicked()
        {
            if let Some(t) = next {
                self.time = t;
                self.playing = false;
            }
            ui.close_menu();
        }
    }

    /// The **View ▸ Onion Skinning** controls: a master enable plus how many
    /// ghost frames to show before / after the playhead, the frame step between
    /// ghosts, and the nearest ghost's opacity (farther ghosts fade off). Ghosts
    /// the comp at neighbouring frames behind the live frame so hand-keyed timing
    /// reads at a glance (cool = past, warm = future). The sliders are disabled
    /// while onion skinning is off.
    fn onion_menu(&mut self, ui: &mut egui::Ui) {
        let o = &mut self.onion;
        ui.checkbox(&mut o.enabled, "Onion skinning")
            .on_hover_text("Ghost neighbouring frames behind the playhead (timing aid)");
        ui.add_enabled_ui(o.enabled, |ui| {
            let max = crate::onion::OnionSkin::MAX_PER_SIDE;
            ui.add(egui::Slider::new(&mut o.before, 0..=max).text("Before"))
                .on_hover_text("Ghost frames shown before the playhead (cool tint)");
            ui.add(egui::Slider::new(&mut o.after, 0..=max).text("After"))
                .on_hover_text("Ghost frames shown after the playhead (warm tint)");
            ui.add(egui::Slider::new(&mut o.step, 1..=10).text("Frame step"))
                .on_hover_text("Frames between successive ghosts (1 = every frame)");
            ui.add(
                egui::Slider::new(&mut o.opacity, 0.05..=1.0)
                    .text("Opacity")
                    .fixed_decimals(2),
            )
            .on_hover_text("Opacity of the ghost nearest the playhead; farther ghosts fade");
        });
    }

    /// The **Window** menu: a checkbox per dockable panel (Layers / Properties /
    /// Timeline) to show or hide it, plus *Show all panels* (reset) and *Hide all
    /// panels* (maximize the canvas). The central Preview viewport is always
    /// present and so has no toggle. Mirrors After Effects' *Window* menu and
    /// Affinity's *View ▸ Studio* show/hide.
    fn window_menu(&mut self, ui: &mut egui::Ui) {
        for panel in Panel::ALL {
            let mut shown = self.panels.is_shown(panel);
            if ui
                .checkbox(&mut shown, panel.label())
                .on_hover_text("Show or hide this panel")
                .clicked()
            {
                self.panels.toggle(panel);
            }
        }
        ui.separator();
        if ui
            .add_enabled(
                !self.panels.all_shown(),
                egui::Button::new(format!("{}  Show all panels", icons::PANEL)),
            )
            .on_hover_text("Restore the default four-panel workspace")
            .clicked()
        {
            self.panels.show_all();
            ui.close_menu();
        }
        if ui
            .add_enabled(
                !self.panels.all_hidden(),
                egui::Button::new(format!("{}  Hide all panels", icons::PANEL)),
            )
            .on_hover_text("Leave only the preview viewport")
            .clicked()
        {
            self.panels.hide_all();
            ui.close_menu();
        }
    }
}
