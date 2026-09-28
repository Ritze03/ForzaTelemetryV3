//! Debug tab: every field of the latest received `ForzaPacket`, raw, as name → value.
//!
//! The field list comes from the packet's derived `{:#?}` output (one `name: value,` line
//! per field), so it can't drift from `packet.rs`. Why not a hand-written list: ~85 fields
//! that would go stale the first time the struct changes.

use crate::i18n::tr;
use crate::icons;
use crate::packet::ForzaPacket;

/// `(field name, formatted value)` for every packet field, in declaration order.
/// Floats get a fixed 3 decimals; integers print as-is.
pub fn fields(pkt: &ForzaPacket) -> Vec<(String, String)> {
    format!("{pkt:#?}")
        .lines()
        .filter_map(|line| {
            let (name, value) = line.trim().trim_end_matches(',').split_once(": ")?;
            let is_float = value.contains(['.', 'e', 'N', 'i']); // 1.5, 1e-7, NaN, inf
            let value = match value.parse::<f32>() {
                Ok(v) if is_float => format!("{v:.3}"),
                _ => value.to_string(),
            };
            Some((name.to_string(), value))
        })
        .collect()
}

pub fn show(ui: &mut egui::Ui, app: &mut crate::app::ForzaApp) {
    ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
    egui::ScrollArea::vertical().show(ui, |ui| {
        crate::theme::card(ui, tr("Raw Telemetry"), |ui| {
            let Some(pkt) = &app.telemetry.latest else {
                ui.label(egui::RichText::new(tr("No telemetry yet")).color(crate::theme::TEXT_DIM));
                return;
            };
            let rows = fields(pkt);
            if ui.button(format!("{}  {}", icons::COPY, tr("Copy"))).clicked() {
                let dump: Vec<String> = rows.iter().map(|(n, v)| format!("{n}: {v}")).collect();
                ui.ctx().copy_text(dump.join("\n"));
            }
            ui.add_space(4.0);
            egui::Grid::new("debug_raw_telemetry")
                .num_columns(2)
                .striped(true)
                .spacing([24.0, 2.0])
                .show(ui, |ui| {
                    for (name, value) in &rows {
                        ui.label(egui::RichText::new(name).monospace().color(crate::theme::TEXT_DIM));
                        ui.label(egui::RichText::new(value).monospace().color(crate::theme::TEXT));
                        ui.end_row();
                    }
                });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_cover_every_packet_field_with_float_precision() {
        let pkt = ForzaPacket {
            is_race_on: 1,
            current_engine_rpm: 1234.567,
            gear: 3,
            ..Default::default()
        };
        let rows = fields(&pkt);
        // One row per field: the Debug dump has a line per field plus the two brace lines.
        assert_eq!(rows.len(), format!("{pkt:#?}").lines().count() - 2);
        let get = |n: &str| rows.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        assert_eq!(get("is_race_on"), Some("1"));
        assert_eq!(get("current_engine_rpm"), Some("1234.567"));
        assert_eq!(get("gear"), Some("3"));
        assert_eq!(get("yaw"), Some("0.000"));
    }
}
