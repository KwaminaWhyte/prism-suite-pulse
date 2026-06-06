//! The bottom timeline / graph editor panel (with its property chips) and the
//! central preview panel.

use super::{EditorMode, PulseApp};
use crate::comp::Prop;
use crate::{graph, icons, preview, timeline};
use egui::Sense;

impl PulseApp {
    pub(super) fn timeline_panel(&mut self, root: &mut egui::Ui) {
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
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
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

    pub(super) fn preview_panel(&mut self, root: &mut egui::Ui) {
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
