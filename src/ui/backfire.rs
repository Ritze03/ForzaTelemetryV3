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
                ui.label(
                    RichText::new(tr("Triggers Backfire by spamming 'W'")).color(Color32::GRAY),
                );
                ui.add_space(8.0);

                // ── Activation ───────────────────────────────────────────
                crate::theme::card(ui, tr("Activation"), |ui| {
                    crate::theme::styled_checkbox(ui, &mut app.config.backfire_enabled, tr("Enabled"));
                });

                // ── RPM Range ────────────────────────────────────────────
                crate::theme::card(ui, tr("RPM Range"), |ui| {
                    crate::theme::styled_checkbox(ui, &mut app.config.backfire_dynamic_rpm, tr("Dynamic RPM"));
                    if app.config.backfire_dynamic_rpm {
                        slider_row(ui, tr("Min:"), &mut app.config.backfire_dynamic_min_pct, 0.0..=100.0, 1.0, 1, "%");
                        slider_row(ui, tr("Max:"), &mut app.config.backfire_dynamic_max_pct, 0.0..=100.0, 1.0, 1, "%");
                        ui.label(
                            RichText::new(format!(
                                "{}: {:.0} \u{2013} {:.0} RPM",
                                tr("Range"), app.backfire.last_min_rpm, app.backfire.last_max_rpm,
                            ))
                            .size(11.0)
                            .color(Color32::GRAY),
                        );
                    } else {
                        slider_row(ui, tr("Min RPM:"), &mut app.config.backfire_min_rpm, 0.0..=20000.0, 50.0, 0, "");
                        slider_row(ui, tr("Max RPM:"), &mut app.config.backfire_max_rpm, 0.0..=20000.0, 50.0, 0, "");
                    }
                    slider_row(ui, tr("RPM interval:"), &mut app.config.backfire_interval_rpm, 0.0..=2000.0, 10.0, 0, "");
                });

                // ── Key Press ────────────────────────────────────────────
                crate::theme::card(ui, tr("Key Press"), |ui| {
                    crate::theme::styled_checkbox(ui,
                        &mut app.config.backfire_dynamic_duration,
                        tr("Dynamic key press duration"),
                    );
                    if app.config.backfire_dynamic_duration {
                        use crate::config::BackfireDynamicMode;
                        setting_row(ui, tr("Mode:"), |ui| {
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
                        });
                    } else {
                        slider_row(ui, tr("Key press duration:"), &mut app.config.backfire_accel_time_ms, 1..=50, 1.0, 0, " ms");
                    }
                });

                // ── Conditions ───────────────────────────────────────────
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
        // Right column intentionally left empty for now.
        let _ = &mut cols[1];
    });
}

/// A slider settings row in the Automatic Gearbox "advanced" style: label in the
/// left half, a slider rail filling the right half with a fixed-width value
/// spinner (~72px, fits "100.0%") pinned to the far right so rows never drift.
fn slider_row<N: egui::emath::Numeric>(
    ui: &mut Ui,
    label: &str,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    step: f64,
    decimals: usize,
    suffix: &str,
) {
    const VALUE_W: f32 = 72.0;
    ui.columns(2, |c| {
        c[0].label(label);
        c[1].horizontal(|ui| {
            let rail = (ui.available_width() - VALUE_W - ui.spacing().item_spacing.x).max(40.0);
            ui.spacing_mut().slider_width = rail;
            ui.add(egui::Slider::new(&mut *value, range.clone()).step_by(step).show_value(false));
            ui.add_sized(
                [VALUE_W, ui.spacing().interact_size.y],
                egui::DragValue::new(&mut *value)
                    .range(range)
                    .speed(step.max(0.01))
                    .fixed_decimals(decimals)
                    .suffix(suffix),
            );
        });
    });
}

/// A settings row with a control (e.g. a combobox) in the right half. The label
/// is vertically centred against the control's row height.
fn setting_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.columns(2, |c| {
        let h = c[0].spacing().interact_size.y;
        c[0].allocate_ui_with_layout(
            egui::vec2(c[0].available_width(), h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| ui.label(label),
        );
        add(&mut c[1]);
    });
}
