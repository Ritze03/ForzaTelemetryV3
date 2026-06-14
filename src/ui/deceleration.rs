use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    ui.heading("Deceleration Test");
    ui.label(
        RichText::new("Measure stopping distance and time over a configurable speed range.")
            .color(Color32::GRAY),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("From:");
        ui.add(
            egui::DragValue::new(&mut app.decel_start_kmh)
                .range(0.0..=400.0)
                .speed(1.0)
                .suffix(" km/h"),
        );
        ui.label("To:");
        ui.add(
            egui::DragValue::new(&mut app.decel_end_kmh)
                .range(0.0..=400.0)
                .speed(1.0)
                .suffix(" km/h"),
        );
    });

    ui.horizontal(|ui| {
        ui.checkbox(&mut app.perf_test.decel.dynamic_mode, "Dynamic mode");
        ui.label(
            RichText::new("(auto-starts on heavy braking above start speed)")
                .size(11.0)
                .color(Color32::GRAY),
        );
    });

    ui.add_space(12.0);

    let test = &app.perf_test.decel;
    if test.running {
        ui.label(RichText::new("⏱ Running…").color(Color32::YELLOW));
        ui.add(
            egui::ProgressBar::new(test.progress)
                .fill(Color32::from_rgb(220, 60, 60))
                .text(format!("{:.0}%", test.progress * 100.0)),
        );
        ui.label(format!("Decel: {:.2} g", test.current_g));
    } else if let Some(t) = test.result_secs {
        ui.label(
            RichText::new(format!("✅  {t:.3} s"))
                .size(28.0)
                .strong()
                .color(Color32::from_rgb(60, 210, 100)),
        );
    } else {
        ui.label(
            RichText::new("Waiting for trigger…").color(Color32::GRAY),
        );
    }

    ui.add_space(12.0);
    if ui.button("Reset").clicked() {
        app.perf_test.decel.reset();
    }
}
