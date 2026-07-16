use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::coop::Role;
use crate::i18n::tr;

/// Player hue (0..360) → a saturated, readable marker colour.
pub fn hue_color(hue: f32) -> Color32 {
    Color32::from(egui::ecolor::Hsva::new(hue / 360.0, 0.85, 0.98, 1.0))
}

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    let role = app.coop.role();

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |cols| {
            identity_and_pacing(&mut cols[0], app);
            session_panel(&mut cols[1], app, role);
        });
    });
}

fn identity_and_pacing(ui: &mut Ui, app: &mut ForzaApp) {
    ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
    crate::theme::card(ui, tr("Your Identity"), |ui| {
        // Name: label | text field, in the same two-column layout as the colour
        // row below, so the field lines up with (and matches the width of) the slider.
        let name_changed = ui.columns(2, |c| {
            crate::theme::row_label(&mut c[0], tr("Player name"));
            c[1].add(
                egui::TextEdit::singleline(&mut app.config.coop_name)
                    .hint_text(crate::theme::placeholder(tr("Player")))
                    .desired_width(c[1].available_width()),
            )
            .changed()
        });
        if name_changed {
            let (n, h) = (app.config.coop_name.clone(), app.config.coop_hue);
            app.coop.update_identity(&n, h);
        }

        // Colour: label | slider + a swatch preview pinned to the right (where a
        // value spinner sits on other rows).
        let changed = ui.columns(2, |c| {
            crate::theme::row_label(&mut c[0], tr("Player color"));
            c[1].horizontal(|ui| {
                const SW: f32 = 22.0;
                let rail = (ui.available_width() - SW - ui.spacing().item_spacing.x).max(40.0);
                ui.spacing_mut().slider_width = rail;
                let r = ui.add(egui::Slider::new(&mut app.config.coop_hue, 0.0..=360.0).show_value(false));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(SW, ui.spacing().interact_size.y), egui::Sense::hover());
                let sq = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(SW.min(rect.height())));
                ui.painter().rect_filled(sq, 4.0, hue_color(app.config.coop_hue));
                ui.painter().rect_stroke(sq, 4.0, egui::Stroke::new(1.0, Color32::from_gray(40)), egui::StrokeKind::Inside);
                r.changed()
            })
            .inner
        });
        if changed {
            let (n, h) = (app.config.coop_name.clone(), app.config.coop_hue);
            app.coop.update_identity(&n, h);
        }

        ui.label(
            RichText::new(tr("Others see this name + colour; your own map arrow uses the colour only."))
                .size(11.0)
                .color(Color32::GRAY),
        );
    });

    crate::theme::card(ui, tr("Pacing"), |ui| {
        if crate::theme::slider_row(ui, tr("Packet Buffer Size"), &mut app.config.coop_buffer_ms, 0..=500, 10.0, 0, " ms").changed() {
            app.coop.set_buffer_ms(app.config.coop_buffer_ms);
        }
        ui.label(
            RichText::new(tr(
                "Delays remote players by this much to smooth out network jitter.\n\
                 0 = lowest latency; raise it if other cars stutter on the map.",
            ))
            .size(11.0)
            .color(Color32::GRAY),
        );
    });
}

fn session_panel(ui: &mut Ui, app: &mut ForzaApp, role: Role) {
    use crate::icons;
    ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
    crate::theme::card(ui, tr("Session"), |ui| {
        // Connection status badge (moved here from the top of the tab).
        let (col, txt) = match role {
            Role::Off => (crate::theme::FAINT, tr("Offline")),
            Role::Host => (crate::theme::ACCENT, tr("Hosting")),
            Role::Client => (crate::theme::GOOD, tr("Joined")),
        };
        ui.colored_label(col, format!("{}  {}", icons::CIRCLE, txt));
        // Status line
        let status = app.coop.status();
        if !status.is_empty() {
            ui.label(RichText::new(status).color(Color32::from_gray(200)));
        }
        if let Some(err) = app.coop.error() {
            ui.colored_label(Color32::from_rgb(230, 120, 120), format!("⚠ {err}"));
        }
        ui.add_space(4.0);

        match role {
            Role::Off => {
                if ui
                    .add_sized(
                        [ui.available_width(), 30.0],
                        crate::theme::primary_button(format!("{}  {}", icons::GLOBE, tr("Host Session"))),
                    )
                    .clicked()
                {
                    let (n, h, b) = (
                        app.config.coop_name.clone(),
                        app.config.coop_hue,
                        app.config.coop_buffer_ms,
                    );
                    app.coop.start_host(app.config.coop_port, &n, h, b);
                }
                ui.add_space(8.0);
                ui.label(tr("…or join with a code"));
                ui.add_space(2.0);
                // Join button pinned to the right; the code field fills the rest.
                let join_clicked = ui.horizontal(|ui| {
                    let can_join = !app.coop_join_input.trim().is_empty();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let clicked = ui
                            .add_enabled(
                                can_join,
                                egui::Button::new(format!("{}  {}", icons::LINK, tr("Join"))),
                            )
                            .clicked();
                        ui.add(
                            egui::TextEdit::singleline(&mut app.coop_join_input)
                                .hint_text(crate::theme::placeholder("blue-fox-rapid-owl"))
                                .desired_width(ui.available_width()),
                        );
                        clicked
                    })
                    .inner
                })
                .inner;
                if join_clicked {
                    let (words, n, h, b) = (
                        app.coop_join_input.clone(),
                        app.config.coop_name.clone(),
                        app.config.coop_hue,
                        app.config.coop_buffer_ms,
                    );
                    app.config.coop_last_code = words.clone();
                    app.config.save();
                    app.coop.start_client(&words, &n, h, b);
                }
            }
            Role::Host => {
                if let Some((got, total)) = app.coop.download() {
                    // First run on this machine: fetching the cloudflared binary.
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0));
                        ui.label(RichText::new(tr("Downloading cloudflared…"))
                            .color(Color32::from_gray(200)));
                    });
                    ui.add_space(4.0);
                    let mb = |b: u64| b as f32 / 1_048_576.0;
                    let (frac, text) = match total {
                        Some(t) if t > 0 => (
                            (got as f32 / t as f32).clamp(0.0, 1.0),
                            format!("{:.1} / {:.1} MB", mb(got), mb(t)),
                        ),
                        _ => (0.0, format!("{:.1} MB", mb(got))),
                    };
                    let mut bar = egui::ProgressBar::new(frac)
                        .text(text)
                        .desired_width(ui.available_width());
                    if total.is_none() {
                        bar = bar.animate(true); // size unknown → indeterminate sweep
                    }
                    ui.add(bar);
                    ui.add_space(8.0);
                    stop_button(ui, app, tr("Cancel"));
                } else {
                    ui.label(tr("Share this code so others can join"));
                    ui.add_space(2.0);
                    match app.coop.words() {
                        Some(words) => share_code(ui, app, &words),
                        None => {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(16.0));
                                ui.label(RichText::new(tr("Starting tunnel…")).color(Color32::GRAY));
                            });
                        }
                    }
                    if let Some(lan) = app.coop.lan_url() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(tr("Same network? Lower latency with"))
                            .size(11.0).color(crate::theme::FAINT));
                        ui.add(egui::Label::new(RichText::new(lan).monospace().size(13.0)).selectable(true));
                    }
                    ui.add_space(8.0);
                    stop_button(ui, app, tr("Stop Hosting"));
                }
            }
            Role::Client => {
                if let Some(words) = app.coop.words() {
                    ui.horizontal(|ui| {
                        ui.label(tr("Connected to"));
                        ui.label(RichText::new(words).monospace().strong());
                    });
                }
                ui.add_space(8.0);
                stop_button(ui, app, tr("Leave Session"));
            }
        }
    });

    roster_panel(ui, app);
}

fn share_code(ui: &mut Ui, app: &mut ForzaApp, words: &str) {
    use crate::icons;
    egui::Frame::new()
        .fill(crate::theme::FIELD)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Selectable so it can be copied by hand if the button fails.
                ui.add(
                    egui::Label::new(RichText::new(words).monospace().size(16.0).strong())
                        .selectable(true),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let just_copied = app
                        .coop_copied_at
                        .map(|t| t.elapsed().as_secs_f32() < 1.5)
                        .unwrap_or(false);
                    let label = if just_copied {
                        format!("{}  {}", icons::CHECK, tr("Copied"))
                    } else {
                        format!("{}  {}", icons::COPY, tr("Copy"))
                    };
                    if ui.button(label).clicked() {
                        ui.ctx().copy_text(words.to_string());
                        app.coop_copied_at = Some(std::time::Instant::now());
                    }
                });
            });
        });
}

fn roster_panel(ui: &mut Ui, app: &ForzaApp) {
    let roster = app.coop.roster();
    let my_id = app.coop.my_id();
    crate::theme::card(ui, &format!("{} ({})", tr("Players"), roster.len()), |ui| {
        if roster.is_empty() {
            ui.label(RichText::new(tr("No one here yet.")).color(Color32::GRAY));
        }
        for p in roster {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 6.0, hue_color(p.hue));
                ui.label(RichText::new(&p.name).size(14.0));
                if p.id == my_id {
                    ui.label(RichText::new(tr("(you)")).size(11.0).color(Color32::GRAY));
                }
            });
        }
    });
}

fn stop_button(ui: &mut Ui, app: &mut ForzaApp, label: &str) {
    use crate::icons;
    if ui
        .add_sized(
            [ui.available_width(), 28.0],
            crate::theme::danger_button(format!("{}  {}", icons::TIMES, label)),
        )
        .clicked()
    {
        app.coop.stop();
    }
}
