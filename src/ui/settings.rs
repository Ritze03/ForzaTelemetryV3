use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::config::{GameMode, Theme};

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        // ── Game ─────────────────────────────────────────────────
        ui.group(|ui| {
            ui.heading("Game");
            ui.add_space(4.0);

            egui::ComboBox::from_label("Target game")
                .selected_text(match app.config.game_mode {
                    GameMode::ForzaHorizon6    => "Forza Horizon 6",
                    GameMode::ForzaMotorsport7 => "Forza Motorsport 7 (Untested)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut app.config.game_mode,
                        GameMode::ForzaHorizon6,
                        "Forza Horizon 6",
                    );
                    ui.selectable_value(
                        &mut app.config.game_mode,
                        GameMode::ForzaMotorsport7,
                        "Forza Motorsport 7 (Untested)",
                    );
                });

            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "FH6: hides fuel, shows sprint times when not in race.\n\
                     FM7: shows all fields.",
                )
                .size(11.0)
                .color(Color32::GRAY),
            );
        });

        ui.add_space(8.0);

        // ── Network ─────────────────────────────────────────────
        ui.group(|ui| {
            ui.heading("Network");
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Listen port:");
                ui.add(
                    egui::DragValue::new(&mut app.pending_port)
                        .range(1024..=65535),
                );
                let changed = app.pending_port != app.config.listen_port;
                let btn = egui::Button::new("Apply").fill(if changed {
                    Color32::from_rgb(60, 120, 200)
                } else {
                    Color32::TRANSPARENT
                });
                if ui.add(btn).clicked() && changed {
                    let port = app.pending_port;
                    app.restart_receiver(port);
                }
            });
            ui.label(
                RichText::new("Avoid ports 5200–5300 (used by the game).")
                    .size(11.0)
                    .color(Color32::GRAY),
            );
        });

        ui.add_space(8.0);

        // ── Display ─────────────────────────────────────────────
        ui.group(|ui| {
            ui.heading("Display");
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label("Speed unit:");
                ui.radio_value(&mut app.config.use_mph, false, "km/h");
                ui.radio_value(&mut app.config.use_mph, true, "mph");
            });

            ui.horizontal(|ui| {
                ui.label("Tire temp unit:");
                ui.radio_value(&mut app.config.use_fahrenheit, false, "°C");
                ui.radio_value(&mut app.config.use_fahrenheit, true, "°F");
            });

            ui.horizontal(|ui| {
                ui.label("Boost / pressure:");
                ui.radio_value(&mut app.config.use_bar, false, "PSI");
                ui.radio_value(&mut app.config.use_bar, true, "bar");
            });

            ui.horizontal(|ui| {
                ui.label("Theme:");
                ui.radio_value(&mut app.config.theme, Theme::Dark, "Dark");
                ui.radio_value(&mut app.config.theme, Theme::Light, "Light");
            });

            ui.horizontal(|ui| {
                ui.label("FPS limit:");
                ui.add(
                    egui::Slider::new(&mut app.config.fps_limit, 15.0..=165.0)
                        .step_by(1.0)
                        .suffix(" fps"),
                );
            });

            ui.checkbox(&mut app.config.always_on_top, "Always on top");
        });

        ui.add_space(8.0);

        // ── Per-car (active car) ─────────────────────────────────
        ui.group(|ui| {
            let ordinal = app.last_car_ordinal;
            ui.heading("Current Car");
            if ordinal == 0 {
                ui.label(
                    RichText::new("No car detected yet — drive to load car settings.")
                        .color(Color32::GRAY),
                );
                return;
            }

            let car = app.car_settings.get_or_default(ordinal);

            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut car.name);
            });

            ui.horizontal(|ui| {
                ui.label("Shift indicator:");
                ui.add(
                    egui::Slider::new(&mut car.shift_low_pct, 50.0..=99.0)
                        .suffix("%")
                        .text("low"),
                );
                ui.add(
                    egui::Slider::new(&mut car.shift_high_pct, 50.0..=100.0)
                        .suffix("%")
                        .text("high"),
                );
            });

            ui.label(
                RichText::new(format!(
                    "Max measured RPM: {:.0}  →  shift at {:.0} / {:.0}",
                    car.max_rpm_measured,
                    car.shift_low_rpm(),
                    car.shift_high_rpm()
                ))
                .color(Color32::GRAY),
            );

            if ui.small_button("Reset max RPM").clicked() {
                car.max_rpm_measured = 0.0;
            }
        });

        ui.add_space(8.0);

        // ── Power curve ──────────────────────────────────────────
        ui.group(|ui| {
            ui.heading("Power Curve");
            ui.horizontal(|ui| {
                ui.label("RPM step size:");
                ui.add(
                    egui::Slider::new(&mut app.config.power_curve_step, 25.0..=500.0)
                        .step_by(25.0)
                        .suffix(" rpm"),
                );
            });
        });

        ui.add_space(8.0);

        // ── Save ─────────────────────────────────────────────────
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new(format!("{}  Save Settings", crate::icons::FLOPPY)).size(16.0))
                .clicked()
            {
                app.config.save();
                app.car_settings.save();
            }
            ui.label(
                RichText::new("Settings are also auto-saved on exit.")
                    .size(11.0)
                    .color(Color32::GRAY),
            );
        });
    });
}
