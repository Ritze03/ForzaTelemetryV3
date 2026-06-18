use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    ui.columns(2, |cols| {
        // ── Left: Backfire ───────────────────────────────────────────
        {
            let ui = &mut cols[0];
            ui.heading("Backfire");
            ui.label(
                RichText::new(
                    "Simulates backfire sounds by pressing W when off-throttle in the RPM range.",
                )
                .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.backfire_enabled, "Enabled");
            ui.add_space(4.0);

            ui.checkbox(&mut app.config.backfire_dynamic_rpm, "Dynamic RPM");
            if app.config.backfire_dynamic_rpm {
                ui.label(
                    RichText::new(format!(
                        "Range: {:.0} \u{2013} {:.0} RPM",
                        app.backfire.last_min_rpm, app.backfire.last_max_rpm,
                    ))
                    .size(12.0)
                    .color(Color32::GRAY),
                );
            } else {
                ui.horizontal(|ui| {
                    ui.label("Min RPM:");
                    ui.add(
                        egui::DragValue::new(&mut app.config.backfire_min_rpm)
                            .range(0.0..=20000.0)
                            .speed(50.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Max RPM:");
                    ui.add(
                        egui::DragValue::new(&mut app.config.backfire_max_rpm)
                            .range(0.0..=20000.0)
                            .speed(50.0),
                    );
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("RPM interval:");
                ui.add(
                    egui::DragValue::new(&mut app.config.backfire_interval_rpm)
                        .range(0.0..=2000.0)
                        .speed(10.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label("Key press duration:");
                ui.add(
                    egui::DragValue::new(&mut app.config.backfire_accel_time_ms)
                        .range(1..=50)
                        .speed(1.0)
                        .suffix(" ms"),
                );
            });
            ui.add_space(4.0);
            ui.checkbox(
                &mut app.config.backfire_test_mode,
                "Test mode (ignores throttle/RPM conditions)",
            );
        }

        // ── Right: Automatic Gearbox (DSG) ──────────────────────────
        {
            let ui = &mut cols[1];
            ui.heading("Automatic Gearbox (DSG)");
            ui.label(
                RichText::new(
                    "VW DSG-style automatic shifting via key presses. \
                     Upshifts at the configured RPM threshold, downshifts at low RPM, \
                     kickdown on full throttle.",
                )
                .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.dsg_enabled, "Enabled");
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Shift RPM:");
                ui.add(
                    egui::Slider::new(&mut app.config.dsg_shift_rpm_pct, 70.0..=100.0)
                        .suffix("%")
                        .step_by(1.0),
                );
            });
            let upshift_hint = app.telemetry.latest.as_ref().map(|p| {
                let threshold = p.engine_max_rpm * (app.config.dsg_shift_rpm_pct / 100.0);
                format!("({:.0} RPM at current redline)", threshold)
            });
            if let Some(hint) = upshift_hint {
                ui.label(RichText::new(hint).size(11.0).color(Color32::GRAY));
            }

            ui.add_space(8.0);

            // Calibration data — always shown, automatically populated
            let any_data = app.dsg.gear_max_speeds.iter().skip(1).any(|&s| s > 0.0);
            if any_data {
                ui.label(RichText::new("Calibrated shift points:").strong());
                ui.label(
                    RichText::new("(recorded automatically at redline per gear)")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(4.0);
                egui::Grid::new("dsg_shift_grid")
                    .num_columns(2)
                    .spacing([24.0, 4.0])
                    .show(ui, |ui| {
                        for (i, &max_kmh) in
                            app.dsg.gear_max_speeds.iter().enumerate().skip(1)
                        {
                            if max_kmh > 0.0 {
                                ui.label(format!("Gear {}:", i));
                                ui.label(
                                    RichText::new(format!("{max_kmh:.0} km/h"))
                                        .color(Color32::from_rgb(60, 210, 100)),
                                );
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(4.0);
                if ui.button("Clear calibration").clicked() {
                    app.dsg.gear_max_speeds = [0.0; 11];
                    app.config.dsg_gear_max_speeds = [0.0; 11];
                    app.config.save();
                }
            } else {
                ui.label(
                    RichText::new(
                        "No calibration data yet.\n\
                         Drive through each gear to redline and it will be recorded automatically.",
                    )
                    .color(Color32::GRAY),
                );
            }
        }
    });
}
