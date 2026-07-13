#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod coop;
mod engines;
mod i18n;
mod iconcache;
mod icons;
mod input;
mod labels;
mod listeners;
mod network;
mod packet;
mod recorder;
mod telemetry;
mod theme;
mod ui;

use app::ForzaApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Forza Telemetry V3")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 660.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Forza Telemetry V3",
        options,
        Box::new(|cc| Ok(Box::new(ForzaApp::new(cc)))),
    )
}
