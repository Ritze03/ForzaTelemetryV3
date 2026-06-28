use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::tr;

pub fn show_backfire(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("backfire_scroll")
        .show(ui, |ui| {
            ui.heading(tr("Backfire"));
            ui.label(
                RichText::new(tr("Triggers Backfire by spamming 'W'"))
                    .color(Color32::GRAY),
            );
            ui.add_space(8.0);

            ui.checkbox(&mut app.config.backfire_enabled, tr("Enabled"));
            ui.add_space(4.0);

            ui.checkbox(&mut app.config.backfire_dynamic_rpm, tr("Dynamic RPM"));
            if app.config.backfire_dynamic_rpm {
                ui.horizontal(|ui| {
                    ui.label(tr("Min:"));
                    ui.add(
                        egui::Slider::new(&mut app.config.backfire_dynamic_min_pct, 0.0..=100.0)
                            .suffix("%")
                            .step_by(1.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Max:"));
                    ui.add(
                        egui::Slider::new(&mut app.config.backfire_dynamic_max_pct, 0.0..=100.0)
                            .suffix("%")
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
                    ui.add(
                        egui::DragValue::new(&mut app.config.backfire_min_rpm)
                            .range(0.0..=20000.0)
                            .speed(50.0),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Max RPM:"));
                    ui.add(
                        egui::DragValue::new(&mut app.config.backfire_max_rpm)
                            .range(0.0..=20000.0)
                            .speed(50.0),
                    );
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(tr("RPM interval:"));
                ui.add(
                    egui::DragValue::new(&mut app.config.backfire_interval_rpm)
                        .range(0.0..=2000.0)
                        .speed(10.0),
                );
            });
            ui.horizontal(|ui| {
                ui.label(tr("Key press duration:"));
                ui.add(
                    egui::DragValue::new(&mut app.config.backfire_accel_time_ms)
                        .range(1..=50)
                        .speed(1.0)
                        .suffix(" ms"),
                );
            });
            ui.add_space(4.0);
            ui.checkbox(
                &mut app.config.backfire_disable_standstill,
                tr("Disable if standing still"),
            );
            ui.checkbox(
                &mut app.config.backfire_test_mode,
                tr("Test mode (ignores throttle/RPM conditions)"),
            );
        });
}
