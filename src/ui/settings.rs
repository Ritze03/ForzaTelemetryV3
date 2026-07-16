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

            right.add_space(8.0);

            // ── Hotkeys ──────────────────────────────────────────────
            right.group(|ui| {
                ui.label(crate::theme::section_label(tr("Hotkeys")));
                ui.add_space(4.0);

                use crate::config::{GateMode, HotkeyAction, HotkeyScope};
                let mut changed = false;

                // Rebind rows, grouped by scope.
                for (scope, heading) in [
                    (HotkeyScope::Global, tr("Global (while in-game)")),
                    (HotkeyScope::AppFocused, tr("In-app")),
                ] {
                    ui.label(RichText::new(heading).size(11.0).color(Color32::GRAY));
                    for action in HotkeyAction::ALL.iter().copied().filter(|a| a.scope() == scope) {
                        ui.horizontal(|ui| {
                            ui.label(tr(action.label()));
                            let capturing = app.rebinding == Some(action);
                            let text = if capturing {
                                tr("Press a key…").to_string()
                            } else {
                                app.config.hotkeys.bindings.get(&action).map(|b| b.label()).unwrap_or_default()
                            };
                            if ui.button(text).clicked() {
                                app.rebinding = if capturing { None } else { Some(action) };
                            }
                        });
                    }
                }

                // Capture the next key while rebinding.
                if let Some(action) = app.rebinding {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        app.rebinding = None;
                    } else if let Some(hk) = ui.input(|i| {
                        i.events.iter().find_map(|e| match e {
                            egui::Event::Key { key, pressed: true, .. } => crate::keymap::HotKey::from_egui(*key),
                            _ => None,
                        })
                    }) {
                        if hk != crate::keymap::HotKey::Escape {
                            let m = ui.input(|i| i.modifiers);
                            app.config.hotkeys.bindings.insert(action, crate::keymap::HotkeyBinding {
                                mods: crate::keymap::Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, sup: false },
                                key: hk,
                            });
                            app.rebinding = None;
                            changed = true;
                        }
                    }
                }

                ui.add_space(6.0);

                // Detection mode.
                ui.horizontal(|ui| {
                    ui.label(tr("Trigger when:"));
                    egui::ComboBox::from_id_salt("hk_gate_mode")
                        .selected_text(match app.config.hotkeys.gate_mode {
                            GateMode::TelemetryLive => tr("Telemetry live"),
                            GateMode::WindowFocus => tr("Game window focused"),
                        })
                        .show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::TelemetryLive, tr("Telemetry live")).changed();
                            changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::WindowFocus, tr("Game window focused")).changed();
                        });
                });

                if app.config.hotkeys.gate_mode == GateMode::WindowFocus {
                    // Linux: per-compositor method + optional custom command.
                    #[cfg(target_os = "linux")]
                    {
                        use crate::config::FocusMethod;
                        ui.horizontal(|ui| {
                            ui.label(tr("Method:"));
                            egui::ComboBox::from_id_salt("hk_focus_method")
                                .selected_text(format!("{:?}", app.config.hotkeys.focus_method))
                                .show_ui(ui, |ui| {
                                    changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Hyprland, "Hyprland").changed();
                                    changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::X11, "X11").changed();
                                    changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Custom, tr("Custom")).changed();
                                });
                        });
                        if app.config.hotkeys.focus_method == FocusMethod::Custom {
                            ui.horizontal(|ui| {
                                ui.label(tr("Command:"));
                                changed |= ui.text_edit_singleline(&mut app.config.hotkeys.custom_cmd).changed();
                                if ui.button(tr("Test")).clicked() {
                                    app.focus_preview = app.focus.query_now().unwrap_or_else(|e| format!("error: {e}"));
                                }
                            });
                            if !app.focus_preview.is_empty() {
                                ui.label(RichText::new(format!("→ {}", app.focus_preview)).size(11.0).color(Color32::GRAY));
                            }
                        }
                    }

                    // Game match + Detect (all platforms).
                    ui.horizontal(|ui| {
                        ui.label(tr("Game window match:"));
                        changed |= ui.text_edit_singleline(&mut app.config.hotkeys.game_match).changed();
                        let label = match app.detect_until {
                            Some(t) => {
                                let secs = t.saturating_duration_since(std::time::Instant::now()).as_secs() + 1;
                                format!("{} {}", tr("Detecting…"), secs)
                            }
                            None => tr("Detect").to_string(),
                        };
                        if ui.button(label).clicked() && app.detect_until.is_none() {
                            app.detect_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                        }
                    });
                }

                // Input focus gate + poll rate.
                changed |= crate::theme::styled_checkbox(ui, &mut app.config.hotkeys.input_focus_gate, tr("Only send inputs when game focused")).changed();
                ui.horizontal(|ui| {
                    ui.label(tr("Focus check rate:"));
                    changed |= ui.add(egui::Slider::new(&mut app.config.hotkeys.focus_poll_hz, 1.0..=20.0).step_by(1.0).suffix(" Hz")).changed();
                });

                // Status light (Linux capture permission).
                #[cfg(target_os = "linux")]
                {
                    use crate::hotkeys::HotkeyStatus;
                    let (dot, msg) = match app.hotkeys.status() {
                        HotkeyStatus::Ok => ("🟢", tr("Keyboard capture active")),
                        _ => ("🔴", tr("No input access — add your user to the 'input' group: sudo usermod -aG input $USER, then re-login")),
                    };
                    ui.label(format!("{dot} {msg}"));
                }

                // Window-focus detection status (the query can fail even when
                // capture works — e.g. the compositor tool is missing).
                if app.config.hotkeys.gate_mode == GateMode::WindowFocus
                    && app.focus.status() == crate::focus::FocusStatus::QueryFailed
                {
                    ui.label(
                        RichText::new(tr("🔴 Focus detection failed — check the method/command"))
                            .size(11.0)
                            .color(Color32::from_rgb(220, 100, 100)),
                    );
                }

                if changed { app.sync_hotkeys(); }
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
