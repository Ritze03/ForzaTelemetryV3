use egui::{Color32, RichText, Ui};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints};

use crate::app::ForzaApp;
use crate::i18n::tr;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    // Controls row
    ui.horizontal(|ui| {
        if ui.button(tr("Clear live")).clicked() {
            app.power_capture.clear();
            app.power_plot_auto_bounds = true;
        }

        ui.add_space(8.0);
        let has_live_curve = !app.power_capture.power_series.is_empty();
        if ui
            .add_enabled(has_live_curve, egui::Button::new(tr("Save reference")))
            .clicked()
        {
            app.saved_power_curve = Some(app.power_capture.snapshot());
            app.power_capture.clear();
            app.power_plot_auto_bounds = true;
        }

        ui.add_space(8.0);
        let has_saved = app.saved_power_curve.is_some();
        if ui
            .add_enabled(has_saved, egui::Button::new(tr("Clear reference")))
            .clicked()
        {
            app.saved_power_curve = None;
        }

        if app.telemetry.is_connected {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{} {}", crate::icons::CIRCLE, tr("Full-throttle to capture")))
                    .color(Color32::from_rgb(60, 200, 90)),
            );
        }
    });

    ui.add_space(4.0);

    let engine_max_rpm = if app.cached_engine_max_rpm > 0.0 {
        app.cached_engine_max_rpm
    } else {
        8000.0
    };

    // Detection ON  → show boost only when positive pressure was actually captured.
    // Detection OFF → always show boost (no filtering).
    let saved_curve = app.saved_power_curve.as_ref();

    let has_boost_data = if app.config.power_curve_forced_induction {
        app.power_capture.boost_series.iter().any(|&[_, v]| v > 0.05)
            || saved_curve
                .map(|curve| curve.boost_series.iter().any(|&[_, v]| v > 0.05))
                .unwrap_or(false)
            || (app.config.power_curve_save_fi_state && app.fi_detected)
    } else {
        true
    };

    // Remaining height after the controls row.
    // Each group adds ~30 px overhead (inner margins + label + spacing).
    let avail_h = ui.available_height();
    let group_oh = 30.0_f32;
    let gap = 4.0_f32;

    let (power_h, boost_h) = if has_boost_data {
        let total = avail_h - 2.0 * group_oh - gap;
        (total * 2.0 / 3.0, total / 3.0)
    } else {
        (avail_h - group_oh, 0.0)
    };

    // Snapshot the flag NOW (before any plot renders).
    // Plot closures run during .show() — before the click checks below — so they
    // never see a flag that was set in the same frame. By snapshotting here we
    // guarantee both plots use the same value, and any middle-click that happens
    // DURING this frame will set the flag to true and survive to the next frame.
    let apply_auto_bounds = app.power_plot_auto_bounds;

    // ── Power & Torque chart ─────────────────────────────────────
    ui.group(|ui| {
        ui.label(RichText::new(tr("Power & Torque vs RPM")).strong());
        let saved_power_series = saved_curve.map(|curve| curve.power_series.clone());
        let saved_torque_series = saved_curve.map(|curve| curve.torque_series.clone());
        let power_pts: PlotPoints = PlotPoints::new(app.power_capture.power_series.clone());
        let torque_pts: PlotPoints = PlotPoints::new(app.power_capture.torque_series.clone());

        let power_resp = Plot::new("power_plot")
            .legend(Legend::default().position(egui_plot::Corner::RightBottom).follow_insertion_order(true))
            .height(power_h)
            .x_axis_label(tr("RPM"))
            .y_axis_label("PS / Nm")
            .include_x(0.0)
            .include_x(engine_max_rpm)
            .show(ui, |plot_ui| {
                if apply_auto_bounds {
                    plot_ui.set_auto_bounds([true, true]);
                }
                if let Some(saved_power_series) = saved_power_series.as_ref() {
                    plot_ui.line(
                        Line::new(tr("Saved Power (PS)"), saved_power_series.clone())
                            .color(Color32::from_rgba_unmultiplied(80, 160, 240, 90))
                            .width(1.5),
                    );
                }
                if let Some(saved_torque_series) = saved_torque_series.as_ref() {
                    plot_ui.line(
                        Line::new(tr("Saved Torque (Nm)"), saved_torque_series.clone())
                            .color(Color32::from_rgba_unmultiplied(240, 140, 40, 90))
                            .width(1.5),
                    );
                }
                plot_ui.line(
                    Line::new(tr("Power (PS)"), power_pts)
                        .color(Color32::from_rgb(80, 160, 240))
                        .width(2.5),
                );
                plot_ui.line(
                    Line::new(tr("Torque (Nm)"), torque_pts)
                        .color(Color32::from_rgb(240, 140, 40))
                        .width(2.5),
                );
            });
        if power_resp.response.clicked_by(egui::PointerButton::Middle) {
            app.power_plot_auto_bounds = true;
        }
    });

    // ── Boost bar chart (only when forced induction + positive data) ──
    if has_boost_data {
        ui.add_space(gap);

        ui.group(|ui| {
            ui.label(RichText::new(tr("Boost vs RPM")).strong());
            let use_bar = app.config.use_bar;
            let live_max_boost = app
                .power_capture
                .boost_series
                .iter()
                .map(|&[_, psi]| if use_bar { psi * 0.0689476 } else { psi })
                .fold(0.0_f64, f64::max);
            let saved_max_boost = saved_curve
                .map(|curve| {
                    curve
                        .boost_series
                        .iter()
                        .map(|&[_, psi]| if use_bar { psi * 0.0689476 } else { psi })
                        .fold(0.0_f64, f64::max)
                })
                .unwrap_or(0.0);
            let max_boost = live_max_boost.max(saved_max_boost);
            let min_headroom = if use_bar { 0.25 } else { 3.0 };
            let boost_top = if max_boost.is_finite() {
                max_boost + (max_boost.abs() * 0.15).max(min_headroom)
            } else {
                min_headroom
            };

            let step = app.config.power_curve_step as f64;
            let live_color = Color32::from_rgb(180, 80, 220);
            let saved_bg_color = Color32::from_rgba_unmultiplied(180, 80, 220, 90);
            let saved_fg_color = Color32::from_rgb(0x47, 0x23, 0x55);

            // RPM→value maps for per-bar comparison
            let saved_map: std::collections::HashMap<i32, f64> = saved_curve
                .map(|curve| {
                    curve.boost_series.iter()
                        .map(|&[rpm, psi]| {
                            let val = if use_bar { psi * 0.0689476 } else { psi };
                            (rpm.round() as i32, val)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let live_map: std::collections::HashMap<i32, f64> = app.power_capture.boost_series.iter()
                .map(|&[rpm, psi]| {
                    let val = if use_bar { psi * 0.0689476 } else { psi };
                    (rpm.round() as i32, val)
                })
                .collect();

            // Per-bar: higher value → background, lower value → foreground
            let mut bg_live: Vec<Bar> = Vec::new();
            let mut fg_live: Vec<Bar> = Vec::new();
            let mut bg_saved: Vec<Bar> = Vec::new();
            let mut fg_saved: Vec<Bar> = Vec::new();

            for &[rpm, psi] in &app.power_capture.boost_series {
                let val = if use_bar { psi * 0.0689476 } else { psi };
                let bar = Bar::new(rpm, val).fill(live_color).width(step * 0.8);
                if saved_map.get(&(rpm.round() as i32)).is_some_and(|&sv| val < sv) {
                    fg_live.push(bar);
                } else {
                    bg_live.push(bar);
                }
            }
            if let Some(curve) = saved_curve {
                for &[rpm, psi] in &curve.boost_series {
                    let val = if use_bar { psi * 0.0689476 } else { psi };
                    if live_map.get(&(rpm.round() as i32)).is_some_and(|&lv| val < lv) {
                        fg_saved.push(Bar::new(rpm, val).fill(saved_fg_color).width(step * 0.8));
                    } else {
                        bg_saved.push(Bar::new(rpm, val).fill(saved_bg_color).width(step * 0.8));
                    }
                }
            }

            let boost_label = if use_bar { tr("Boost (bar)") } else { tr("Boost (PSI)") };
            let boost_resp = Plot::new("boost_plot")
                .height(boost_h)
                .x_axis_label(tr("RPM"))
                .y_axis_label(boost_label)
                .include_x(0.0)
                .include_x(engine_max_rpm)
                .include_y(boost_top)
                .show(ui, |plot_ui| {
                    if apply_auto_bounds {
                        plot_ui.set_auto_bounds([true, true]);
                    }
                    // Background (higher) bars drawn first, foreground (lower) bars on top
                    if !bg_live.is_empty() {
                        plot_ui.bar_chart(BarChart::new(tr("Boost"), bg_live));
                    }
                    if !bg_saved.is_empty() {
                        plot_ui.bar_chart(BarChart::new(tr("Saved Boost"), bg_saved));
                    }
                    if !fg_live.is_empty() {
                        plot_ui.bar_chart(BarChart::new(tr("Boost"), fg_live));
                    }
                    if !fg_saved.is_empty() {
                        plot_ui.bar_chart(BarChart::new(tr("Saved Boost"), fg_saved));
                    }
                });
            if boost_resp.response.clicked_by(egui::PointerButton::Middle) {
                app.power_plot_auto_bounds = true;
            }
        });
    }

    // If the flag was true when we entered this frame it has been consumed
    // (set_auto_bounds was called). Clear it — but only if a click during this
    // frame didn't re-set it to true (in which case we leave it for next frame).
    if apply_auto_bounds {
        app.power_plot_auto_bounds = false;
    }

    ui.add_space(6.0);
}
