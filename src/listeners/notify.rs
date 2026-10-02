//! D26 HUD notifications, listener side: the [`Notifier`] queue and the change watcher.
//!
//! Events come from two places: hotkeys (handled on the listener thread) and UI settings
//! (the Gearbox tab, Mini-Settings), which reach this thread through the per-frame config
//! push. Rather than hooking each source, [`Notifier::watch`] diffs the relevant state once
//! per loop pass, so every route to the same change (G key, tab toggle, profile load, the
//! automatic switch to Race in a race) produces the same message exactly once. Calibration
//! *start* has no state that distinguishes "reset" from "never started", so those are pushed
//! explicitly from the reset sites in `worker.rs`.
//!
//! The queue is plain and bounded; the snapshot carries a copy and the HUD
//! (`hud::notify`) decides what is still alive.

use crate::config::{AppConfig, GearboxMode, OverlayConfig};
use crate::i18n::tr;
use crate::overlay::snapshot::{NotifKind, Notification};

/// Entries kept in the queue (the HUD draws at most `hud::notify::MAX_VISIBLE`).
const KEEP: usize = 8;
/// Drop entries older than this, seconds (a bit beyond the HUD's `TTL_SECS`).
const PRUNE_AFTER: f64 = 4.0;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    GearboxToggle(bool),
    GearboxMode(GearboxMode),
    Backfire(bool),
    CalibrationStarted,
    /// Calibrated max rpm.
    CalibrationDone(f32),
}

impl Event {
    /// Whether the user has this event type switched on (the master switch is checked apart).
    fn enabled(self, c: &OverlayConfig) -> bool {
        match self {
            Event::GearboxToggle(_) => c.notif_gearbox_toggle,
            Event::GearboxMode(_) => c.notif_gearbox_mode,
            Event::Backfire(_) => c.notif_backfire,
            Event::CalibrationStarted | Event::CalibrationDone(_) => c.notif_calibration,
        }
    }

    /// Message text (translated now) and dot colour.
    pub fn message(self) -> (String, NotifKind) {
        let on_off = |name: &'static str, on: bool| {
            (format!("{}: {}", tr(name), if on { tr("ON") } else { tr("OFF") }), if on { NotifKind::On } else { NotifKind::Off })
        };
        match self {
            Event::GearboxToggle(on) => on_off("Gearbox", on),
            Event::Backfire(on) => on_off("Backfire", on),
            Event::GearboxMode(m) => (format!("{}: {}", tr("Gearbox mode"), m.label()), NotifKind::Info),
            Event::CalibrationStarted => (tr("Calibration started").to_string(), NotifKind::Info),
            Event::CalibrationDone(rpm) => (format!("{}: {} rpm", tr("Calibration done"), rpm.round() as i64), NotifKind::On),
        }
    }
}

/// The state [`Notifier::watch`] compares against.
#[derive(Clone, Copy, PartialEq)]
struct Watched {
    dsg_enabled: bool,
    backfire_enabled: bool,
    mode: GearboxMode,
    engaged: bool,
}

#[derive(Default)]
pub struct Notifier {
    items: Vec<Notification>,
    next_id: u64,
    prev: Option<Watched>,
    /// Something was pushed since the last [`Self::take_new`] (forces a snapshot publish).
    fresh: bool,
}

impl Notifier {
    /// Queue `ev` if notifications and this event type are on and the overlay is enabled.
    pub fn push(&mut self, cfg: &OverlayConfig, ev: Event, now: f64) {
        if !cfg.enabled || !cfg.notif_on || !ev.enabled(cfg) {
            return;
        }
        let (text, kind) = ev.message();
        self.next_id += 1;
        self.items.retain(|n| now - n.created < PRUNE_AFTER);
        if self.items.len() >= KEEP {
            self.items.remove(0);
        }
        self.items.push(Notification { id: self.next_id, text, kind, created: now });
        self.fresh = true;
    }

    /// Diff the watched state against the last pass and queue what changed. The first call
    /// only records the baseline (starting the app announces nothing). `engaged` = the
    /// gearbox has a usable calibration; `in_race` = a real race (race position ≠ 0).
    pub fn watch(&mut self, app: &AppConfig, in_race: bool, engaged: bool, max_rpm: f32, now: f64) {
        let cur = Watched {
            dsg_enabled: app.dsg_enabled,
            backfire_enabled: app.backfire_enabled,
            mode: app.dsg_effective_mode(in_race),
            engaged,
        };
        if let Some(prev) = self.prev.replace(cur) {
            let o = &app.overlay;
            if cur.dsg_enabled != prev.dsg_enabled {
                self.push(o, Event::GearboxToggle(cur.dsg_enabled), now);
            } else if cur.dsg_enabled && cur.mode != prev.mode {
                // Not in the same pass as the toggle: switching on already says "ON".
                self.push(o, Event::GearboxMode(cur.mode), now);
            }
            if cur.backfire_enabled != prev.backfire_enabled {
                self.push(o, Event::Backfire(cur.backfire_enabled), now);
            }
            if cur.engaged && !prev.engaged {
                self.push(o, Event::CalibrationDone(max_rpm), now);
            }
        }
    }

    /// True once after something new was queued.
    pub fn take_new(&mut self) -> bool {
        std::mem::take(&mut self.fresh)
    }

    /// The queue, oldest first, for the snapshot.
    pub fn items(&self) -> &[Notification] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AppConfig {
        let mut c = AppConfig::default();
        c.overlay.enabled = true;
        c.dsg_enabled = false;
        c.backfire_enabled = false;
        c.dsg_auto_race_mode = true;
        c.dsg_gearbox_mode = GearboxMode::Sport;
        c
    }

    fn texts(n: &Notifier) -> Vec<&str> {
        n.items().iter().map(|x| x.text.as_str()).collect()
    }

    #[test]
    fn first_pass_is_a_baseline_only() {
        let mut n = Notifier::default();
        n.watch(&cfg(), false, false, 0.0, 1.0);
        assert!(n.items().is_empty());
        assert!(!n.take_new());
    }

    #[test]
    fn toggles_and_modes_each_announce_once() {
        let mut n = Notifier::default();
        let mut c = cfg();
        c.dsg_enabled = true;
        n.watch(&c, false, false, 0.0, 1.0);
        c.dsg_enabled = false;
        n.watch(&c, false, false, 0.0, 2.0);
        n.watch(&c, false, false, 0.0, 2.1); // unchanged: nothing
        c.dsg_enabled = true;
        c.backfire_enabled = true;
        n.watch(&c, false, false, 0.0, 3.0);
        assert_eq!(texts(&n), ["Gearbox: OFF", "Gearbox: ON", "Backfire: ON"]);
        assert!(n.take_new() && !n.take_new());
        // Manual mode change while on, then the automatic switch to Race in a race and back.
        c.dsg_gearbox_mode = GearboxMode::Street;
        n.watch(&c, false, false, 0.0, 3.1);
        n.watch(&c, true, false, 0.0, 3.2);
        n.watch(&c, false, false, 0.0, 3.3);
        let t = texts(&n);
        assert_eq!(&t[3..], ["Gearbox mode: Street", "Gearbox mode: Race", "Gearbox mode: Street"]);
    }

    #[test]
    fn mode_changes_are_silent_while_the_gearbox_is_off() {
        let mut n = Notifier::default();
        let mut c = cfg();
        n.watch(&c, false, false, 0.0, 1.0);
        c.dsg_gearbox_mode = GearboxMode::Race;
        n.watch(&c, false, false, 0.0, 2.0);
        assert!(n.items().is_empty());
    }

    #[test]
    fn calibration_done_on_engage_and_toggles_respect_config() {
        let mut n = Notifier::default();
        let mut c = cfg();
        n.watch(&c, false, false, 0.0, 1.0);
        n.watch(&c, false, true, 8499.6, 2.0);
        assert_eq!(texts(&n), ["Calibration done: 8500 rpm"]);
        n.push(&c.overlay, Event::CalibrationStarted, 3.0);
        assert_eq!(n.items().len(), 2);
        c.overlay.notif_calibration = false;
        n.push(&c.overlay, Event::CalibrationStarted, 4.0);
        c.overlay.notif_calibration = true;
        c.overlay.notif_on = false;
        n.push(&c.overlay, Event::CalibrationStarted, 4.0);
        c.overlay.notif_on = true;
        c.overlay.enabled = false;
        n.push(&c.overlay, Event::CalibrationStarted, 4.0);
        assert_eq!(n.items().len(), 2);
    }

    #[test]
    fn queue_is_bounded_and_pruned() {
        let mut n = Notifier::default();
        let c = cfg();
        for i in 0..20 {
            n.push(&c.overlay, Event::CalibrationStarted, 1.0 + i as f64 * 0.01);
        }
        assert_eq!(n.items().len(), KEEP);
        n.push(&c.overlay, Event::CalibrationStarted, 100.0);
        assert_eq!(n.items().len(), 1);
    }
}
