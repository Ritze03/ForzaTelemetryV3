use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    ui.columns(2, |cols| {
        // ── Left: Backfire ───────────────────────────────────────────
        {
            let ui = &mut cols[0];
            ui.heading("Backfire");
            ui.label(
                RichText::new("Simulates backfire sounds by pressing W when off-throttle in the RPM range.")
                    .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.backfire_enabled, "Enabled");
            ui.add_space(4.0);

            ui.checkbox(&mut app.config.backfire_dynamic_rpm, "Dynamic RPM");
            if app.config.backfire_dynamic_rpm {
                ui.label(
                    RichText::new(format!(
                        "Range: {:.0} – {:.0} RPM",
                        app.backfire.last_min_rpm,
                        app.backfire.last_max_rpm
                    ))
                    .color(Color32::GRAY)
                    .size(12.0),
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
                        .range(10..=500)
                        .speed(5.0)
                        .suffix(" ms"),
                );
            });

            ui.add_space(4.0);
            ui.checkbox(&mut app.config.backfire_test_mode, "Test mode (ignores throttle/RPM)");
        }

        // ── Right: Automatic Gearbox (DSG) ──────────────────────────
        {
            let ui = &mut cols[1];
            ui.heading("Automatic Gearbox (DSG)");
            ui.label(
                RichText::new("VW DSG-style automatic shifting. Upshifts at high RPM, downshifts at low RPM, kickdown on full throttle.")
                    .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.dsg_enabled, "Enabled");
            ui.add_space(4.0);

            let calibration_changed = ui.checkbox(&mut app.config.dsg_calibration_mode, "Calibration mode").changed();
            if calibration_changed && !app.config.dsg_calibration_mode {
                // Sync gear max speeds back to config when calibration is turned off
                app.config.dsg_gear_max_speeds = app.dsg.gear_max_speeds;
                app.config.save();
            }

            ui.label(
                RichText::new("Drive through each gear at full throttle to the redline to calibrate shift points.")
                    .size(11.0)
                    .color(Color32::GRAY),
            );

            ui.add_space(8.0);

            // Shift points table
            let any_calibrated = app.dsg.gear_max_speeds.iter().skip(1).any(|&s| s > 0.0);
            if any_calibrated {
                ui.label(RichText::new("Calibrated shift points:").strong());
                egui::Grid::new("dsg_shift_grid")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for (i, &max_kmh) in app.dsg.gear_max_speeds.iter().enumerate().skip(1) {
                            if max_kmh > 0.0 {
                                ui.label(format!("Gear {i}:"));
                                ui.label(
                                    RichText::new(format!("{max_kmh:.0} km/h"))
                                        .color(Color32::from_rgb(60, 210, 100)),
                                );
                                ui.end_row();
                            }
                        }
                    });
                if ui.button("Clear calibration").clicked() {
                    app.dsg.gear_max_speeds = [0.0; 11];
                    app.config.dsg_gear_max_speeds = [0.0; 11];
                }
            } else {
                ui.label(RichText::new("No calibration data yet.").color(Color32::GRAY));
            }
        }
    });
}
