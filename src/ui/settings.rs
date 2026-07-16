use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::{tr, Language};

/// Two-column control row: label in the left half, control in the right half —
/// the styling-guide layout (see docs/ui/STYLING-GUIDE.md).
fn control_row(ui: &mut Ui, label: &str, right: impl FnOnce(&mut Ui)) {
    ui.columns(2, |c| {
        crate::theme::row_label(&mut c[0], label);
        right(&mut c[1]);
    });
}

/// A dim sub-heading inside a category card (e.g. "Global (while in-game)").
fn sub_heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(11.0).color(crate::theme::TEXT_DIM).strong());
}

/// A coloured status dot + message (● renders in the font; emoji don't).
fn status_dot(ui: &mut Ui, ok: bool, msg: &str) {
    ui.horizontal(|ui| {
        let col = if ok { Color32::from_rgb(80, 200, 120) } else { Color32::from_rgb(220, 100, 100) };
        ui.label(RichText::new("\u{25CF}").color(col));
        ui.label(RichText::new(msg).size(11.0));
    });
}

/// A small grey hint line under a control.
fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(Color32::GRAY));
}

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
        ui.columns(2, |cols| {
            // ── LEFT COLUMN ──────────────────────────────────────────
            let left = &mut cols[0];
            left.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
            left.add_space(8.0);

            crate::theme::card(left, tr("Network"), |ui| {
                control_row(ui, tr("Listen port:"), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                        ui.add(egui::DragValue::new(&mut app.pending_port).range(1024..=65535));
                    });
                });
                hint(ui, tr("Avoid ports 5200–5300 (used by the game)."));
            });

            crate::theme::card(left, tr("Co-Op"), |ui| {
                control_row(ui, tr("Host port:"), |ui| {
                    ui.add(egui::DragValue::new(&mut app.config.coop_port).range(1024..=65535));
                });
                hint(ui, tr("Local port the tunnel points at. Change only if it clashes with another app."));
            });

            crate::theme::card(left, tr("Display"), |ui| {
                control_row(ui, tr("Language:"), |ui| {
                    egui::ComboBox::from_id_salt("language_combo")
                        .selected_text(app.config.language.label())
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for lang in Language::ALL {
                                ui.selectable_value(&mut app.config.language, lang, lang.label());
                            }
                        });
                });
                control_row(ui, tr("Speed unit:"), |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut app.config.use_mph, false, "km/h");
                        ui.radio_value(&mut app.config.use_mph, true, "mph");
                    });
                });
                control_row(ui, tr("Tire temp unit:"), |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut app.config.use_fahrenheit, false, "°C");
                        ui.radio_value(&mut app.config.use_fahrenheit, true, "°F");
                    });
                });
                control_row(ui, tr("Boost / pressure:"), |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut app.config.use_bar, true, "bar");
                        ui.radio_value(&mut app.config.use_bar, false, "PSI");
                    });
                });
                let fps_on = app.config.fps_limit_enabled;
                crate::theme::checkbox_row_with(ui, &mut app.config.fps_limit_enabled, tr("FPS limit"), |ui| {
                    if fps_on {
                        ui.add(
                            egui::Slider::new(&mut app.config.fps_limit, 5.0..=120.0)
                                .step_by(1.0)
                                .suffix(" fps"),
                        );
                    }
                });
                crate::theme::checkbox_row(ui, &mut app.config.always_on_top, tr("Always on top"));
            });

            // ── RIGHT COLUMN ─────────────────────────────────────────
            let right = &mut cols[1];
            right.spacing_mut().item_spacing.y = 0.0;
            right.add_space(8.0);

            crate::theme::card(right, tr("Hotkey"), |ui| hotkey_card(ui, app));
            crate::theme::card(right, tr("Input"), |ui| input_card(ui, app));
            crate::theme::card(right, tr("Repository"), |ui| repo_card(ui));
        });
    });
}

/// The "Hotkey" category: rebind rows grouped by scope.
fn hotkey_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config::{HotkeyAction, HotkeyScope};
    let mut changed = false;

    for (scope, heading) in [
        (HotkeyScope::Global, tr("Global (while in-game)")),
        (HotkeyScope::AppFocused, tr("In-app")),
    ] {
        sub_heading(ui, heading);
        for action in HotkeyAction::ALL.iter().copied().filter(|a| a.scope() == scope) {
            let capturing = app.rebinding == Some(action);
            let text = if capturing {
                tr("Press a key…").to_string()
            } else {
                app.config.hotkeys.bindings.get(&action).map(|b| b.label()).unwrap_or_default()
            };
            control_row(ui, tr(action.label()), |ui| {
                let h = ui.spacing().interact_size.y;
                if ui.add_sized([ui.available_width(), h], egui::Button::new(text)).clicked() {
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

    if changed { app.sync_hotkeys(); }
}

/// The "Input" category: window detection + synthetic-input gating.
fn input_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config::GateMode;
    let mut changed = false;

    // ── Window Detection ──
    sub_heading(ui, tr("Window Detection"));
    control_row(ui, tr("Active if:"), |ui| {
        egui::ComboBox::from_id_salt("hk_gate_mode")
            .selected_text(match app.config.hotkeys.gate_mode {
                GateMode::TelemetryLive => tr("Telemetry live"),
                GateMode::WindowFocus => tr("Game window focused"),
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::TelemetryLive, tr("Telemetry live")).changed();
                changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::WindowFocus, tr("Game window focused")).changed();
            });
    });

    if app.config.hotkeys.gate_mode == GateMode::WindowFocus {
        #[cfg(target_os = "linux")]
        {
            use crate::config::FocusMethod;
            control_row(ui, tr("Window Detection Method"), |ui| {
                egui::ComboBox::from_id_salt("hk_focus_method")
                    .selected_text(match app.config.hotkeys.focus_method {
                        FocusMethod::Hyprland => "Hyprland",
                        FocusMethod::X11 => "X11",
                        FocusMethod::Custom => tr("Custom"),
                    })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Hyprland, "Hyprland").changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::X11, "X11").changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Custom, tr("Custom")).changed();
                    });
            });
            if app.config.hotkeys.focus_method == FocusMethod::Custom {
                control_row(ui, tr("Command:"), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("Test")).clicked() {
                            app.focus_preview = app.focus.query_now().unwrap_or_else(|e| format!("error: {e}"));
                        }
                        changed |= ui.add(egui::TextEdit::singleline(&mut app.config.hotkeys.custom_cmd).desired_width(ui.available_width())).changed();
                    });
                });
                if !app.focus_preview.is_empty() {
                    hint(ui, &format!("\u{2192} {}", app.focus_preview));
                }
            }
        }

        control_row(ui, tr("Game Window Title:"), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                changed |= ui.add(egui::TextEdit::singleline(&mut app.config.hotkeys.game_match).desired_width(ui.available_width())).changed();
            });
        });

        // Focus-detection status (the query can fail even when capture works).
        if app.focus.status() == crate::focus::FocusStatus::QueryFailed {
            status_dot(ui, false, tr("Focus detection failed — check the method/command"));
        }
    }

    // Linux capture-permission status.
    #[cfg(target_os = "linux")]
    {
        use crate::hotkeys::HotkeyStatus;
        match app.hotkeys.status() {
            HotkeyStatus::Ok => status_dot(ui, true, tr("Keyboard capture active")),
            _ => status_dot(ui, false, tr("No input access — add your user to the 'input' group: sudo usermod -aG input $USER, then re-login")),
        }
    }

    // ── Send Input ──
    sub_heading(ui, tr("Send Input"));
    changed |= crate::theme::slider_row(ui, tr("Focus check rate:"), &mut app.config.hotkeys.focus_poll_hz, 1.0..=20.0, 1.0, 0, " Hz").changed();
    changed |= crate::theme::checkbox_row(ui, &mut app.config.hotkeys.input_focus_gate, tr("Only send inputs when game focused")).changed();

    if changed { app.sync_hotkeys(); }
}

/// The "Repository" category: project link + credits.
fn repo_card(ui: &mut Ui) {
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
    hint(ui, tr("Font licences: assets/fonts/"));
}
