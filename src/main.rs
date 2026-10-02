#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod coop;
mod engines;
mod focus;
mod gamepad;
mod gamedata;
mod hotkeys;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))] // drawn only by the overlay (Linux, Windows)
mod hud;
mod i18n;
mod iconcache;
mod icons;
mod input;
mod keymap;
mod labels;
mod listeners;
mod minimap;
mod network;
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))] // the overlay runtime is Linux + Windows
mod overlay;
mod packet;
mod telemetry;
mod theme;
mod ui;

use app::ForzaApp;

fn main() -> eframe::Result<()> {
    // Dev only: FORZA_OVERLAY_TEST=1 [FORZA_OVERLAY_OUTPUT=DP-1 | DISPLAY2 on Windows]. Held
    // until the window closes.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
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
