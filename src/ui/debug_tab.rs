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

/// Background-loaded car name DB (`gamedata::cars`). `CarDb::load` blocks ~0.4-1 s cold, so it
/// runs on a thread and is polled each frame (same mpsc pattern as the minimap loader).
#[derive(Default)]
pub enum CarDbState {
    #[default]
    NotStarted,
    Loading(String, std::sync::mpsc::Receiver<Result<crate::gamedata::cars::CarDb, String>>),
    Ready(String, crate::gamedata::cars::CarDb),
    Failed(String, String),
}

impl CarDbState {
    fn lang(&self) -> Option<&str> {
        match self {
            Self::NotStarted => None,
            Self::Loading(l, _) | Self::Ready(l, _) | Self::Failed(l, _) => Some(l),
        }
    }

    /// Start (or restart on a UI-language change) the load, and collect a finished one.
    fn poll(&mut self, ctx: &egui::Context) {
        let want = crate::i18n::language_code();
        if self.lang() != Some(want) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(crate::gamedata::cars::CarDb::load(want));
            });
            *self = Self::Loading(want.to_string(), rx);
        }
        if let Self::Loading(l, rx) = self {
            match rx.try_recv() {
                Ok(Ok(db)) => *self = Self::Ready(std::mem::take(l), db),
                Ok(Err(e)) => *self = Self::Failed(std::mem::take(l), e),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    *self = Self::Failed(std::mem::take(l), "loader thread died".into());
                }
            }
        }
    }
}

/// Why the HUD considers the game paused: one line per rule that fired (empty = not paused).
fn pause_reasons(pkt: &ForzaPacket, experimental: bool) -> Vec<&'static str> {
    use crate::listeners::hud::{garage_paused, hud_paused};
    let mut r = Vec::new();
    if pkt.is_race_on == 0 {
        r.push(tr("Race off (is_race_on = 0)"));
    }
    if pkt.engine_max_rpm <= 0.0 {
        r.push(tr("No max RPM (engine_max_rpm <= 0)"));
    }
    if pkt.yaw == 0.0 && pkt.pitch == 0.0 && pkt.roll == 0.0 {
        r.push(tr("Zero attitude (yaw = pitch = roll = 0)"));
    }
    if experimental && garage_paused(pkt).is_some() {
        r.push(tr("Garage rule (level + handbrake 255 + standing still)"));
    }
    debug_assert_eq!(!r.is_empty(), hud_paused(pkt, experimental));
    r
}

fn row(ui: &mut egui::Ui, name: &str, value: impl Into<String>) {
    ui.label(egui::RichText::new(name).color(crate::theme::TEXT_DIM));
    ui.label(egui::RichText::new(value.into()).monospace().color(crate::theme::TEXT));
    ui.end_row();
}

fn raw_card(ui: &mut egui::Ui, app: &crate::app::ForzaApp) {
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
}

fn derived_card(ui: &mut egui::Ui, app: &crate::app::ForzaApp) {
    let dim = |t: &str| egui::RichText::new(t.to_string()).color(crate::theme::TEXT_DIM);
    crate::theme::card(ui, tr("Derived from telemetry"), |ui| {
        let yes_no = |b: bool| if b { tr("yes") } else { tr("no") };
        egui::Grid::new("debug_derived")
            .num_columns(2)
            .striped(true)
            .spacing([24.0, 2.0])
            .show(ui, |ui| {
                let exp = app.config.experimental_pause_detection;
                row(ui, tr("Experimental pause detection"), yes_no(exp));
                if let Some(pkt) = &app.telemetry.latest {
                    let reasons = pause_reasons(pkt, exp);
                    row(ui, tr("Paused"), yes_no(!reasons.is_empty()));
                    row(ui, tr("Pause reason"), if reasons.is_empty() { "-".into() } else { reasons.join("\n") });
                    let in_race = pkt.race_position != 0;
                    row(ui, tr("In race (race position set)"), yes_no(in_race));
                    row(ui, tr("Gearbox: selected mode"), app.config.dsg_gearbox_mode.label());
                    row(ui, tr("Gearbox: effective mode"), app.config.dsg_effective_mode(in_race).label());
                    let in_drift = app.hud_mode == crate::overlay::snapshot::HudMode::Drift;
                    row(ui, tr("HUD mode"), if in_drift { tr("Drift") } else { tr("Race / free roam") });
                    row(
                        ui,
                        tr("Gearbox: resolved"),
                        app.config.dsg_resolved_mode(in_race, in_drift).map_or(tr("off"), |m| m.label()),
                    );
                } else {
                    row(ui, tr("Paused"), "-");
                }
                let cal = app.dynamic_max_rpm;
                row(ui, tr("Calibrated max RPM"), if cal > 0.0 { format!("{cal:.0}") } else { tr("not calibrated").into() });
                row(ui, tr("Season (wall clock)"), crate::minimap::season_display_name(crate::minimap::current_season()));
            });
        ui.add_space(6.0);
        ui.label(crate::theme::section_label(tr("Car")));
        match &app.debug_cars {
            CarDbState::NotStarted | CarDbState::Loading(..) => {
                ui.label(dim(tr("Loading car names from the game install...")));
            }
            CarDbState::Failed(_, e) => {
                ui.label(dim(tr("Car names unavailable (FH6 install not found?)")));
                ui.label(dim(e));
            }
            CarDbState::Ready(_, db) => {
                let Some(pkt) = &app.telemetry.latest else {
                    ui.label(dim(tr("No telemetry yet")));
                    return;
                };
                let ord = pkt.car_ordinal as u32;
                egui::Grid::new("debug_car").num_columns(2).striped(true).spacing([24.0, 2.0]).show(ui, |ui| {
                    row(ui, tr("Ordinal"), ord.to_string());
                    row(ui, tr("Language"), format!("{} ({})", db.lang, if db.from_cache { tr("cache") } else { tr("install scan") }));
                    row(ui, tr("Install"), format!("{} ({} {})", db.media.display(), db.len(), tr("cars")));
                    match db.lookup(ord) {
                        Some(c) => {
                            row(ui, tr("Name"), c.display.clone());
                            row(ui, tr("Make"), c.make.clone().unwrap_or_else(|| tr("unknown").to_string()));
                            row(ui, tr("Media name"), c.media_name.clone());
                        }
                        None => row(ui, tr("Name"), tr("not in the car database")),
                    }
                });
            }
        }
    });
}

pub fn show(ui: &mut egui::Ui, app: &mut crate::app::ForzaApp) {
    app.debug_cars.poll(ui.ctx());
    ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
    let app = &*app;
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |cols| {
            raw_card(&mut cols[0], app);
            derived_card(&mut cols[1], app);
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
