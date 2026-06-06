//! Pulse — motion-graphics / compositing app, app #3 of the Prism creative
//! suite. v0 scaffold entry point.

// egui 0.34 deprecates several menu/panel aliases mid-cycle; silence the churn.
#![allow(deprecated)]

mod app;
mod comp;
mod graph;
mod icons;
mod preview;
mod render;
mod theme;
mod timeline;

use app::PulseApp;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_title("Pulse"),
        ..Default::default()
    };

    eframe::run_native(
        "Pulse",
        options,
        Box::new(|cc| Ok(Box::new(PulseApp::new(cc)))),
    )
}
