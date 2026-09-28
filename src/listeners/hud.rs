//! HUD data on the listener thread: everything the overlay's [`HudSnapshot`] carries beyond
//! the raw packet. Lives here, not in `app.rs`, because the frame loop stops while the game
//! covers the window — the listener keeps running (see `worker.rs`).
//!
//! All pieces are pure and clocked by an explicit `now` ([`hud_clock`] seconds), so tests
//! drive them with synthetic time.

use std::sync::Arc;

use crate::config::{AppConfig, OverlayConfig};
use crate::listeners::lap_trace::LapTrace;
use crate::overlay::snapshot::{
    DriftChip, DriftInfo, HudEvents, HudMode, HudSnapshot, MinimapCalib, PlaceChange,
};
use crate::packet::ForzaPacket;

// ── Race/drift classifier (D27) ─────────────────────────────────────────────
//
// In races `current_lap` is a lap timer: it rises at exactly the packet clock's rate
// (Δcurrent_lap ≈ Δtimestamp_ms / 1000). In drift events it is the score: flat between
// drifts, then rising at hundreds–thousands of points per second or jumping. So we judge
// short windows by their rate Δcurrent_lap / Δt and switch mode only after several windows
// agree. All thresholds are guesses until checked against real FH6 packets (D21/D27).

/// Length of one judged window, seconds of packet time. Long enough to average out
/// packet jitter (±1 ms on a 16 ms step is ±6 % per sample), short enough to react fast.
pub const MODE_WINDOW_SECS: f32 = 0.5;
/// A window whose rate is within 1 ± this is timer-like (race evidence).
pub const MODE_RATE_TOL: f32 = 0.15;
/// A gap between packets longer than this (s) breaks the window: a hitch, pause or load
/// would otherwise read as a huge rate.
pub const MODE_MAX_GAP_SECS: f32 = 0.25;
/// `current_lap` dropping by more than this is a lap wrap / restart / score reset: the
/// window restarts and the drop counts as nothing.
pub const MODE_DROP_EPS: f32 = 0.05;
/// Consecutive drift-evidence windows needed to switch Race → Drift (1.5 s).
/// Longer than the way back: a false Drift swaps the race block away mid-race.
pub const DRIFT_ENTER_WINDOWS: u32 = 3;
/// Consecutive race-evidence windows needed to switch Drift → Race (1 s). A running
/// timer is unambiguous, so returning is quicker.
pub const RACE_ENTER_WINDOWS: u32 = 2;

/// Race vs drift from how `current_lap` moves. A **flat** window (no change) is *no
/// evidence* either way: a drift score sits still between drifts, but so does a frozen
/// timer (countdown at 0, results screen), and free roam sends 0 — so only a rising value
/// decides. Drift evidence needs a rise at a non-timer rate.
#[derive(Debug, Default)]
pub struct ModeClassifier {
    mode: HudMode,
    /// Previous sample `(current_lap, timestamp_ms)`; `None` after a pause/gap/reset.
    prev: Option<(f32, u32)>,
    win_dt: f32,
    win_dl: f32,
    /// Consecutive windows voting for the *other* mode.
    streak: u32,
}

impl ModeClassifier {
    /// Feed one packet. `is_race_on == false` (paused) keeps the mode and restarts the
    /// window, so the pause itself is never measured.
    pub fn update(&mut self, current_lap: f32, timestamp_ms: u32, is_race_on: bool) -> HudMode {
        if !is_race_on {
            self.restart_window(None);
            return self.mode;
        }
        let Some((pl, pt)) = self.prev else {
            self.restart_window(Some((current_lap, timestamp_ms)));
            return self.mode;
        };
        let dt = timestamp_ms.wrapping_sub(pt) as f32 / 1000.0;
        if dt == 0.0 {
            return self.mode; // duplicate packet
        }
        let dl = current_lap - pl;
        if dt > MODE_MAX_GAP_SECS || dl < -MODE_DROP_EPS {
            self.restart_window(Some((current_lap, timestamp_ms)));
            return self.mode;
        }
        self.prev = Some((current_lap, timestamp_ms));
        self.win_dt += dt;
        self.win_dl += dl.max(0.0);
        if self.win_dt >= MODE_WINDOW_SECS {
            let vote = if self.win_dl == 0.0 {
                None // flat: no evidence
            } else if (self.win_dl / self.win_dt - 1.0).abs() <= MODE_RATE_TOL {
                Some(HudMode::Race)
            } else {
                Some(HudMode::Drift)
            };
            self.win_dt = 0.0;
            self.win_dl = 0.0;
            match vote {
                Some(m) if m != self.mode => {
                    self.streak += 1;
                    let need = match m {
                        HudMode::Drift => DRIFT_ENTER_WINDOWS,
                        HudMode::Race => RACE_ENTER_WINDOWS,
                    };
                    if self.streak >= need {
                        self.mode = m;
                        self.streak = 0;
                    }
                }
                Some(_) => self.streak = 0, // agrees with the current mode
                None => {}                  // flat: neither extends nor breaks a streak
            }
        }
        self.mode
    }

    fn restart_window(&mut self, prev: Option<(f32, u32)>) {
        self.prev = prev;
        self.win_dt = 0.0;
        self.win_dl = 0.0;
    }
}

// ── Drift gain window (D10) ──────────────────────────────────────────────────

/// The "+N" chip: score gained over each `interval`-second window.
#[derive(Debug, Default)]
pub struct DriftWindow {
    /// `(start time, score at start)` of the running window.
    start: Option<(f64, f32)>,
    chip: Option<DriftChip>,
}

impl DriftWindow {
    /// Feed the live score. Windows run back to back (next start = previous end, so the
    /// cadence doesn't drift with packet timing); a score drop (reset/crash) restarts the
    /// window at the new score instead of producing a negative chip.
    pub fn update(&mut self, score: f32, now: f64, interval: f32) {
        let interval = f64::from(interval.max(0.1));
        let (t0, s0) = *self.start.get_or_insert((now, score));
        if score < s0 {
            self.start = Some((now, score));
        } else if now - t0 >= interval {
            self.chip = Some(DriftChip { gain: score - s0, at: now });
            // Back to back, unless we fell more than a window behind (a stall).
            let t1 = if now - t0 < 2.0 * interval { t0 + interval } else { now };
            self.start = Some((t1, score));
        }
    }

    /// Stop the running window (paused, or not drifting). The last chip is kept so its
    /// animation can finish; the next `update` starts a fresh window.
    pub fn stop(&mut self) {
        self.start = None;
    }

    pub fn window_start(&self) -> Option<f64> {
        self.start.map(|(t, _)| t)
    }

    pub fn chip(&self) -> Option<DriftChip> {
        self.chip
    }
}

// ── Visibility target ────────────────────────────────────────────────────────

/// Paused this long (s) → hidden. Short enough to clear the screen promptly on the pause
/// menu, long enough to ride out a one-packet `is_race_on` blip (research §6).
pub const PAUSE_HIDE_SECS: f64 = 0.3;
/// No packet for this long (s) → hidden and `connected = false`. Same 2 s as the
/// listener's `STALE_AFTER` for the TelemetryLive hotkey gate.
pub const NO_PACKET_HIDE_SECS: f64 = 2.0;

/// Facts the visibility target is computed from.
#[derive(Debug, Clone, Copy, Default)]
pub struct VisFacts {
    pub enabled: bool,
    pub hud_hidden: bool,
    pub focus_only: bool,
    pub game_focused: bool,
    pub last_packet_at: Option<f64>,
    pub paused_since: Option<f64>,
}

/// Should the HUD be up? Shows immediately when all gates pass; hides once a pause has
/// lasted [`PAUSE_HIDE_SECS`] or packets have been gone [`NO_PACKET_HIDE_SECS`].
pub fn visible_target(f: &VisFacts, now: f64) -> bool {
    f.enabled
        && !f.hud_hidden
        && (!f.focus_only || f.game_focused)
        && f.last_packet_at.is_some_and(|t| now - t < NO_PACKET_HIDE_SECS)
        && f.paused_since.is_none_or(|t| now - t < PAUSE_HIDE_SECS)
}

// ── Tracker: packet stream → snapshot ─────────────────────────────────────────

/// Per-packet HUD state. Fed only while the overlay is enabled; reset when it's switched on
/// so a stale "previous packet" can't fire a bogus place change.
#[derive(Default)]
pub struct HudTracker {
    classifier: ModeClassifier,
    drift: DriftWindow,
    lap: LapTrace,
    mode: HudMode,
    lap_delta: Option<f32>,
    events: HudEvents,
    pkt: ForzaPacket,
    /// `pkt` is a real packet (not the default).
    have_pkt: bool,
    last_packet_at: Option<f64>,
    /// When the drift score last rose (X1′'s live dot).
    last_rise_at: Option<f64>,
}

impl HudTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feed one packet received at `now`.
    pub fn on_packet(&mut self, pkt: &ForzaPacket, cfg: &OverlayConfig, now: f64) {
        let running = pkt.is_race_on != 0;
        if self.have_pkt {
            let prev = &self.pkt;
            if pkt.car_ordinal != prev.car_ordinal {
                self.lap.reset();
            }
            if pkt.race_position != prev.race_position
                && pkt.race_position != 0
                && prev.race_position != 0
            {
                self.events.place_change = Some(PlaceChange {
                    at: now,
                    gained: pkt.race_position < prev.race_position,
                });
            }
            if pkt.lap_number == prev.lap_number.wrapping_add(1) {
                self.events.lap_completed_at = Some(now);
            }
            if pkt.gear != prev.gear {
                self.events.gear_changed_at = Some(now);
            }
        }
        match (running, self.events.paused_since) {
            (false, None) => self.events.paused_since = Some(now),
            (true, Some(_)) => self.events.paused_since = None,
            _ => {}
        }

        self.mode = self.classifier.update(pkt.current_lap, pkt.timestamp_ms, running);
        match self.mode {
            HudMode::Race => {
                self.drift.stop();
                if running {
                    self.lap_delta = self.lap.update(pkt.lap_number, pkt.distance_traveled, pkt.current_lap);
                }
            }
            HudMode::Drift => {
                // Scores would poison the best-lap trace.
                self.lap.reset();
                self.lap_delta = None;
                if running {
                    if self.have_pkt && pkt.current_lap > self.pkt.current_lap {
                        self.last_rise_at = Some(now);
                    }
                    self.drift.update(pkt.current_lap, now, cfg.drift_chip_secs);
                } else {
                    self.drift.stop();
                }
            }
        }

        self.pkt = pkt.clone();
        self.have_pkt = true;
        self.last_packet_at = Some(now);
    }

    /// Build the snapshot for `now`. `facts` needs only the gates the tracker doesn't own
    /// (`enabled`, `hud_hidden`, `focus_only`, `game_focused`); packet timing is filled in.
    pub fn snapshot(
        &self,
        mut facts: VisFacts,
        cfg: &Arc<OverlayConfig>,
        app: &AppConfig,
        now: f64,
    ) -> HudSnapshot {
        facts.last_packet_at = self.last_packet_at;
        facts.paused_since = self.events.paused_since;
        let max = self.pkt.engine_max_rpm;
        let drifting = self.mode == HudMode::Drift;
        HudSnapshot {
            pkt: self.pkt.clone(),
            built_at: now,
            visible: visible_target(&facts, now),
            connected: self.last_packet_at.is_some_and(|t| now - t < NO_PACKET_HIDE_SECS),
            paused: self.have_pkt && self.pkt.is_race_on == 0,
            hud_hidden: facts.hud_hidden,
            redline_rpm: cfg.redline_frac * max,
            shift_rpm: cfg.shift_frac * max,
            mode: self.mode,
            lap_delta: self.lap_delta,
            drift: DriftInfo {
                score: if drifting { self.pkt.current_lap } else { 0.0 },
                best: if drifting { self.pkt.best_lap } else { 0.0 },
                window_start: self.drift.window_start(),
                interval: cfg.drift_chip_secs,
                chip: self.drift.chip(),
                last_rise_at: if drifting { self.last_rise_at } else { None },
            },
            events: self.events,
            cfg: cfg.clone(),
            use_mph: app.use_mph,
            minimap: MinimapCalib {
                px_per_m: app.minimap_px_per_m,
                origin_x: app.minimap_world_origin_x,
                origin_z: app.minimap_world_origin_z,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the classifier over `secs` seconds of 60 Hz packets, `lap(t)` giving
    /// current_lap at packet time t. Returns the mode after each packet.
    fn run(c: &mut ModeClassifier, t0: f32, secs: f32, lap: impl Fn(f32) -> f32) -> Vec<HudMode> {
        let n = (secs * 60.0) as u32;
        (0..n)
            .map(|i| {
                let t = t0 + i as f32 / 60.0;
                c.update(lap(t), (t * 1000.0).round() as u32, true)
            })
            .collect()
    }

    #[test]
    fn classifier_timer_stays_race() {
        let mut c = ModeClassifier::default();
        let modes = run(&mut c, 0.0, 10.0, |t| 3.0 + t);
        assert!(modes.iter().all(|&m| m == HudMode::Race));
    }

    #[test]
    fn classifier_score_switches_to_drift_after_hysteresis() {
        let mut c = ModeClassifier::default();
        // Drift score: +40 points every packet (2400/s).
        let modes = run(&mut c, 0.0, 3.0, |t| (t * 60.0).floor() * 40.0);
        let first = modes.iter().position(|&m| m == HudMode::Drift).unwrap_or(usize::MAX);
        // Not before DRIFT_ENTER_WINDOWS windows (1.5 s = 90 packets), but soon after.
        assert!((85..=100).contains(&first), "switched at packet {first}");
        assert_eq!(modes.last(), Some(&HudMode::Drift));
    }

    #[test]
    fn classifier_uneven_jumps_and_flat_stretches_stay_drift() {
        let mut c = ModeClassifier::default();
        run(&mut c, 0.0, 3.0, |t| (t * 60.0).floor() * 40.0);
        // Now flat for 5 s (not drifting): flat is no evidence → stays Drift.
        let modes = run(&mut c, 3.0, 5.0, |_| 7200.0);
        assert!(modes.iter().all(|&m| m == HudMode::Drift));
        // Uneven jumps: +1500 every 0.7 s.
        let modes = run(&mut c, 8.0, 5.0, |t| 7200.0 + ((t - 8.0) / 0.7).floor() * 1500.0);
        assert!(modes.iter().all(|&m| m == HudMode::Drift));
    }

    #[test]
    fn classifier_single_contrary_window_does_not_flip() {
        let mut c = ModeClassifier::default();
        run(&mut c, 0.0, 5.0, |t| t);
        // One window (0.5 s) of score-like jumps inside a race, then the timer again.
        run(&mut c, 5.0, 0.5, |t| 5.0 + (t - 5.0) * 50.0);
        let modes = run(&mut c, 5.5, 3.0, |t| 30.0 + (t - 5.5));
        assert!(modes.iter().all(|&m| m == HudMode::Race), "one window flipped the mode");
    }

    #[test]
    fn classifier_back_to_race_after_timer_windows() {
        let mut c = ModeClassifier::default();
        run(&mut c, 0.0, 3.0, |t| (t * 60.0).floor() * 40.0);
        let modes = run(&mut c, 3.0, 2.0, |t| t - 3.0); // new race: timer from 0
        let first = modes.iter().position(|&m| m == HudMode::Race).unwrap_or(usize::MAX);
        assert!((55..=70).contains(&first), "switched back at packet {first}");
    }

    #[test]
    fn classifier_frozen_timer_and_free_roam_zero_stay_race() {
        let mut c = ModeClassifier::default();
        run(&mut c, 0.0, 5.0, |t| t);
        // Results screen: timer frozen at 83.4 s for 10 s.
        let modes = run(&mut c, 5.0, 10.0, |_| 83.4);
        assert!(modes.iter().all(|&m| m == HudMode::Race));
        let modes = run(&mut c, 15.0, 10.0, |_| 0.0);
        assert!(modes.iter().all(|&m| m == HudMode::Race));
    }

    #[test]
    fn classifier_remembers_mode_across_pause_and_ignores_resets() {
        let mut c = ModeClassifier::default();
        run(&mut c, 0.0, 3.0, |t| (t * 60.0).floor() * 40.0);
        // Paused 20 s: mode kept, and the resume gap is not measured.
        for i in 0..100 {
            assert_eq!(c.update(7000.0, 3000 + i * 200, false), HudMode::Drift);
        }
        // Score reset to 0 then rising again: still Drift.
        let modes = run(&mut c, 23.0, 2.0, |t| ((t - 23.0) * 60.0).floor() * 25.0);
        assert!(modes.iter().all(|&m| m == HudMode::Drift));
        // Race side: a lap wrap (timer drops to 0) is not evidence of anything.
        let mut c = ModeClassifier::default();
        run(&mut c, 0.0, 5.0, |t| 60.0 + t);
        let modes = run(&mut c, 5.0, 3.0, |t| t - 5.0);
        assert!(modes.iter().all(|&m| m == HudMode::Race));
    }

    #[test]
    fn classifier_tolerates_packet_jitter() {
        let mut c = ModeClassifier::default();
        // Timestamps jitter ±2 ms around 60 Hz while the lap timer is exact.
        for i in 0..600u32 {
            let t = i as f32 / 60.0;
            let jitter = [0i32, 2, -2, 1, -1][(i % 5) as usize];
            let ts = ((t * 1000.0) as i32 + jitter) as u32;
            assert_eq!(c.update(t, ts, true), HudMode::Race, "packet {i}");
        }
    }

    #[test]
    fn drift_window_emits_gain_every_interval() {
        let mut w = DriftWindow::default();
        // Score +100 per second, sampled at 60 Hz for 12 s, 5 s windows.
        let mut chips = Vec::new();
        for i in 0..=720 {
            let now = i as f64 / 60.0;
            w.update(now as f32 * 100.0, now, 5.0);
            if let Some(c) = w.chip() {
                if chips.last() != Some(&c) {
                    chips.push(c);
                }
            }
        }
        assert_eq!(chips.len(), 2, "{chips:?}");
        assert!((chips[0].gain - 500.0).abs() < 2.0 && (chips[0].at - 5.0).abs() < 0.02);
        assert!((chips[1].gain - 500.0).abs() < 2.0 && (chips[1].at - 10.0).abs() < 0.02);
        // Windows are back to back: the running one started at 10 s.
        assert_eq!(w.window_start(), Some(10.0));
    }

    #[test]
    fn drift_window_score_reset_and_stop_restart_the_window() {
        let mut w = DriftWindow::default();
        w.update(1000.0, 0.0, 5.0);
        w.update(1400.0, 3.0, 5.0);
        w.update(0.0, 4.0, 5.0); // crash: score reset
        assert_eq!(w.window_start(), Some(4.0));
        w.update(300.0, 9.0, 5.0);
        let c = w.chip().map(|c| c.gain);
        assert_eq!(c, Some(300.0), "gain counted from the reset, never negative");
        // Paused: window stops, chip kept; resume starts a fresh window.
        w.stop();
        assert_eq!(w.window_start(), None);
        assert!(w.chip().is_some());
        w.update(300.0, 60.0, 5.0);
        assert_eq!(w.window_start(), Some(60.0));
    }

    fn facts() -> VisFacts {
        VisFacts {
            enabled: true,
            last_packet_at: Some(10.0),
            ..Default::default()
        }
    }

    #[test]
    fn visibility_pause_hides_after_delay_and_shows_immediately() {
        let mut f = facts();
        assert!(visible_target(&f, 10.0));
        f.paused_since = Some(10.0);
        assert!(visible_target(&f, 10.2), "a 200 ms blip stays up");
        assert!(!visible_target(&f, 10.31), "hidden after 300 ms paused");
        f.paused_since = None;
        assert!(visible_target(&f, 10.31), "resume shows immediately");
    }

    #[test]
    fn visibility_no_packets_hides_after_two_seconds() {
        let f = facts();
        assert!(visible_target(&f, 11.9));
        assert!(!visible_target(&f, 12.0));
        let never = VisFacts { last_packet_at: None, ..facts() };
        assert!(!visible_target(&never, 0.0));
    }

    #[test]
    fn visibility_focus_only_hide_toggle_and_enable() {
        let f = VisFacts { focus_only: true, game_focused: false, ..facts() };
        assert!(!visible_target(&f, 10.0));
        let f = VisFacts { focus_only: true, game_focused: true, ..facts() };
        assert!(visible_target(&f, 10.0));
        let f = VisFacts { focus_only: false, game_focused: false, ..facts() };
        assert!(visible_target(&f, 10.0), "focus ignored unless focus_only");
        let f = VisFacts { hud_hidden: true, ..facts() };
        assert!(!visible_target(&f, 10.0));
        let f = VisFacts { enabled: false, ..facts() };
        assert!(!visible_target(&f, 10.0));
    }

    #[test]
    fn tracker_events_rpm_and_pause_flow_into_snapshot() {
        let cfg = Arc::new(OverlayConfig::default());
        let app = AppConfig::default();
        let mut tr = HudTracker::new();
        let mut p = ForzaPacket {
            is_race_on: 1,
            engine_max_rpm: 8000.0,
            race_position: 5,
            gear: 3,
            ..Default::default()
        };
        tr.on_packet(&p, &cfg, 1.0);
        p.race_position = 4;
        p.gear = 4;
        p.lap_number = 1;
        p.timestamp_ms = 16;
        tr.on_packet(&p, &cfg, 1.016);
        let live = VisFacts { enabled: true, ..Default::default() };
        let s = tr.snapshot(live, &cfg, &app, 1.02);
        assert!(s.visible && s.connected && !s.paused);
        assert_eq!(s.events.place_change, Some(PlaceChange { at: 1.016, gained: true }));
        assert_eq!(s.events.gear_changed_at, Some(1.016));
        assert_eq!(s.events.lap_completed_at, Some(1.016));
        assert!((s.redline_rpm - 6800.0).abs() < 0.1 && (s.shift_rpm - 7440.0).abs() < 0.1);

        p.is_race_on = 0;
        tr.on_packet(&p, &cfg, 2.0);
        let s = tr.snapshot(live, &cfg, &app, 2.1);
        assert!(s.paused && s.visible, "not hidden before 300 ms");
        let s = tr.snapshot(live, &cfg, &app, 2.4);
        assert!(!s.visible, "hidden after 300 ms paused");
        let s = tr.snapshot(live, &cfg, &app, 4.1);
        assert!(!s.connected, "no packet for 2 s");
    }

    #[test]
    fn tracker_records_last_score_rise_only_in_drift() {
        let cfg = Arc::new(OverlayConfig::default());
        let app = AppConfig::default();
        let live = VisFacts { enabled: true, ..Default::default() };
        let mut tr = HudTracker::new();
        let mut p = ForzaPacket { is_race_on: 1, ..Default::default() };
        // Score jumps of 100 every 0.1 s: drift evidence, Drift after the hysteresis.
        let mut t = 0.0_f64;
        for i in 0..240_u32 {
            t = f64::from(i) / 60.0;
            p.timestamp_ms = (t * 1000.0).round() as u32;
            p.current_lap = (i / 6) as f32 * 100.0;
            tr.on_packet(&p, &cfg, t);
        }
        let s = tr.snapshot(live, &cfg, &app, t);
        assert_eq!(s.mode, HudMode::Drift);
        let rise = s.drift.last_rise_at.unwrap_or(-1.0);
        assert!(t - rise < 0.1, "last rise {rise} at {t}");
        // Flat score: the rise time stays put (the dot goes grey on the HUD side).
        for i in 240..300_u32 {
            let t = f64::from(i) / 60.0;
            p.timestamp_ms = (t * 1000.0).round() as u32;
            tr.on_packet(&p, &cfg, t);
        }
        assert_eq!(tr.snapshot(live, &cfg, &app, 5.0).drift.last_rise_at, Some(rise));
        // Race mode never reports one (a lap timer rises every packet).
        let mut race = HudTracker::new();
        for i in 0..60_u32 {
            let t = f64::from(i) / 60.0;
            p.timestamp_ms = (t * 1000.0).round() as u32;
            p.current_lap = t as f32;
            race.on_packet(&p, &cfg, t);
        }
        assert_eq!(race.snapshot(live, &cfg, &app, 1.0).drift.last_rise_at, None);
    }
}
