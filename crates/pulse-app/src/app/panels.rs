//! The bottom timeline / graph editor panel (with its property chips) and the
//! central preview panel.

use super::{EditorMode, GizmoDrag, PulseApp};
use crate::comp::Prop;
use crate::gizmo::{self, GizmoGeom, Handle};
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
            // The preview surface is click-and-drag so the transform gizmo can be
            // grabbed directly on the canvas.
            let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
            let avail = painter.clip_rect();
            let ctx = ui.ctx().clone();

            // Render the comp at the playhead through the *real* offline compositor
            // (capped res, cached by fingerprint, persistent footage cache) and
            // draw it as the preview image — footage, precomps, effects, masks,
            // mattes, motion blur, time-remap, and expressions all show real
            // composited pixels.
            let comps = self.project_comps();
            let id = self.comp.id;
            // Serve the frame from the RAM-preview cache (filled by the worker pool
            // off the UI thread); pass the comp's fps + duration so it knows the
            // work area to cache for real-time loop playback.
            let tex =
                self.preview
                    .texture(&ctx, comps, id, self.time, self.comp.fps, self.comp.duration);

            let (center, scale) = if let Some(tex) = &tex {
                preview::paint_image(&painter, avail, &self.comp, tex)
            } else {
                // No texture (degenerate comp): fall back to the fitted backdrop so
                // overlays still have a consistent mapping.
                preview::comp_fit(avail, self.comp.width, self.comp.height)
            };

            // Overlays must align to the frame actually on screen. The render runs
            // off the UI thread, so during playback the shown frame lags the live
            // playhead — draw the ghosts / outlines / gizmo at the shown frame's
            // time (falling back to the live time before the first frame arrives)
            // so they don't lead the pixels.
            let display_t = self.preview.shown_time().unwrap_or(self.time);

            // Onion-skin ghosts (timing aid) and editor overlays paint on top of
            // the rendered image, pixel-aligned to it via the same mapping.
            preview::paint_onion(&painter, avail, &self.comp, &self.onion, display_t);
            preview::paint_overlays(
                &painter,
                &self.comp,
                display_t,
                self.selected,
                center,
                scale,
            );
            self.handle_gizmo(ui, &resp, &painter, avail, display_t);
        });
    }

    /// Drive the on-canvas transform gizmo for the selected layer: hit-test the
    /// handles, start/continue/end a drag, key the edited transform at the
    /// playhead, and paint the gizmo (highlighting the hot/held handle).
    fn handle_gizmo(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        avail: egui::Rect,
        // The time the displayed frame was rendered for: the gizmo is built, hit-
        // tested, and keyed at this time so it tracks the pixels on screen (which
        // lag the live playhead during off-thread playback) rather than leading
        // them.
        t: f32,
    ) {
        let Some(idx) = self.selected else { return };
        if idx >= self.comp.layers.len() {
            return;
        }
        let (center, scale) = preview::comp_fit(avail, self.comp.width, self.comp.height);
        let Some(geom) = GizmoGeom::build(&self.comp, idx, t) else {
            return;
        };

        // Pointer → comp space, the space the gizmo geometry lives in.
        let pointer_comp = resp
            .hover_pos()
            .or(resp.interact_pointer_pos())
            .map(|p| gizmo::screen_to_comp(p.x, p.y, center.x, center.y, scale));
        // Hit tolerance in comp px: ~8 screen px back-projected through the fit.
        let tol = 8.0 / scale.max(1e-6);

        // Begin a drag: on press, grab whichever handle is under the pointer.
        if resp.drag_started() {
            if let Some(pc) = pointer_comp {
                if let Some(handle) = gizmo::hit_test(&geom, pc, tol) {
                    self.gizmo_drag = Some(GizmoDrag {
                        layer: idx,
                        handle,
                        time: t,
                        start_tf: self.comp.layers[idx].transform(t),
                        parent: gizmo::parent_matrix(&self.comp, idx, t),
                        start_comp: pc,
                    });
                }
            }
        }

        // Continue an active drag: recompute the result against the live pointer
        // and key the changed properties at the grab time.
        if let Some(drag) = self.gizmo_drag {
            if drag.layer == idx && resp.dragged() {
                if let Some(cur) = resp.interact_pointer_pos() {
                    let cur_comp = gizmo::screen_to_comp(cur.x, cur.y, center.x, center.y, scale);
                    let result = gizmo::drag(
                        drag.handle,
                        drag.start_tf,
                        drag.parent,
                        drag.start_comp,
                        cur_comp,
                    );
                    let layer = &mut self.comp.layers[idx];
                    for (prop, value) in result.keys() {
                        layer.track_mut(prop).set_key(drag.time, value);
                    }
                }
            }
        }
        if resp.drag_stopped() {
            self.gizmo_drag = None;
        }

        // Determine the "hot" handle for the highlight: the held one while
        // dragging, else whatever the pointer hovers.
        let hot = if let Some(drag) = self.gizmo_drag.filter(|d| d.layer == idx) {
            Some(drag.handle)
        } else {
            pointer_comp.and_then(|pc| gizmo::hit_test(&geom, pc, tol))
        };

        // Re-derive the geometry after any edit so the painted gizmo tracks the
        // layer this frame (the transform may have just changed).
        let painted = GizmoGeom::build(&self.comp, idx, t).unwrap_or(geom);
        preview::paint_gizmo(painter, &painted, center, scale, hot);

        // A resize-style cursor hint over an active handle.
        if hot.is_some() {
            let cursor = match hot {
                Some(Handle::Move) => egui::CursorIcon::Move,
                Some(Handle::Rotate) => egui::CursorIcon::Grab,
                Some(Handle::Anchor) => egui::CursorIcon::Crosshair,
                _ => egui::CursorIcon::ResizeNwSe,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
    }
}
