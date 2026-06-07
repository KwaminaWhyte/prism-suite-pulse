//! The selected layer's properties panel: transform tracks, masks, effects,
//! spatial effects, parenting, and track-matte controls, plus the per-widget
//! editors those rows delegate to.

use super::PulseApp;
use crate::comp::{
    blend_label, expr_last_error, source_from_path, AlphaMode, BlendMode, Ease, Effect, ExprCtx,
    Fill, FootageSource, Interp, LayerBlend, LayerKind, Mask, MaskMode, MatteMode, Prop, ShapeItem,
    ShapePrimitive, SpatialEffect, Stroke, TextAlign, Track,
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

                // The properties body can far exceed the window height (transform
                // tracks + effects + spatial + masks + text/shape sections), so
                // scroll it. `auto_shrink([false, false])` keeps the panel full
                // width/height even when the content is short.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
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
                                        if ui.selectable_label(cur == kind, kind.label()).clicked()
                                        {
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

                        // Per-layer blend mode: how this layer composites over the
                        // layers beneath it. Only meaningful for layers that draw
                        // their own pixels (a null draws nothing; an adjustment
                        // grades in place rather than compositing).
                        if self.comp.layers[idx].kind.draws_own_pixels() {
                            self.blend_row(ui, idx);
                        }

                        // Shape content (rect / ellipse / polygon / star + fill/stroke),
                        // shown only for shape layers.
                        if self.comp.layers[idx].kind == LayerKind::Shape {
                            section(ui, ("sec_shape", idx), "Shape", |ui| {
                                self.shape_section(ui, idx);
                            });
                        }

                        // Text content (string + type settings + fill/stroke), shown
                        // only for text layers.
                        if self.comp.layers[idx].kind == LayerKind::Text {
                            section(ui, ("sec_text", idx), "Text", |ui| {
                                self.text_section(ui, idx);
                            });
                        }

                        // Footage content (still / image-sequence source + alpha /
                        // fps / loop options), shown only for footage layers.
                        if self.comp.layers[idx].kind == LayerKind::Footage {
                            section(ui, ("sec_footage", idx), "Footage", |ui| {
                                self.footage_section(ui, idx);
                            });
                        }

                        // Precomp reference (target comp + time offset), shown only
                        // for precomp layers.
                        if self.comp.layers[idx].kind == LayerKind::Precomp {
                            section(ui, ("sec_precomp", idx), "Precomp", |ui| {
                                self.precomp_section(ui, idx);
                            });
                        }

                        // Time remap (enable toggle + keyframable source-time
                        // track), shown only for time-based layers (footage /
                        // precomp) — the sources whose playback it can retime.
                        if matches!(
                            self.comp.layers[idx].kind,
                            LayerKind::Footage | LayerKind::Precomp
                        ) {
                            let t = self.time;
                            section(ui, ("sec_time_remap", idx), "Time remap", |ui| {
                                self.time_remap_section(ui, idx, t);
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
                                    .on_hover_text(
                                        "Blur this layer's motion across the comp shutter",
                                    );
                                if self.comp.layers[idx].motion_blur
                                    && !self.comp.motion_blur.enabled
                                {
                                    ui.weak("(comp switch off)")
                                        .on_hover_text("Enable Comp ▸ Motion blur to see it");
                                }
                            });
                        }

                        let t = self.time;
                        section(ui, ("sec_transform", idx), "Transform", |ui| {
                            for prop in Prop::ALL {
                                self.property_row(ui, idx, prop, t);
                            }
                        });

                        // Effect stack (color-correction passes). Nulls draw nothing, so
                        // an effect stack on them would do nothing — hide the section.
                        if self.comp.layers[idx].kind != LayerKind::Null {
                            section(ui, ("sec_effects", idx), "Effects", |ui| {
                                self.effects_section(ui, idx);
                            });
                        }

                        // Spatial effects (Gaussian Blur / Drop Shadow / Glow). Only
                        // meaningful for layers that draw their own pixels (a null draws
                        // nothing; an adjustment's grade is per-pixel, not a buffer pass).
                        if self.comp.layers[idx].kind.draws_own_pixels() {
                            section(ui, ("sec_spatial", idx), "Spatial effects", |ui| {
                                self.spatial_effects_section(ui, idx);
                            });
                        }

                        // Masks carve the layer's coverage — only meaningful for layers
                        // that draw their own pixels (a null/adjustment has no coverage).
                        if self.comp.layers[idx].kind.draws_own_pixels() {
                            section(ui, ("sec_masks", idx), "Masks", |ui| {
                                self.masks_section(ui, idx);
                            });
                        }
                    });
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

    /// The layer's **footage** editor: pick a still or an image sequence from
    /// disk, choose the alpha interpretation, and (for sequences) set an fps
    /// override and loop / hold-last playback. Picking a numbered file offers to
    /// detect the whole sequence on disk; otherwise it loads as a single still.
    fn footage_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        let comp_fps = self.comp.fps;
        let footage = &mut self.comp.layers[idx].footage;

        // Current source summary.
        match &footage.source {
            Some(src) => {
                ui.horizontal(|ui| {
                    ui.label(src.kind_label());
                });
                ui.add(
                    egui::Label::new(egui::RichText::new(src.display()).weak().small())
                        .truncate(),
                );
                if let FootageSource::Sequence { count, .. } = src {
                    ui.weak(format!("{count} frames"));
                }
            }
            None => {
                ui.weak("No source. Import a still or image sequence.");
            }
        }

        // Import buttons: a single still, or a sequence (auto-detected from a
        // numbered file the user picks).
        ui.horizontal(|ui| {
            if ui
                .button(format!("{}  Still…", icons::ADD_LAYER))
                .on_hover_text("Pick a single image file")
                .clicked()
            {
                if let Some(path) = footage_pick_dialog("Import still image") {
                    footage.source = Some(FootageSource::still(path));
                }
            }
            if ui
                .button(format!("{}  Sequence…", icons::ADD_LAYER))
                .on_hover_text("Pick any frame of a numbered image sequence")
                .clicked()
            {
                if let Some(path) = footage_pick_dialog("Import image sequence (pick any frame)") {
                    footage.source = Some(source_from_path(&path));
                }
            }
        });
        if footage.source.is_some()
            && ui
                .button(format!("{}  Clear", icons::TRASH))
                .on_hover_text("Remove this footage source")
                .clicked()
        {
            footage.source = None;
        }

        ui.separator();

        // Alpha interpretation.
        ui.horizontal(|ui| {
            ui.label("Alpha");
            egui::ComboBox::from_id_salt(("footage_alpha", idx))
                .selected_text(footage.alpha.label())
                .show_ui(ui, |ui| {
                    for a in AlphaMode::ALL {
                        if ui
                            .selectable_label(footage.alpha == a, a.label())
                            .clicked()
                        {
                            footage.alpha = a;
                        }
                    }
                });
        });

        // Sequence playback: fps override + loop / hold (only meaningful for a
        // multi-frame sequence, but shown whenever a source is set).
        let is_seq = matches!(footage.source, Some(FootageSource::Sequence { .. }));
        ui.add_enabled_ui(is_seq, |ui| {
            ui.horizontal(|ui| {
                let mut override_on = footage.fps.is_some();
                if ui
                    .checkbox(&mut override_on, "FPS override")
                    .on_hover_text("Play the sequence at a custom rate (else the comp's fps)")
                    .changed()
                {
                    footage.fps = override_on.then_some(comp_fps);
                }
                if let Some(fps) = footage.fps.as_mut() {
                    ui.add(egui::DragValue::new(fps).range(0.1..=240.0).suffix(" fps"));
                } else {
                    ui.weak(format!("comp: {comp_fps:.0} fps"));
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut footage.looping, "Loop")
                    .on_hover_text("Wrap back to the first frame past the end")
                    .changed()
                    && footage.looping
                {
                    footage.hold_last = false;
                }
                if ui
                    .checkbox(&mut footage.hold_last, "Hold last")
                    .on_hover_text("Freeze on the last frame past the end")
                    .changed()
                    && footage.hold_last
                {
                    footage.looping = false;
                }
            });
        });
    }

    /// The layer's **precomp** editor: pick which comp this layer nests (from the
    /// project's other comps) and set a time-offset shift. The referenced comp is
    /// rendered recursively at render/export time.
    ///
    /// Self-reference and cycles are allowed in the picker (the renderer's cycle
    /// guard breaks them — a cyclic precomp simply renders nothing) but the active
    /// comp is flagged in the list so the user knows it would loop.
    fn precomp_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        let active_id = self.comp.id;
        let current = self.comp.layers[idx].precomp.source;
        // Build the list of selectable comps: every *other* comp in the project,
        // plus the active comp itself (flagged — it self-references).
        let others: Vec<(u64, String)> = self
            .others
            .iter()
            .map(|c| (c.id, c.display_name()))
            .collect();
        let current_label = match current {
            Some(id) if id == active_id => format!("{} (self)", self.comp.display_name()),
            Some(id) => others
                .iter()
                .find(|(cid, _)| *cid == id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| format!("Comp {id} (missing)")),
            None => "None".to_owned(),
        };

        let mut chosen: Option<Option<u64>> = None;
        ui.horizontal(|ui| {
            ui.label("Source comp");
            egui::ComboBox::from_id_salt(("precomp_src", idx))
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_none(), "None").clicked() {
                        chosen = Some(None);
                    }
                    for (cid, name) in &others {
                        if ui
                            .selectable_label(current == Some(*cid), name)
                            .clicked()
                        {
                            chosen = Some(Some(*cid));
                        }
                    }
                    // The active comp itself (self-reference): allowed, but the
                    // cycle guard renders it as nothing.
                    let self_label = format!("{} (self — renders nothing)", self.comp.display_name());
                    if ui
                        .selectable_label(current == Some(active_id), self_label)
                        .clicked()
                    {
                        chosen = Some(Some(active_id));
                    }
                });
        });
        if let Some(next) = chosen {
            self.comp.layers[idx].precomp.source = next;
        }

        if others.is_empty() && current != Some(active_id) {
            ui.weak("No other comps yet. Use Layer ▸ Pre-compose to create one.");
        }

        // Time offset (seconds added to the host time before sampling the nested
        // comp — a minimal time-remap shift).
        ui.horizontal(|ui| {
            ui.label("Time offset");
            ui.add(
                egui::DragValue::new(&mut self.comp.layers[idx].precomp.time_offset)
                    .speed(0.01)
                    .suffix(" s"),
            )
            .on_hover_text("Shift the nested comp earlier/later on this timeline");
        });

        ui.weak("Nested comp renders at export; the preview shows a placeholder quad.");
    }

    /// The layer's **time-remap** editor (After Effects' *Enable Time Remap*): an
    /// enable toggle that seeds AE-style default keys, then the remap *source
    /// time* shown as a keyframable property (reusing the same value slider +
    /// keyframe + `fx` expression UI as the transform rows). When enabled on a
    /// time-based layer the source is sampled at this remapped time instead of the
    /// comp time, letting the user freeze / reverse / retime playback.
    fn time_remap_section(&mut self, ui: &mut egui::Ui, idx: usize, t: f32) {
        let comp_duration = self.comp.duration;
        let comp_fps = self.comp.fps;
        // The source's natural duration (seconds), used to seed an identity ramp
        // when the user enables the remap: footage = frames / fps; precomp = the
        // referenced comp's duration. `None` (a still / unknown) seeds a single
        // identity key instead.
        let source_duration = self.source_duration_for(idx, comp_fps);

        // Enable toggle. Switching it on seeds AE-style default keys (an identity
        // ramp 0 → source_duration over the comp span, or a single identity key
        // when the duration is unknown); switching it off keeps the keys so the
        // user can re-enable without losing a hand-tuned curve.
        let mut enabled = self.comp.layers[idx].time_remap.enabled;
        if ui
            .checkbox(&mut enabled, "Enable Time Remap")
            .on_hover_text("Drive this layer's source time with a keyframable curve")
            .changed()
        {
            self.comp.layers[idx].time_remap.enabled = enabled;
            if enabled {
                self.comp.layers[idx]
                    .time_remap
                    .seed_default(comp_duration, source_duration);
            }
        }

        if !self.comp.layers[idx].time_remap.enabled {
            ui.weak("Off — the source plays at the comp time (1:1).");
            return;
        }

        // Reuse the generic keyframable-track row (value slider + add-key + fx
        // expression + interpolation) for the remap's source-time track.
        self.track_row(
            ui,
            ("time_remap", idx),
            "Remap value",
            " s",
            0.0..=comp_duration.max(1.0),
            t,
            |layer| &layer.time_remap.track,
            |layer| &mut layer.time_remap.track,
        );
    }

    /// The source's natural duration (seconds) for layer `idx`, used to seed the
    /// time-remap identity ramp: a footage **sequence** is `frames / fps` (fps
    /// override or comp fps), a **still** has none, and a **precomp** is its
    /// referenced comp's duration. `None` when there's nothing meaningful to ramp
    /// to (a still, an unwired/missing reference).
    fn source_duration_for(&self, idx: usize, comp_fps: f32) -> Option<f32> {
        let layer = self.comp.layers.get(idx)?;
        match layer.kind {
            LayerKind::Footage => match layer.footage.source.as_ref()? {
                FootageSource::Sequence { count, .. } => {
                    let fps = layer.footage.fps.unwrap_or(comp_fps).max(0.1);
                    Some(*count as f32 / fps)
                }
                FootageSource::Still { .. } => None,
            },
            LayerKind::Precomp => {
                let id = layer.precomp.source?;
                // The referenced comp is either the active comp (self) or one of
                // the project's other comps.
                if id == self.comp.id {
                    Some(self.comp.duration)
                } else {
                    self.others.iter().find(|c| c.id == id).map(|c| c.duration)
                }
            }
            _ => None,
        }
    }

    /// The layer's **effect stack** editor: an "Add effect" menu, then each
    /// effect with reorder / remove controls and per-parameter sliders. Effects
    /// process the layer's own color (solid) or the layers below (adjustment).
    fn effects_section(&mut self, ui: &mut egui::Ui, idx: usize) {
        ui.horizontal(|ui| {
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

    /// The **blend-mode** selector for layer `idx`: a combo of the suite's 18
    /// shared blend modes (Normal … Luminosity), grouped separable then HSL.
    /// Choosing a mode changes how the layer composites over everything beneath
    /// it in the renderer (Normal is plain source-over).
    fn blend_row(&mut self, ui: &mut egui::Ui, idx: usize) {
        let current = self.comp.layers[idx].blend_mode();
        let mut chosen: Option<BlendMode> = None;
        ui.horizontal(|ui| {
            ui.label("Blend");
            egui::ComboBox::from_id_salt(("blend", idx))
                .selected_text(blend_label(current))
                .show_ui(ui, |ui| {
                    for (i, &mode) in BlendMode::ALL.iter().enumerate() {
                        // A faint divider before the HSL group (the last 4 modes).
                        if i == BlendMode::ALL.len() - 4 {
                            ui.separator();
                        }
                        if ui
                            .selectable_label(current == mode, blend_label(mode))
                            .clicked()
                        {
                            chosen = Some(mode);
                        }
                    }
                });
        });
        if let Some(mode) = chosen {
            self.comp.layers[idx].blend = LayerBlend(mode);
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

    /// One property: live value slider + keyframe controls + an "fx" expression
    /// toggle that reveals a per-property expression editor.
    fn property_row(&mut self, ui: &mut egui::Ui, idx: usize, prop: Prop, t: f32) {
        let (range, suffix) = prop.range();
        // Expression-resolved value (post-expression) for the live read-out, and
        // whether this property currently carries an expression — both computed
        // before the mutable layer borrow below (they read `self.comp`).
        let resolved = self.comp.layer_value(idx, prop, t);
        let has_expr = self.comp.layers[idx].track(prop).has_expression();

        let layer = &mut self.comp.layers[idx];
        let key_count = layer.track(prop).keys.len();

        // The slider edits the *keyframed value at the playhead*. When an
        // expression is active it overrides the keyframes at render time, so the
        // slider edits the underlying value the expression sees as `value`.
        let mut value = layer.value(prop, t);

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(prop.label()).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // "fx" toggle: enable/disable an expression on this property.
                let fx = ui
                    .selectable_label(has_expr, "fx")
                    .on_hover_text("Toggle an expression on this property");
                if fx.clicked() {
                    let track = layer.track_mut(prop);
                    track.expression = if has_expr {
                        None
                    } else {
                        // Seed with `value` so enabling fx is value-neutral until
                        // the user edits it (the property keeps its keyframed value).
                        Some("value".to_string())
                    };
                }
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

        // Expression editor: a text field bound to the property's expression,
        // shown only while fx is on. Shows the resolved value and flags a
        // parse/eval error (the render falls back to the keyframed value).
        if let Some(expr) = layer.track_mut(prop).expression.as_mut() {
            let errored = !expr.trim().is_empty() && expr_last_error(expr);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let mut edit = egui::TextEdit::singleline(expr)
                    .id_salt(("expr", idx, prop.label()))
                    .hint_text("value + wiggle(2, 30)")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace);
                if errored {
                    // Tint the field red when the expression failed to evaluate.
                    edit = edit.text_color(egui::Color32::from_rgb(0xE0, 0x5A, 0x5A));
                }
                ui.add(edit);
            });
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if errored {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xE0, 0x5A, 0x5A),
                        "expression error — using keyframed value",
                    );
                } else {
                    ui.weak(format!("= {resolved:.2}{suffix}"));
                }
            });
        }

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

    /// A generic **keyframable-track row** for a non-[`Prop`] track (the
    /// time-remap source-time curve): the same value slider + add-key + `fx`
    /// expression + interpolation UI `property_row` gives transform properties,
    /// but driven by `get`/`get_mut` accessors so it can edit any [`Track`] on the
    /// layer. `id` salts the widgets; `label`/`suffix`/`range` style the slider.
    #[allow(clippy::too_many_arguments)]
    fn track_row(
        &mut self,
        ui: &mut egui::Ui,
        id: (&'static str, usize),
        label: &str,
        suffix: &str,
        range: std::ops::RangeInclusive<f32>,
        t: f32,
        get: impl Fn(&crate::comp::PulseLayer) -> &Track,
        get_mut: impl Fn(&mut crate::comp::PulseLayer) -> &mut Track,
    ) {
        let layer = &mut self.comp.layers[id.1];
        let key_count = get(layer).keys.len();
        let mut value = get(layer).sample(t, 0.0);
        // Expression-resolved value for the live read-out (the track may carry an
        // expression that offsets/drives the keyframed value).
        let has_expr = get(layer).has_expression();
        let resolved = get(layer).sample_expr(
            t,
            0.0,
            ExprCtx {
                time: t,
                value: 0.0,
                fps: self.comp.fps,
                duration: self.comp.duration,
                index: id.1,
            },
        );
        let layer = &mut self.comp.layers[id.1];

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let fx = ui
                    .selectable_label(has_expr, "fx")
                    .on_hover_text("Toggle an expression on this property");
                if fx.clicked() {
                    let track = get_mut(layer);
                    track.expression = if has_expr {
                        None
                    } else {
                        Some("value".to_string())
                    };
                }
                ui.weak(format!("{key_count} {}", icons::KEYFRAME));
            });
        });

        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::Slider::new(&mut value, range)
                    .suffix(suffix.to_owned())
                    .clamping(egui::SliderClamping::Never),
            );
            if resp.changed() {
                get_mut(layer).set_key(t, value);
            }
            if ui
                .button(icons::ADD_KEY)
                .on_hover_text("Add keyframe @ playhead")
                .clicked()
            {
                get_mut(layer).set_key(t, value);
            }
        });

        // Expression editor (shown while fx is on).
        if let Some(expr) = get_mut(layer).expression.as_mut() {
            let errored = !expr.trim().is_empty() && expr_last_error(expr);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let mut edit = egui::TextEdit::singleline(expr)
                    .id_salt(("expr", id.0, id.1))
                    .hint_text("time * 0.5")
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace);
                if errored {
                    edit = edit.text_color(egui::Color32::from_rgb(0xE0, 0x5A, 0x5A));
                }
                ui.add(edit);
            });
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if errored {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xE0, 0x5A, 0x5A),
                        "expression error — using keyframed value",
                    );
                } else {
                    ui.weak(format!("= {resolved:.2}{suffix}"));
                }
            });
        }

        // Interpolation selector — only when the playhead sits on a key.
        if let Some(current) = get(layer).interp_at(t) {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.weak(current.label());
                if let Some(next) = interp_picker(ui, current) {
                    if next != current {
                        get_mut(layer).set_interp(t, next);
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
        Effect::HueSaturation {
            hue,
            saturation,
            lightness,
        } => {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label("Hue");
                ui.add(egui::Slider::new(hue, -180.0..=180.0).suffix("°"));
            });
            slider(ui, "Saturation", saturation, -1.0, 1.0);
            slider(ui, "Lightness", lightness, -1.0, 1.0);
        }
        Effect::Curves { points } => {
            // Five draggable control sliders at inputs 0, ¼, ½, ¾, 1, plus a
            // reset-to-identity button. (A full draggable curve canvas lands
            // with the typed-Property graph-editor rebuild.)
            const LABELS: [&str; 5] = ["0.00", "0.25", "0.50", "0.75", "1.00"];
            for (i, label) in LABELS.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(*label);
                    ui.add(egui::Slider::new(&mut points[i], 0.0..=1.0).text("out"));
                });
            }
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if ui.button("Reset").clicked() {
                    *points = Effect::CURVE_IDENTITY;
                }
            });
        }
        Effect::ColorBalance {
            shadows,
            midtones,
            highlights,
        } => {
            color_balance_range(ui, idx, ei, 0, "Shadows", shadows);
            color_balance_range(ui, idx, ei, 1, "Midtones", midtones);
            color_balance_range(ui, idx, ei, 2, "Highlights", highlights);
        }
    }
}

/// One tonal-range row of [`Effect::ColorBalance`]: a label and three
/// red/green/blue sliders (`-1..=1`). `range` salts the slider ids per range.
fn color_balance_range(
    ui: &mut egui::Ui,
    idx: usize,
    ei: usize,
    range: usize,
    label: &str,
    rgb: &mut [f32; 3],
) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(label).strong());
    });
    for (ch, name) in ["R", "G", "B"].iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            ui.label(*name);
            ui.push_id(("colorbalance", idx, ei, range, ch), |ui| {
                ui.add(egui::Slider::new(&mut rgb[ch], -1.0..=1.0));
            });
        });
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

/// Pop a native file picker filtered to the image formats `prism-io` decodes,
/// returning the chosen path (or `None` if cancelled).
fn footage_pick_dialog(title: &str) -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_title(title)
        .add_filter("Images", prism_io::SUPPORTED_EXTENSIONS)
        .pick_file()
}

/// A collapsible Properties section (Affinity-style Studio panel): a titled
/// [`egui::CollapsingHeader`], open by default, whose body is rendered by `add`.
/// `id` salts the header so per-layer open/closed state doesn't collide.
fn section<R>(
    ui: &mut egui::Ui,
    id: (&str, usize),
    title: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) {
    egui::CollapsingHeader::new(egui::RichText::new(title).heading().size(15.0))
        .id_salt(id)
        .default_open(true)
        .show_unindented(ui, add);
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
