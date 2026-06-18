use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    ui.heading("Acceleration Test");
    ui.label(
        RichText::new("Configure a speed range, then accelerate through it.")
            .color(Color32::GRAY),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("Start:");
        ui.add(
            egui::DragValue::new(&mut app.config.accel_start_kmh)
                .range(0.0..=300.0)
                .speed(1.0)
                .suffix(" km/h"),
        );
        ui.label("End:");
        ui.add(
            egui::DragValue::new(&mut app.config.accel_end_kmh)
                .range(0.0..=300.0)
                .speed(1.0)
                .suffix(" km/h"),
        );
    });

    ui.add_space(8.0);

    let test = &app.perf_test.accel;
    if test.running {
        ui.label(
            RichText::new(format!("{} Running\u{2026}", crate::icons::CLOCK))
                .color(Color32::YELLOW),
        );
        ui.add(
            egui::ProgressBar::new(test.progress)
                .fill(Color32::from_rgb(60, 180, 90))
                .text(format!("{:.0}%", test.progress * 100.0)),
        );
        ui.label(format!("G: {:.2} g", test.current_g));
    } else if let Some(t) = test.result_secs {
        ui.label(
            RichText::new(format!("{}  {t:.3} s", crate::icons::CHECK))
                .size(28.0)
                .strong()
                .color(Color32::from_rgb(60, 210, 100)),
        );
    } else {
        ui.label(
            RichText::new("Waiting for speed crossing\u{2026}").color(Color32::GRAY),
        );
    }

    ui.add_space(8.0);
    if ui.button("Reset test").clicked() {
        app.perf_test.accel.reset();
    }
}
