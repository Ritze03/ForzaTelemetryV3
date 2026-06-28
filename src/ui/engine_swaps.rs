use egui::{Color32, RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::tr;

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    ui.heading(tr("Engine Swap Reference"));
    ui.label(
        RichText::new(tr("Display-only reference table. All engines available in Forza Horizon 6."))
            .color(Color32::GRAY),
    );
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label(format!("{} {}", crate::icons::SEARCH, tr("Search:")));
        ui.text_edit_singleline(&mut app.engine_search);
        if ui.small_button(crate::icons::TIMES).clicked() {
            app.engine_search.clear();
        }
        ui.label(
            RichText::new(format!(
                "{} {}",
                filtered_count(app), tr("engines")
            ))
            .color(Color32::GRAY),
        );
    });

    ui.add_space(4.0);
    ui.separator();

    let search = app.engine_search.to_lowercase();

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("engine_table")
            .num_columns(4)
            .striped(true)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                // Header
                ui.label(RichText::new(tr("In-Game Label")).strong());
                ui.label(RichText::new(tr("Source Vehicle")).strong());
                ui.label(RichText::new(tr("Engine Name")).strong());
                ui.label(RichText::new(tr("HP")).strong());
                ui.end_row();

                for engine in &app.engines {
                    if !search.is_empty() {
                        let haystack = format!(
                            "{} {} {}",
                            engine.engine_label, engine.source_vehicle, engine.engine_name
                        )
                        .to_lowercase();
                        if !haystack.contains(&search) {
                            continue;
                        }
                    }

                    ui.label(&engine.engine_label);
                    ui.label(
                        RichText::new(&engine.source_vehicle)
                            .color(Color32::GRAY),
                    );
                    ui.label(&engine.engine_name);
                    ui.label(
                        RichText::new(format!("{}", engine.horsepower))
                            .color(Color32::from_rgb(255, 200, 60)),
                    );
                    ui.end_row();
                }
            });
    });
}

fn filtered_count(app: &ForzaApp) -> usize {
    if app.engine_search.is_empty() {
        return app.engines.len();
    }
    let search = app.engine_search.to_lowercase();
    app.engines
        .iter()
        .filter(|e| {
            format!("{} {} {}", e.engine_label, e.source_vehicle, e.engine_name)
                .to_lowercase()
                .contains(&search)
        })
        .count()
}
