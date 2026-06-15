use egui::{Align, Color32, Layout, Rect, RichText, Stroke, Ui, UiBuilder, Vec2, pos2};

use crate::app::{ForzaApp, GForceStats};
use crate::config::{GameMode, SprintType, TireDisplayStyle, TireSlipStyle};
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

    // ── Top row: speed | gear | RPM + shift bar ───────────────────
    ui.horizontal(|ui| {
        // Both speed and gear use allocate_exact_size so the parent sees identical heights,
        // guaranteeing center-alignment places both blocks at the same Y.
        let block_h = 90.0_f32;
        let legend_color = Color32::from_rgb(140, 140, 140);

        // Speed
        let speed = if app.config.use_mph { pkt.speed_mph() } else { pkt.speed_kmh() };
        let unit_str = if app.config.use_mph { "Mph" } else { "Km/h" };
        let (speed_rect, _) = ui.allocate_exact_size(Vec2::new(140.0, block_h), egui::Sense::hover());
        let p = ui.painter();
        let speed_anchor = pos2(speed_rect.center().x, speed_rect.top() + 2.0);
        let speed_font   = egui::FontId::proportional(64.0);
        match app.config.speed_align {
            crate::config::TextAlign::Right => {
                p.text(speed_anchor, egui::Align2::CENTER_TOP,
                    format!("{:>3.0}", speed), speed_font, Color32::WHITE);
            }
            crate::config::TextAlign::Center => {
                p.text(speed_anchor, egui::Align2::CENTER_TOP,
                    format!("{:.0}", speed), speed_font, Color32::WHITE);
            }
            crate::config::TextAlign::RightPlaceholder => {
                // Compute how many leading zeros the placeholder needs.
                // Gray string: zeros in the prefix positions, spaces elsewhere.
                // White string: spaces in the prefix positions, digits elsewhere.
                // The two strings never share a painted position, so there is no overlap.
                let digits = format!("{:.0}", speed).len().min(3);
                let zeros  = 3 - digits;
                let gray_str  = "0".repeat(zeros) + &" ".repeat(digits);
                let white_str = format!("{:>3.0}", speed);
                p.text(speed_anchor, egui::Align2::CENTER_TOP, gray_str,  speed_font.clone(), Color32::from_rgb(70, 70, 70));
                p.text(speed_anchor, egui::Align2::CENTER_TOP, white_str, speed_font,         Color32::WHITE);
            }
        }
        p.text(
            pos2(speed_rect.left() + 4.0, speed_rect.bottom()),
            egui::Align2::LEFT_BOTTOM,
            unit_str,
            egui::FontId::proportional(12.0),
            legend_color,
        );
        if app.config.show_speed_delta {
            let sign = if app.speed_delta_kmh >= 0.0 { "+" } else { "" };
            p.text(
                pos2(speed_rect.right() - 4.0, speed_rect.bottom()),
                egui::Align2::RIGHT_BOTTOM,
                format!("{sign}{:.1}", app.speed_delta_kmh),
                egui::FontId::proportional(11.0),
                legend_color,
            );
        }

        ui.separator();

        // Gear
        let gear_str = match pkt.gear {
            0 => "R".to_string(),
            1..=9 => pkt.gear.to_string(),
            _ => "N".to_string(),
        };
        let (gear_rect, _) = ui.allocate_exact_size(Vec2::new(100.0, block_h), egui::Sense::hover());
        let p = ui.painter();
        let gear_fmt = match app.config.gear_align {
            crate::config::TextAlign::Right | crate::config::TextAlign::RightPlaceholder => format!("{:>2}", gear_str),
            crate::config::TextAlign::Center => gear_str,
        };
        p.text(
            pos2(gear_rect.center().x, gear_rect.top() + 2.0),
            egui::Align2::CENTER_TOP,
            gear_fmt,
            egui::FontId::proportional(64.0),
            Color32::YELLOW,
        );
        p.text(
            pos2(gear_rect.left() + 4.0, gear_rect.bottom()),
            egui::Align2::LEFT_BOTTOM,
            "Gear",
            egui::FontId::proportional(12.0),
            legend_color,
        );

        ui.separator();

        // RPM + shift indicator
        ui.vertical(|ui| {
            let max_rpm = pkt.engine_max_rpm.max(1.0);

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
            draw_shift_bar(ui, rect, &pkt, app.config.shift_low_pct, app.config.shift_high_pct, max_rpm);
        });
    });

    ui.separator();

    // ── Responsive block layout ────────────────────────────────────
    const NUM_BLOCKS: usize = 6;
    let min_w = app.config.dashboard_block_width;
    let gap = 8.0_f32;
    let avail = ui.available_width();
    let per_row = ((avail + gap) / (min_w + gap)).max(1.0).floor() as usize;
    // Dynamic width: expand modules so they fill the full available width.
    let block_w = if app.config.dynamic_width {
        (avail - (per_row as f32 - 1.0) * gap) / per_row as f32
    } else {
        min_w
    };

    let sep_color = ui.visuals().widgets.noninteractive.bg_stroke.color;

    for row_start in (0..NUM_BLOCKS).step_by(per_row) {
        let row_end = (row_start + per_row).min(NUM_BLOCKS);

        let row = ui.horizontal(|ui| {
            // Zero out automatic inter-widget spacing so block_w = (avail-(N-1)*gap)/N
            // is exact. Spacing inside each pane is restored to normal.
            let orig_spacing = ui.spacing().item_spacing;
            ui.spacing_mut().item_spacing.x = 0.0;

            let mut sep_xs: Vec<f32> = Vec::new();
            for block_idx in row_start..row_end {
                if block_idx > row_start {
                    ui.add_space(gap);
                }

                // True bounded pane: clips painter and bounds available_width().
                let pane_rect = Rect::from_min_size(
                    ui.cursor().min,
                    Vec2::new(block_w, ui.available_height()),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(pane_rect)
                        .layout(Layout::top_down(Align::LEFT)),
                    |ui| {
                        ui.spacing_mut().item_spacing = orig_spacing;
                        ui.set_min_width(block_w);
                        match block_idx {
                            0 => show_inputs_block(ui, app, &pkt),
                            1 => show_car_block(ui, app, &pkt),
                            2 => show_race_block(ui, app, &pkt),
                            3 => show_tires_block(ui, app, &pkt),
                            4 => show_gforce_block(ui, app, &pkt),
                            5 => show_suspension_block(ui, app, &pkt),
                            _ => {}
                        }
                    },
                );

                sep_xs.push(pane_rect.right() + gap / 2.0);
            }
            sep_xs
        });

        // Draw full-height vertical separators using post-layout painter
        let rr = row.response.rect;
        for x in row.inner {
            ui.painter().line_segment(
                [pos2(x, rr.top()), pos2(x, rr.bottom())],
                Stroke::new(1.0, sep_color),
            );
        }

        // Horizontal separator below every row
        ui.separator();
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
        pkt.power_ps().max(0.0), app.max_power_ps
    ));
    ui.label(format!(
        "Torque: {:>5.0} Nm   (max {:>5.0})",
        pkt.torque_nm().max(0.0), app.max_torque_nm
    ));
    ui.label(format!(
        "Boost:  {:5.2} {boost_unit}  (max {:5.2})",
        boost_cur.max(0.0), boost_max.max(0.0)
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
        let stype = &app.config.sprint_type;
        let show_other = app.config.sprint_show_other;

        let c100  = cumulative_time(&[st.zero_to_hundred]);
        let c200  = cumulative_time(&[st.zero_to_hundred, st.hundred_to_two]);
        let c300  = cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three]);
        let c400  = cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three, st.three_to_four]);
        let c500  = cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three, st.three_to_four, st.four_to_five]);

        let (lbl0, lbl1, lbl2, lbl3, lbl4) = match stype {
            SprintType::Incremental => ("0 → 100", "100 → 200", "200 → 300", "300 → 400", "400 → 500"),
            SprintType::Absolute    => ("0 → 100", "0 → 200",   "0 → 300",   "0 → 400",   "0 → 500"),
        };
        sprint_row(ui, lbl0, st.zero_to_hundred, c100,  stype, false);
        sprint_row(ui, lbl1, st.hundred_to_two,  c200,  stype, show_other);
        sprint_row(ui, lbl2, st.two_to_three,    c300,  stype, show_other);
        sprint_row(ui, lbl3, st.three_to_four,   c400,  stype, show_other);
        sprint_row(ui, lbl4, st.four_to_five,    c500,  stype, show_other);
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
    ui.heading("Tires");
    ui.add_space(4.0);
    match app.config.tire_display_style {
        TireDisplayStyle::Separate => show_tires_separate(ui, app, pkt),
        TireDisplayStyle::Combined => show_tires_combined(ui, app, pkt),
    }
}

fn show_tires_separate(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f   = app.config.use_fahrenheit;
    let is_fh6  = app.config.game_mode == GameMode::ForzaHorizon6;
    let slip_style = &app.config.tire_slip_style;

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
            for &slip in &[
                pkt.tire_combined_slip_fl, pkt.tire_combined_slip_fr,
                pkt.tire_combined_slip_rl, pkt.tire_combined_slip_rr,
            ] {
                match slip_style {
                    TireSlipStyle::Values => slip_label(ui, slip),
                    TireSlipStyle::Graph  => draw_slip_circle(ui, slip, false),
                    TireSlipStyle::Both   => draw_slip_circle(ui, slip, true),
                }
            }
            ui.end_row();

            // Water (puddle) row — blue icons
            let water_icon = |v: i32| if v != 0 { crate::icons::TINT } else { "  " };
            ui.label("Water");
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_fl));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_fr));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_rl));
            ui.colored_label(Color32::from_rgb(80, 160, 220), water_icon(pkt.wheel_in_puddle_rr));
            ui.end_row();

            // Rumble row — FM7 only
            if !is_fh6 {
                let rumble = |v: i32| if v != 0 { crate::icons::CIRCLE } else { "  " };
                ui.label("Rumble");
                ui.label(rumble(pkt.wheel_on_rumble_strip_fl));
                ui.label(rumble(pkt.wheel_on_rumble_strip_fr));
                ui.label(rumble(pkt.wheel_on_rumble_strip_rl));
                ui.label(rumble(pkt.wheel_on_rumble_strip_rr));
                ui.end_row();
            }
        });
}

fn show_tires_combined(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let use_f = app.config.use_fahrenheit;

    let n        = 4_f32;
    let gap      = 6.0_f32;
    let left_pad = 3.0_f32;
    let right_pad = 2.0_f32;
    let avail_w  = ui.available_width();  // correctly bounded by the pane's max_rect
    let cell     = (avail_w - left_pad - right_pad - (n - 1.0) * gap) / n;
    let outer_r  = cell / 2.0;
    let inner_r  = outer_r * 0.55;
    let total_w  = left_pad + n * cell + (n - 1.0) * gap;

    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, cell), egui::Sense::hover());
    let hole_bg = ui.visuals().panel_fill;  // actual window background, not the donut dark bg
    let p = ui.painter();

    let tires = [
        ("FL", pkt.tire_temp_fl, pkt.tire_combined_slip_fl, pkt.wheel_in_puddle_fl),
        ("FR", pkt.tire_temp_fr, pkt.tire_combined_slip_fr, pkt.wheel_in_puddle_fr),
        ("RL", pkt.tire_temp_rl, pkt.tire_combined_slip_rl, pkt.wheel_in_puddle_rl),
        ("RR", pkt.tire_temp_rr, pkt.tire_combined_slip_rr, pkt.wheel_in_puddle_rr),
    ];

    let bg        = Color32::from_rgb(20, 20, 20);
    let puddle_c  = Color32::from_rgb(80, 160, 220);
    let rim_c     = Color32::from_rgb(80, 80, 80);
    let line_h    = 12.0_f32;

    for (i, &(label, temp_f, slip, puddle)) in tires.iter().enumerate() {
        let cx = rect.left() + left_pad + i as f32 * (cell + gap) + outer_r;
        let cy = rect.center().y;
        let center = pos2(cx, cy);

        let slip_abs  = slip.abs();
        let grip_color = if slip_abs >= 1.0 {
            Color32::from_rgb(220, 60, 60)
        } else if slip_abs >= 0.8 {
            Color32::from_rgb(230, 160, 40)
        } else {
            Color32::from_rgb(60, 200, 90)
        };

        // 1. Dark background
        p.circle_filled(center, outer_r, bg);

        // 2. Grip ring: fills from inner edge outward proportional to slip
        let fill_r = inner_r + slip_abs.min(1.0) * (outer_r - inner_r);
        p.circle_filled(center, fill_r, grip_color);

        // 3. Punch out the hollow center with the real window background
        p.circle_filled(center, inner_r, hole_bg);

        // 4. Inner outline 1 px inside inner_r to cover grip-fill bleed
        p.circle_stroke(center, inner_r - 1.0, Stroke::new(1.5, rim_c));

        // 5. Outer outline — blue if in puddle
        let outline = if puddle != 0 { puddle_c } else { rim_c };
        p.circle_stroke(center, outer_r, Stroke::new(1.5, outline));

        // 5. Text: label / temp / slip — stacked, centered on cy
        let temp_val = if use_f { temp_f } else { ForzaPacket::tire_temp_celsius(temp_f) };
        let temp_unit = if use_f { "°F" } else { "°C" };
        let temp_str = format!("{:.0}{temp_unit}", temp_val);
        let temp_c = temp_color(temp_val, use_f);
        let slip_str = format!("{:.2}", slip);

        p.text(pos2(cx, cy - line_h), egui::Align2::CENTER_CENTER,
            label,    egui::FontId::proportional(11.0), Color32::WHITE);
        p.text(pos2(cx, cy),          egui::Align2::CENTER_CENTER,
            temp_str, egui::FontId::proportional(10.0), temp_c);
        p.text(pos2(cx, cy + line_h), egui::Align2::CENTER_CENTER,
            slip_str, egui::FontId::proportional(10.0), grip_color);
    }
}

fn show_gforce_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    ui.heading("G-Forces");
    ui.add_space(4.0);

    let lat  = pkt.acceleration_x / 9.81;
    let lon  = pkt.acceleration_z / 9.81;
    let vert = pkt.acceleration_y / 9.81;

    let avail_w   = ui.available_width();
    let left_pad  = 3.0_f32;
    let right_pad = 2.0_f32;
    let gap       = 8.0_f32;

    // Compute the exact pixel height of the text panel from font metrics so the
    // circle never grows taller than the labels sitting next to it.
    // Content: 2 small headers (12px) + 6 body labels + add_space(4) + 8 inter-item gaps.
    let body_h = ui.text_style_height(&egui::TextStyle::Body);
    let sp_y   = ui.spacing().item_spacing.y;
    let text_h = 2.0 * 12.0 + 6.0 * body_h + 8.0 * sp_y + 4.0;

    let plot_size = ((avail_w - left_pad - right_pad) / 2.0 - gap)
        .min(text_h)
        .clamp(40.0, 200.0);

    ui.horizontal(|ui| {
        ui.add_space(left_pad);
        draw_gforce_plot(ui, lat, lon, &app.gforce_stats, plot_size);
        ui.add_space(gap);
        ui.vertical(|ui| {
            ui.label(RichText::new("Current:").size(12.0).color(Color32::GRAY));
            ui.label(format!("  Lat:  {:+.2} g", lat));
            ui.label(format!("  Long: {:+.2} g", lon));
            ui.label(format!("  Vert: {:+.2} g", vert));
            ui.add_space(4.0);
            ui.label(RichText::new("Peak:").size(12.0).color(Color32::GRAY));
            ui.colored_label(Color32::YELLOW, format!("  Lat:  {:.2} g", app.gforce_stats.max_lateral));
            ui.colored_label(Color32::YELLOW, format!("  Long: {:.2} g", app.gforce_stats.max_longitudinal));
            ui.colored_label(Color32::YELLOW, format!("  Vert: {:.2} g", app.gforce_stats.max_vertical));
        });
    });
}

fn show_suspension_block(ui: &mut Ui, app: &ForzaApp, pkt: &ForzaPacket) {
    let susp = &app.suspension_stats;

    ui.heading("Suspension");
    ui.add_space(4.0);

    let travels = [
        pkt.normalized_suspension_travel_fl,
        pkt.normalized_suspension_travel_fr,
        pkt.normalized_suspension_travel_rl,
        pkt.normalized_suspension_travel_rr,
    ];

    egui::Grid::new("susp_grid")
        .num_columns(5)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            for lbl in ["FL", "FR", "RL", "RR"] {
                ui.label(RichText::new(lbl).strong());
            }
            ui.end_row();

            // Bar row
            ui.label("");
            for (i, &cur) in travels.iter().enumerate() {
                draw_susp_bar(ui, cur, susp.min[i], susp.max[i], susp.initialized, 24.0, 80.0);
            }
            ui.end_row();

            // Current row
            ui.label(RichText::new("Cur").size(11.0).color(Color32::GRAY));
            for &cur in &travels {
                ui.label(RichText::new(format!("{:.2}", cur)).size(11.0));
            }
            ui.end_row();

            // 5s min row
            if susp.initialized {
                ui.label(RichText::new("Min").size(11.0).color(Color32::from_rgb(180, 80, 80)));
                for i in 0..4 {
                    ui.label(RichText::new(format!("{:.2}", susp.min[i])).size(11.0)
                        .color(Color32::from_rgb(180, 80, 80)));
                }
                ui.end_row();

                ui.label(RichText::new("Max").size(11.0).color(Color32::from_rgb(80, 180, 80)));
                for i in 0..4 {
                    ui.label(RichText::new(format!("{:.2}", susp.max[i])).size(11.0)
                        .color(Color32::from_rgb(80, 180, 80)));
                }
                ui.end_row();
            }
        });

    ui.add_space(4.0);
    ui.label(RichText::new("Min/Max over last 5 seconds").size(10.0).color(Color32::GRAY));
}

// ── Visual widgets ─────────────────────────────────────────────────

fn draw_steering(ui: &mut Ui, steer: i8) {
    let desired = Vec2::new(ui.available_width(), 18.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, 4.0, Color32::from_rgb(40, 40, 40));

    let norm = (steer as f32 / 127.0).clamp(-1.0, 1.0);
    let cx = rect.center().x;
    let end_x = cx + norm * (rect.width() / 2.0);

    if norm.abs() > 0.001 {
        let (fill_left, fill_right, fill_rounding) = if norm >= 0.0 {
            // Right turn: fill center → right edge; round right corners to match background
            (cx, end_x, egui::CornerRadius { nw: 0, ne: 4, sw: 0, se: 4 })
        } else {
            // Left turn: fill left edge → center; round left corners to match background
            (end_x, cx, egui::CornerRadius { nw: 4, ne: 0, sw: 4, se: 0 })
        };
        let fill = egui::Rect::from_x_y_ranges(fill_left..=fill_right, rect.top()..=rect.bottom());
        painter.rect_filled(fill, fill_rounding, Color32::from_rgb(50, 200, 80));
    }

    // Center tick — blue
    painter.line_segment(
        [pos2(cx, rect.top()), pos2(cx, rect.bottom())],
        Stroke::new(2.0, Color32::from_rgb(80, 120, 220)),
    );
}

fn draw_gforce_plot(ui: &mut Ui, lat: f32, lon: f32, stats: &GForceStats, size: f32) {
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

    // Peak marker — always visible, no fade
    let peak_mag = (stats.peak_lateral.powi(2) + stats.peak_longitudinal.powi(2)).sqrt();
    if peak_mag > 0.01 {
        let (pdx, pdy) = clip_to_circle(
            -(stats.peak_lateral / max_g * radius),
            stats.peak_longitudinal / max_g * radius,
            radius,
        );
        painter.circle_stroke(
            pos2(center.x + pdx, center.y + pdy),
            4.0,
            Stroke::new(1.5, Color32::from_rgb(255, 140, 0)),
        );
    }

    // Current dot
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

fn draw_susp_bar(ui: &mut Ui, current: f32, min: f32, max: f32, has_data: bool, w: f32, h: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, 2.0, Color32::from_rgb(40, 40, 40));

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

fn draw_slip_circle(ui: &mut Ui, slip: f32, show_value: bool) {
    let abs = slip.abs();
    let r = 13.0_f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(r * 2.0 + 2.0), egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();

    painter.circle_filled(center, r, Color32::from_rgb(20, 20, 20));
    painter.circle_stroke(center, r, Stroke::new(1.0, Color32::from_rgb(80, 80, 80)));

    let fill_r = abs.min(1.0) * r;
    let base_color = if abs >= 1.0 {
        let t = ((abs - 1.0) / 1.0).clamp(0.0, 1.0);
        Color32::from_rgb(
            (220.0 - 110.0 * t) as u8,
            (60.0  -  30.0 * t) as u8,
            (60.0  -  30.0 * t) as u8,
        )
    } else if abs >= 0.8 {
        Color32::from_rgb(230, 160, 40)
    } else {
        Color32::from_rgb(60, 200, 90)
    };

    // In "Both" mode, dim the fill so the white value text is readable
    let brightness: f32 = if show_value {
        if abs >= 1.0 { 0.25 } else { 0.5 }
    } else {
        1.0
    };
    let fill_color = Color32::from_rgb(
        (base_color.r() as f32 * brightness) as u8,
        (base_color.g() as f32 * brightness) as u8,
        (base_color.b() as f32 * brightness) as u8,
    );

    if fill_r > 0.5 {
        painter.circle_filled(center, fill_r, fill_color);
    }

    if show_value {
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            format!("{:.2}", slip),
            egui::FontId::proportional(9.0),
            Color32::WHITE,
        );
    }
}

// ── Shift bar ──────────────────────────────────────────────────────

fn draw_shift_bar(
    ui: &mut Ui,
    rect: egui::Rect,
    pkt: &ForzaPacket,
    low_pct: f32,
    high_pct: f32,
    max_rpm: f32,
) {
    let painter = ui.painter();
    let cur  = (pkt.current_engine_rpm / max_rpm).clamp(0.0, 1.0);
    let low  = (low_pct / 100.0).clamp(0.0, 1.0);
    let high = (high_pct / 100.0).clamp(0.0, 1.0);

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

fn sprint_row(
    ui: &mut Ui,
    label: &str,
    segment: Option<f32>,
    cumulative: Option<f32>,
    stype: &SprintType,
    show_other: bool,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{label:12}")).size(14.0).strong());

        let (main, secondary) = match stype {
            SprintType::Incremental => (segment, cumulative),
            SprintType::Absolute    => (cumulative, segment),
        };

        match main {
            Some(t) => {
                ui.label(
                    RichText::new(format!("{t:.3}s"))
                        .size(14.0)
                        .color(Color32::from_rgb(60, 210, 100)),
                );
                if show_other {
                    if let Some(s) = secondary {
                        ui.label(RichText::new(format!("({s:.3}s)")).size(12.0).color(Color32::GRAY));
                    }
                }
            }
            None => {
                ui.label(RichText::new("--").color(Color32::GRAY));
            }
        }
    });
}

fn cumulative_time(splits: &[Option<f32>]) -> Option<f32> {
    splits.iter().copied().collect::<Option<Vec<_>>>().map(|v| v.iter().sum())
}

fn input_bar(ui: &mut Ui, label: &str, val: u8, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(format!("{label:11}"));
        let pct_w = 46.0_f32; // wide enough for "100%"
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
