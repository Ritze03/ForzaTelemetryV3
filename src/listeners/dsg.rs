use std::collections::VecDeque;
use std::io::Write;
use std::time::{Duration, Instant};

use crate::config::{app_data_dir, AppConfig, GearboxMode};
use crate::input::{char_to_key, InputSender};
use crate::packet::ForzaPacket;

/// Minimum settle time after a confirmed shift before another may be commanded.
const SETTLE_MS: u64 = 70;
/// If the expected gear hasn't appeared within this window, assume desync and accept reality.
const SHIFT_CONFIRM_TIMEOUT_MS: u64 = 500;
/// After a desync timeout, hold off re-commanding this long so a late-landing shift is observed
/// and reconciled instead of double-commanded (prevents overshoot).
const POST_TIMEOUT_COOLDOWN_MS: u64 = 400;
/// Throttle (0..1) at or above which a full-throttle event arms the kickdown cooldown.
const KICKDOWN_THROTTLE: f32 = 0.95;
/// At or below this speed the box targets 1st gear (ready to launch).
const STANDSTILL_KMH: f32 = 5.0;
/// Number of valid samples kept per gear for the median calibration.
const CALIB_WINDOW: usize = 10;
/// Calibrate a gear only once the engine is past this fraction of the detected redline — high
/// enough that the kmh/rpm extrapolation is accurate, low enough to lock in within one pull.
const CALIB_RPM_FRAC: f32 = 0.60;
/// Tyre slip (abs) above which a calibration sample is rejected (wheelspin corrupts kmh/rpm).
const CALIB_SLIP: f32 = 0.8;
/// Tyre slip (abs) at or above which an upshift is suppressed (don't shift on a redline spike).
const UPSHIFT_SLIP: f32 = 1.0;
/// Cruise hysteresis: the cruise downshift point sits this fraction of the shift point below the
/// upshift target, so a just-completed cruise upshift can't immediately bounce back down.
const CRUISE_HYSTERESIS: f32 = 0.10;
/// Engine RPM above this multiple of the road-speed-implied RPM means the wheels are spinning;
/// the speed-based gear math is then garbage, so hold the gear until grip returns.
const SPIN_RPM_FACTOR: f32 = 1.15;

/// Shift execution state. While `Shifting` we wait for `expected` to appear (or time out)
/// before commanding anything else — this avoids key spam and tolerates the brief "N" flash
/// that slow-shifting cars emit mid-shift.
enum ShiftPhase {
    Idle,
    Shifting { expected: i32, since: Instant },
}

/// Pre-shift snapshot, captured when a shift is commanded and written out (with post-shift RPM +
/// speed) once the shift confirms. Only kept while `dsg_log_shifts` is on.
struct PendingLog {
    game_time_ms: u32,
    from_gear: i32,
    to_gear: i32,
    rpm_pre: f32,
    speed_pre: f32,
    accel_pct: u8,
    brake_pct: u8,
}

pub struct DsgListener {
    /// Extrapolated speed at full redline (100% RPM) per gear index (0 unused; gears 1–10).
    /// The actual shift-point speed is derived live as `* (shift_rpm_pct/100)`. Session-only.
    pub gear_redline_speeds: [f32; 11],
    /// Last ≤10 valid redline-speed estimates per gear; the committed value is their median.
    /// Never locked — the median keeps updating so a wrong value self-corrects.
    gear_samples: [VecDeque<f32>; 11],
    /// False until the driver manually shifts out of 1st. While false the box stays hands-off so
    /// the driver can do a clean first-gear redline pull (calibrates gear 1 + nails the redline).
    pub engaged: bool,
    /// Whether we've actually seen 1st gear this session — engagement requires a real 1st→up shift,
    /// not merely starting in a high gear (e.g. spawning in 4th).
    seen_first_gear: bool,
    phase: ShiftPhase,
    last_shift_done: Option<Instant>,
    kickdown_triggered: bool,
    kickdown_cooldown_until: Option<Instant>,
    /// After a desync timeout, suppress new commands until this instant (absorbs late shifts).
    resync_until: Option<Instant>,
    /// Pre-shift snapshot awaiting its post-shift values (shift logging only).
    pending_log: Option<PendingLog>,
    // ── Debug telemetry (read by the Fun-tab debug panel) ──
    pub dbg_desired_gear: i32,
    pub dbg_effective_max_rpm: f32,
    pub dbg_shift_threshold: f32,
    /// Seconds left on the cooldown countdown; negative = waiting for throttle release; 0 = inactive.
    pub dbg_kickdown_secs_left: f32,
    pub desync_count: u32,
    pub last_desync: Option<Instant>,
}

impl DsgListener {
    pub fn new() -> Self {
        // Start every launch with a fresh shift log (the next shift recreates it with a header).
        let _ = std::fs::remove_file(app_data_dir().join("dsg_shift_log.csv"));
        Self {
            gear_redline_speeds: [0.0; 11],
            gear_samples: std::array::from_fn(|_| VecDeque::new()),
            engaged: false,
            seen_first_gear: false,
            phase: ShiftPhase::Idle,
            last_shift_done: None,
            kickdown_triggered: false,
            kickdown_cooldown_until: None,
            resync_until: None,
            pending_log: None,
            dbg_desired_gear: 0,
            dbg_effective_max_rpm: 0.0,
            dbg_shift_threshold: 0.0,
            dbg_kickdown_secs_left: 0.0,
            desync_count: 0,
            last_desync: None,
        }
    }

    pub fn reset_calibration(&mut self) {
        self.gear_redline_speeds = [0.0; 11];
        self.gear_samples = std::array::from_fn(|_| VecDeque::new());
    }

    /// Clear the shift state machine (e.g. on car change).
    pub fn reset_state(&mut self) {
        self.engaged = false;
        self.seen_first_gear = false;
        self.phase = ShiftPhase::Idle;
        self.last_shift_done = None;
        self.kickdown_triggered = false;
        self.kickdown_cooldown_until = None;
        self.resync_until = None;
        self.pending_log = None;
        self.desync_count = 0;
        self.last_desync = None;
    }

    /// The gear a shift is currently waiting on, if any (for the debug panel).
    pub fn debug_expected(&self) -> Option<i32> {
        match self.phase {
            ShiftPhase::Shifting { expected, .. } => Some(expected),
            ShiftPhase::Idle => None,
        }
    }

    pub fn update(
        &mut self,
        pkt: &ForzaPacket,
        cfg: &AppConfig,
        input: &InputSender,
        dynamic_max_rpm: f32,
    ) {
        if pkt.is_race_on == 0 {
            return;
        }

        let rpm = pkt.current_engine_rpm;
        let kmh = pkt.speed_kmh();
        // Auto Detect is the only source: the live-detected redline (clean even under
        // wheelspin/handbrake — see app.rs). Until it's known we can't do anything.
        let effective_max_rpm = dynamic_max_rpm;
        self.dbg_effective_max_rpm = effective_max_rpm;
        self.dbg_shift_threshold = effective_max_rpm * (cfg.dsg_shift_rpm_pct / 100.0);

        let gear = pkt.gear as i32;
        let in_drive_gear = (1..=10).contains(&gear);

        // Engage only after a real first-gear pull: the car must have actually been in 1st and then
        // shifted up into a *forward* gear. Starting already in a high gear (e.g. spawning in 4th)
        // must NOT engage, and neither must shifting 1st→Reverse — that passes through Neutral
        // (gear 10+, R is 0), which `>= 2` wrongly counted as an upshift.
        if gear == 1 {
            self.seen_first_gear = true;
        }
        if self.seen_first_gear && (2..=9).contains(&gear) {
            self.engaged = true;
        }

        // ── Continuous calibration (runs even before we engage, to learn gear 1) ────────────
        // Extrapolate each gear's speed at the full redline (100% RPM) from the current
        // RPM/speed ratio. Conditions: past 60% of the redline (accurate ratio), little wheel
        // slip (<0.8), springs loaded (≥0.1), and moving straight (velocity aligned with heading
        // within ~5%) so the road-speed magnitude reflects what the wheels are doing.
        // Gear 1 must be calibrated first (the driver's manual redline pull): until it is, don't
        // record any higher gear — every later gear is extrapolated off the redline that pull nails.
        let first_gear_ready = self.gear_redline_speeds[1] > 0.0;
        if in_drive_gear && effective_max_rpm > 0.0 && kmh > 5.0 && (gear == 1 || first_gear_ready) {
            let gear_idx = gear as usize;

            let no_slip = pkt.tire_slip_ratio_fl.abs() < CALIB_SLIP
                && pkt.tire_slip_ratio_fr.abs() < CALIB_SLIP
                && pkt.tire_slip_ratio_rl.abs() < CALIB_SLIP
                && pkt.tire_slip_ratio_rr.abs() < CALIB_SLIP;

            let springs_loaded = pkt.normalized_suspension_travel_fl >= 0.1
                && pkt.normalized_suspension_travel_fr >= 0.1
                && pkt.normalized_suspension_travel_rl >= 0.1
                && pkt.normalized_suspension_travel_rr >= 0.1;

            let speed_ms = pkt.speed.abs();
            let moving_straight = speed_ms > 0.1 && pkt.velocity_z >= 0.95 * speed_ms;

            if rpm >= CALIB_RPM_FRAC * effective_max_rpm && no_slip && springs_loaded && moving_straight {
                // Rolling median of the last 10 valid redline-speed estimates. Never "locked":
                // a wrong value is corrected as the window slides, and the median rejects outliers.
                let buf = &mut self.gear_samples[gear_idx];
                buf.push_back(kmh * effective_max_rpm / rpm);
                while buf.len() > CALIB_WINDOW {
                    buf.pop_front();
                }
                self.gear_redline_speeds[gear_idx] = median(buf).floor();
            }
        }

        // Hands off until enabled, engaged (driver shifted out of 1st), and a redline is known.
        if !cfg.dsg_enabled || !self.engaged || effective_max_rpm <= 0.0 {
            return;
        }

        let now = Instant::now();

        // ── Shift execution state machine ───────────────────────────────────────
        // While a shift is in flight, send nothing until the expected gear appears or we time
        // out. The 500 ms timeout applies regardless of a transitional "N".
        if let ShiftPhase::Shifting { expected, since } = self.phase {
            if gear == expected {
                self.phase = ShiftPhase::Idle;
                self.last_shift_done = Some(now);
                // Shift confirmed → write the row with the post-shift RPM + speed.
                if let Some(log) = self.pending_log.take() {
                    write_shift_log(&log, rpm, kmh);
                }
            } else if since.elapsed() >= Duration::from_millis(SHIFT_CONFIRM_TIMEOUT_MS) {
                // Desync: sync to the actual gear, then hold off re-commanding to absorb a late shift.
                self.phase = ShiftPhase::Idle;
                self.resync_until = Some(now + Duration::from_millis(POST_TIMEOUT_COOLDOWN_MS));
                self.desync_count += 1;
                self.last_desync = Some(now);
                self.pending_log = None; // drop — a desynced shift would log misleading values
            }
            return;
        }

        // Idle: only act on a real drive gear.
        if !in_drive_gear {
            return;
        }

        // Post-desync cooldown: observe the live gear but issue no new command yet.
        if let Some(t) = self.resync_until {
            if now < t {
                return;
            }
            self.resync_until = None;
        }

        // Respect post-shift settle time.
        if let Some(done) = self.last_shift_done {
            if done.elapsed() < Duration::from_millis(SETTLE_MS) {
                return;
            }
        }

        // ── Kickdown cooldown ──────────────────────────────────────────────────
        // Arm on any full-throttle event. The lower gear is held "ready" (the gentle cruise
        // upshift is suppressed) both WHILE the throttle is still pressed after a kickdown and for
        // the cooldown window after release — so easing off mid-pull doesn't immediately upshift
        // out of the gear we just grabbed. The hard redline upshift still fires for protection.
        let throttle = if pkt.power > 0.0 { pkt.accel as f32 / 255.0 } else { 0.0 };
        if throttle >= KICKDOWN_THROTTLE {
            self.kickdown_triggered = true;
        } else if self.kickdown_triggered && pkt.accel == 0 {
            self.kickdown_cooldown_until =
                Some(now + Duration::from_secs_f32(cfg.dsg_kickdown_cooldown_secs.max(0.0)));
            self.kickdown_triggered = false;
        }
        // Read AFTER arming so a cooldown that starts on this exact release frame already counts —
        // otherwise the cruise upshift slips through the one frame between release and the timer.
        let in_cooldown = self.kickdown_cooldown_until.map(|t| now < t).unwrap_or(false);
        self.dbg_kickdown_secs_left = if self.kickdown_triggered {
            -1.0
        } else {
            self.kickdown_cooldown_until
                .and_then(|t| t.checked_duration_since(now))
                .map(|d| d.as_secs_f32())
                .unwrap_or(0.0)
        };

        // Hold the lower gear while still on the throttle after a kickdown, and through the cooldown.
        let hold_lower_gear = self.kickdown_triggered || in_cooldown;
        let desired = self
            .select_desired_gear(pkt, cfg, gear, effective_max_rpm, hold_lower_gear)
            .max(1);
        self.dbg_desired_gear = desired;

        // ── Command a single confirmed step toward the target ──────────────────
        if desired != gear {
            let step: i32 = if desired > gear { 1 } else { -1 };
            let expected = gear + step;
            let key_char = if step > 0 { 'e' } else { 'q' };
            if let Some(key) = char_to_key(key_char) {
                input.press(key, 10);
            }
            self.phase = ShiftPhase::Shifting { expected, since: now };
            if cfg.dsg_log_shifts {
                self.pending_log = Some(PendingLog {
                    game_time_ms: pkt.timestamp_ms,
                    from_gear: gear,
                    to_gear: expected,
                    rpm_pre: rpm,
                    speed_pre: kmh,
                    accel_pct: (pkt.accel as u16 * 100 / 255) as u8,
                    brake_pct: (pkt.brake as u16 * 100 / 255) as u8,
                });
            }
        }
    }

    /// Predicted engine RPM for `gear` at the current speed, referenced to the full redline.
    /// Returns `None` for uncalibrated gears.
    fn predicted_rpm(&self, gear: i32, kmh: f32, effective_max_rpm: f32) -> Option<f32> {
        if !(1..=10).contains(&gear) {
            return None;
        }
        let redline_speed = self.gear_redline_speeds[gear as usize];
        if redline_speed <= 0.0 {
            return None;
        }
        Some(effective_max_rpm * kmh / redline_speed)
    }

    /// Choose the ideal gear for the current driving intent.
    ///
    /// Three rules, in order: a hard redline upshift (the user's "shift at Max RPM × Shift RPM"
    /// rule, gated on grip + speed), a gentle cruise upshift toward the throttle-demanded target
    /// (so chill modes settle into tall gears at low revs), and a lazy downshift when revs fall
    /// below the demand. The cruise-upshift lands at ≥ target and the downshift fires only below
    /// target, so the two can't oscillate; the redline upshift is kept apart from the downshift
    /// by the over-rev guard.
    fn select_desired_gear(
        &self,
        pkt: &ForzaPacket,
        cfg: &AppConfig,
        current_gear: i32,
        effective_max_rpm: f32,
        hold_lower_gear: bool,
    ) -> i32 {
        let kmh = pkt.speed_kmh();

        // Stopped / crawling → 1st, ready to launch.
        if kmh < STANDSTILL_KMH {
            return 1;
        }

        let cur_redline = self.gear_redline_speeds[current_gear as usize];
        // Hold an uncalibrated gear so the continuous calibration can sample it (the driver/box
        // revs it past 60% on the way up, which records its redline speed). Without a value we
        // can't reason about shift points, and we must never shift out of an unknown gear.
        if cur_redline <= 0.0 {
            return current_gear;
        }

        let rpm = pkt.current_engine_rpm;
        let shift_threshold = effective_max_rpm * (cfg.dsg_shift_rpm_pct / 100.0);

        // ── Wheelspin guard ────────────────────────────────────────────────────
        // If the engine is turning well above what the road speed implies for this gear, the wheels
        // are spinning and the road speed is too low to trust. Every gear decision below is built on
        // road-speed-derived predicted RPM, so on a wheelspinning kickdown that math drops into
        // absurdly low gears (then over-revs and jumps high). Hold the gear until grip returns.
        let pred_cur = effective_max_rpm * kmh / cur_redline;
        if rpm > pred_cur * SPIN_RPM_FACTOR {
            return current_gear;
        }

        // ── 1) Hard redline upshift ────────────────────────────────────────────
        // At/above the shift point, with grip and genuine road speed (not a wheelspin spike).
        let slip_ok = pkt.tire_slip_ratio_fl.abs() < UPSHIFT_SLIP
            && pkt.tire_slip_ratio_fr.abs() < UPSHIFT_SLIP
            && pkt.tire_slip_ratio_rl.abs() < UPSHIFT_SLIP
            && pkt.tire_slip_ratio_rr.abs() < UPSHIFT_SLIP;
        if current_gear < 10
            && rpm >= shift_threshold
            && slip_ok
            && kmh >= (cfg.dsg_upshift_speed_pct / 100.0) * cur_redline
        {
            return current_gear + 1;
        }

        // Race whenever an actual race is on (auto-switch) or it's the selected mode. Race ignores
        // the cruise target entirely — it always wants the full powerband, so it holds the low gear
        // and only upshifts at the redline (the cruise upshift below never fires when cruise = 1.0).
        // In an actual race iff we have a race position (P0 = free roam, P1+ = race).
        let is_race = cfg.dsg_effective_mode(pkt.race_position != 0) == GearboxMode::Race;

        // Throttle-demanded target RPM. Throttle only counts when the engine is making power.
        let throttle = if pkt.power > 0.0 { pkt.accel as f32 / 255.0 } else { 0.0 };
        let full_thr = (cfg.dsg_full_throttle_pct / 100.0).clamp(0.05, 1.0);
        let target_rpm = if is_race || throttle >= full_thr {
            // Full powerband: Race always, or any mode once the throttle reaches the full-throttle
            // threshold — drop gears to keep the engine high for power.
            shift_threshold
        } else {
            // Economical (part throttle, non-race): a light press must not snap a gear down. The
            // target only climbs from the cruise floor up to the downshift deadzone as the throttle
            // approaches the full-throttle threshold, letting revs build in the current gear.
            let cruise = cfg.dsg_active_tuning().cruise_rpm_pct / 100.0;
            let deadzone = cfg.dsg_downshift_deadzone_pct / 100.0;
            let eco = throttle / full_thr; // 0..1 across the part-throttle range
            shift_threshold * (cruise + (deadzone - cruise).max(0.0) * eco)
        };

        // ── 2) Cruise upshift ──────────────────────────────────────────────────
        // Ease into a taller (already-calibrated) gear when it would still sit at/above the
        // target — keeps revs low while cruising. Skipped while braking (let it engine-brake)
        // and during the post-kickdown cooldown (stay in the lower gear, ready to go).
        // Economical cruise upshift: ease into a taller (calibrated) gear when it would still sit
        // at/above the throttle-demanded target, keeping revs near the Cruise RPM target. This is
        // intentionally NOT gated by Upshift speed — that slider is the full-throttle / wheelspin
        // gate on the hard upshift above; gating this one would force the car to rev high before
        // every part-throttle upshift and defeat the economical behaviour. Suppressed while holding
        // the lower gear after a kickdown (still on throttle, or during the cooldown).
        if !hold_lower_gear && pkt.brake == 0 && current_gear < 10 {
            if let Some(pred_next) = self.predicted_rpm(current_gear + 1, kmh, effective_max_rpm) {
                if pred_next >= target_rpm {
                    return current_gear + 1;
                }
            }
        }

        // ── 3) Downshift ───────────────────────────────────────────────────────
        // Triggered once revs fall below the demand AND below the deadzone hysteresis. We then
        // pick the *deepest* lower gear the powerband buffer allows, evaluating each gear jump
        // individually: jump = the RPM rise from the current gear into candidate g (so it scales
        // with how short g is vs. the current gear). The buffer demands `buffer%` of that jump be
        // left as headroom below the redline — a short gear (big jump) needs more room, so we
        // won't slam into the limiter or hop past a sensible gear. The state machine still steps
        // one gear at a time toward the chosen target.
        // Race ignores the cruise deadzone: it always wants the powerband, so the trigger is the
        // shift point and the powerband buffer provides the hunt protection. Street and Sport stay
        // lazy — hold the gear until revs fall below the deadzone hysteresis.
        // Race and any full-throttle kickdown drop straight into the powerband. Otherwise the
        // cruise downshift sits a fixed band BELOW the upshift target so a just-completed cruise
        // upshift (which lands at ≥ target) can't instantly trigger a downshift back — that
        // boundary coincidence (down point == up target) was the up/down/up hunt on acceleration.
        let down_point = if is_race || throttle >= full_thr {
            target_rpm
        } else {
            (target_rpm - CRUISE_HYSTERESIS * shift_threshold).max(0.0)
        };
        if rpm < down_point && current_gear > 1 {
            if let Some(pred_cur) = self.predicted_rpm(current_gear, kmh, effective_max_rpm) {
                // A full-throttle kickdown uses its own (usually smaller) buffer so it drops deeper
                // into the powerband than a lazy coasting/braking downshift.
                let buffer = if throttle >= full_thr {
                    cfg.dsg_kickdown_powerband_buffer_pct / 100.0
                } else {
                    cfg.dsg_downshift_powerband_buffer_pct / 100.0
                };
                let mut target = current_gear;
                for g in (1..current_gear).rev() {
                    let Some(pred_g) = self.predicted_rpm(g, kmh, effective_max_rpm) else {
                        break; // uncalibrated lower gear → never drop into the unknown
                    };
                    // The landing must clear the shift point by `buffer%` of the inter-gear RPM
                    // jump. Capping at the shift point (not the absolute redline) keeps the landing
                    // below the upshift trigger — so it won't bounce straight back up — while the
                    // buffer alone decides how far below: 0% lets it ride up to the shift point
                    // (into the powerband top), higher values land progressively lower. This also
                    // blocks the post-upshift hunt: the just-vacated gear sits at the shift point.
                    let jump = (pred_g - pred_cur).max(0.0);
                    if pred_g + buffer * jump >= shift_threshold {
                        break; // this gear (and any deeper) breaches the buffer
                    }
                    target = g;
                    if pred_g >= target_rpm {
                        break; // revs back up to the demand → no need to drop further
                    }
                }
                if target != current_gear {
                    return target;
                }
            }
        }

        current_gear
    }
}

/// Append one shift to `dsg_shift_log.csv` in the app data dir (writing a header if new).
/// Best-effort: any IO error is silently ignored — logging must never disrupt shifting.
fn write_shift_log(log: &PendingLog, rpm_post: f32, speed_post: f32) {
    let path = app_data_dir().join("dsg_shift_log.csv");
    let new_file = !path.exists();
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    if new_file {
        let _ = writeln!(
            f,
            "game_time_ms,event,gear_from,gear_to,rpm_pre,rpm_post,speed_pre_kmh,speed_post_kmh,accel_pct,brake_pct"
        );
    }
    let event = if log.to_gear > log.from_gear { "up" } else { "down" };
    let _ = writeln!(
        f,
        "{},{},{},{},{:.0},{:.0},{:.1},{:.1},{},{}",
        log.game_time_ms,
        event,
        log.from_gear,
        log.to_gear,
        log.rpm_pre,
        rpm_post,
        log.speed_pre,
        speed_post,
        log.accel_pct,
        log.brake_pct,
    );
}

/// Median of the samples (mean of the two middle values for an even count). 0.0 if empty.
fn median(samples: &VecDeque<f32>) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut v: Vec<f32> = samples.iter().copied().collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}
