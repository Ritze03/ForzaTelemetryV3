use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let connected = app.telemetry.is_connected;

    ui.columns(2, |cols| {
        // ── Left: Sprint timers ─────────────────────────────────
        {
            let ui = &mut cols[0];
            ui.heading("Sprint Timers");
            ui.label(RichText::new("Auto-triggered on speed crossings").color(Color32::GRAY));
            ui.add_space(8.0);

            egui::Grid::new("sprint_grid")
                .num_columns(2)
                .spacing([24.0, 8.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("0 → 100 km/h").size(15.0).strong());
                    sprint_result(ui, app.sprint_timer.zero_to_hundred, connected);
                    ui.end_row();

                    ui.label(RichText::new("100 → 200 km/h").size(15.0).strong());
                    sprint_result(ui, app.sprint_timer.hundred_to_two, connected);
                    ui.end_row();
                });

            ui.add_space(12.0);
            if ui.button("Reset sprint timers").clicked() {
                app.sprint_timer.reset();
            }
        }

        // ── Right: Configurable acceleration test ───────────────
        {
            let ui = &mut cols[1];
            ui.heading("Acceleration Test");
            ui.label(RichText::new("Configure range, then accelerate").color(Color32::GRAY));
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("Start:");
                ui.add(
                    egui::DragValue::new(&mut app.accel_start_kmh)
                        .range(0.0..=300.0)
                        .speed(1.0)
                        .suffix(" km/h"),
                );
                ui.label("End:");
                ui.add(
                    egui::DragValue::new(&mut app.accel_end_kmh)
                        .range(0.0..=300.0)
                        .speed(1.0)
                        .suffix(" km/h"),
                );
            });

            ui.add_space(8.0);

            let test = &app.perf_test.accel;
            if test.running {
                ui.label(
                    RichText::new(format!("{} Running…", crate::icons::CLOCK))
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
                ui.label(RichText::new("Waiting for speed crossing…").color(Color32::GRAY));
            }

            ui.add_space(8.0);
            if ui.button("Reset test").clicked() {
                app.perf_test.accel.reset();
            }
        }
    });
}

fn sprint_result(ui: &mut Ui, result: Option<f32>, connected: bool) {
    match result {
        Some(t) => {
            ui.label(
                RichText::new(format!("{t:.3} s"))
                    .size(22.0)
                    .strong()
                    .color(Color32::from_rgb(60, 210, 100)),
            );
        }
        None if connected => {
            ui.label(RichText::new("Waiting…").color(Color32::GRAY));
        }
        None => {
            ui.label(RichText::new("—").color(Color32::GRAY));
        }
    }
}
