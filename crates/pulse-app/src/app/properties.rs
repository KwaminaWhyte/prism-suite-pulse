//! The selected layer's properties panel: transform tracks, masks, effects,
//! spatial effects, parenting, and track-matte controls, plus the per-widget
//! editors those rows delegate to.

use super::PulseApp;
use crate::comp::{
    Ease, Effect, Fill, Interp, LayerKind, Mask, MaskMode, MatteMode, Prop, ShapeItem,
    ShapePrimitive, SpatialEffect, Stroke, TextAlign,
};
use crate::{icons, render};
use egui::Color32;

impl PulseApp {
    pub(super) fn properties_panel(&mut self, root: &mut egui::Ui) {
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

                // Shape content (rect / ellipse / polygon / star + fill/stroke),
                // shown only for shape layers.
                if self.comp.layers[idx].kind == LayerKind::Shape {
                    ui.separator();
                    self.shape_section(ui, idx);
                }

                // Text content (string + type settings + fill/stroke), shown
                // only for text layers.
                if self.comp.layers[idx].kind == LayerKind::Text {
                    ui.separator();
                    self.text_section(ui, idx);
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

                // Spatial effects (Gaussian Blur / Drop Shadow / Glow). Only
                // meaningful for layers that draw their own pixels (a null draws
                // nothing; an adjustment's grade is per-pixel, not a buffer pass).
                if self.comp.layers[idx].kind.draws_own_pixels() {
                    ui.separator();
                    self.spatial_effects_section(ui, idx);
                }

                // Masks carve the layer's coverage — only meaningful for layers
                // that draw their own pixels (a null/adjustment has no coverage).
                if self.comp.layers[idx].kind.draws_own_pixels() {
                    ui.separator();
                    self.masks_section(ui, idx);
                }
            });
    }

    /// The layer's **mask** editor: an "Add mask" menu (rectangle / ellipse),
    /// then each mask with its mode / invert / opacity / feather / expansion
    /// controls and reorder / remove buttons. Masks fold top-down into the
    /// layer's coverage.
    fn masks_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        // Size a fresh mask to roughly half the layer's base quad.
        let half_w = self.comp.width as f32 * render::LAYER_HALF_FRAC * 0.5;
        let half_h = self.comp.height as f32 * render::LAYER_HALF_FRAC * 0.5;
        ui.horizontal(|ui| {
            ui.heading("Masks");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(format!("{}  Add", icons::ADD_KEY), |ui| {
                    if ui.button("Rectangle").clicked() {
                        let n = self.comp.layers[idx].masks.len() + 1;
                        let mut m = Mask::rect(half_w, half_h);
                        m.name = format!("Mask {n}");
                        self.comp.layers[idx].masks.push(m);
                        ui.close_menu();
                    }
                    if ui.button("Ellipse").clicked() {
                        let n = self.comp.layers[idx].masks.len() + 1;
                        let mut m = Mask::ellipse(half_w, half_h);
                        m.name = format!("Mask {n}");
                        self.comp.layers[idx].masks.push(m);
                        ui.close_menu();
                    }
                });
            });
        });

        if self.comp.layers[idx].masks.is_empty() {
            ui.weak("No masks. Add one to carve this layer's coverage.");
            return;
        }

        let mut to_remove: Option<usize> = None;
        let mut to_move: Option<(usize, bool)> = None;
        let n = self.comp.layers[idx].masks.len();
        for mi in 0..n {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.comp.layers[idx].masks[mi].name).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(icons::TRASH).on_hover_text("Remove").clicked() {
                        to_remove = Some(mi);
                    }
                    if ui
                        .add_enabled(mi > 0, egui::Button::new(icons::ARROW_UP))
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        to_move = Some((mi, true));
                    }
                    if ui
                        .add_enabled(mi + 1 < n, egui::Button::new(icons::ARROW_DOWN))
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        to_move = Some((mi, false));
                    }
                });
            });
            mask_params(ui, idx, mi, &mut self.comp.layers[idx].masks[mi]);
        }

        if let Some(mi) = to_remove {
            self.comp.layers[idx].masks.remove(mi);
        }
        if let Some((mi, up)) = to_move {
            let masks = &mut self.comp.layers[idx].masks;
            let other = if up { mi.wrapping_sub(1) } else { mi + 1 };
            if other < masks.len() {
                masks.swap(mi, other);
            }
        }
    }

    /// The shape layer's **content** editor: an "Add shape" menu (rectangle /
    /// ellipse / polygon / star), then each item with its primitive parameters,
    /// fill, and stroke, plus reorder / remove. Items composite bottom-up.
    fn shape_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        // Size a fresh shape to roughly half the layer's base quad.
        let half = self.comp.width as f32 * render::LAYER_HALF_FRAC * 0.5;
        ui.horizontal(|ui| {
            ui.heading("Shape");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(format!("{}  Add", icons::ADD_KEY), |ui| {
                    let prims = [
                        (
                            "Rectangle",
                            ShapePrimitive::Rectangle {
                                half_w: half,
                                half_h: half,
                                radius: 0.0,
                            },
                        ),
                        ("Ellipse", ShapePrimitive::Ellipse { rx: half, ry: half }),
                        (
                            "Polygon",
                            ShapePrimitive::Polygon {
                                points: 5,
                                radius: half,
                            },
                        ),
                        (
                            "Star",
                            ShapePrimitive::Star {
                                points: 5,
                                outer: half,
                                inner: half * 0.5,
                            },
                        ),
                    ];
                    for (label, prim) in prims {
                        if ui.button(label).clicked() {
                            self.comp.layers[idx].shape.items.push(ShapeItem::new(prim));
                            ui.close_menu();
                        }
                    }
                });
            });
        });

        if self.comp.layers[idx].shape.items.is_empty() {
            ui.weak("No shapes. Click Add to draw one.");
            return;
        }

        let mut to_remove: Option<usize> = None;
        let mut to_move: Option<(usize, bool)> = None;
        let n = self.comp.layers[idx].shape.items.len();
        for si in 0..n {
            ui.separator();
            ui.horizontal(|ui| {
                let label = self.comp.layers[idx].shape.items[si].primitive.label();
                ui.label(egui::RichText::new(label).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(icons::TRASH).on_hover_text("Remove").clicked() {
                        to_remove = Some(si);
                    }
                    if ui
                        .add_enabled(si > 0, egui::Button::new(icons::ARROW_UP))
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        to_move = Some((si, true));
                    }
                    if ui
                        .add_enabled(si + 1 < n, egui::Button::new(icons::ARROW_DOWN))
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        to_move = Some((si, false));
                    }
                });
            });
            shape_item_params(ui, idx, si, &mut self.comp.layers[idx].shape.items[si]);
        }

        if let Some(si) = to_remove {
            self.comp.layers[idx].shape.items.remove(si);
        }
        if let Some((si, up)) = to_move {
            let items = &mut self.comp.layers[idx].shape.items;
            let other = if up { si.wrapping_sub(1) } else { si + 1 };
            if other < items.len() {
                items.swap(si, other);
            }
        }
    }

    /// The text layer's content editor: a multi-line string, type settings
    /// (size / tracking / leading / alignment), and a fill / stroke (reused from
    /// the shape system). The text is drawn with the built-in stroke font.
    fn text_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        ui.heading("Text");
        let text = &mut self.comp.layers[idx].text;

        ui.add(
            egui::TextEdit::multiline(&mut text.text)
                .desired_rows(2)
                .hint_text("Type text…")
                .desired_width(f32::INFINITY),
        );

        ui.horizontal(|ui| {
            ui.label("Size");
            ui.add(egui::Slider::new(&mut text.size, 8.0..=600.0).suffix(" px"));
        });
        ui.horizontal(|ui| {
            ui.label("Tracking");
            ui.add(egui::Slider::new(&mut text.tracking, -50.0..=200.0).suffix(" px"));
        });
        ui.horizontal(|ui| {
            ui.label("Leading");
            ui.add(
                egui::Slider::new(&mut text.leading, 0.0..=800.0)
                    .suffix(" px")
                    .text("(0 = auto)"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Align");
            egui::ComboBox::from_id_salt(("text_align", idx))
                .selected_text(text.align.label())
                .show_ui(ui, |ui| {
                    for a in TextAlign::ALL {
                        if ui.selectable_label(text.align == a, a.label()).clicked() {
                            text.align = a;
                        }
                    }
                });
        });

        // Fill toggle + color/opacity.
        ui.horizontal(|ui| {
            let mut on = text.fill.is_some();
            if ui.checkbox(&mut on, "Fill").changed() {
                text.fill = on.then(Fill::default);
            }
            if let Some(fill) = text.fill.as_mut() {
                rgb_button(ui, (idx, 0, 2), &mut fill.color);
            }
        });
        if let Some(fill) = text.fill.as_mut() {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label("Fill opacity");
                ui.add(egui::Slider::new(&mut fill.opacity, 0.0..=1.0));
            });
        }

        // Stroke toggle + color/width/opacity.
        ui.horizontal(|ui| {
            let mut on = text.stroke.is_some();
            if ui.checkbox(&mut on, "Stroke").changed() {
                text.stroke = on.then(Stroke::default);
            }
            if let Some(stroke) = text.stroke.as_mut() {
                rgb_button(ui, (idx, 1, 2), &mut stroke.color);
            }
        });
        if let Some(stroke) = text.stroke.as_mut() {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label("Stroke width");
                ui.add(egui::Slider::new(&mut stroke.width, 0.0..=80.0).suffix(" px"));
            });
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label("Stroke opacity");
                ui.add(egui::Slider::new(&mut stroke.opacity, 0.0..=1.0));
            });
        }
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

    /// The layer's **spatial effect stack** editor: an "Add" menu (Gaussian
    /// Blur / Drop Shadow / Glow), then each effect with reorder / remove
    /// controls and per-parameter sliders. Spatial effects convolve / bloom /
    /// shadow the layer's whole rendered buffer, after its color-correction
    /// stack, masks, and track matte.
    fn spatial_effects_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        ui.horizontal(|ui| {
            ui.heading("Spatial effects");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(format!("{}  Add", icons::ADD_KEY), |ui| {
                    for eff in SpatialEffect::defaults() {
                        if ui.button(eff.label()).clicked() {
                            self.comp.layers[idx].spatial_effects.push(eff);
                            ui.close_menu();
                        }
                    }
                });
            });
        });

        if self.comp.layers[idx].spatial_effects.is_empty() {
            ui.weak("No spatial effects. Add blur, shadow, or glow.");
            return;
        }

        let mut to_remove: Option<usize> = None;
        let mut to_move: Option<(usize, bool)> = None;
        let n = self.comp.layers[idx].spatial_effects.len();
        for ei in 0..n {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(self.comp.layers[idx].spatial_effects[ei].label()).strong(),
                );
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
            spatial_effect_params(ui, idx, ei, &mut self.comp.layers[idx].spatial_effects[ei]);
        }

        if let Some(ei) = to_remove {
            self.comp.layers[idx].spatial_effects.remove(ei);
        }
        if let Some((ei, up)) = to_move {
            let effects = &mut self.comp.layers[idx].spatial_effects;
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

/// Parameter sliders / color pickers for one [`SpatialEffect`], editing it in
/// place. `idx`/`ei` salt widget ids so multiple effects don't collide.
fn spatial_effect_params(ui: &mut egui::Ui, idx: usize, ei: usize, effect: &mut SpatialEffect) {
    let slider = |ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32, suffix: &str| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(label);
            ui.add(egui::Slider::new(v, lo..=hi).suffix(suffix.to_owned()));
        });
    };
    match effect {
        SpatialEffect::GaussianBlur {
            sigma_x,
            sigma_y,
            repeat_edge,
        } => {
            slider(ui, "Blur X", sigma_x, 0.0, 100.0, " px");
            slider(ui, "Blur Y", sigma_y, 0.0, 100.0, " px");
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.checkbox(repeat_edge, "Repeat edge pixels")
                    .on_hover_text("Clamp the kernel to the edge instead of fading to transparent");
            });
        }
        SpatialEffect::DropShadow {
            color,
            opacity,
            angle,
            distance,
            softness,
            shadow_only,
        } => {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label("Color");
                rgb_button(ui, (idx, ei, 0), color);
            });
            slider(ui, "Opacity", opacity, 0.0, 1.0, "");
            slider(ui, "Direction", angle, -180.0, 180.0, "°");
            slider(ui, "Distance", distance, 0.0, 200.0, " px");
            slider(ui, "Softness", softness, 0.0, 100.0, " px");
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.checkbox(shadow_only, "Shadow only")
                    .on_hover_text("Drop the layer, keeping just its shadow");
            });
        }
        SpatialEffect::Glow {
            threshold,
            radius,
            intensity,
        } => {
            slider(ui, "Threshold", threshold, 0.0, 1.0, "");
            slider(ui, "Radius", radius, 0.0, 100.0, " px");
            slider(ui, "Intensity", intensity, 0.0, 4.0, "");
        }
    }
}

/// Parameter controls for one [`Mask`], editing it in place: name, the
/// boolean mode + invert toggle, and opacity / feather / expansion sliders.
/// `idx`/`mi` salt widget ids so multiple masks don't collide.
fn mask_params(ui: &mut egui::Ui, idx: usize, mi: usize, mask: &mut Mask) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label("Name");
        ui.add(
            egui::TextEdit::singleline(&mut mask.name)
                .id_salt(("mask_name", idx, mi))
                .desired_width(120.0),
        );
    });
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label("Mode");
        egui::ComboBox::from_id_salt(("mask_mode", idx, mi))
            .selected_text(mask.mode.label())
            .show_ui(ui, |ui| {
                for mode in MaskMode::ALL {
                    if ui
                        .selectable_label(mask.mode == mode, mode.label())
                        .clicked()
                    {
                        mask.mode = mode;
                    }
                }
            });
        ui.checkbox(&mut mask.inverted, "Invert")
            .on_hover_text("Show the layer outside the shape instead of inside");
    });
    let slider = |ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(label);
            ui.add(egui::Slider::new(v, lo..=hi));
        });
    };
    slider(ui, "Opacity", &mut mask.opacity, 0.0, 1.0);
    slider(ui, "Feather", &mut mask.feather, 0.0, 200.0);
    slider(ui, "Expansion", &mut mask.expansion, -200.0, 200.0);
}

/// Parameter controls for one [`ShapeItem`], editing it in place: the
/// primitive's parameters, its local offset, and fill / stroke toggles with
/// their color and sliders. `idx`/`si` salt widget ids so multiple items don't
/// collide.
fn shape_item_params(ui: &mut egui::Ui, idx: usize, si: usize, item: &mut ShapeItem) {
    let slider = |ui: &mut egui::Ui, label: &str, v: &mut f32, lo: f32, hi: f32, suffix: &str| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(label);
            ui.add(egui::Slider::new(v, lo..=hi).suffix(suffix.to_owned()));
        });
    };
    let int_slider = |ui: &mut egui::Ui, label: &str, v: &mut u32, lo: u32, hi: u32| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(label);
            ui.add(egui::Slider::new(v, lo..=hi));
        });
    };

    match &mut item.primitive {
        ShapePrimitive::Rectangle {
            half_w,
            half_h,
            radius,
        } => {
            slider(ui, "Width", half_w, 1.0, 800.0, " px");
            slider(ui, "Height", half_h, 1.0, 800.0, " px");
            slider(ui, "Roundness", radius, 0.0, 400.0, " px");
        }
        ShapePrimitive::Ellipse { rx, ry } => {
            slider(ui, "Radius X", rx, 1.0, 800.0, " px");
            slider(ui, "Radius Y", ry, 1.0, 800.0, " px");
        }
        ShapePrimitive::Polygon { points, radius } => {
            int_slider(ui, "Points", points, 3, 24);
            slider(ui, "Radius", radius, 1.0, 800.0, " px");
        }
        ShapePrimitive::Star {
            points,
            outer,
            inner,
        } => {
            int_slider(ui, "Points", points, 2, 24);
            slider(ui, "Outer", outer, 1.0, 800.0, " px");
            slider(ui, "Inner", inner, 1.0, 800.0, " px");
        }
    }
    slider(ui, "Offset X", &mut item.offset_x, -800.0, 800.0, " px");
    slider(ui, "Offset Y", &mut item.offset_y, -800.0, 800.0, " px");

    // Fill toggle + color/opacity.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let mut on = item.fill.is_some();
        if ui.checkbox(&mut on, "Fill").changed() {
            item.fill = on.then(Fill::default);
        }
        if let Some(fill) = item.fill.as_mut() {
            rgb_button(ui, (idx, si, 0), &mut fill.color);
        }
    });
    if let Some(fill) = item.fill.as_mut() {
        slider(ui, "Fill opacity", &mut fill.opacity, 0.0, 1.0, "");
    }

    // Stroke toggle + color/width/opacity.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let mut on = item.stroke.is_some();
        if ui.checkbox(&mut on, "Stroke").changed() {
            item.stroke = on.then(Stroke::default);
        }
        if let Some(stroke) = item.stroke.as_mut() {
            rgb_button(ui, (idx, si, 1), &mut stroke.color);
        }
    });
    if let Some(stroke) = item.stroke.as_mut() {
        slider(ui, "Stroke width", &mut stroke.width, 0.0, 80.0, " px");
        slider(ui, "Stroke opacity", &mut stroke.opacity, 0.0, 1.0, "");
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
