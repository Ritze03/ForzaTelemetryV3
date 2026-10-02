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
    fn key(&self) -> Option<&str> {
        match self {
            Self::NotStarted => None,
            Self::Loading(l, _) | Self::Ready(l, _) | Self::Failed(l, _) => Some(l),
        }
    }

    /// Start (or restart on a UI-language or configured-install change) the load, and collect a
    /// finished one. `dir` is `AppConfig::fh6_install_dir` (empty = auto-detect).
    fn poll(&mut self, ctx: &egui::Context, dir: &str) {
        let want = crate::i18n::language_code();
        let key = format!("{want}\n{dir}");
        if self.key() != Some(&key) {
            let (tx, rx) = std::sync::mpsc::channel();
            let dir = dir.trim().to_string();
            std::thread::spawn(move || {
                let over = (!dir.is_empty()).then(|| std::path::PathBuf::from(dir));
                let _ = tx.send(crate::gamedata::cars::CarDb::load_from(over.as_deref(), want));
            });
            *self = Self::Loading(key, rx);
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

/// Status dot (green pass / red fail). `●` renders in the font; emoji don't.
fn dot(ui: &mut egui::Ui, ok: bool) {
    ui.label(egui::RichText::new("\u{25CF}").color(if ok { crate::theme::GOOD } else { crate::theme::DANGER }));
}

/// One check: dot, name, raw value + threshold.
fn check_row(ui: &mut egui::Ui, ok: bool, name: &str, detail: String) {
    dot(ui, ok);
    ui.label(egui::RichText::new(name).color(crate::theme::TEXT_DIM));
    ui.label(egui::RichText::new(detail).monospace().color(crate::theme::TEXT));
    ui.end_row();
}

/// A per-wheel check on one row: the dot is "all four pass", each wheel's value is coloured by
/// its own result. Wheel order FL FR RL RR.
fn wheels_row(ui: &mut egui::Ui, ok: [bool; 4], name: &str, vals: [f32; 4], threshold: &str) {
    dot(ui, ok.iter().all(|&b| b));
    ui.label(egui::RichText::new(name).color(crate::theme::TEXT_DIM));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for ((label, v), ok) in ["FL", "FR", "RL", "RR"].into_iter().zip(vals).zip(ok) {
            let col = if ok { crate::theme::GOOD } else { crate::theme::DANGER };
            ui.label(egui::RichText::new(format!("{label} {v:.2}")).monospace().color(col));
        }
        ui.label(egui::RichText::new(threshold).monospace().color(crate::theme::TEXT_DIM));
    });
    ui.end_row();
}

/// Group heading with its overall "is it happening right now" dot.
fn group_heading(ui: &mut egui::Ui, title: &str, ok: bool, status: &str) {
    ui.horizontal(|ui| {
        dot(ui, ok);
        ui.label(egui::RichText::new(title).strong().color(crate::theme::TEXT));
        ui.label(egui::RichText::new(status).color(crate::theme::TEXT_DIM));
    });
}

/// Every condition behind RPM calibration, one row each, from `ForzaApp::dsg.calib` (built by the
/// same pure fns in `listeners/calib.rs` that gate the real logic). Why: "calibration doesn't work
/// for this car" must show exactly which check fails.
fn calibration_section(ui: &mut egui::Ui, app: &crate::app::ForzaApp) {
    let dim = |t: &str| egui::RichText::new(t.to_string()).color(crate::theme::TEXT_DIM);
    ui.label(crate::theme::section_label(tr("Calibration checks")));
    if app.telemetry.latest.is_none() {
        ui.label(dim(tr("No telemetry yet")));
        return;
    }
    use crate::listeners::calib::{CALIB_MIN_KMH, CALIB_RPM_FRAC, CALIB_SLIP, CALIB_STRAIGHT_FRAC, CALIB_SUSPENSION, MAX_RPM_SLIP};
    let c = &app.dsg.calib;
    let r = &c.raw;
    let yes_no = |b: bool| if b { tr("yes") } else { tr("no") };
    let grid = |id: &'static str| egui::Grid::new(id).num_columns(3).striped(true).spacing([12.0, 2.0]);

    // 1. Max RPM capture
    let m = &c.max_rpm;
    group_heading(ui, tr("1. Max RPM capture"), m.all(), if m.all() { tr("capturing now") } else { tr("not capturing") });
    grid("debug_calib_maxrpm").show(ui, |ui| {
        check_row(ui, m.race_on, tr("Race on"), "is_race_on != 0".into());
        check_row(ui, m.power_positive, tr("Engine power"), format!("power {:.0} > 0", r.power));
        check_row(ui, m.handbrake_off, tr("Handbrake released"), format!("hand_brake {} = 0", r.hand_brake));
        wheels_row(ui, m.slip_ok, tr("Tyre slip"), r.slip, &format!("|slip| <= {MAX_RPM_SLIP}"));
        let cal = app.dynamic_max_rpm;
        check_row(ui, cal > 0.0, tr("Max RPM so far"), if cal > 0.0 { format!("{cal:.0}") } else { tr("none yet").into() });
    });

    // 2. Calibrated (engage)
    let e = &c.engage;
    ui.add_space(4.0);
    group_heading(ui, tr("2. Calibrated (box engages)"), e.engaged, if e.engaged { tr("engaged") } else { tr("not engaged yet") });
    grid("debug_calib_engage").show(ui, |ui| {
        check_row(ui, e.engaged, tr("Engaged"), yes_no(e.engaged).into());
        check_row(ui, e.prev_ok, tr("Previous forward gear"), format!("{} in 1..=9", e.prev_gear));
        check_row(ui, e.gear_ok, tr("Current gear"), format!("{} in 2..=10", e.gear));
        check_row(ui, e.upshift, tr("Upshift"), format!("{} > {}", e.gear, e.prev_gear));
    });

    // 3. Gear-map sample
    let g = &c.gear_map;
    ui.add_space(4.0);
    group_heading(ui, tr("3. Gear-map sample"), g.all(), if g.all() { tr("sampling now") } else { tr("not sampling") });
    grid("debug_calib_gearmap").show(ui, |ui| {
        check_row(ui, g.race_on, tr("Race on"), "is_race_on != 0".into());
        check_row(ui, g.in_gear, tr("In a forward gear"), format!("{} in 1..=10", e.gear));
        check_row(
            ui,
            g.redline_known,
            tr("Redline known"),
            if r.max_rpm > 0.0 { format!("{:.0} > 0", r.max_rpm) } else { format!("0 > 0 ({})", tr("capture it first")) },
        );
        check_row(ui, g.moving, tr("Moving"), format!("{:.1} km/h > {CALIB_MIN_KMH}", r.kmh));
        check_row(
            ui,
            g.rpm_high,
            tr("RPM high enough"),
            format!("{:.0} / {:.0} = {:.0}% >= {:.0}%", r.rpm, r.max_rpm, r.rpm_ratio * 100.0, CALIB_RPM_FRAC * 100.0),
        );
        wheels_row(ui, g.slip_ok, tr("Tyre slip"), r.slip, &format!("|slip| < {CALIB_SLIP}"));
        wheels_row(ui, g.springs_ok, tr("Suspension travel"), r.suspension, &format!(">= {CALIB_SUSPENSION}"));
        check_row(
            ui,
            g.straight,
            tr("Moving straight"),
            format!("vel_z/speed {:.2} >= {CALIB_STRAIGHT_FRAC}", r.straight_ratio),
        );
    });

    // Gear map: what the sampling has produced so far.
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tr("Gear map")).strong().color(crate::theme::TEXT));
    let gears: Vec<usize> = (1..=10)
        .filter(|&i| app.dsg.gear_sample_counts[i] > 0 || app.dsg.gear_redline_speeds[i] > 0.0)
        .collect();
    if gears.is_empty() {
        ui.label(dim(tr("no samples yet")));
    } else {
        egui::Grid::new("debug_calib_gears").num_columns(3).striped(true).spacing([16.0, 2.0]).show(ui, |ui| {
            ui.label(dim(tr("Gear")));
            ui.label(dim(tr("Samples")));
            ui.label(dim(tr("Redline speed")));
            ui.end_row();
            for i in gears {
                let mono = |t: String| egui::RichText::new(t).monospace().color(crate::theme::TEXT);
                ui.label(mono(i.to_string()));
                ui.label(mono(format!("{}/10", app.dsg.gear_sample_counts[i])));
                let v = app.dsg.gear_redline_speeds[i];
                ui.label(mono(if v > 0.0 { format!("{v:.0} km/h") } else { "-".into() }));
                ui.end_row();
            }
        });
    }
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
        calibration_section(ui, app);
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
    app.debug_cars.poll(ui.ctx(), &app.config.fh6_install_dir);
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
