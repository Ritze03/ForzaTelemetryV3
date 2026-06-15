use egui::{Color32, RichText, Ui};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints};

use crate::app::ForzaApp;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    // Controls row
    ui.horizontal(|ui| {
        ui.heading("Power Curve");
        ui.add_space(16.0);

        ui.label("Step:");
        ui.add(
            egui::DragValue::new(&mut app.config.power_curve_step)
                .range(50.0..=500.0)
                .speed(10.0)
                .suffix(" rpm"),
        );

        ui.add_space(8.0);
        if ui.button("Clear").clicked() {
            app.power_capture.clear();
            app.power_plot_auto_bounds = true;
        }

        let pt_count = app.power_capture.power_series.len();
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("{pt_count} points captured"))
                .color(Color32::GRAY),
        );

        if app.telemetry.is_connected {
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!("{} Full-throttle to capture", crate::icons::CIRCLE))
                    .color(Color32::from_rgb(60, 200, 90)),
            );
        }
    });

    ui.add_space(4.0);

    let engine_max_rpm = app.telemetry.latest.as_ref()
        .map(|p| p.engine_max_rpm as f64)
        .unwrap_or(8000.0);

    // Detection ON  → show boost only when positive pressure was actually captured.
    // Detection OFF → always show boost (no filtering).
    let has_boost_data = if app.config.power_curve_forced_induction {
        app.power_capture.boost_series.iter().any(|&[_, v]| v > 0.05)
    } else {
        true
    };

    // Remaining height after the controls row.
    // Each group adds ~30 px overhead (inner margins + label + spacing).
    let avail_h = ui.available_height();
    let group_oh = 30.0_f32;
    let gap = 4.0_f32;

    let (power_h, boost_h) = if has_boost_data {
        let total = (avail_h - 2.0 * group_oh - gap).max(200.0);
        (total * 2.0 / 3.0, total / 3.0)
    } else {
        ((avail_h - group_oh).max(200.0), 0.0)
    };

    // Snapshot the flag NOW (before any plot renders).
    // Plot closures run during .show() — before the click checks below — so they
    // never see a flag that was set in the same frame. By snapshotting here we
    // guarantee both plots use the same value, and any middle-click that happens
    // DURING this frame will set the flag to true and survive to the next frame.
    let apply_auto_bounds = app.power_plot_auto_bounds;

    // ── Power & Torque chart ─────────────────────────────────────
    ui.group(|ui| {
        ui.label(RichText::new("Power & Torque vs RPM").strong());
        let power_pts: PlotPoints =
            PlotPoints::new(app.power_capture.power_series.clone());
        let torque_pts: PlotPoints =
            PlotPoints::new(app.power_capture.torque_series.clone());

        let power_resp = Plot::new("power_plot")
            .legend(Legend::default().position(egui_plot::Corner::RightBottom))
            .height(power_h)
            .x_axis_label("RPM")
            .y_axis_label("PS / Nm")
            .include_x(0.0)
            .include_x(engine_max_rpm)
            .show(ui, |plot_ui| {
                if apply_auto_bounds {
                    plot_ui.set_auto_bounds([true, true]);
                }
                plot_ui.line(
                    Line::new("Power (PS)", power_pts)
                        .color(Color32::from_rgb(80, 160, 240))
                        .width(2.0),
                );
                plot_ui.line(
                    Line::new("Torque (Nm)", torque_pts)
                        .color(Color32::from_rgb(240, 140, 40))
                        .width(2.0),
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
            ui.label(RichText::new("Boost vs RPM").strong());
            let use_bar = app.config.use_bar;
            let max_boost = app
                .power_capture
                .boost_series
                .iter()
                .map(|&[_, psi]| if use_bar { psi * 0.0689476 } else { psi })
                .fold(f64::NEG_INFINITY, f64::max);
            let min_headroom = if use_bar { 0.25 } else { 3.0 };
            let boost_top = if max_boost.is_finite() {
                max_boost + (max_boost.abs() * 0.15).max(min_headroom)
            } else {
                min_headroom
            };

            let bars: Vec<Bar> = app
                .power_capture
                .boost_series
                .iter()
                .map(|&[rpm, psi]| {
                    let val = if use_bar { psi * 0.0689476 } else { psi };
                    Bar::new(rpm, val)
                        .fill(Color32::from_rgb(180, 80, 220))
                        .width(app.config.power_curve_step as f64 * 0.8)
                })
                .collect();

            let boost_label = if use_bar { "Boost (bar)" } else { "Boost (PSI)" };
            let boost_resp = Plot::new("boost_plot")
                .height(boost_h)
                .x_axis_label("RPM")
                .y_axis_label(boost_label)
                .include_y(boost_top)
                .show(ui, |plot_ui| {
                    if apply_auto_bounds {
                        plot_ui.set_auto_bounds([true, true]);
                    }
                    plot_ui.bar_chart(BarChart::new("Boost", bars));
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
}
