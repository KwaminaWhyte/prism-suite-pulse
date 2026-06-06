//! The top menu bar (file/layer/playback commands + status) and the comp's
//! motion-blur settings popup.

use super::PulseApp;
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
}
