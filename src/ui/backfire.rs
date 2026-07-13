use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::tr;

pub fn show_backfire(ui: &mut Ui, app: &mut ForzaApp) {
    // Fixed spinner width — fits the widest value ("20000") so the number boxes
    // don't grow/shift as digits are added.
    const SPIN_W: f32 = 64.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("backfire_scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
            ui.add_space(8.0);
            ui.label(
                RichText::new(tr("Triggers Backfire by spamming 'W'"))
                    .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            // ── Activation ───────────────────────────────────────────────
            crate::theme::card(ui, tr("Activation"), |ui| {
                crate::theme::styled_checkbox(ui, &mut app.config.backfire_enabled, tr("Enabled"));
            });

            // ── RPM Range ────────────────────────────────────────────────
            crate::theme::card(ui, tr("RPM Range"), |ui| {
                crate::theme::styled_checkbox(ui, &mut app.config.backfire_dynamic_rpm, tr("Dynamic RPM"));
                if app.config.backfire_dynamic_rpm {
                    ui.horizontal(|ui| {
                        ui.label(tr("Min:"));
                        ui.add(
                            egui::Slider::new(&mut app.config.backfire_dynamic_min_pct, 0.0..=100.0)
                                .suffix("%")
                                .fixed_decimals(1)
                                .step_by(1.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr("Max:"));
                        ui.add(
                            egui::Slider::new(&mut app.config.backfire_dynamic_max_pct, 0.0..=100.0)
                                .suffix("%")
                                .fixed_decimals(1)
                                .step_by(1.0),
                        );
                    });
                    ui.label(
                        RichText::new(format!(
                            "{}: {:.0} \u{2013} {:.0} RPM",
                            tr("Range"), app.backfire.last_min_rpm, app.backfire.last_max_rpm,
                        ))
                        .size(11.0)
                        .color(Color32::GRAY),
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.label(tr("Min RPM:"));
                        ui.add_sized(
                            [SPIN_W, ui.spacing().interact_size.y],
                            egui::DragValue::new(&mut app.config.backfire_min_rpm)
                                .range(0.0..=20000.0)
                                .speed(50.0),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr("Max RPM:"));
                        ui.add_sized(
                            [SPIN_W, ui.spacing().interact_size.y],
                            egui::DragValue::new(&mut app.config.backfire_max_rpm)
                                .range(0.0..=20000.0)
                                .speed(50.0),
                        );
                    });
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(tr("RPM interval:"));
                    ui.add_sized(
                        [SPIN_W, ui.spacing().interact_size.y],
                        egui::DragValue::new(&mut app.config.backfire_interval_rpm)
                            .range(0.0..=2000.0)
                            .speed(10.0),
                    );
                });
            });

            // ── Key Press ────────────────────────────────────────────────
            crate::theme::card(ui, tr("Key Press"), |ui| {
                crate::theme::styled_checkbox(ui,
                    &mut app.config.backfire_dynamic_duration,
                    tr("Dynamic key press duration"),
                );
                if app.config.backfire_dynamic_duration {
                    use crate::config::BackfireDynamicMode;
                    ui.horizontal(|ui| {
                        ui.label(tr("Mode:"));
                        egui::ComboBox::from_id_salt("backfire_dyn_mode")
                            .selected_text(match app.config.backfire_dynamic_mode {
                                BackfireDynamicMode::TimeBased => tr("Time-based"),
                                BackfireDynamicMode::PacketBased => tr("Packet-based"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut app.config.backfire_dynamic_mode,
                                    BackfireDynamicMode::PacketBased,
                                    tr("Packet-based"),
                                );
                                ui.selectable_value(
                                    &mut app.config.backfire_dynamic_mode,
                                    BackfireDynamicMode::TimeBased,
                                    tr("Time-based"),
                                );
                            });
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label(tr("Key press duration:"));
                        ui.add_sized(
                            [SPIN_W, ui.spacing().interact_size.y],
                            egui::DragValue::new(&mut app.config.backfire_accel_time_ms)
                                .range(1..=50)
                                .speed(1.0)
                                .suffix(" ms"),
                        );
                    });
                }
            });

            // ── Conditions ───────────────────────────────────────────────
            crate::theme::card(ui, tr("Conditions"), |ui| {
                crate::theme::styled_checkbox(ui,
                    &mut app.config.backfire_disable_standstill,
                    tr("Disable if standing still"),
                );
                crate::theme::styled_checkbox(ui,
                    &mut app.config.backfire_test_mode,
                    tr("Test mode (ignores throttle/RPM conditions)"),
                );
            });
        });
}
