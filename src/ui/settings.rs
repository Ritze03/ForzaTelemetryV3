use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::{tr, Language};

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |cols| {
            // ── LEFT COLUMN ──────────────────────────────────────────
            let left = &mut cols[0];

            // ── Network ──────────────────────────────────────────────
            left.group(|ui| {
                ui.label(crate::theme::section_label(tr("Network")));
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

            // ── Co-Op ────────────────────────────────────────────────
            left.group(|ui| {
                ui.label(crate::theme::section_label(tr("Co-Op")));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(tr("Host port:"));
                    ui.add(egui::DragValue::new(&mut app.config.coop_port).range(1024..=65535));
                });
                ui.label(
                    RichText::new(tr("Local port the tunnel points at. Change only if it clashes with another app."))
                        .size(11.0)
                        .color(Color32::GRAY),
                );
            });

            left.add_space(8.0);

            // ── Display ──────────────────────────────────────────────
            left.group(|ui| {
                ui.label(crate::theme::section_label(tr("Display")));
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
                    crate::theme::styled_checkbox(ui, &mut app.config.fps_limit_enabled, tr("FPS limit:"));
                    if app.config.fps_limit_enabled {
                        ui.add(
                            egui::Slider::new(&mut app.config.fps_limit, 5.0..=120.0)
                                .step_by(1.0)
                                .suffix(" fps"),
                        );
                    }
                });

                crate::theme::styled_checkbox(ui, &mut app.config.always_on_top, tr("Always on top"));
            });

            // ── RIGHT COLUMN ─────────────────────────────────────────
            let right = &mut cols[1];

            // ── Recording ────────────────────────────────────────────
            right.group(|ui| {
                ui.label(crate::theme::section_label(tr("Recording")));
                ui.add_space(4.0);

                let recording = app.recorder.is_some();
                let rec_label = if recording {
                    let n = app.recorder.as_ref().map(|r| r.packets).unwrap_or(0);
                    format!("{}  {} ({} pkts)", crate::icons::STOP, tr("Stop Recording"), n)
                } else {
                    format!("{}  {}", crate::icons::CIRCLE, tr("Record"))
                };
                let rec_btn = if recording {
                    crate::theme::danger_button(rec_label)
                } else {
                    crate::theme::secondary_button(rec_label)
                };
                if ui.add(rec_btn).clicked() {
                    app.toggle_recording();
                }
                if recording {
                    ui.label(RichText::new(tr("Recording live telemetry to a file…"))
                        .size(11.0).color(Color32::from_rgb(225, 90, 90)));
                }

                ui.add_space(8.0);
                let files = crate::recorder::list_recordings();
                if files.is_empty() {
                    ui.label(RichText::new(tr("No recordings yet.")).size(11.0).color(Color32::GRAY));
                } else {
                    ui.horizontal(|ui| {
                        let sel = app.replay_selected.filter(|&i| i < files.len()).unwrap_or(0);
                        egui::ComboBox::from_id_salt("replay_file")
                            .selected_text(files[sel].1.clone())
                            .show_ui(ui, |ui| {
                                for (i, (_, name)) in files.iter().enumerate() {
                                    ui.selectable_value(&mut app.replay_selected, Some(i), name.clone());
                                }
                            });
                        if ui.add(crate::theme::secondary_button(tr("Export CSV"))).clicked() {
                            app.csv_export_msg = Some(match crate::recorder::export_csv(&files[sel].0) {
                                Ok(p) => format!("{} {}", tr("Saved"), p.display()),
                                Err(e) => format!("{} {e}", tr("CSV export failed:")),
                            });
                        }
                        if ui
                            .add_sized(
                                egui::vec2(24.0, 24.0),
                                egui::Button::new(
                                    RichText::new(crate::icons::TIMES).color(crate::theme::DANGER),
                                )
                                .fill(Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    Color32::from_rgba_unmultiplied(0xE1, 0x55, 0x54, 82),
                                )),
                            )
                            .clicked()
                        {
                            crate::recorder::delete_recording(&files[sel].0);
                            app.replay_selected = None;
                            app.csv_export_msg = None;
                        }
                    });
                    if let Some(msg) = &app.csv_export_msg {
                        ui.label(RichText::new(msg).size(11.0).color(Color32::from_rgb(110, 190, 110)));
                    }
                }
            });

            right.add_space(8.0);

            // GitHub link
            right.group(|ui| {
                ui.label(crate::theme::section_label(tr("Repository")));
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
                ui.hyperlink_to(
                    tr("Geist font — Vercel (OFL)"),
                    "https://github.com/vercel/geist-font",
                );
                ui.hyperlink_to(
                    tr("Nerd Fonts — Ryan L McIntyre (MIT)"),
                    "https://github.com/ryanoasis/nerd-fonts",
                );
                ui.label(
                    RichText::new(tr("Font licences: assets/fonts/"))
                        .size(11.0)
                        .color(Color32::GRAY),
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
