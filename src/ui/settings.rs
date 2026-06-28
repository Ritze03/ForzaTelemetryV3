use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::config::{DashboardPreset, GameMode};
use crate::i18n::{tr, Language};

static PRESET_ALE:   &str = include_str!("../../assets/configs/ale.json");
static PRESET_RITZE: &str = include_str!("../../assets/configs/ritze.json");

const PRESET_NAMES: &[&str] = &["Ale (halb)", "Ritze (ganz)"];
const PRESET_DATA:  &[&str] = &[PRESET_ALE, PRESET_RITZE];

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |cols| {
            // ── LEFT COLUMN ──────────────────────────────────────────
            let left = &mut cols[0];

            // ── Game ─────────────────────────────────────────────────
            left.group(|ui| {
                ui.heading(tr("Game"));
                ui.add_space(4.0);

                egui::ComboBox::from_label(tr("Target game"))
                    .selected_text(match app.config.game_mode {
                        GameMode::ForzaHorizon6    => "Forza Horizon 6",
                        GameMode::ForzaMotorsport7 => tr("Forza Motorsport 7 (Untested)"),
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
                            tr("Forza Motorsport 7 (Untested)"),
                        );
                    });

                ui.add_space(2.0);
                ui.label(
                    RichText::new(tr(
                        "FH6: hides fuel, shows sprint times when not in race.\n\
                         FM7: shows all fields.",
                    ))
                    .size(11.0)
                    .color(Color32::GRAY),
                );
            });

            left.add_space(8.0);

            // ── Load Preset ───────────────────────────────────────────
            left.group(|ui| {
                ui.heading(tr("Load Preset"));
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let selected_label = app.pending_preset
                        .map(|i| PRESET_NAMES[i])
                        .unwrap_or(tr("— select —"));

                    egui::ComboBox::from_id_salt("preset_combo")
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for (i, name) in PRESET_NAMES.iter().enumerate() {
                                ui.selectable_value(&mut app.pending_preset, Some(i), *name);
                            }
                        });

                    if ui.button(tr("Load Preset")).clicked() {
                        if let Some(idx) = app.pending_preset {
                            if let Ok(preset) = serde_json::from_str::<DashboardPreset>(PRESET_DATA[idx]) {
                                preset.apply_to(&mut app.config);
                            }
                            app.pending_preset = None;
                        }
                    }
                });

                ui.add_space(2.0);
                ui.label(
                    RichText::new(tr("Applies dashboard layout only. Other settings unchanged."))
                        .size(11.0)
                        .color(Color32::GRAY),
                );
            });

            left.add_space(8.0);

            // ── Network ──────────────────────────────────────────────
            left.group(|ui| {
                ui.heading(tr("Network"));
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label(tr("Listen port:"));
                    ui.add(
                        egui::DragValue::new(&mut app.pending_port)
                            .range(1024..=65535),
                    );
                    let changed = app.pending_port != app.config.listen_port;
                    let btn = egui::Button::new(tr("Apply")).fill(if changed {
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
                    RichText::new(tr("Avoid ports 5200–5300 (used by the game)."))
                        .size(11.0)
                        .color(Color32::GRAY),
                );
            });

            left.add_space(8.0);

            // ── Display ──────────────────────────────────────────────
            left.group(|ui| {
                ui.heading(tr("Display"));
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    ui.label(tr("Language:"));
                    egui::ComboBox::from_id_salt("language_combo")
                        .selected_text(app.config.language.label())
                        .show_ui(ui, |ui| {
                            for lang in Language::ALL {
                                ui.selectable_value(&mut app.config.language, lang, lang.label());
                            }
                        });
                });

                ui.horizontal(|ui| {
                    ui.label(tr("Speed unit:"));
                    ui.radio_value(&mut app.config.use_mph, false, "km/h");
                    ui.radio_value(&mut app.config.use_mph, true, "mph");
                });

                ui.horizontal(|ui| {
                    ui.label(tr("Tire temp unit:"));
                    ui.radio_value(&mut app.config.use_fahrenheit, false, "°C");
                    ui.radio_value(&mut app.config.use_fahrenheit, true, "°F");
                });

                ui.horizontal(|ui| {
                    ui.label(tr("Boost / pressure:"));
                    ui.radio_value(&mut app.config.use_bar, true, "bar");
                    ui.radio_value(&mut app.config.use_bar, false, "PSI");
                });

                ui.horizontal(|ui| {
                    ui.checkbox(&mut app.config.fps_limit_enabled, tr("FPS limit:"));
                    if app.config.fps_limit_enabled {
                        ui.add(
                            egui::Slider::new(&mut app.config.fps_limit, 5.0..=120.0)
                                .step_by(1.0)
                                .suffix(" fps"),
                        );
                    }
                });

                ui.checkbox(&mut app.config.always_on_top, tr("Always on top"));
            });

            // ── RIGHT COLUMN ─────────────────────────────────────────
            let right = &mut cols[1];

            // GitHub link
            right.group(|ui| {
                ui.heading(tr("Repository"));
                ui.add_space(4.0);
                ui.hyperlink_to(
                    "github.com/Ritze03/ForzaTelemetryV3",
                    "https://github.com/Ritze03/ForzaTelemetryV3",
                );
                ui.add_space(4.0);
                ui.label(tr("Credits:"));
                ui.hyperlink_to(
                    tr("Le0_X8 — seasonal map images"),
                    "https://www.reddit.com/r/ForzaHorizon/comments/1td6qzb/8096x_hires_seasonal_maps_of_fh6_from_the_early/",
                );
            });

        });

        ui.add_space(8.0);

        // ── Save (full width, below columns) ─────────────────────────
        ui.horizontal(|ui| {
            if ui
                .button(RichText::new(format!("{}  {}", crate::icons::FLOPPY, tr("Save Settings"))).size(16.0))
                .clicked()
            {
                app.config.save();
            }
            ui.label(
                RichText::new(tr("Settings are also auto-saved on exit."))
                    .size(11.0)
                    .color(Color32::GRAY),
            );
        });
    });
}
