//! The **Effects & Presets** browser panel: a searchable, categorised list of
//! every addable effect, with a click-to-add affordance.
//!
//! Replaces the awkwardness of the two flat "Add" menus (one per stack) with
//! After Effects' *Effects & Presets* model — a search box that type-filters the
//! whole effect registry, grouped into collapsible category folders. The pure
//! search/ranking lives in [`crate::comp::effect_browser`]; this panel just
//! renders the filtered groups and, on a click, appends the chosen effect to the
//! selected layer's matching stack.

use super::PulseApp;
use crate::comp::{filter_grouped, BrowserEntry, NewEffect, Stack};
use crate::icons;

impl PulseApp {
    /// Append the effect described by `entry` to the selected layer's matching
    /// stack (per-pixel colour effects → `effects`; whole-buffer spatial effects
    /// → `spatial_effects`). No-op when no layer is selected. Returns whether an
    /// effect was added (so the caller can surface a status line).
    pub(super) fn add_browser_effect(&mut self, entry: &BrowserEntry) -> bool {
        let Some(idx) = self.selected else {
            return false;
        };
        let Some(layer) = self.comp.layers.get_mut(idx) else {
            return false;
        };
        match entry.instantiate() {
            NewEffect::Color(e) => layer.effects.push(e),
            NewEffect::Spatial(e) => layer.spatial_effects.push(e),
        }
        true
    }

    pub(super) fn effects_panel(&mut self, root: &mut egui::Ui) {
        egui::SidePanel::left("effects_browser")
            .default_width(230.0)
            .show_inside(root, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.heading("Effects & Presets");
                });
                ui.add_space(2.0);

                // Search box: a magnifying-glass prefix, the query field, and a
                // clear button (enabled only when there's something to clear).
                ui.horizontal(|ui| {
                    ui.label(icons::SEARCH);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.effect_query)
                            .hint_text("Search effects…")
                            .desired_width(f32::INFINITY)
                            .id_salt("effect_search"),
                    );
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !self.effect_query.is_empty(),
                            egui::Button::new(format!("{}  Clear", icons::CLEAR)).small(),
                        )
                        .clicked()
                    {
                        self.effect_query.clear();
                    }
                    // A hint about where a click lands.
                    if let Some(idx) = self.selected {
                        if let Some(layer) = self.comp.layers.get(idx) {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.weak(format!("→ {}", layer.name))
                                        .on_hover_text("Effects add to the selected layer");
                                },
                            );
                        }
                    }
                });
                ui.separator();

                if self.selected.is_none() {
                    ui.weak("Select a layer to add effects to it.");
                }

                let groups = filter_grouped(&self.effect_query);
                if groups.is_empty() {
                    ui.add_space(8.0);
                    ui.weak(format!("No effects match “{}”.", self.effect_query.trim()));
                    return;
                }

                // The chosen entry (deferred so the borrow of `groups` ends before
                // we mutate the comp).
                let mut chosen: Option<BrowserEntry> = None;
                let querying = !self.effect_query.trim().is_empty();

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (cat, hits) in &groups {
                            // While searching, open every folder (results are
                            // scarce); idle, start collapsed so the panel is tidy.
                            egui::CollapsingHeader::new(egui::RichText::new(cat.label()).strong())
                                .id_salt(("effect_cat", cat.label()))
                                .default_open(querying)
                                .show(ui, |ui| {
                                    for hit in hits {
                                        let entry = hit.entry;
                                        let stack_tag = match entry.stack {
                                            Stack::Color => "color",
                                            Stack::Spatial => "buffer",
                                        };
                                        let resp = ui
                                            .add(egui::Button::new(format!(
                                                "{}  {}",
                                                icons::EFFECT,
                                                entry.name
                                            )))
                                            .on_hover_text(format!(
                                            "Add “{}” ({stack_tag} effect) to the selected layer",
                                            entry.name
                                        ));
                                        if resp.clicked() {
                                            chosen = Some(*entry);
                                        }
                                    }
                                });
                        }
                    });

                if let Some(entry) = chosen {
                    if self.add_browser_effect(&entry) {
                        self.status = Some(format!("Added {}", entry.name));
                    } else {
                        self.status = Some("Select a layer first".to_string());
                    }
                }
            });
    }
}
