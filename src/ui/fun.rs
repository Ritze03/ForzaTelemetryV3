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
                    "Pedal response for {} mode — >1 softens initial throttle, <1 sharpens it. \
                     Curve + gear-selection overlay is on the right.",
                    app.config.dsg_gearbox_mode.label()
                ))
                .size(10.0)
                .color(Color32::GRAY),
            );

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

/// Accelerator gamma curve (X = pedal, Y = output) with a gear-selection overlay: a translucent
/// stepped line shows which gear the box selects at the current speed for each pedal position,
/// each plateau labelled with the gear and meeting the curve at its kickdown point. Two dots track
/// the live pedal (raw on the diagonal, gamma'd on the curve). Sizeable via `size`.
fn viz_gamma_gears(ui: &mut Ui, app: &ForzaApp, size: f32) {
    let size = size.max(60.0);
    let (resp, p) = ui.allocate_painter(egui::vec2(size, size), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, VIZ_BG);
    let at = |px: f32, py: f32| {
        egui::pos2(
            r.left() + px.clamp(0.0, 1.0) * r.width(),
            r.bottom() - py.clamp(0.0, 1.0) * r.height(),
        )
    };
    p.line_segment([r.left_bottom(), r.right_top()], egui::Stroke::new(1.0, Color32::from_gray(55)));

    let pkt = app.telemetry.latest.as_ref();
    let accel = pkt.map(|q| q.accel as f32 / 255.0).unwrap_or(0.0);
    let speed = pkt.map(|q| q.speed_kmh()).unwrap_or(0.0);
    let in_race = pkt.map(|q| q.race_position != 0).unwrap_or(false);
    let mode = app.config.dsg_effective_mode(in_race);
    let is_race = mode == GearboxMode::Race;
    let gamma = app.config.dsg_effective_tuning(in_race).accel_gamma.max(0.05);
    let redlines = app.dsg.gear_redline_speeds;
    let max_rpm = app.dsg.dbg_effective_max_rpm.max(1.0);
    let shift = app.dsg.dbg_shift_threshold.max(1.0);
    let cruise = if is_race { 1.0 } else { app.config.dsg_effective_tuning(in_race).cruise_rpm_pct / 100.0 };
    let deadzone = app.config.dsg_downshift_deadzone_pct / 100.0;
    let full_thr = (app.config.dsg_full_throttle_pct / 100.0).clamp(0.05, 1.0);
    let maxg = (1..=10).rev().find(|&g| redlines[g as usize] > 0.0).unwrap_or(0);

    // ── Gear-selection step overlay (translucent, towards the background) ──
    if maxg > 0 && speed > 1.0 {
        let pred = |g: i32| max_rpm * speed / redlines[g as usize];
        // Throttle-demanded target RPM as a function of effective throttle (mirrors the box).
        let target_of = |th: f32| {
            if is_race || th >= full_thr {
                shift
            } else {
                shift * (cruise + (deadzone - cruise).max(0.0) * (th / full_thr))
            }
        };
        // Gear chosen at effective throttle `th`: tallest gear that won't over-rev and still meets
        // the target; if even the lowest available gear lugs, hold that lowest one.
        let select = |th: f32| -> i32 {
            let tg = target_of(th);
            let (mut best, mut lowest) = (0, 0);
            for g in 1..=maxg {
                if redlines[g as usize] > 0.0 && pred(g) <= shift * 1.002 {
                    if lowest == 0 {
                        lowest = g;
                    }
                    if pred(g) >= tg {
                        best = g;
                    }
                }
            }
            if best == 0 {
                lowest
            } else {
                best
            }
        };

        // Build constant-gear runs across the pedal axis.
        let n = 64;
        let mut runs: Vec<(f32, f32, i32)> = Vec::new();
        let mut run_lo = 0.0_f32;
        let mut prev_g = select(0.0);
        for i in 1..=n {
            let x = i as f32 / n as f32;
            let g = select(x.powf(gamma));
            if g != prev_g {
                runs.push((run_lo, x, prev_g));
                run_lo = x;
                prev_g = g;
            }
        }
        runs.push((run_lo, 1.0, prev_g));

        let line_c = Color32::from_rgba_unmultiplied(240, 100, 90, 170);
        let fill_c = Color32::from_rgba_unmultiplied(240, 100, 90, 26);
        let lbl_c = Color32::from_rgba_unmultiplied(245, 140, 130, 230);
        let mut prev_h = 0.0;
        for (i, &(xlo, xhi, g)) in runs.iter().enumerate() {
            let h = xhi.powf(gamma); // plateau meets the curve at its right edge (the kickdown point)
            let xr = at(xhi, 0.0).x;
            p.rect_filled(
                egui::Rect::from_min_max(at(xlo, h), egui::pos2(xr, r.bottom())),
                0.0,
                fill_c,
            );
            p.line_segment([at(xlo, h), at(xhi, h)], egui::Stroke::new(2.0, line_c));
            if i > 0 {
                p.line_segment([at(xlo, prev_h), at(xlo, h)], egui::Stroke::new(2.0, line_c));
            }
            p.text(
                at((xlo + xhi) * 0.5, h * 0.5),
                egui::Align2::CENTER_CENTER,
                g.to_string(),
                egui::FontId::monospace(13.0),
                lbl_c,
            );
            prev_h = h;
        }
    }

    // ── Gamma curve ──
    let curve: Vec<egui::Pos2> = (0..=48).map(|i| { let x = i as f32 / 48.0; at(x, x.powf(gamma)) }).collect();
    p.add(egui::Shape::line(curve, egui::Stroke::new(2.0, VIZ_GREEN)));

    // ── Live pedal dots ──
    let x = accel.clamp(0.0, 1.0);
    p.circle_filled(at(x, x), 3.5, Color32::from_gray(170));
    p.circle_filled(at(x, x.powf(gamma)), 4.0, Color32::from_rgb(255, 210, 60));

    p.rect_stroke(r, 3.0, egui::Stroke::new(1.0, Color32::from_gray(70)), egui::StrokeKind::Inside);
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
    let is_race_mode = mode == GearboxMode::Race;
    let cruise_frac = app.config.dsg_effective_tuning(in_race).cruise_rpm_pct / 100.0;
    // Cruise downshift point as a fraction of the shift point (mirrors dsg::CRUISE_HYSTERESIS = 0.10).
    let down0_frac = (cruise_frac - 0.10).max(0.0);

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
        RichText::new("GEAR MAP \u{2014} each gear's speed range (downshift \u{2192} max)")
            .monospace()
            .size(11.0)
            .color(VIZ_DIM),
    );
    viz_gear_map(ui, &redlines, shift_pct, down0_frac, is_race_mode, kmh, gear, desired);

    // ── Accelerator gamma curve + gear-selection overlay ──
    ui.label(
        RichText::new("ACCEL \u{2192} GEAR \u{2014} gamma curve + selected gear at this speed")
            .monospace()
            .size(11.0)
            .color(VIZ_DIM),
    );
    let gsize = ui.available_width().min(240.0);
    viz_gamma_gears(ui, app, gsize);

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

/// Stacked gear-range chart: one row per calibrated gear, each row's bar spanning that gear's REAL
/// speed range on a shared axis — from its downshift speed up to its max (shift-point) speed. These
/// ranges OVERLAP between neighbours by the shift hysteresis, which the staggered rows make visible.
/// Current gear is highlighted, desired outlined, and the live road speed is a full-height marker.
fn viz_gear_map(
    ui: &mut Ui,
    redlines: &[f32; 11],
    shift_pct: f32,
    down0_frac: f32,
    is_race: bool,
    kmh: f32,
    cur_gear: i32,
    desired: i32,
) {
    let border = egui::Stroke::new(1.0, Color32::from_gray(60));
    let maxg = (1..=10).rev().find(|&g| redlines[g as usize] > 0.0).unwrap_or(0);
    let row_h = 16.0;
    let h = maxg.max(1) as f32 * row_h + 16.0; // + bottom speed-axis label strip
    let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width(), h), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, VIZ_BG);

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

    // Max speed in a gear = its shift-point speed. Downshift speed (range start) = the cruise
    // downshift fraction of that for normal modes; Race tiles at the shift points (no hysteresis).
    let up = |g: i32| redlines[g as usize] * shift_pct / 100.0;
    let lo = |g: i32| -> f32 {
        if g <= 1 {
            0.0
        } else if is_race {
            up(g - 1)
        } else {
            down0_frac * up(g)
        }
    };
    let max_speed = (up(maxg) * 1.12).max(1.0);
    let gutter = 18.0;
    let axis_l = r.left() + gutter;
    let axis_r = r.right() - 4.0;
    let sx = |s: f32| axis_l + (s / max_speed).clamp(0.0, 1.0) * (axis_r - axis_l);
    let area_top = r.top() + 2.0;
    let area_bot = r.bottom() - 14.0;

    for g in 1..=maxg {
        let g_lo = lo(g);
        let g_hi = up(g);
        // Gear 1 at the bottom, higher gears stacked upward.
        let row_bot = area_bot - (g - 1) as f32 * row_h;
        let row_top = row_bot - row_h + 3.0;
        let cy = (row_top + row_bot) * 0.5;
        let x0 = sx(g_lo);
        let x1 = (sx(g_hi)).max(x0 + 3.0); // keep a sliver visible even if collapsed (Race)
        let bar = egui::Rect::from_min_max(egui::pos2(x0, row_top), egui::pos2(x1, row_bot));
        let fillc = if g == cur_gear { VIZ_GREEN } else { Color32::from_rgb(46, 84, 70) };
        p.rect_filled(bar, 2.0, fillc);
        if g == desired && g != cur_gear {
            p.rect_stroke(bar, 2.0, egui::Stroke::new(1.5, VIZ_AMBER), egui::StrokeKind::Inside);
        }
        // Gear number in the left gutter.
        let numcol = if g == cur_gear { VIZ_GREEN } else { Color32::from_gray(205) };
        p.text(
            egui::pos2(r.left() + gutter * 0.5, cy),
            egui::Align2::CENTER_CENTER,
            g.to_string(),
            egui::FontId::monospace(12.0),
            numcol,
        );
        // Downshift speed at the bar's left edge (skip gear 1, which starts at 0).
        if g > 1 {
            p.text(
                egui::pos2(x0 - 3.0, cy),
                egui::Align2::RIGHT_CENTER,
                format!("{g_lo:.0}"),
                egui::FontId::monospace(9.0),
                Color32::from_gray(150),
            );
        }
        // Max (shift-point) speed just past the bar's right edge.
        p.text(
            egui::pos2(x1 + 4.0, cy),
            egui::Align2::LEFT_CENTER,
            format!("{g_hi:.0}"),
            egui::FontId::monospace(9.0),
            VIZ_DIM,
        );
    }

    // Live speed marker spanning all rows.
    let xs = sx(kmh);
    p.line_segment(
        [egui::pos2(xs, area_top), egui::pos2(xs, area_bot)],
        egui::Stroke::new(2.0, Color32::WHITE),
    );
    p.text(
        egui::pos2(xs, r.bottom() - 1.0),
        egui::Align2::CENTER_BOTTOM,
        format!("{kmh:.0} km/h"),
        egui::FontId::monospace(9.0),
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
