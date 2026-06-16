#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod engines;
mod icons;
mod listeners;
mod network;
mod packet;
mod telemetry;
mod ui;

use app::ForzaApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Forza Telemetry V3")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Forza Telemetry V3",
        options,
        Box::new(|cc| Ok(Box::new(ForzaApp::new(cc)))),
    )
}
