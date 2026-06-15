use egui::{Color32, RichText, Stroke, Ui, Vec2, pos2};

use crate::app::{ForzaApp, GForceStats};
use crate::config::GameMode;
use crate::packet::ForzaPacket;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let Some(pkt) = app.telemetry.latest.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(
                    "Waiting for telemetry…\n\nEnable Data Out in Forza:\nSETTINGS → HUD AND GAMEPLAY → Data Out",
                )
                .size(18.0)
                .color(Color32::GRAY),
            );
        });
        return;
    };

    let car_ord = app.last_car_ordinal;
    let car_cfg = app.car_settings.cars.get(&car_ord).cloned().unwrap_or_default();

    // ── Top row: speed | gear | RPM + shift bar ───────────────────
    ui.horizontal(|ui| {
        // Speed
        ui.vertical(|ui| {
            let speed = if app.config.use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };
            let unit = if app.config.use_mph { "mph" } else { "km/h" };
            ui.label(RichText::new(format!("{:>3.0}", speed)).size(64.0).strong());
            ui.label(RichText::new(unit).size(14.0).color(Color32::GRAY));
        });

        ui.separator();

        // Gear
        ui.vertical(|ui| {
            let gear_str = match pkt.gear {
                0 => "R".to_string(),
                1..=9 => pkt.gear.to_string(),
                _ => "N".to_string(),
            };
            ui.label(
                RichText::new(format!("{:>2}", gear_str))
                    .size(64.0)
                    .strong()
                    .color(Color32::YELLOW),
            );
            ui.label(RichText::new("gear").size(14.0).color(Color32::GRAY));
        });

        ui.separator();

        // RPM + shift indicator
        ui.vertical(|ui| {
            let max_rpm = if car_cfg.max_rpm_measured > 0.0 {
                car_cfg.max_rpm_measured
            } else {
                pkt.engine_max_rpm.max(1.0)
            };

            ui.label(
                RichText::new(format!("RPM: {:>5.0}", pkt.current_engine_rpm))
                    .size(28.0)
                    .strong(),
            );
            ui.label(
                RichText::new(format!("max: {:>5.0}", max_rpm))
                    .size(12.0)
                    .color(Color32::GRAY),
            );

            let bar_size = Vec2::new(ui.available_width().min(400.0), 28.0);
            let (rect, _) = ui.allocate_exact_size(bar_size, egui::Sense::hover());
            draw_shift_bar(ui, rect, &pkt, &car_cfg, max_rpm);
        });
    });

    ui.separator();

    // ── Responsive block layout ────────────────────────────────────
    // Blocks: Inputs, Car, Race, Tires, G-Forces — width from config, wrapping.
    const NUM_BLOCKS: usize = 5;
    let block_w = app.config.dashboard_block_width;
    let gap = 8.0_f32;
    let avail = ui.available_width();
    let per_row = ((avail + gap) / (block_w + gap)).max(1.0).floor() as usize;

    for row_start in (0..NUM_BLOCKS).step_by(per_row) {
        let row_end = (row_start + per_row).min(NUM_BLOCKS);
        ui.horizontal(|ui| {
            for block_idx in row_start..row_end {
                ui.vertical(|ui| {
                    ui.set_width(block_w);
                    match block_idx {
                        0 => show_inputs_block(ui, app, &pkt),
                        1 => show_car_block(ui, app, &pkt),
                        2 => show_race_block(ui, app, &pkt),
                        3 => show_tires_block(ui, app, &pkt),
                        4 => show_gforce_block(ui, app, &pkt),
                        _ => {}
                    }
                });
                if block_idx + 1 < row_end {
                    ui.separator();
                }
            }
        });
        if row_end < NUM_BLOCKS {
            ui.add_space(gap);
        }
    }
}

// ── Block renderers ────────────────────────────────────────────────

fn show_inputs_block(ui: &mut Ui, _app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("Inputs");
    ui.add_space(4.0);

    input_bar(ui, "Accel",     pkt.accel,      Color32::from_rgb(60, 200, 90));
    input_bar(ui, "Brake",     pkt.brake,      Color32::from_rgb(220, 60, 60));
    input_bar(ui, "Clutch",    pkt.clutch,     Color32::from_rgb(80, 140, 220));
    input_bar(ui, "HandBrake", pkt.hand_brake, Color32::from_rgb(230, 150, 40));

    ui.add_space(6.0);
    ui.label("Steer");
    draw_steering(ui, pkt.steer);
}

fn show_car_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("Car");
    ui.add_space(4.0);

    let car_ord = app.last_car_ordinal;
    let car_cfg = app.car_settings.cars.get(&car_ord).cloned().unwrap_or_default();
    let name = if car_cfg.name.is_empty() {
        format!("Ordinal #{}", pkt.car_ordinal)
    } else {
        car_cfg.name.clone()
    };

    ui.label(RichText::new(name).size(16.0).strong());
    ui.label(format!("Class {} — PI {}", app.cached_car_class_str, app.cached_car_pi));
    ui.label(format!("{} | {} cyl", app.cached_drivetrain_str, app.cached_num_cylinders));

    ui.add_space(8.0);

    let (boost_cur, boost_max, boost_unit) = if app.config.use_bar {
        (pkt.boost * 0.0689476, app.max_boost_psi * 0.0689476, "bar")
    } else {
        (pkt.boost, app.max_boost_psi, "PSI")
    };

    ui.label(format!(
        "Power:  {:>5.0} PS   (max {:>5.0})",
        pkt.power_ps(), app.max_power_ps
    ));
    ui.label(format!(
        "Torque: {:>5.0} Nm   (max {:>5.0})",
        pkt.torque_nm(), app.max_torque_nm
    ));
    ui.label(format!(
        "Boost:  {:>5.2} {boost_unit}  (max {boost_max:.2})",
        boost_cur
    ));

    if app.config.game_mode == GameMode::ForzaMotorsport7 {
        ui.add_space(4.0);
        ui.label(format!("Fuel:   {:.0}%", pkt.fuel * 100.0));
        ui.add(
            egui::ProgressBar::new(pkt.fuel)
                .fill(Color32::from_rgb(60, 160, 240))
                .desired_width(160.0),
        );
    }
}

fn show_race_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let is_fh6 = app.config.game_mode == GameMode::ForzaHorizon6;

    if is_fh6 && pkt.race_position == 0 {
        ui.heading("Sprint Times");
        ui.add_space(4.0);

        let st = &app.sprint_timer;
        sprint_row(ui, "0 → 100",   st.zero_to_hundred, None);
        sprint_row(ui, "100 → 200", st.hundred_to_two,
            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two]));
        sprint_row(ui, "200 → 300", st.two_to_three,
            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three]));
        sprint_row(ui, "300 → 400", st.three_to_four,
            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three, st.three_to_four]));
        sprint_row(ui, "400 → 500", st.four_to_five,
            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three, st.three_to_four, st.four_to_five]));
    } else {
        ui.heading("Race");
        ui.add_space(4.0);

        ui.label(format!("Position: P{}", pkt.race_position));
        ui.label(format!("Lap:      {}", pkt.lap_number));
        ui.add_space(6.0);
        ui.label(RichText::new(format!("Current   {}", fmt_lap(pkt.current_lap))).size(15.0));
        ui.label(RichText::new(format!("Last      {}", fmt_lap(pkt.last_lap))).size(15.0));
        ui.label(
            RichText::new(format!("Best      {}", fmt_lap(pkt.best_lap)))
                .size(15.0)
                .color(Color32::from_rgb(255, 210, 40)),
        );
        ui.add_space(8.0);
        ui.label(format!("Race time: {}", fmt_lap(pkt.current_race_time)));
        ui.label(format!("Distance:  {:.1} km", pkt.distance_traveled / 1000.0));
    }
}

fn show_tires_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f = app.config.use_fahrenheit;
    let susp = &app.suspension_stats;

    ui.heading("Tires");
    ui.add_space(4.0);

    egui::Grid::new("tire_grid")
        .num_columns(5)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            for lbl in ["FL", "FR", "RL", "RR"] {
                ui.label(RichText::new(lbl).strong());
            }
            ui.end_row();

            ui.label("Temp");
            tire_temp_label(ui, pkt.tire_temp_fl, use_f);
            tire_temp_label(ui, pkt.tire_temp_fr, use_f);
            tire_temp_label(ui, pkt.tire_temp_rl, use_f);
            tire_temp_label(ui, pkt.tire_temp_rr, use_f);
            ui.end_row();

            ui.label("Slip");
            slip_label(ui, pkt.tire_combined_slip_fl);
            slip_label(ui, pkt.tire_combined_slip_fr);
            slip_label(ui, pkt.tire_combined_slip_rl);
            slip_label(ui, pkt.tire_combined_slip_rr);
            ui.end_row();

            ui.label("Susp");
            let travels = [
                pkt.normalized_suspension_travel_fl,
                pkt.normalized_suspension_travel_fr,
                pkt.normalized_suspension_travel_rl,
                pkt.normalized_suspension_travel_rr,
            ];
            for (i, &cur) in travels.iter().enumerate() {
                draw_susp_bar(ui, cur, susp.min[i], susp.max[i], susp.initialized());
            }
            ui.end_row();

            let puddle = |v: i32| if v != 0 { crate::icons::TINT } else { "  " };
            let rumble = |v: i32| if v != 0 { crate::icons::CIRCLE } else { "  " };

            ui.label("Puddle");
            ui.label(puddle(pkt.wheel_in_puddle_fl));
            ui.label(puddle(pkt.wheel_in_puddle_fr));
            ui.label(puddle(pkt.wheel_in_puddle_rl));
            ui.label(puddle(pkt.wheel_in_puddle_rr));
            ui.end_row();

            ui.label("Rumble");
            ui.label(rumble(pkt.wheel_on_rumble_strip_fl));
            ui.label(rumble(pkt.wheel_on_rumble_strip_fr));
            ui.label(rumble(pkt.wheel_on_rumble_strip_rl));
            ui.label(rumble(pkt.wheel_on_rumble_strip_rr));
            ui.end_row();
        });
}

fn show_gforce_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("G-Forces");
    ui.add_space(4.0);

    let lat  = pkt.acceleration_x / 9.81;
    let lon  = pkt.acceleration_z / 9.81;
    let vert = pkt.acceleration_y / 9.81;

    ui.horizontal(|ui| {
        draw_gforce_plot(ui, lat, lon, &app.gforce_stats);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(format!("Lat:  {:+.2} g  (max {:.2})", lat,  app.gforce_stats.max_lateral));
            ui.label(format!("Long: {:+.2} g  (max {:.2})", lon,  app.gforce_stats.max_longitudinal));
            ui.label(format!("Vert: {:+.2} g  (max {:.2})", vert, app.gforce_stats.max_vertical));
            ui.add_space(6.0);
            ui.label(
                RichText::new("Brake + HB at 100% to reset")
                    .size(10.0)
                    .color(Color32::GRAY),
            );
        });
    });
}

// ── Visual widgets ─────────────────────────────────────────────────

fn draw_steering(ui: &mut Ui, steer: i8) {
    let desired = Vec2::new(ui.available_width().min(340.0), 18.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, 4.0, Color32::from_rgb(40, 40, 40));

    let norm = (steer as f32 / 127.0).clamp(-1.0, 1.0);
    let cx = rect.center().x;
    let end_x = cx + norm * (rect.width() / 2.0);

    if norm.abs() > 0.001 {
        let (fill_left, fill_right) = if norm >= 0.0 {
            (cx, end_x)
        } else {
            (end_x, cx)
        };
        let fill = egui::Rect::from_x_y_ranges(fill_left..=fill_right, rect.top()..=rect.bottom());
        let color = if norm >= 0.0 {
            Color32::from_rgb(50, 200, 80)
        } else {
            Color32::from_rgb(80, 120, 220)
        };
        painter.rect_filled(fill, 0.0, color);
    }

    // Center tick
    painter.line_segment(
        [pos2(cx, rect.top()), pos2(cx, rect.bottom())],
        Stroke::new(2.0, Color32::WHITE),
    );
}

fn draw_gforce_plot(ui: &mut Ui, lat: f32, lon: f32, stats: &GForceStats) {
    let size = 110.0_f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let max_g = 3.0_f32;
    let radius = size / 2.0 - 4.0;

    // Background circle
    painter.circle_filled(center, radius, Color32::from_rgb(28, 28, 28));
    painter.circle_stroke(center, radius, Stroke::new(1.0, Color32::from_rgb(80, 80, 80)));

    // Guide rings at 1G and 2G
    for g in [1.0_f32, 2.0] {
        painter.circle_stroke(
            center,
            g / max_g * radius,
            Stroke::new(0.5, Color32::from_rgb(55, 55, 55)),
        );
    }

    // Crosshair lines
    let dim = Color32::from_rgb(55, 55, 55);
    painter.line_segment([pos2(center.x - radius, center.y), pos2(center.x + radius, center.y)], Stroke::new(0.5, dim));
    painter.line_segment([pos2(center.x, center.y - radius), pos2(center.x, center.y + radius)], Stroke::new(0.5, dim));

    // Peak marker (orange ring) — fades after 5s, hidden below 1g magnitude
    let peak_mag = (stats.peak_lateral.powi(2) + stats.peak_longitudinal.powi(2)).sqrt();
    let peak_alpha: u8 = match stats.peak_fade_start {
        None => 255,
        Some(t) => {
            let e = t.elapsed().as_secs_f32();
            if e >= 5.0 { 0 } else { (255.0 * (1.0 - e / 5.0)) as u8 }
        }
    };
    if peak_alpha > 0 && peak_mag >= 1.0 {
        // Negate axes (same as current dot below) then circle-clip
        let (pdx, pdy) = clip_to_circle(
            -(stats.peak_lateral / max_g * radius),
            stats.peak_longitudinal / max_g * radius,
            radius,
        );
        painter.circle_stroke(
            pos2(center.x + pdx, center.y + pdy),
            4.0,
            Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 140, 0, peak_alpha)),
        );
    }

    // Current dot (white) — both axes negated so right-turn → right, brake → down
    let (dx, dy) = clip_to_circle(
        -(lat / max_g * radius),
        lon / max_g * radius,
        radius,
    );
    painter.circle_filled(pos2(center.x + dx, center.y + dy), 4.0, Color32::WHITE);
}

fn clip_to_circle(dx: f32, dy: f32, r: f32) -> (f32, f32) {
    let d = (dx * dx + dy * dy).sqrt();
    if d > r { let s = r / d; (dx * s, dy * s) } else { (dx, dy) }
}

fn draw_susp_bar(ui: &mut Ui, current: f32, min: f32, max: f32, has_data: bool) {
    let w = 16.0_f32;
    let h = 52.0_f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, 2.0, Color32::from_rgb(40, 40, 40));

    // Fill from top (0 = stretched/top, 1 = compressed/bottom fill)
    let cur = current.clamp(0.0, 1.0);
    let fill_h = cur * h;
    let fill = egui::Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - fill_h),
        rect.max,
    );
    let color = if cur < 0.33 {
        Color32::from_rgb(80, 120, 220)
    } else if cur < 0.66 {
        Color32::from_rgb(50, 200, 80)
    } else {
        Color32::from_rgb(230, 140, 40)
    };
    painter.rect_filled(fill, 0.0, color);

    // Min/max ticks
    if has_data {
        let min_y = rect.bottom() - (min.clamp(0.0, 1.0) * h);
        let max_y = rect.bottom() - (max.clamp(0.0, 1.0) * h);
        painter.line_segment(
            [pos2(rect.left(), min_y), pos2(rect.right(), min_y)],
            Stroke::new(1.5, Color32::from_rgb(180, 80, 80)),
        );
        painter.line_segment(
            [pos2(rect.left(), max_y), pos2(rect.right(), max_y)],
            Stroke::new(1.5, Color32::from_rgb(80, 180, 80)),
        );
    }
}

// ── Shift bar ──────────────────────────────────────────────────────

fn draw_shift_bar(
    ui: &mut Ui,
    rect: egui::Rect,
    pkt: &ForzaPacket,
    car: &crate::config::CarSettings,
    max_rpm: f32,
) {
    let painter = ui.painter();
    let cur  = (pkt.current_engine_rpm / max_rpm).clamp(0.0, 1.0);
    let low  = (car.shift_low_rpm() / max_rpm).clamp(0.0, 1.0);
    let high = (car.shift_high_rpm() / max_rpm).clamp(0.0, 1.0);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(40, 40, 40));

    let green_end = low.min(cur);
    if green_end > 0.0 {
        painter.rect_filled(sub_rect(rect, 0.0, green_end), 4.0, Color32::from_rgb(50, 180, 80));
    }
    let yellow_end = high.min(cur);
    if yellow_end > low {
        painter.rect_filled(sub_rect(rect, low, yellow_end), 0.0, Color32::from_rgb(220, 180, 40));
    }
    if cur > high {
        painter.rect_filled(sub_rect(rect, high, cur), 0.0, Color32::from_rgb(220, 50, 50));
    }

    for &pct in &[low, high] {
        let x = rect.left() + rect.width() * pct;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(2.0, Color32::WHITE),
        );
    }

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{:.0} / {:.0}", pkt.current_engine_rpm, max_rpm),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
}

fn sub_rect(r: egui::Rect, start: f32, end: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        pos2(r.left() + r.width() * start, r.top()),
        pos2(r.left() + r.width() * end, r.bottom()),
    )
}

// ── Helpers ────────────────────────────────────────────────────────

fn sprint_row(ui: &mut Ui, label: &str, segment: Option<f32>, cumulative: Option<f32>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label:12}")).size(14.0).strong());
        match segment {
            Some(t) => {
                ui.label(
                    RichText::new(format!("{t:.3}s"))
                        .size(14.0)
                        .color(Color32::from_rgb(60, 210, 100)),
                );
                if let Some(c) = cumulative {
                    ui.label(RichText::new(format!("({c:.3}s)")).size(12.0).color(Color32::GRAY));
                }
            }
            None => { ui.label(RichText::new("--").color(Color32::GRAY)); }
        }
    });
}

fn cumulative_time(splits: &[Option<f32>]) -> Option<f32> {
    splits.iter().copied().collect::<Option<Vec<_>>>().map(|v| v.iter().sum())
}

fn input_bar(ui: &mut Ui, label: &str, val: u8, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(format!("{label:11}"));
        let pct_w = 38.0_f32;
        let bar_w = (ui.available_width() - pct_w).max(40.0);
        ui.add(
            egui::ProgressBar::new(val as f32 / 255.0)
                .fill(color)
                .desired_width(bar_w),
        );
        ui.label(format!("{:.0}%", val as f32 / 255.0 * 100.0));
    });
}

fn tire_temp_label(ui: &mut Ui, temp_f: f32, use_f: bool) {
    let (val, unit) = if use_f {
        (temp_f, "°F")
    } else {
        (ForzaPacket::tire_temp_celsius(temp_f), "°C")
    };
    ui.colored_label(temp_color(val, use_f), format!("{val:.0}{unit}"));
}

fn temp_color(val: f32, is_f: bool) -> Color32 {
    let (cold, warm, hot) = if is_f { (140.0, 200.0, 250.0) } else { (60.0, 93.0, 121.0) };
    if val < cold {
        Color32::from_rgb(100, 140, 220)
    } else if val < warm {
        Color32::from_rgb(60, 200, 90)
    } else if val < hot {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(220, 60, 60)
    }
}

fn slip_label(ui: &mut Ui, slip: f32) {
    let abs = slip.abs();
    let color = if abs >= 1.0 {
        Color32::from_rgb(220, 60, 60)
    } else if abs >= 0.8 {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(60, 200, 90)
    };
    ui.colored_label(color, format!("{slip:.2}"));
}

fn fmt_lap(secs: f32) -> String {
    if secs <= 0.0 {
        return "--:--.---".to_string();
    }
    let m = (secs / 60.0) as u32;
    let s = secs % 60.0;
    format!("{m}:{s:06.3}")
}
