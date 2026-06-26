use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::config::GearboxMode;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    ui.columns(2, |cols| {
        // ── Left: Backfire ───────────────────────────────────────────
        {
            let ui = &mut cols[0];
            ui.heading("Backfire");
            ui.label(
                RichText::new("Triggers Backfire by spamming 'W'")
                    .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.backfire_enabled, "Enabled");
            ui.add_space(4.0);

            ui.checkbox(&mut app.config.backfire_dynamic_rpm, "Dynamic RPM");
            if app.config.backfire_dynamic_rpm {
                ui.horizontal(|ui| {
                    ui.label("Min:");
                    ui.add(
                        egui::Slider::new(&mut app.config.backfire_dynamic_min_pct, 0.0..=100.0)
                            .suffix("%")
                            .step_by(1.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("Max:");
                    ui.add(
                        egui::Slider::new(&mut app.config.backfire_dynamic_max_pct, 0.0..=100.0)
                            .suffix("%")
                            .step_by(1.0),
                    );
                });
                ui.label(
                    RichText::new(format!(
                        "Range: {:.0} \u{2013} {:.0} RPM",
                        app.backfire.last_min_rpm, app.backfire.last_max_rpm,
                    ))
                    .size(11.0)
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
                &mut app.config.backfire_disable_standstill,
                "Disable if standing still",
            );
            ui.checkbox(
                &mut app.config.backfire_test_mode,
                "Test mode (ignores throttle/RPM conditions)",
            );
        }

        // ── Right: Automatic Gearbox (DSG) — scrolls within its own column ──
        {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt("gearbox_scroll")
                .show(&mut cols[1], |ui| {
            ui.heading("Automatic Gearbox");
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.dsg_enabled, "Enabled");
            ui.label(
                RichText::new("Drive a full first-gear pull to redline and shift to 2nd manually \
                               to engage — that calibrates 1st and the redline.")
                    .size(11.0)
                    .color(Color32::GRAY),
            );
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Shift RPM:");
                ui.add(
                    egui::Slider::new(&mut app.config.dsg_shift_rpm_pct, 70.0..=100.0)
                        .suffix("%")
                        .step_by(1.0),
                );
            });
            {
                let effective_max = if app.dsg.dbg_effective_max_rpm > 0.0 {
                    app.dsg.dbg_effective_max_rpm
                } else {
                    app.telemetry.latest.as_ref().map(|p| p.engine_max_rpm).unwrap_or(0.0)
                };
                if effective_max > 0.0 {
                    let threshold = effective_max * (app.config.dsg_shift_rpm_pct / 100.0);
                    ui.label(
                        RichText::new(format!("Max RPM ceiling: {:.0} RPM", threshold))
                            .size(11.0)
                            .color(Color32::GRAY),
                    );
                }
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Upshift speed:");
                ui.add(
                    egui::Slider::new(&mut app.config.dsg_upshift_speed_pct, 50.0..=100.0)
                        .suffix("%")
                        .step_by(1.0),
                );
            });
            ui.label(
                RichText::new("Min speed (as % of the gear's top speed) before a redline upshift — \
                               rejects wheelspin spikes.")
                    .size(10.0)
                    .color(Color32::GRAY),
            );

            ui.add_space(8.0);

            // ── Gearbox mode (separate from Shift RPM) ──────────────────
            ui.horizontal(|ui| {
                ui.label("Gearbox mode:");
                egui::ComboBox::from_id_salt("dsg_mode_combo")
                    .selected_text(app.config.dsg_gearbox_mode.label())
                    .show_ui(ui, |ui| {
                        for mode in [GearboxMode::Street, GearboxMode::Sport, GearboxMode::Race] {
                            ui.selectable_value(
                                &mut app.config.dsg_gearbox_mode,
                                mode,
                                mode.label(),
                            );
                        }
                    });
            });
            let mode_hint = match app.config.dsg_gearbox_mode {
                GearboxMode::Street => "Relaxed: upshifts early into tall gears, low cruising revs.",
                GearboxMode::Sport  => "Balanced: holds mid revs while cruising.",
                GearboxMode::Race   => "Aggressive: stays high in the powerband.",
            };
            ui.label(RichText::new(mode_hint).size(11.0).color(Color32::GRAY));

            egui::CollapsingHeader::new("Advanced")
                .id_salt("dsg_advanced")
                .show(ui, |ui| {
                    if app.config.dsg_gearbox_mode == GearboxMode::Race {
                        ui.label(
                            RichText::new("Race holds the full powerband — no cruise target.")
                                .size(10.0)
                                .color(Color32::GRAY),
                        );
                    } else {
                        let tuning = app.config.dsg_active_tuning_mut();
                        ui.horizontal(|ui| {
                            ui.label("Cruise RPM:");
                            ui.add(
                                egui::Slider::new(&mut tuning.cruise_rpm_pct, 20.0..=90.0)
                                    .suffix("%")
                                    .step_by(1.0),
                            );
                        });
                        ui.label(
                            RichText::new("Low-throttle target as % of Max RPM ceiling — sets the \
                                           gear the box settles into while cruising.")
                                .size(10.0)
                                .color(Color32::GRAY),
                        );
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Kickdown cooldown:");
                        ui.add(
                            egui::Slider::new(&mut app.config.dsg_kickdown_cooldown_secs, 0.0..=10.0)
                                .suffix(" s")
                                .step_by(0.5),
                        );
                    });
                    ui.label(
                        RichText::new("After a full-throttle event, hold the lower gear ready for \
                                       this long before easing back up.")
                            .size(10.0)
                            .color(Color32::GRAY),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Downshift deadzone:");
                        ui.add(
                            egui::Slider::new(&mut app.config.dsg_downshift_deadzone_pct, 0.0..=90.0)
                                .suffix("%")
                                .step_by(1.0),
                        );
                    });
                    ui.label(
                        RichText::new("While cruising, hold the gear until revs fall below this % of the shift RPM.")
                            .size(10.0)
                            .color(Color32::GRAY),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Powerband buffer:");
                        ui.add(
                            egui::Slider::new(&mut app.config.dsg_downshift_powerband_buffer_pct, 0.0..=100.0)
                                .suffix("%")
                                .step_by(1.0),
                        );
                    });
                    ui.label(
                        RichText::new("Headroom below redline required to drop into a gear, as % \
                                       of that gear's RPM jump from the current gear. Higher = \
                                       shallower downshifts; 0% = anything up to the redline.")
                            .size(10.0)
                            .color(Color32::GRAY),
                    );
                });

            ui.add_space(8.0);

            // Calibration data — always shown, automatically populated
            let any_data = app.dsg.gear_redline_speeds.iter().skip(1).any(|&s| s > 0.0);
            if any_data {
                ui.label(RichText::new("Calibrated shift points:").strong());
                ui.label(
                    RichText::new("(recorded automatically)")
                        .size(11.0)
                        .color(Color32::GRAY),
                );
                ui.add_space(4.0);
                let pct = app.config.dsg_shift_rpm_pct;
                egui::Grid::new("dsg_shift_grid")
                    .num_columns(2)
                    .spacing([24.0, 4.0])
                    .show(ui, |ui| {
                        for (i, &redline_kmh) in
                            app.dsg.gear_redline_speeds.iter().enumerate().skip(1)
                        {
                            if redline_kmh > 0.0 {
                                let shift_kmh = redline_kmh * (pct / 100.0);
                                ui.label(format!("Gear {}:", i));
                                ui.label(
                                    RichText::new(format!("{shift_kmh:.0} km/h"))
                                        .color(Color32::from_rgb(60, 210, 100)),
                                );
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(4.0);
                if ui.button("Clear calibration").clicked() {
                    // Full wipe — same as a car change: gear data, detected redline and the
                    // engaged flag all go back to zero, so a fresh manual first-gear pull is needed.
                    app.dsg.reset_calibration();
                    app.dsg.reset_state();
                    app.dynamic_max_rpm = 0.0;
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

            ui.add_space(8.0);
            ui.checkbox(&mut app.config.dsg_log_shifts, "Log shifts to CSV");
            if app.config.dsg_log_shifts {
                ui.label(
                    RichText::new(format!(
                        "{}",
                        crate::config::app_data_dir().join("dsg_shift_log.csv").display()
                    ))
                    .size(10.0)
                    .color(Color32::GRAY),
                );
            }

            ui.checkbox(&mut app.config.dsg_debug, "Debug");
            if app.config.dsg_debug {
                let recent_desync = app
                    .dsg
                    .last_desync
                    .map(|t| t.elapsed().as_secs_f32() < 3.0)
                    .unwrap_or(false);
                if recent_desync {
                    ui.label(
                        RichText::new("Gear desync detected!")
                            .strong()
                            .color(Color32::from_rgb(230, 90, 90)),
                    );
                }

                let cur_gear = app
                    .telemetry
                    .latest
                    .as_ref()
                    .map(|p| p.gear as i32)
                    .unwrap_or(0);
                egui::Grid::new("dsg_debug_grid")
                    .num_columns(2)
                    .spacing([16.0, 2.0])
                    .show(ui, |ui| {
                        ui.label("Engaged:");
                        ui.label(if app.dsg.engaged { "yes" } else { "no (rev 1st & shift)" });
                        ui.end_row();

                        ui.label("Current gear:");
                        ui.label(format!("{cur_gear}"));
                        ui.end_row();

                        ui.label("Target gear:");
                        ui.label(format!("{}", app.dsg.dbg_desired_gear));
                        ui.end_row();

                        ui.label("Shifting to:");
                        ui.label(match app.dsg.debug_expected() {
                            Some(g) => format!("{g}"),
                            None => "\u{2014}".to_string(),
                        });
                        ui.end_row();

                        ui.label("Redline:");
                        ui.label(format!("{:.0} RPM", app.dsg.dbg_effective_max_rpm));
                        ui.end_row();

                        ui.label("Upshift @:");
                        ui.label(format!("{:.0} RPM", app.dsg.dbg_shift_threshold));
                        ui.end_row();

                        ui.label("Kickdown cooldown:");
                        {
                            let secs = app.dsg.dbg_kickdown_secs_left;
                            let txt = if secs < 0.0 {
                                "waiting for release".to_string()
                            } else if secs > 0.0 {
                                format!("{:.1}s", secs)
                            } else {
                                "\u{2014}".to_string()
                            };
                            ui.label(txt);
                        }
                        ui.end_row();

                        ui.label("Desyncs:");
                        ui.label(format!("{}", app.dsg.desync_count));
                        ui.end_row();
                    });
            }
        });
        }
    });
}
