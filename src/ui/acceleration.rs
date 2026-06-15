use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::config::GameMode;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let connected = app.telemetry.is_connected;

    ui.columns(2, |cols| {
        // ── Left: Sprint timers ─────────────────────────────────
        {
            let ui = &mut cols[0];
            ui.heading("Sprint Timers");
            ui.label(RichText::new("Auto-triggered on speed crossings").color(Color32::GRAY));
            ui.add_space(8.0);

            let is_fh6 = app.config.game_mode == GameMode::ForzaHorizon6;
            let st = &app.sprint_timer;

            egui::Grid::new("sprint_grid")
                .num_columns(3)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    sprint_row(ui, "0 → 100 km/h",   st.zero_to_hundred, None, connected);
                    ui.end_row();

                    sprint_row(ui, "100 → 200 km/h", st.hundred_to_two,
                        cumulative_time(&[st.zero_to_hundred, st.hundred_to_two]),
                        connected);
                    ui.end_row();

                    if is_fh6 {
                        sprint_row(ui, "200 → 300 km/h", st.two_to_three,
                            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three]),
                            connected);
                        ui.end_row();

                        sprint_row(ui, "300 → 400 km/h", st.three_to_four,
                            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three, st.three_to_four]),
                            connected);
                        ui.end_row();

                        sprint_row(ui, "400 → 500 km/h", st.four_to_five,
                            cumulative_time(&[st.zero_to_hundred, st.hundred_to_two, st.two_to_three, st.three_to_four, st.four_to_five]),
                            connected);
                        ui.end_row();
                    }
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

fn sprint_row(ui: &mut Ui, label: &str, segment: Option<f32>, cumulative: Option<f32>, connected: bool) {
    ui.label(RichText::new(label).size(15.0).strong());
    match segment {
        Some(t) => {
            ui.label(
                RichText::new(format!("{t:.3} s"))
                    .size(22.0)
                    .strong()
                    .color(Color32::from_rgb(60, 210, 100)),
            );
            if let Some(c) = cumulative {
                ui.label(RichText::new(format!("({c:.3} s)")).color(Color32::GRAY));
            } else {
                ui.label("");
            }
        }
        None if connected => {
            ui.label(RichText::new("Waiting…").color(Color32::GRAY));
            ui.label("");
        }
        None => {
            ui.label(RichText::new("—").color(Color32::GRAY));
            ui.label("");
        }
    }
}

fn cumulative_time(splits: &[Option<f32>]) -> Option<f32> {
    splits.iter().copied().collect::<Option<Vec<_>>>().map(|v| v.iter().sum())
}
