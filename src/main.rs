#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod coop;
mod engines;
mod focus;
mod hotkeys;
#[allow(dead_code)] // filled in by the overlay plan's I6
mod hud;
mod i18n;
mod iconcache;
mod icons;
mod input;
mod keymap;
mod labels;
mod listeners;
#[allow(dead_code)] // filled in by the overlay plan's I5
mod minimap;
mod network;
#[allow(dead_code)] // most of the API is wired up by the overlay plan's I4/I7
mod overlay;
mod packet;
mod telemetry;
mod theme;
mod ui;

use app::ForzaApp;

fn main() -> eframe::Result<()> {
    // Dev only: FORZA_OVERLAY_TEST=1 [FORZA_OVERLAY_OUTPUT=DP-1]. Held until the window closes.
    #[cfg(target_os = "linux")]
    let _overlay = overlay::spawn_dev_test();

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
