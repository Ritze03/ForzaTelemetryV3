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
                RichText::new("● Full-throttle to capture")
                    .color(Color32::from_rgb(60, 200, 90)),
            );
        }
    });

    ui.add_space(4.0);

    let available = ui.available_size();
    let chart_h = (available.y - 180.0).max(200.0);

    // ── Power & Torque chart ─────────────────────────────────────
    ui.group(|ui| {
        ui.label(RichText::new("Power & Torque vs RPM").strong());
        let power_pts: PlotPoints =
            PlotPoints::new(app.power_capture.power_series.clone());
        let torque_pts: PlotPoints =
            PlotPoints::new(app.power_capture.torque_series.clone());

        Plot::new("power_plot")
            .legend(Legend::default())
            .height(chart_h * 0.55)
            .x_axis_label("RPM")
            .y_axis_label("PS / Nm")
            .show(ui, |plot_ui| {
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
    });

    ui.add_space(4.0);

    // ── Boost bar chart ──────────────────────────────────────────
    ui.group(|ui| {
        ui.label(RichText::new("Boost vs RPM").strong());
        let bars: Vec<Bar> = app
            .power_capture
            .boost_series
            .iter()
            .map(|&[rpm, psi]| {
                Bar::new(rpm, psi)
                    .fill(Color32::from_rgb(180, 80, 220))
                    .width(app.config.power_curve_step as f64 * 0.8)
            })
            .collect();

        Plot::new("boost_plot")
            .height(chart_h * 0.35)
            .x_axis_label("RPM")
            .y_axis_label("Boost (PSI)")
            .show(ui, |plot_ui| {
                plot_ui.bar_chart(BarChart::new("Boost", bars));
            });
    });
}
