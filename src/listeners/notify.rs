//! D26 HUD notifications, listener side: the [`Notifier`] queue and the change watcher.
//!
//! Events come from two places: hotkeys (handled on the listener thread) and UI settings
//! (the Gearbox tab, Mini-Settings), which reach this thread through the per-frame config
//! push. Rather than hooking each source, [`Notifier::watch`] diffs the relevant state once
//! per loop pass, so every route to the same change (G key, tab toggle, profile load, the
//! automatic switch to Race in a race) produces the same message exactly once. Calibration
//! *start* has no state that distinguishes "reset" from "never started", so those are pushed
//! explicitly from the reset sites in `worker.rs`, and "Calibration started" is also a level
//! rule (an uncalibrated car being driven, once per episode; see [`Notifier::watch`]).
//!
//! Every event belongs to a [`NotifGroup`]; pushing one replaces the live pill of its group
//! in place (same slot, new text, TTL restarted) rather than stacking, so rapid same-type
//! messages (Started, Shift, Done; ON then OFF) never pile up.
//!
//! The queue is plain and bounded; the snapshot carries a copy and the HUD
//! (`hud::notify`) decides what is still alive.

use crate::config::{AppConfig, GearboxMode, OverlayConfig};
use crate::hud::notify as hud;
use crate::i18n::tr;
use crate::overlay::snapshot::{NotifGroup, NotifKind, Notification};

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
    /// First gear-map data arrived while uncalibrated: the cue to rev out and shift.
    ShiftAtRedline,
    /// Calibrated max rpm.
    CalibrationDone(f32),
}

impl Event {
    /// Which pill this event writes to (a new one replaces the live pill of the same group).
    /// *Why Gearbox and GearboxMode are two groups:* "ON/OFF" and "mode: Race" are different
    /// facts; merging them would let a mode change wipe the ON/OFF state the user just saw.
    pub fn group(self) -> NotifGroup {
        match self {
            Event::GearboxToggle(_) => NotifGroup::Gearbox,
            Event::GearboxMode(_) => NotifGroup::GearboxMode,
            Event::Backfire(_) => NotifGroup::Backfire,
            Event::CalibrationStarted | Event::ShiftAtRedline | Event::CalibrationDone(_) => NotifGroup::Calibration,
        }
    }

    /// Whether the user has this event type switched on (the master switch is checked apart).
    fn enabled(self, c: &OverlayConfig) -> bool {
        match self {
            Event::GearboxToggle(_) => c.notif_gearbox_toggle,
            Event::GearboxMode(_) => c.notif_gearbox_mode,
            Event::Backfire(_) => c.notif_backfire,
            Event::CalibrationStarted | Event::ShiftAtRedline | Event::CalibrationDone(_) => c.notif_calibration,
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
            Event::ShiftAtRedline => (tr("Shift at redline").to_string(), NotifKind::Hint),
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
    /// "Shift at redline" was already handled this calibration cycle. Set when it fires (or
    /// would have, with its toggle off) and cleared by every route into a new cycle: an
    /// explicit [`Event::CalibrationStarted`], [`Self::rearm_shift_hint`], the gear-1 map
    /// emptying, or the box engaging.
    shift_hinted: bool,
    /// The car "Calibration started" was already announced for in the current uncalibrated
    /// episode. Cleared when the box is calibrated, and by an explicit Clear RPM calibration
    /// that is not itself seen (nothing is being driven), so the next drive announces it.
    started_for: Option<i32>,
    /// The car being driven as of the last [`Self::watch`] (None = paused / no car).
    driving: Option<i32>,
    /// Something was pushed since the last [`Self::take_new`] (forces a snapshot publish).
    fresh: bool,
}

impl Notifier {
    /// Queue `ev` if notifications and this event type are on and the overlay is enabled.
    /// A live pill of the same group is rewritten in place instead (see the module docs).
    pub fn push(&mut self, cfg: &OverlayConfig, ev: Event, now: f64) {
        if ev == Event::CalibrationStarted {
            self.shift_hinted = false; // a new cycle, whatever the toggles say
            // Told if it can be seen right now; otherwise the level rule fires on the next drive.
            self.started_for = self.driving;
        }
        if !cfg.enabled || !cfg.notif_on || !ev.enabled(cfg) {
            return;
        }
        let (text, kind) = ev.message();
        let group = ev.group();
        self.fresh = true;
        if let Some(n) = self.items.iter_mut().find(|n| n.group == group && hud::is_live(n, now)) {
            // Same slot (id) so the stack does not move; text/kind swap, TTL restarts. The
            // fade-in is backdated by the current opacity so it neither blinks nor re-fades.
            n.born = hud::rewritten_born(n, now);
            n.text = text;
            n.kind = kind;
            n.created = now;
            return;
        }
        self.next_id += 1;
        self.items.retain(|n| now - n.created < PRUNE_AFTER);
        if self.items.len() >= KEEP {
            self.items.remove(0);
        }
        self.items.push(Notification { id: self.next_id, group, text, kind, created: now, born: now });
    }

    /// Start a new "Shift at redline" cycle (the gear map was just cleared on purpose).
    pub fn rearm_shift_hint(&mut self) {
        self.shift_hinted = false;
    }

    /// Diff the watched state against the last pass and queue what changed. The first call
    /// only records the baseline (starting the app announces nothing, the shift hint included).
    /// `engaged` = the gearbox has a usable calibration; `gear1_mapped` = gear 1's gear-map entry has data
    /// (`gear_redline_speeds[1] > 0`); `in_race` = a real race (race position ≠ 0);
    /// `driving` = the car ordinal while the game is running and not paused
    /// (`!hud_paused`, which includes race-on), else None.
    ///
    /// **Calibration started** is a level rule: driving a car that is not calibrated
    /// (`!engaged`) announces it once per episode (`started_for`). Episodes restart on a car
    /// change, once calibrated, or via the explicit Clear RPM calibration push. The gearbox
    /// switch is deliberately not part of it: calibration runs, and feeds the HUD's shift
    /// cue, whether or not the box is on.
    #[allow(clippy::too_many_arguments)]
    pub fn watch(
        &mut self,
        app: &AppConfig,
        in_race: bool,
        engaged: bool,
        gear1_mapped: bool,
        max_rpm: f32,
        driving: Option<i32>,
        now: f64,
    ) {
        self.driving = driving;
        let cur = Watched {
            dsg_enabled: app.dsg_enabled,
            backfire_enabled: app.backfire_enabled,
            mode: app.dsg_effective_mode(in_race),
            engaged,
        };
        let baseline = self.prev.is_none();
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
        if engaged {
            self.started_for = None; // calibrated: the next uncalibrated stretch is a new episode
        } else if driving.is_some() && self.started_for != driving {
            self.push(&app.overlay, Event::CalibrationStarted, now); // sets `started_for`
        }
        // A level ("uncalibrated and gear 1 has data, not yet told"), not an edge, so it also
        // covers a map that was already there when a cycle was restarted (Clear RPM
        // calibration keeps the gear map). The baseline pass counts as already told.
        if engaged || !gear1_mapped {
            // Cycle over (calibrated), or the map was emptied (reset / car change): re-arm.
            self.shift_hinted = false;
        } else if !self.shift_hinted {
            self.shift_hinted = true;
            if !baseline {
                self.push(&app.overlay, Event::ShiftAtRedline, now);
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
    use crate::hud::notify::{live, stack_layout, TTL_SECS};
    use egui::vec2;

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

    /// `watch` with only the toggles in play: not driving, nothing calibrated.
    fn idle(n: &mut Notifier, c: &AppConfig, t: f64) {
        n.watch(c, false, false, false, 0.0, None, t);
    }

    #[test]
    fn first_pass_is_a_baseline_only() {
        let mut n = Notifier::default();
        idle(&mut n, &cfg(), 1.0);
        assert!(n.items().is_empty());
        assert!(!n.take_new());
    }

    #[test]
    fn toggles_and_modes_each_announce_once() {
        // Spaced beyond the TTL so replacement does not merge them: this checks the diff.
        let gap = TTL_SECS + 0.5;
        let mut n = Notifier::default();
        let mut c = cfg();
        c.dsg_enabled = true;
        idle(&mut n, &c, 1.0);
        c.dsg_enabled = false;
        idle(&mut n, &c, 1.0 + gap);
        idle(&mut n, &c, 1.0 + gap + 0.1); // unchanged: nothing
        c.dsg_enabled = true;
        c.backfire_enabled = true;
        idle(&mut n, &c, 1.0 + 2.0 * gap);
        assert_eq!(texts(&n), ["Gearbox: OFF", "Gearbox: ON", "Backfire: ON"]);
        assert!(n.take_new() && !n.take_new());
        // Manual mode change while on, then the automatic switch to Race in a race and back.
        let t = 1.0 + 3.0 * gap;
        c.dsg_gearbox_mode = GearboxMode::Street;
        n.watch(&c, false, false, false, 0.0, None, t);
        assert_eq!(n.items().last().unwrap().text, "Gearbox mode: Street");
        n.watch(&c, true, false, false, 0.0, None, t + gap);
        assert_eq!(n.items().last().unwrap().text, "Gearbox mode: Race");
        n.watch(&c, false, false, false, 0.0, None, t + 2.0 * gap);
        assert_eq!(n.items().last().unwrap().text, "Gearbox mode: Street");
    }

    #[test]
    fn mode_changes_are_silent_while_the_gearbox_is_off() {
        let mut n = Notifier::default();
        let mut c = cfg();
        idle(&mut n, &c, 1.0);
        c.dsg_gearbox_mode = GearboxMode::Race;
        idle(&mut n, &c, 2.0);
        assert!(n.items().is_empty());
    }

    #[test]
    fn calibration_done_on_engage_and_toggles_respect_config() {
        let mut n = Notifier::default();
        let mut c = cfg();
        idle(&mut n, &c, 1.0);
        n.watch(&c, false, true, false, 8499.6, None, 2.0);
        assert_eq!(texts(&n), ["Calibration done: 8500 rpm"]);
        // Same group: a later Started replaces the pill.
        n.push(&c.overlay, Event::CalibrationStarted, 3.0);
        assert_eq!(texts(&n), ["Calibration started"]);
        let before = n.items().to_vec();
        c.overlay.notif_calibration = false;
        n.push(&c.overlay, Event::CalibrationStarted, 3.1);
        c.overlay.notif_calibration = true;
        c.overlay.notif_on = false;
        n.push(&c.overlay, Event::CalibrationStarted, 3.2);
        c.overlay.notif_on = true;
        c.overlay.enabled = false;
        n.push(&c.overlay, Event::CalibrationStarted, 3.3);
        assert_eq!(n.items(), &before[..], "disabled pushes change nothing");
    }

    /// Drive the watcher the way the worker does: (engaged, gear 1 has map data, car driven).
    fn step(n: &mut Notifier, c: &AppConfig, engaged: bool, g1: bool, t: f64) {
        n.watch(c, false, engaged, g1, 8500.0, None, t);
    }

    #[test]
    fn calibration_sequence_is_one_pill_that_changes() {
        let mut n = Notifier::default();
        let c = cfg();
        step(&mut n, &c, false, false, 0.1); // baseline
        n.push(&c.overlay, Event::CalibrationStarted, 0.2);
        step(&mut n, &c, false, false, 0.2); // nothing mapped yet: no hint
        assert_eq!(texts(&n), ["Calibration started"]);
        let id = n.items()[0].id;
        step(&mut n, &c, false, true, 0.3); // gear 1 gets its first data
        assert_eq!(texts(&n), ["Shift at redline"]);
        assert_eq!(n.items()[0].kind, NotifKind::Hint);
        for t in 4..10 {
            step(&mut n, &c, false, true, t as f64 / 10.0); // data keeps accumulating: no repeat
        }
        assert_eq!(n.items().len(), 1);
        step(&mut n, &c, true, true, 1.0); // the shift
        assert_eq!(texts(&n), ["Calibration done: 8500 rpm"]);
        assert_eq!(n.items()[0].kind, NotifKind::On);
        assert_eq!(n.items()[0].id, id, "same slot throughout");
        step(&mut n, &c, true, true, 1.1);
        assert_eq!(n.items().len(), 1);
    }

    #[test]
    fn same_group_replaces_in_place_and_other_groups_stack() {
        let mut n = Notifier::default();
        let mut c = cfg();
        idle(&mut n, &c, 1.0);
        // Gearbox ON then OFF quickly: one pill, "OFF".
        c.dsg_enabled = true;
        idle(&mut n, &c, 1.1);
        c.dsg_enabled = false;
        idle(&mut n, &c, 1.3);
        assert_eq!(texts(&n), ["Gearbox: OFF"]);
        assert_eq!(n.items()[0].kind, NotifKind::Off);
        // Backfire and calibration are other groups: they stack.
        c.backfire_enabled = true;
        idle(&mut n, &c, 1.4);
        n.push(&c.overlay, Event::CalibrationStarted, 1.5);
        assert_eq!(texts(&n), ["Gearbox: OFF", "Backfire: ON", "Calibration started"]);
        let ids: Vec<u64> = n.items().iter().map(|x| x.id).collect();
        // Replace the first (oldest) one: the others keep their slots and the stack order.
        c.dsg_enabled = true;
        idle(&mut n, &c, 1.6);
        assert_eq!(texts(&n), ["Gearbox: ON", "Backfire: ON", "Calibration started"]);
        assert_eq!(n.items().iter().map(|x| x.id).collect::<Vec<_>>(), ids);
        let g = &n.items()[0];
        assert_eq!(g.created, 1.6, "TTL restarted");
        // The HUD lists newest slot first; its layout is untouched by replacement.
        let seen: Vec<&str> = live(n.items(), 1.7).iter().map(|x| x.text.as_str()).collect();
        assert_eq!(seen, ["Calibration started", "Backfire: ON", "Gearbox: ON"]);
        let sizes = vec![vec2(100.0, 38.0); 3];
        let r = stack_layout(crate::config::HudCell::TopCenter, &sizes, vec2(1920.0, 1080.0), 40.0, 8.0);
        assert!(r[0].min.y < r[1].min.y && r[1].min.y < r[2].min.y);
    }

    #[test]
    fn replacing_after_the_ttl_makes_a_new_pill() {
        let mut n = Notifier::default();
        let c = cfg();
        n.push(&c.overlay, Event::Backfire(true), 1.0);
        n.push(&c.overlay, Event::Backfire(false), 1.0 + TTL_SECS + 0.1); // old one is gone from the HUD
        assert_eq!(texts(&n), ["Backfire: ON", "Backfire: OFF"]);
        assert!(n.items()[1].id > n.items()[0].id);
    }

    #[test]
    fn a_replacement_restarts_the_ttl_and_continues_the_fade() {
        use crate::hud::notify::alpha_of;
        let mut n = Notifier::default();
        let c = cfg();
        n.push(&c.overlay, Event::Backfire(true), 1.0);
        let late = 1.0 + TTL_SECS - 0.2; // mid fade-out
        let a = alpha_of(&n.items()[0], late);
        assert!(a > 0.0 && a < 1.0);
        n.push(&c.overlay, Event::Backfire(false), late);
        let p = &n.items()[0];
        assert_eq!(p.created, late);
        assert!((alpha_of(p, late) - a).abs() < 1e-3, "no pop");
        assert!(live(n.items(), late + TTL_SECS - 0.05).len() == 1, "alive for a full TTL again");
    }

    #[test]
    fn no_shift_hint_when_calibrated_or_on_the_baseline() {
        let c = cfg();
        // Already calibrated (restored profile / past the shift): never.
        let mut n = Notifier::default();
        step(&mut n, &c, true, false, 0.1);
        step(&mut n, &c, true, true, 0.2);
        assert!(n.items().is_empty());
        // App starts uncalibrated with a map already there: that is not news.
        let mut n = Notifier::default();
        step(&mut n, &c, false, true, 0.1);
        step(&mut n, &c, false, true, 0.2);
        assert!(n.items().is_empty());
    }

    /// How many pushes happened in this step (each sets the fresh flag).
    fn pushed(n: &mut Notifier) -> bool {
        n.take_new()
    }

    #[test]
    fn shift_hint_rearms_on_each_new_cycle() {
        let mut n = Notifier::default();
        let c = cfg();
        step(&mut n, &c, false, false, 0.1);
        step(&mut n, &c, false, true, 0.2);
        assert_eq!(texts(&n), ["Shift at redline"]);
        assert!(pushed(&mut n));
        step(&mut n, &c, false, true, 0.25);
        assert!(!pushed(&mut n), "once per cycle");
        // Car change while uncalibrated: map emptied, new data, new hint.
        step(&mut n, &c, false, false, 0.3);
        step(&mut n, &c, false, true, 0.4);
        assert!(pushed(&mut n));
        // Clear gear map (explicit re-arm, even if the map refills within one pass).
        n.rearm_shift_hint();
        step(&mut n, &c, false, true, 0.5);
        assert!(pushed(&mut n));
        // Calibrated, then Clear RPM calibration keeps the map: Started, then the hint at
        // once (same pill, so only the hint is left showing).
        step(&mut n, &c, true, true, 0.6);
        n.take_new();
        n.push(&c.overlay, Event::CalibrationStarted, 0.7);
        step(&mut n, &c, false, true, 0.7);
        assert_eq!(texts(&n), ["Shift at redline"]);
        n.take_new();
        // Map emptied alone (no explicit call) also re-arms.
        step(&mut n, &c, false, false, 0.8);
        step(&mut n, &c, false, true, 0.9);
        assert!(pushed(&mut n));
    }

    #[test]
    fn shift_hint_respects_the_calibration_toggle() {
        let mut n = Notifier::default();
        let mut c = cfg();
        c.overlay.notif_calibration = false;
        step(&mut n, &c, false, false, 0.1);
        step(&mut n, &c, false, true, 0.2);
        assert!(n.items().is_empty());
        // Toggling on mid-cycle does not retroactively announce it.
        c.overlay.notif_calibration = true;
        step(&mut n, &c, false, true, 0.3);
        assert!(n.items().is_empty());
        // Master switch off too.
        c.overlay.notif_on = false;
        n.push(&c.overlay, Event::CalibrationStarted, 0.4);
        step(&mut n, &c, false, true, 0.4);
        assert!(n.items().is_empty());
    }

    // ── Calibration started: the level rule ──

    /// One pass with a car on screen (`Some`) or paused (`None`), uncalibrated unless `engaged`.
    fn drive(n: &mut Notifier, c: &AppConfig, car: Option<i32>, engaged: bool, t: f64) {
        n.watch(c, false, engaged, false, 8500.0, car, t);
    }

    fn started_count(n: &mut Notifier) -> usize {
        // Pills expire: one pushed since the last call counts as one (fresh flag).
        usize::from(n.take_new())
    }

    #[test]
    fn started_for_the_first_uncalibrated_car_once_driving() {
        let mut n = Notifier::default();
        let c = cfg();
        drive(&mut n, &c, None, false, 0.1); // baseline, in the pause menu
        drive(&mut n, &c, None, false, 0.2);
        assert!(n.items().is_empty(), "paused: nothing");
        drive(&mut n, &c, Some(7), false, 0.3); // unpaused
        assert_eq!(texts(&n), ["Calibration started"]);
        assert_eq!(started_count(&mut n), 1);
        // Keeps driving, then pauses and resumes the same car: no repeat.
        drive(&mut n, &c, Some(7), false, 0.4);
        drive(&mut n, &c, None, false, 5.0);
        drive(&mut n, &c, Some(7), false, 6.0);
        assert_eq!(started_count(&mut n), 0);
        assert_eq!(n.items().len(), 1);
    }

    #[test]
    fn started_when_the_app_starts_mid_drive() {
        let mut n = Notifier::default();
        drive(&mut n, &cfg(), Some(7), false, 0.1); // the baseline pass already drives
        assert_eq!(texts(&n), ["Calibration started"]);
    }

    #[test]
    fn started_again_for_a_car_change_and_after_calibrating() {
        let mut n = Notifier::default();
        let c = cfg();
        drive(&mut n, &c, Some(7), false, 0.1);
        started_count(&mut n);
        drive(&mut n, &c, Some(8), false, 5.0); // car change
        assert_eq!(started_count(&mut n), 1);
        drive(&mut n, &c, Some(8), true, 6.0); // calibrated
        started_count(&mut n);
        drive(&mut n, &c, Some(8), false, 10.0); // uncalibrated again (Clear done elsewhere)
        assert_eq!(started_count(&mut n), 1);
        // Changing to a restored (already calibrated) car: nothing.
        drive(&mut n, &c, Some(9), true, 12.0);
        started_count(&mut n);
        drive(&mut n, &c, Some(9), true, 13.0);
        assert_eq!(started_count(&mut n), 0);
        // Back to an uncalibrated car: a new episode.
        drive(&mut n, &c, Some(7), false, 20.0);
        assert_eq!(started_count(&mut n), 1);
    }

    #[test]
    fn started_not_for_an_already_calibrated_car_and_not_while_off() {
        let mut n = Notifier::default();
        let mut c = cfg();
        drive(&mut n, &c, Some(7), true, 0.1); // restored profile
        drive(&mut n, &c, Some(7), true, 0.2);
        assert!(n.items().is_empty());
        // The gearbox switch is not part of the rule: calibration runs either way.
        c.dsg_enabled = false;
        let mut m = Notifier::default();
        drive(&mut m, &c, Some(7), false, 0.1);
        assert_eq!(texts(&m), ["Calibration started"]);
        // The calibration toggle and the master switch silence it, and it is not
        // announced retroactively when switched back on mid-episode.
        let mut q = Notifier::default();
        c.overlay.notif_calibration = false;
        drive(&mut q, &c, Some(7), false, 0.1);
        c.overlay.notif_calibration = true;
        drive(&mut q, &c, Some(7), false, 0.2);
        assert!(q.items().is_empty());
    }

    #[test]
    fn explicit_clear_while_driving_is_not_announced_twice() {
        let mut n = Notifier::default();
        let c = cfg();
        drive(&mut n, &c, Some(7), true, 0.1); // calibrated
        // Clear RPM calibration: explicit push, then the watch pass sees it uncalibrated.
        n.push(&c.overlay, Event::CalibrationStarted, 1.0);
        n.take_new();
        drive(&mut n, &c, Some(7), false, 1.0);
        assert!(!n.take_new(), "the explicit push already told it");
        // Cleared while paused: told on the next drive.
        drive(&mut n, &c, Some(7), true, 2.0);
        drive(&mut n, &c, None, true, 2.1);
        n.push(&c.overlay, Event::CalibrationStarted, 2.2);
        n.take_new();
        drive(&mut n, &c, None, false, 2.3);
        drive(&mut n, &c, Some(7), false, 2.4);
        assert!(n.take_new());
    }

    #[test]
    fn queue_is_pruned() {
        let mut n = Notifier::default();
        let c = cfg();
        n.push(&c.overlay, Event::CalibrationStarted, 1.0);
        n.push(&c.overlay, Event::Backfire(true), 1.0);
        n.push(&c.overlay, Event::CalibrationStarted, 1.1); // replaces
        assert_eq!(n.items().len(), 2);
        // Old entries drop out when something new is queued much later.
        n.push(&c.overlay, Event::CalibrationStarted, 100.0);
        assert_eq!(n.items().len(), 1);
    }
}
