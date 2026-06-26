use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::config::GearboxMode;

pub fn show_backfire(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("backfire_scroll")
        .show(ui, |ui| {
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
        });
}

pub fn show_gearbox(ui: &mut Ui, app: &mut ForzaApp) {
    ui.columns(2, |cols| {
    // ── Left half: controls ─────────────────────────────────────────
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("gearbox_scroll")
        .show(&mut cols[0], |ui| {
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

            ui.checkbox(
                &mut app.config.dsg_auto_race_mode,
                "Auto Race mode in races",
            );
            if app.config.dsg_auto_race_mode {
                let in_race = app
                    .telemetry
                    .latest
                    .as_ref()
                    .map(|p| p.race_position != 0)
                    .unwrap_or(false);
                let active = app.config.dsg_effective_mode(in_race);
                ui.label(
                    RichText::new(format!(
                        "Active: {}{}",
                        active.label(),
                        if in_race { " (race detected)" } else { "" }
                    ))
                    .size(11.0)
                    .color(Color32::from_rgb(60, 210, 100)),
                );
            }

            ui.add_space(8.0);
            // ── Accelerator gamma curve (per mode) ──────────────────────
            ui.horizontal(|ui| {
                ui.label("Accelerator gamma:");
                let tuning = app.config.dsg_active_tuning_mut();
                ui.add(egui::Slider::new(&mut tuning.accel_gamma, 0.3..=3.0).step_by(0.05));
            });
            ui.label(
                RichText::new(format!(
                    "Pedal response for {} mode — >1 softens initial throttle, <1 sharpens it.",
                    app.config.dsg_gearbox_mode.label()
                ))
                .size(10.0)
                .color(Color32::GRAY),
            );
            // Sizeable visualization (input → output). Pass any side length to resize.
            let viz_size = ui.available_width().min(200.0);
            let cur_accel = app
                .telemetry
                .latest
                .as_ref()
                .map(|p| p.accel as f32 / 255.0)
                .unwrap_or(0.0);
            gamma_curve_viz(ui, app.config.dsg_active_tuning().accel_gamma, viz_size, cur_accel);

            egui::CollapsingHeader::new("Advanced")
                .id_salt("dsg_advanced")
                .show(ui, |ui| {
                    let is_race = app.config.dsg_gearbox_mode == GearboxMode::Race;
                    if is_race {
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
                        ui.add_enabled(
                            !is_race,
                            egui::Slider::new(&mut app.config.dsg_downshift_deadzone_pct, 0.0..=90.0)
                                .suffix("%")
                                .step_by(1.0),
                        );
                    });
                    ui.label(
                        RichText::new(if is_race {
                            "Unused in Race — it always stays in the powerband."
                        } else {
                            "While cruising, hold the gear until revs fall below this % of the shift RPM."
                        })
                        .size(10.0)
                        .color(Color32::GRAY),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Full throttle threshold:");
                        ui.add_enabled(
                            !is_race,
                            egui::Slider::new(&mut app.config.dsg_full_throttle_pct, 50.0..=100.0)
                                .suffix("%")
                                .step_by(1.0),
                        );
                    });
                    ui.label(
                        RichText::new(if is_race {
                            "Unused in Race — always full powerband."
                        } else {
                            "Below this throttle the box stays economical (revs up to the deadzone); \
                             at/above it the full powerband is used. Does not affect kickdown."
                        })
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
                    ui.horizontal(|ui| {
                        ui.label("Kickdown powerband buffer:");
                        ui.add(
                            egui::Slider::new(&mut app.config.dsg_kickdown_powerband_buffer_pct, 0.0..=100.0)
                                .suffix("%")
                                .step_by(1.0),
                        );
                    });
                    ui.label(
                        RichText::new("Same, but for full-throttle kickdowns. Lower than the \
                                       Powerband buffer = kickdowns drop a gear deeper for power.")
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

    // ── Right half: live visualization ──────────────────────────────
    gearbox_viz(&mut cols[1], app);
    });
}

/// Square input→output plot of the accelerator gamma curve (`y = x^gamma`). Sizeable: pass any
/// side length and the curve fills the box, so the layout can place/resize it freely later.
fn gamma_curve_viz(ui: &mut Ui, gamma: f32, size: f32, input: f32) {
    let size = size.max(40.0);
    let (resp, painter) = ui.allocate_painter(egui::vec2(size, size), egui::Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 3.0, Color32::from_gray(24));
    // Inside so the full border is drawn within the rect (Middle clips the right/bottom edge).
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
    let at = |px: f32, py: f32| {
        egui::pos2(
            rect.left() + px.clamp(0.0, 1.0) * rect.width(),
            rect.bottom() - py.clamp(0.0, 1.0) * rect.height(),
        )
    };
    // Linear reference (faint diagonal).
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        egui::Stroke::new(1.0, Color32::from_gray(55)),
    );
    // The gamma curve itself.
    let g = gamma.max(0.05);
    let pts: Vec<egui::Pos2> = (0..=40)
        .map(|i| {
            let x = i as f32 / 40.0;
            at(x, x.powf(g))
        })
        .collect();
    painter.add(egui::Shape::line(
        pts,
        egui::Stroke::new(2.0, Color32::from_rgb(60, 210, 100)),
    ));
    // Current position: raw pedal on the linear reference, and the gamma'd output on the curve.
    let x = input.clamp(0.0, 1.0);
    painter.circle_filled(at(x, x), 3.5, Color32::from_gray(170));
    painter.circle_filled(at(x, x.powf(g)), 4.0, Color32::from_rgb(255, 210, 60));
}

// ── Live gearbox visualization (right half of the tab) ───────────────────────
const VIZ_BG: Color32 = Color32::from_rgb(16, 18, 22);
const VIZ_TRACK: Color32 = Color32::from_rgb(34, 38, 44);
const VIZ_GREEN: Color32 = Color32::from_rgb(70, 220, 120);
const VIZ_AMBER: Color32 = Color32::from_rgb(255, 200, 70);
const VIZ_RED: Color32 = Color32::from_rgb(240, 90, 80);
const VIZ_CYAN: Color32 = Color32::from_rgb(80, 200, 235);
const VIZ_DIM: Color32 = Color32::from_rgb(120, 128, 138);

/// Right-half live telemetry/decision visualization for the gearbox.
fn gearbox_viz(ui: &mut Ui, app: &ForzaApp) {
    let pkt = app.telemetry.latest.as_ref();
    let rpm = pkt.map(|p| p.current_engine_rpm).unwrap_or(0.0);
    let kmh = pkt.map(|p| p.speed_kmh()).unwrap_or(0.0);
    let gear = pkt.map(|p| p.gear as i32).unwrap_or(0);
    let accel = pkt.map(|p| p.accel as f32 / 255.0).unwrap_or(0.0);
    let brake = pkt.map(|p| p.brake as f32 / 255.0).unwrap_or(0.0);
    let in_race = pkt.map(|p| p.race_position != 0).unwrap_or(false);

    let redline = app.dsg.dbg_effective_max_rpm.max(1.0);
    let shift = app.dsg.dbg_shift_threshold;
    let target = app.dsg.dbg_target_rpm;
    let down = app.dsg.dbg_down_point;
    let desired = app.dsg.dbg_desired_gear;
    let rule = app.dsg.dbg_rule;
    let mode = app.config.dsg_effective_mode(in_race);
    let gamma = app.config.dsg_effective_tuning(in_race).accel_gamma.max(0.05);
    let eff_thr = accel.powf(gamma);
    let cooldown = app.dsg.dbg_kickdown_secs_left;
    let redlines = app.dsg.gear_redline_speeds;
    let shift_pct = app.config.dsg_shift_rpm_pct;

    ui.spacing_mut().item_spacing.y = 7.0;

    // ── State header ──
    ui.horizontal(|ui| {
        let gs = match gear {
            0 => "R".to_string(),
            1..=9 => gear.to_string(),
            _ => "N".to_string(),
        };
        ui.label(RichText::new(gs).monospace().size(44.0).strong().color(VIZ_GREEN));
        ui.add_space(6.0);
        ui.vertical(|ui| {
            let tg = match desired {
                1..=9 => desired.to_string(),
                _ => "\u{2014}".to_string(),
            };
            ui.label(RichText::new(format!("target {tg}")).monospace().size(13.0).color(VIZ_DIM));
            ui.label(
                RichText::new(format!("{}  \u{00B7}  {}", mode.label(), rule))
                    .monospace()
                    .size(12.0)
                    .color(VIZ_CYAN),
            );
            if app.dsg.engaged {
                ui.label(RichText::new("\u{25CF} ENGAGED").monospace().size(11.0).color(VIZ_GREEN));
            } else {
                ui.label(
                    RichText::new("\u{25CB} idle \u{2014} rev 1st & shift")
                        .monospace()
                        .size(11.0)
                        .color(VIZ_DIM),
                );
            }
        });
    });

    // ── RPM bar (current vs down / target / shift point) ──
    viz_rpm_bar(ui, rpm, redline, down, target, shift);

    // ── Gear-range speed map (the key one) ──
    ui.label(
        RichText::new("GEAR MAP \u{2014} speed \u{2192} gear (calibrated shift points)")
            .monospace()
            .size(11.0)
            .color(VIZ_DIM),
    );
    viz_gear_map(ui, &redlines, shift_pct, kmh, gear, desired);

    // ── Inputs (gamma'd throttle over raw ghost, brake) ──
    viz_input_bar(ui, "THR", eff_thr, accel, VIZ_GREEN);
    viz_input_bar(ui, "BRK", brake, brake, VIZ_RED);

    // ── Indicator lamps ──
    ui.horizontal(|ui| {
        let lamp = |ui: &mut Ui, on: bool, label: &str, col: Color32| {
            let c = if on { col } else { Color32::from_gray(60) };
            ui.label(RichText::new(format!("\u{25CF} {label}")).monospace().size(11.0).color(c));
        };
        lamp(ui, app.dsg.dbg_wheelspin, "SPIN", VIZ_RED);
        lamp(ui, cooldown != 0.0, "KICK", VIZ_AMBER);
        if cooldown > 0.0 {
            ui.label(RichText::new(format!("{cooldown:.1}s")).monospace().size(11.0).color(VIZ_AMBER));
        } else if cooldown < 0.0 {
            ui.label(RichText::new("armed").monospace().size(11.0).color(VIZ_AMBER));
        }
        let dsync = app.dsg.last_desync.map(|t| t.elapsed().as_secs_f32() < 1.5).unwrap_or(false);
        lamp(ui, dsync, "DESYNC", VIZ_RED);
    });
}

fn viz_rpm_bar(ui: &mut Ui, rpm: f32, redline: f32, down: f32, target: f32, shift: f32) {
    let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, VIZ_TRACK);
    let frac = (rpm / redline).clamp(0.0, 1.0);
    let col = if rpm >= shift {
        VIZ_RED
    } else if target > 0.0 && rpm >= target {
        VIZ_AMBER
    } else {
        VIZ_GREEN
    };
    let fill = egui::Rect::from_min_max(r.min, egui::pos2(r.left() + frac * r.width(), r.bottom()));
    p.rect_filled(fill, 3.0, col);
    let tick = |x_rpm: f32, c: Color32| {
        if x_rpm <= 0.0 {
            return;
        }
        let x = r.left() + (x_rpm / redline).clamp(0.0, 1.0) * r.width();
        p.line_segment([egui::pos2(x, r.top()), egui::pos2(x, r.bottom())], egui::Stroke::new(1.5, c));
    };
    tick(down, VIZ_CYAN);
    tick(target, VIZ_AMBER);
    tick(shift, VIZ_RED);
    p.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        format!("{rpm:.0} rpm"),
        egui::FontId::monospace(11.0),
        Color32::WHITE,
    );
    p.rect_stroke(r, 3.0, egui::Stroke::new(1.0, Color32::from_gray(60)), egui::StrokeKind::Inside);
}

/// Horizontal speed axis with one band per calibrated gear (band edge = that gear's calibrated
/// shift-point speed). Current gear is highlighted, the desired gear outlined, and the live road
/// speed marked — so you can read which gear the box maps each speed to.
fn viz_gear_map(ui: &mut Ui, redlines: &[f32; 11], shift_pct: f32, kmh: f32, cur_gear: i32, desired: i32) {
    let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, VIZ_BG);
    let border = egui::Stroke::new(1.0, Color32::from_gray(60));

    let maxg = (1..=10).rev().find(|&g| redlines[g as usize] > 0.0).unwrap_or(0);
    if maxg == 0 {
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            "no calibration yet",
            egui::FontId::monospace(12.0),
            VIZ_DIM,
        );
        p.rect_stroke(r, 3.0, border, egui::StrokeKind::Inside);
        return;
    }

    // Each gear's upshift (shift-point) speed = redline_speed * shift_rpm_pct.
    let up = |g: i32| redlines[g as usize] * shift_pct / 100.0;
    let max_speed = (up(maxg) * 1.04).max(1.0);
    let band_top = r.top() + 13.0;
    let band_bot = r.bottom() - 12.0;
    let sx = |speed: f32| r.left() + (speed / max_speed).clamp(0.0, 1.0) * r.width();

    for g in 1..=maxg {
        let lo = if g == 1 { 0.0 } else { up(g - 1) };
        let hi = up(g);
        let band = egui::Rect::from_min_max(egui::pos2(sx(lo), band_top), egui::pos2(sx(hi), band_bot));
        let fillc = if g == cur_gear {
            VIZ_GREEN
        } else if g % 2 == 0 {
            Color32::from_rgb(40, 72, 60)
        } else {
            Color32::from_rgb(30, 56, 48)
        };
        p.rect_filled(band, 2.0, fillc);
        if g == desired && g != cur_gear {
            p.rect_stroke(band, 2.0, egui::Stroke::new(1.5, VIZ_AMBER), egui::StrokeKind::Inside);
        }
        let txt_col = if g == cur_gear { Color32::BLACK } else { Color32::from_gray(205) };
        p.text(
            band.center(),
            egui::Align2::CENTER_CENTER,
            g.to_string(),
            egui::FontId::monospace(13.0),
            txt_col,
        );
        // Calibrated shift-point speed at the band's right edge.
        p.text(
            egui::pos2(sx(hi), r.bottom() - 1.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{hi:.0}"),
            egui::FontId::monospace(9.0),
            VIZ_DIM,
        );
    }

    // Live speed marker.
    let xs = sx(kmh);
    p.line_segment(
        [egui::pos2(xs, band_top - 3.0), egui::pos2(xs, band_bot + 2.0)],
        egui::Stroke::new(2.0, Color32::WHITE),
    );
    p.text(
        egui::pos2(xs, r.top() + 1.0),
        egui::Align2::CENTER_TOP,
        format!("{kmh:.0} km/h"),
        egui::FontId::monospace(10.0),
        Color32::WHITE,
    );
    p.rect_stroke(r, 3.0, border, egui::StrokeKind::Inside);
}

fn viz_input_bar(ui: &mut Ui, label: &str, value: f32, ghost: f32, col: Color32) {
    let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width(), 16.0), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 2.0, VIZ_TRACK);
    let bar = egui::Rect::from_min_max(egui::pos2(r.left() + 36.0, r.top() + 2.0), egui::pos2(r.right() - 2.0, r.bottom() - 2.0));
    let ghost_c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 70);
    let gw = egui::Rect::from_min_max(bar.min, egui::pos2(bar.left() + ghost.clamp(0.0, 1.0) * bar.width(), bar.bottom()));
    p.rect_filled(gw, 2.0, ghost_c);
    let vw = egui::Rect::from_min_max(bar.min, egui::pos2(bar.left() + value.clamp(0.0, 1.0) * bar.width(), bar.bottom()));
    p.rect_filled(vw, 2.0, col);
    p.text(
        egui::pos2(r.left() + 4.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(10.0),
        VIZ_DIM,
    );
}
