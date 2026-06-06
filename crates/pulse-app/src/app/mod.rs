//! The Pulse application: composition state, the transport (play/scrub), panels,
//! menus, and the per-frame loop tying the motion model to the preview and
//! timeline.

use crate::comp::{Comp, LayerKind, PulseLayer, ShapeItem, ShapePrimitive};
use crate::graph::GraphState;
use crate::{icons, render, theme};

mod layers;
mod menu;
mod panels;
mod properties;

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
            LayerKind::Shape => (format!("Shape {n}"), self.next_color()),
        };
        let mut layer = PulseLayer::of_kind(kind, name, color);
        match kind {
            LayerKind::Adjustment => {
                layer.scale.set_key(0.0, 3.0); // cover the whole comp
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
            _ => {}
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
