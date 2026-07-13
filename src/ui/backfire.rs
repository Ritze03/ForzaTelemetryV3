use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::tr;

pub fn show_backfire(ui: &mut Ui, app: &mut ForzaApp) {
    // Split into two halves like the Automatic Gearbox tab; for now everything
    // lives in the left column, which keeps the controls from stretching wide.
    ui.spacing_mut().item_spacing.x = 8.0; // ui.columns uses item_spacing.x as the inter-column gap
    ui.columns(2, |cols| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("backfire_scroll")
            .show(&mut cols[0], |ui| {
                ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
                ui.add_space(8.0);

                // ── Activation ───────────────────────────────────────────
                crate::theme::card(ui, tr("Activation"), |ui| {
                    crate::theme::checkbox_row(ui, &mut app.config.backfire_enabled, tr("Enabled"));
                });

                // ── RPM Range ────────────────────────────────────────────
                crate::theme::card(ui, tr("RPM Range"), |ui| {
                    crate::theme::checkbox_row(ui, &mut app.config.backfire_dynamic_rpm, tr("Dynamic RPM"));
                    if app.config.backfire_dynamic_rpm {
                        crate::theme::slider_row(ui, tr("Min:"), &mut app.config.backfire_dynamic_min_pct, 0.0..=100.0, 1.0, 1, "%");
                        crate::theme::slider_row(ui, tr("Max:"), &mut app.config.backfire_dynamic_max_pct, 0.0..=100.0, 1.0, 1, "%");
                        ui.label(
                            RichText::new(format!(
                                "{}: {:.0} \u{2013} {:.0} RPM",
                                tr("Range"), app.backfire.last_min_rpm, app.backfire.last_max_rpm,
                            ))
                            .size(11.0)
                            .color(Color32::GRAY),
                        );
                    } else {
                        crate::theme::slider_row(ui, tr("Min RPM:"), &mut app.config.backfire_min_rpm, 0.0..=20000.0, 50.0, 0, "");
                        crate::theme::slider_row(ui, tr("Max RPM:"), &mut app.config.backfire_max_rpm, 0.0..=20000.0, 50.0, 0, "");
                    }
                    crate::theme::slider_row(ui, tr("RPM interval:"), &mut app.config.backfire_interval_rpm, 0.0..=2000.0, 10.0, 0, "");
                });

                // ── Key Press ────────────────────────────────────────────
                crate::theme::card(ui, tr("Key Press"), |ui| {
                    // The mode dropdown sits to the right of the checkbox (in place of a
                    // slider). Use a local for the toggle so the dropdown closure can
                    // still borrow app.config for the mode field.
                    let mut dynamic = app.config.backfire_dynamic_duration;
                    let show_mode = dynamic;
                    crate::theme::checkbox_row_with(ui, &mut dynamic, tr("Dynamic key press duration"), |ui| {
                        if show_mode {
                            use crate::config::BackfireDynamicMode;
                            egui::ComboBox::from_id_salt("backfire_dyn_mode")
                                .selected_text(match app.config.backfire_dynamic_mode {
                                    BackfireDynamicMode::TimeBased => tr("Time-based"),
                                    BackfireDynamicMode::PacketBased => tr("Packet-based"),
                                })
                                .width(ui.available_width())
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
                        }
                    });
                    app.config.backfire_dynamic_duration = dynamic;

                    if !app.config.backfire_dynamic_duration {
                        crate::theme::slider_row(ui, tr("Key press duration:"), &mut app.config.backfire_accel_time_ms, 1..=50, 1.0, 0, " ms");
                    }
                });

                // ── Conditions ───────────────────────────────────────────
                crate::theme::card(ui, tr("Conditions"), |ui| {
                    crate::theme::checkbox_row(ui, &mut app.config.backfire_disable_standstill, tr("Disable if standing still"));
                    crate::theme::checkbox_row(ui, &mut app.config.backfire_test_mode, tr("Test mode (ignores throttle/RPM conditions)"));
                });
            });
        // Right column intentionally left empty for now.
        let _ = &mut cols[1];
    });
}
