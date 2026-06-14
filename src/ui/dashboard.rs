use egui::{Color32, RichText, Ui, Vec2};

use crate::app::ForzaApp;
use crate::packet::ForzaPacket;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let Some(pkt) = app.telemetry.latest.clone() else {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("Waiting for telemetry…\n\nEnable Data Out in Forza:\nSETTINGS → HUD AND GAMEPLAY → Data Out")
                    .size(18.0)
                    .color(Color32::GRAY),
            );
        });
        return;
    };

    let use_mph = app.config.use_mph;
    let use_f = app.config.use_fahrenheit;
    let car_ord = app.last_car_ordinal;
    let car_cfg = app.car_settings.cars.get(&car_ord).cloned().unwrap_or_default();

    // ── Top row: speed + gear + rpm ────────────────────────────────
    ui.horizontal(|ui| {
        // Speed
        ui.vertical(|ui| {
            let speed = if use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };
            let unit = if use_mph { "mph" } else { "km/h" };
            ui.label(RichText::new(format!("{:.0}", speed)).size(64.0).strong());
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
            ui.label(RichText::new(gear_str).size(64.0).strong().color(Color32::YELLOW));
            ui.label(RichText::new("gear").size(14.0).color(Color32::GRAY));
        });

        ui.separator();

        // RPM + shift indicator
        ui.vertical(|ui| {
            ui.label(
                RichText::new(format!("{:.0} rpm", pkt.current_engine_rpm))
                    .size(28.0)
                    .strong(),
            );
            ui.label(
                RichText::new(format!("max {:.0}", pkt.engine_max_rpm))
                    .size(12.0)
                    .color(Color32::GRAY),
            );

            // Shift indicator bar
            let bar_size = Vec2::new(ui.available_width().min(400.0), 28.0);
            let (rect, _) = ui.allocate_exact_size(bar_size, egui::Sense::hover());
            draw_shift_bar(ui, rect, &pkt, &car_cfg);
        });
    });

    ui.separator();

    // ── Middle: inputs | car info | lap times ──────────────────────
    ui.columns(3, |cols| {
        // Column 0: Inputs
        {
            let ui = &mut cols[0];
            ui.heading("Inputs");
            ui.add_space(4.0);

            input_bar(ui, "Accel", pkt.accel, Color32::from_rgb(60, 200, 90));
            input_bar(ui, "Brake", pkt.brake, Color32::from_rgb(220, 60, 60));
            input_bar(ui, "Clutch", pkt.clutch, Color32::from_rgb(80, 140, 220));
            input_bar(ui, "HandBrake", pkt.hand_brake, Color32::from_rgb(230, 150, 40));

            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "Steer: {}{}",
                    if pkt.steer < 0 { "◀ " } else { "▶ " },
                    pkt.steer.unsigned_abs()
                ))
                .size(14.0),
            );

            ui.add_space(8.0);
            ui.heading("G-Forces");
            ui.label(format!("Lateral:   {:.2} g", pkt.acceleration_x / 9.81));
            ui.label(format!("Long:      {:.2} g", pkt.acceleration_z / 9.81));
            ui.label(format!("Vertical:  {:.2} g", pkt.acceleration_y / 9.81));
        }

        // Column 1: Car info + fuel + boost
        {
            let ui = &mut cols[1];
            ui.heading("Car");
            ui.add_space(4.0);

            let name = if car_cfg.name.is_empty() {
                format!("Ordinal #{}", pkt.car_ordinal)
            } else {
                car_cfg.name.clone()
            };
            ui.label(RichText::new(name).size(16.0).strong());
            ui.label(format!("Class {} — PI {}", pkt.car_class_str(), pkt.car_performance_index));
            ui.label(format!(
                "{} | {} cyl",
                pkt.drivetrain_str(),
                pkt.num_cylinders
            ));

            ui.add_space(8.0);
            ui.label(format!("Power:   {:.0} PS", pkt.power_ps()));
            ui.label(format!("Torque:  {:.0} Nm", pkt.torque_nm()));
            ui.label(format!("Boost:   {:.1} PSI", pkt.boost));
            ui.label(format!("Fuel:    {:.0}%", pkt.fuel * 100.0));

            // Fuel bar
            ui.add(
                egui::ProgressBar::new(pkt.fuel)
                    .fill(Color32::from_rgb(60, 160, 240))
                    .desired_width(160.0),
            );
        }

        // Column 2: Lap times
        {
            let ui = &mut cols[2];
            ui.heading("Race");
            ui.add_space(4.0);

            ui.label(format!("Position: P{}", pkt.race_position));
            ui.label(format!("Lap:      {}", pkt.lap_number));
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Current   {}", fmt_lap(pkt.current_lap)))
                    .size(15.0),
            );
            ui.label(
                RichText::new(format!("Last      {}", fmt_lap(pkt.last_lap)))
                    .size(15.0),
            );
            ui.label(
                RichText::new(format!("Best      {}", fmt_lap(pkt.best_lap)))
                    .size(15.0)
                    .color(Color32::from_rgb(255, 210, 40)),
            );

            ui.add_space(8.0);
            ui.label(format!("Race time: {}", fmt_lap(pkt.current_race_time)));
            ui.label(format!("Distance:  {:.1} km", pkt.distance_traveled / 1000.0));
        }
    });

    ui.separator();

    // ── Bottom: Tires ──────────────────────────────────────────────
    ui.heading("Tires");
    ui.add_space(4.0);

    egui::Grid::new("tire_grid")
        .num_columns(5)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new("FL").strong());
            ui.label(RichText::new("FR").strong());
            ui.label(RichText::new("RL").strong());
            ui.label(RichText::new("RR").strong());
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
            ui.label(format!("{:.0}%", pkt.normalized_suspension_travel_fl * 100.0));
            ui.label(format!("{:.0}%", pkt.normalized_suspension_travel_fr * 100.0));
            ui.label(format!("{:.0}%", pkt.normalized_suspension_travel_rl * 100.0));
            ui.label(format!("{:.0}%", pkt.normalized_suspension_travel_rr * 100.0));
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

// ── Helpers ────────────────────────────────────────────────────────

fn draw_shift_bar(
    ui: &mut Ui,
    rect: egui::Rect,
    pkt: &ForzaPacket,
    car: &crate::config::CarSettings,
) {
    let painter = ui.painter();
    let max_rpm = if car.max_rpm_measured > 0.0 {
        car.max_rpm_measured
    } else {
        pkt.engine_max_rpm.max(1.0)
    };
    let cur = (pkt.current_engine_rpm / max_rpm).clamp(0.0, 1.0);
    let low = (car.shift_low_rpm() / max_rpm).clamp(0.0, 1.0);
    let high = (car.shift_high_rpm() / max_rpm).clamp(0.0, 1.0);

    let bg = Color32::from_rgb(40, 40, 40);
    let rounding = 4.0;
    painter.rect_filled(rect, rounding, bg);

    // Green zone
    let green_end = low.min(cur);
    if green_end > 0.0 {
        let r = sub_rect(rect, 0.0, green_end);
        painter.rect_filled(r, rounding, Color32::from_rgb(50, 180, 80));
    }
    // Yellow zone
    let yellow_start = low;
    let yellow_end = high.min(cur);
    if yellow_end > yellow_start {
        let r = sub_rect(rect, yellow_start, yellow_end);
        painter.rect_filled(r, 0.0, Color32::from_rgb(220, 180, 40));
    }
    // Red zone (shift!)
    if cur > high {
        let r = sub_rect(rect, high, cur);
        painter.rect_filled(r, 0.0, Color32::from_rgb(220, 50, 50));
    }

    // Threshold markers
    for &pct in &[low, high] {
        let x = rect.left() + rect.width() * pct;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(2.0, Color32::WHITE),
        );
    }

    // RPM text overlay
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
        egui::pos2(r.left() + r.width() * start, r.top()),
        egui::pos2(r.left() + r.width() * end, r.bottom()),
    )
}

fn input_bar(ui: &mut Ui, label: &str, val: u8, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(format!("{label:9}"));
        ui.add(
            egui::ProgressBar::new(val as f32 / 255.0)
                .fill(color)
                .desired_width(160.0),
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
    // Color: cold=blue, warm=green, hot=red
    let color = temp_color(val, use_f);
    ui.colored_label(color, format!("{val:.0}{unit}"));
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
    let color = if abs < 0.1 {
        Color32::from_rgb(60, 200, 90)
    } else if abs < 0.5 {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(220, 60, 60)
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
