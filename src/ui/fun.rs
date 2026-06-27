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
    // Two columns (controls | live viz) with a fixed spacer between them.
    const GAP: f32 = 12.0;
    ui.spacing_mut().item_spacing.x = GAP; // ui.columns uses item_spacing.x as the inter-column gap
    ui.columns(2, |cols| {
    // ── Left column: controls ───────────────────────────────────────
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("gearbox_scroll")
        .show(&mut cols[0], |ui| {
            ui.spacing_mut().item_spacing.x = 8.0; // normal spacing inside the column
            ui.heading("Automatic Gearbox");
            ui.add_space(6.0);

            let is_race = app.config.dsg_gearbox_mode == GearboxMode::Race;

            // ── General ──────────────────────────────────────────────────
            ui.group(|ui| {
                ui.label(RichText::new("General").strong());
                ui.add_space(2.0);

                hover(
                    ui.checkbox(&mut app.config.dsg_enabled, "Enabled"),
                    "Lets the box send shift inputs. Stays hands-off until you do one full \
                     first-gear pull to redline and shift to 2nd manually — that calibrates 1st \
                     gear and the true redline.",
                    "On to drive automatically; off to shift yourself.",
                );

                let shift_what = {
                    let effective_max = if app.dsg.dbg_effective_max_rpm > 0.0 {
                        app.dsg.dbg_effective_max_rpm
                    } else {
                        app.telemetry.latest.as_ref().map(|p| p.engine_max_rpm).unwrap_or(0.0)
                    };
                    let base = "Upshift point as % of the detected redline — also the reference \
                                every gear's shift speed scales to.";
                    if effective_max > 0.0 {
                        format!(
                            "{base} Right now that's ≈ {:.0} RPM.",
                            effective_max * app.config.dsg_shift_rpm_pct / 100.0
                        )
                    } else {
                        base.to_string()
                    }
                };
                slider_row(
                    ui,
                    "Shift RPM",
                    &shift_what,
                    "Lower to short-shift (earlier, calmer); raise toward 100% to wring out each gear.",
                    &mut app.config.dsg_shift_rpm_pct,
                    70.0..=100.0,
                    1.0,
                    1,
                    "%",
                );

                slider_row(
                    ui,
                    "Upshift min. speed",
                    "A redline upshift only fires once road speed reaches this % of the gear's \
                     calibrated top speed — blocks false upshifts from wheelspin rev spikes. \
                     Doesn't gate cruise upshifts.",
                    "Raise if it upshifts during wheelspin; otherwise leave it.",
                    &mut app.config.dsg_upshift_speed_pct,
                    50.0..=100.0,
                    1.0,
                    1,
                    "%",
                );

                ui.add_space(2.0);
                setting_row(
                    ui,
                    "Gearbox mode",
                    "Shift personality. Street/Sport cruise economically (upshift early, lazy \
                     downshifts); Race holds the full powerband and ignores the cruise/deadzone \
                     settings.",
                    "Street = relaxed, Sport = balanced, Race = aggressive/track.",
                    |ui| {
                        egui::ComboBox::from_id_salt("dsg_mode_combo")
                            .selected_text(app.config.dsg_gearbox_mode.label())
                            .width(ui.available_width())
                            .show_ui(ui, |ui| {
                                for mode in [GearboxMode::Street, GearboxMode::Sport, GearboxMode::Race] {
                                    ui.selectable_value(
                                        &mut app.config.dsg_gearbox_mode,
                                        mode,
                                        mode.label(),
                                    );
                                }
                            });
                    },
                );
                hover(
                    ui.checkbox(&mut app.config.dsg_auto_race_mode, "Auto Race mode in races"),
                    "Forces Race mode whenever you're in an actual race (position P1+), and \
                     reverts to your chosen mode in free-roam.",
                    "Off to keep your selected mode everywhere.",
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

                // Calibration is shown live in the gear map on the right — just the reset here.
                if app.dsg.gear_redline_speeds.iter().skip(1).any(|&s| s > 0.0) {
                    ui.add_space(4.0);
                    if ui.button("Clear calibration").clicked() {
                        // Full wipe — same as a car change: gear data, detected redline and the
                        // engaged flag go back to zero, so a fresh manual first-gear pull is needed.
                        app.dsg.reset_calibration();
                        app.dsg.reset_state();
                        app.dynamic_max_rpm = 0.0;
                    }
                }
            });

            ui.add_space(6.0);

            // ── Advanced Settings ────────────────────────────────────────
            ui.group(|ui| {
                ui.label(RichText::new("Advanced Settings").strong());
                ui.add_space(2.0);

                slider_row(
                    ui,
                    "Accelerator gamma",
                    "Reshapes the pedal the box reacts to (effective = pedal^gamma). >1 softens the \
                     first part of the pedal (real-car feel), <1 sharpens it; the ends are \
                     unchanged. Set per gearbox mode.",
                    ">1 if it kicks down too eagerly on light throttle; <1 for a sharper response.",
                    &mut app.config.dsg_active_tuning_mut().accel_gamma,
                    0.3..=3.0,
                    0.05,
                    2,
                    "",
                );

                if is_race {
                    ui.label(
                        RichText::new("Race holds the full powerband — no cruise target.")
                            .size(10.0)
                            .color(Color32::GRAY),
                    );
                } else {
                    slider_row(
                        ui,
                        "Cruise RPM",
                        "The rev level the box settles at under light throttle, as % of the shift \
                         point; it upshifts early to keep revs near here while cruising.",
                        "Lower = taller gears / lower revs (economical); higher = holds lower gears \
                         (sportier cruise).",
                        &mut app.config.dsg_active_tuning_mut().cruise_rpm_pct,
                        20.0..=90.0,
                        1.0,
                        1,
                        "%",
                    );
                    slider_row(
                        ui,
                        "Kickdown cooldown",
                        "After a full-throttle burst, holds the lower gear (no early cruise \
                         upshift) for this long once you lift off, so easing off mid-corner doesn't \
                         instantly upshift.",
                        "Longer to stay ready in the low gear after lifting; 0 to upshift as soon \
                         as you ease off.",
                        &mut app.config.dsg_kickdown_cooldown_secs,
                        0.0..=10.0,
                        0.5,
                        1,
                        " s",
                    );
                    slider_row(
                        ui,
                        "Downshift deadzone",
                        "The highest the part-throttle rev target climbs to (% of the shift point) \
                         as you press toward full throttle — the box keeps revs near it and drops a \
                         gear when they fall below.",
                        "Higher = revvier part-throttle, downshifts sooner; lower = lazier, holds \
                         taller gears.",
                        &mut app.config.dsg_downshift_deadzone_pct,
                        0.0..=90.0,
                        1.0,
                        1,
                        "%",
                    );
                    slider_row(
                        ui,
                        "Full throttle threshold",
                        "The throttle % where the box switches from economical (revs up to the \
                         deadzone) to the full powerband (drops gears for power); below it stays \
                         economical.",
                        "Lower so full power needs less pedal; higher to stay economical until \
                         nearly flat out.",
                        &mut app.config.dsg_full_throttle_pct,
                        50.0..=100.0,
                        1.0,
                        1,
                        "%",
                    );
                }
                slider_row(
                    ui,
                    "Powerband buffer",
                    "Headroom below the shift point a downshift must leave, as % of that gear's rev \
                     jump — stops it dropping into a gear that lands near the limiter or hopping \
                     gears. 0% = drop right up to the shift point.",
                    "Higher = shallower, gentler downshifts; lower = deeper, more aggressive.",
                    &mut app.config.dsg_downshift_powerband_buffer_pct,
                    0.0..=100.0,
                    1.0,
                    1,
                    "%",
                );
                if !is_race {
                    slider_row(
                        ui,
                        "Kickdown powerband buffer",
                        "Same as Powerband buffer but for full-throttle kickdowns — usually lower \
                         so a kickdown grabs a gear deeper for power (unused in Race).",
                        "Lower for deeper kickdowns; raise if they land too high / over-rev.",
                        &mut app.config.dsg_kickdown_powerband_buffer_pct,
                        0.0..=100.0,
                        1.0,
                        1,
                        "%",
                    );
                }
            });

            // ── Debug (only when enabled from the status-bar cog) ────────
            if app.config.dsg_show_debug_panel {
                ui.add_space(6.0);
                ui.group(|ui| {
                    hover(
                        ui.checkbox(&mut app.config.dsg_debug, "Debug"),
                        "Shows the live decision state (current/target gear, redline, active rule, \
                         cooldown, desyncs) and reveals the shift-log toggle.",
                        "For tuning or diagnosing shifts.",
                    );
                    if app.config.dsg_debug {
                        hover(
                            ui.checkbox(&mut app.config.dsg_log_shifts, "Log shifts to CSV"),
                            "Appends every shift (pre/post RPM + speed, throttle, brake) to a CSV \
                             for offline analysis; cleared on each launch.",
                            "On to capture a session.",
                        );
                        if app.config.dsg_log_shifts {
                            ui.label(
                                RichText::new(
                                    crate::config::app_data_dir()
                                        .join("dsg_shift_log.csv")
                                        .display()
                                        .to_string(),
                                )
                                .size(10.0)
                                .color(Color32::GRAY),
                            );
                        }
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
                                        format!("{secs:.1}s")
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
    // ── Right column: live visualization ────────────────────────────
    gearbox_viz(&mut cols[1], app);
    });
}

/// A settings row: label (with a What/When hover tooltip) in the left half, control filling the
/// right half. For a combobox; sliders use `slider_row`.
fn setting_row(ui: &mut Ui, label: &str, what: &str, when: &str, add: impl FnOnce(&mut Ui)) {
    ui.columns(2, |c| {
        hover(c[0].label(label), what, when);
        let w = c[1].available_width();
        c[1].spacing_mut().slider_width = (w - 52.0).max(40.0);
        add(&mut c[1]);
    });
}

/// A slider settings row: label (left half) + a rail that fills the right half, with the value in a
/// fixed-width spinner (~6 chars) on the far right so successive rows can't drift.
#[allow(clippy::too_many_arguments)]
fn slider_row(
    ui: &mut Ui,
    label: &str,
    what: &str,
    when: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f64,
    decimals: usize,
    suffix: &str,
) {
    const VALUE_W: f32 = 60.0; // fits "100.0%" — the widest value
    ui.columns(2, |c| {
        hover(c[0].label(label), what, when);
        c[1].horizontal(|ui| {
            let rail = (ui.available_width() - VALUE_W - ui.spacing().item_spacing.x).max(40.0);
            ui.spacing_mut().slider_width = rail;
            ui.add(egui::Slider::new(&mut *value, range.clone()).step_by(step).show_value(false));
            ui.add_sized(
                [VALUE_W, ui.spacing().interact_size.y],
                egui::DragValue::new(&mut *value)
                    .range(range)
                    .speed(step.max(0.01))
                    .fixed_decimals(decimals)
                    .suffix(suffix),
            );
        });
    });
}

/// Attach a two-part "What it does / When to adjust" tooltip to a widget's response.
fn hover(resp: egui::Response, what: &str, when: &str) -> egui::Response {
    resp.on_hover_ui(|ui| {
        ui.set_max_width(300.0);
        ui.label(RichText::new("What it does").strong());
        ui.label(what);
        ui.add_space(4.0);
        ui.label(RichText::new("When to adjust").strong());
        ui.label(when);
    })
}

/// Accelerator gamma curve (X = pedal, Y = output) with a gear-selection overlay: a translucent
/// stepped line shows which gear the box selects at the current speed for each pedal position,
/// each plateau labelled with the gear and meeting the curve at its kickdown point. Two dots track
/// the live pedal (raw on the diagonal, gamma'd on the curve). Sizeable via `size`.
fn viz_gamma_gears(ui: &mut Ui, app: &ForzaApp, size: f32) {
    let size = size.max(60.0);
    let bg = ui.visuals().extreme_bg_color;
    let (resp, p) = ui.allocate_painter(egui::vec2(size, size), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, bg);
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
    // Skipped in Race: gear choice there doesn't depend on the pedal (it always wants the
    // powerband), so the overlay would just be one flat plateau.
    if maxg > 0 && speed > 1.0 && !is_race {
        let pred = |g: i32| max_rpm * speed / redlines[g as usize];
        // Throttle-demanded target RPM as a function of effective throttle (mirrors the box).
        let target_of = |th: f32| {
            if is_race || th >= full_thr {
                shift
            } else {
                shift * (cruise + (deadzone - cruise).max(0.0) * (th / full_thr))
            }
        };
        // The box downshifts when revs fall below the DOWN POINT (target minus the cruise
        // hysteresis) — so the held gear keys off down_point, not the raw target. Using the target
        // dropped each plateau one gear too low (and never showed the actual cruising/held gear).
        let down_point_of = |th: f32| {
            let tgt = target_of(th);
            if is_race || th >= full_thr {
                tgt
            } else {
                (tgt - 0.10 * shift).max(0.0)
            }
        };
        // Gear held at effective throttle `th`: tallest gear that won't over-rev and stays above the
        // down point; if even the lowest available gear is below it, hold that lowest one.
        let select = |th: f32| -> i32 {
            let dp = down_point_of(th);
            let (mut best, mut lowest) = (0, 0);
            for g in 1..=maxg {
                if redlines[g as usize] > 0.0 && pred(g) <= shift * 1.002 {
                    if lowest == 0 {
                        lowest = g;
                    }
                    if pred(g) >= dp {
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
    ui.spacing_mut().item_spacing.x = 8.0; // undo the inter-column gap inside the viz

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
    // Square, filling the space: limited by the column width or the height remaining after the
    // input bars + lamp row drawn below it (~74 px reserve).
    let avail = ui.available_size();
    let gsize = (avail.x.min(avail.y - 74.0)).max(120.0);
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
    let bg = ui.visuals().extreme_bg_color;
    let maxg = (1..=10).rev().find(|&g| redlines[g as usize] > 0.0).unwrap_or(0);
    let row_h = 16.0;
    let h = maxg.max(1) as f32 * row_h + 16.0; // + bottom speed-axis label strip
    let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width(), h), egui::Sense::hover());
    let r = resp.rect;
    p.rect_filled(r, 3.0, bg);

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
