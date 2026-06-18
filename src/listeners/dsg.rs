use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::config::{AppConfig, DsgMaxRpmSource};
use crate::input::{char_to_key, InputSender};
use crate::packet::ForzaPacket;

/// Minimum settle time after a confirmed shift before another may be commanded.
const SETTLE_MS: u64 = 70;
/// If the expected gear hasn't appeared within this window, assume desync and accept reality.
const SHIFT_CONFIRM_TIMEOUT_MS: u64 = 500;
/// After a desync timeout, hold off re-commanding this long so a late-landing shift is observed
/// and reconciled instead of double-commanded (prevents overshoot).
const POST_TIMEOUT_COOLDOWN_MS: u64 = 400;
/// Gear-selection RPM cap as a fraction of the shift threshold — guarantees ≥10% band free.
const POWER_HEADROOM_FRAC: f32 = 0.90;
/// Throttle (0..1) at or above which a downshift counts as a kickdown.
const KICKDOWN_THROTTLE: f32 = 0.95;
/// Brake pedal (0..1) above which brake-induced downshifting engages.
const HARD_BRAKE_THRESHOLD: f32 = 0.5;
/// At or below this speed the box targets 1st gear (ready to launch).
const STANDSTILL_KMH: f32 = 5.0;
/// Engine RPM above this multiple of the road-speed-implied RPM means the wheels are spinning.
const SPIN_RPM_FACTOR: f32 = 1.15;
/// Number of valid samples kept per gear for the median calibration.
const CALIB_WINDOW: usize = 10;

/// Shift execution state. While `Shifting` we wait for `expected` to appear (or time out)
/// before commanding anything else — this avoids key spam and tolerates the brief "N" flash
/// that slow-shifting cars emit mid-shift.
enum ShiftPhase {
    Idle,
    Shifting { expected: i32, since: Instant },
}

pub struct DsgListener {
    /// Extrapolated speed at full redline (100% RPM) per gear index (0 unused; gears 1–10).
    /// The actual shift-point speed is derived live as `* (shift_rpm_pct/100)`. Session-only.
    pub gear_redline_speeds: [f32; 11],
    /// Last ≤10 valid redline-speed estimates per gear; the committed value is their median.
    /// Never locked — the median keeps updating so a wrong value self-corrects.
    gear_samples: [VecDeque<f32>; 11],
    /// Max RPM observed during manual redline calibration (standing still, brake+accel 100%).
    pub measured_redline: f32,
    phase: ShiftPhase,
    last_shift_done: Option<Instant>,
    upshift_pending_since: Option<Instant>,
    kickdown_cooldown_until: Option<Instant>,
    /// After a desync timeout, suppress new commands until this instant (absorbs late shifts).
    resync_until: Option<Instant>,
    // ── Debug telemetry (read by the Fun-tab debug panel) ──
    pub dbg_desired_gear: i32,
    pub dbg_effective_max_rpm: f32,
    pub dbg_shift_threshold: f32,
    pub dbg_kickdown_cooldown: bool,
    pub desync_count: u32,
    pub last_desync: Option<Instant>,
}

impl DsgListener {
    pub fn new() -> Self {
        Self {
            gear_redline_speeds: [0.0; 11],
            gear_samples: std::array::from_fn(|_| VecDeque::new()),
            measured_redline: 0.0,
            phase: ShiftPhase::Idle,
            last_shift_done: None,
            upshift_pending_since: None,
            kickdown_cooldown_until: None,
            resync_until: None,
            dbg_desired_gear: 0,
            dbg_effective_max_rpm: 0.0,
            dbg_shift_threshold: 0.0,
            dbg_kickdown_cooldown: false,
            desync_count: 0,
            last_desync: None,
        }
    }

    pub fn reset_calibration(&mut self) {
        self.gear_redline_speeds = [0.0; 11];
        self.gear_samples = std::array::from_fn(|_| VecDeque::new());
        self.measured_redline = 0.0;
    }

    /// Clear the shift state machine (e.g. on car change).
    pub fn reset_state(&mut self) {
        self.phase = ShiftPhase::Idle;
        self.last_shift_done = None;
        self.upshift_pending_since = None;
        self.kickdown_cooldown_until = None;
        self.resync_until = None;
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
        let max_rpm = pkt.engine_max_rpm;

        // Manual redline calibration: standing still, full brake (no handbrake), full throttle.
        if cfg.dsg_max_rpm_type == DsgMaxRpmSource::Manual
            && kmh < 1.0
            && pkt.brake == 255
            && pkt.hand_brake == 0
            && pkt.accel == 255
        {
            if rpm > self.measured_redline {
                self.measured_redline = rpm;
            }
        }

        // Effective max RPM by source (each falls back to the packet value when not yet available):
        //  - Game Data   → packet engine max RPM,
        //  - Auto Detect → the dynamically detected redline,
        //  - Manual      → the brake-stand measured redline.
        let effective_max_rpm = match cfg.dsg_max_rpm_type {
            DsgMaxRpmSource::GameData => max_rpm,
            DsgMaxRpmSource::AutoDetect => {
                if dynamic_max_rpm > 0.0 { dynamic_max_rpm } else { max_rpm }
            }
            DsgMaxRpmSource::Manual => {
                if self.measured_redline > 0.0 { self.measured_redline } else { max_rpm }
            }
        };
        self.dbg_effective_max_rpm = effective_max_rpm;
        self.dbg_shift_threshold = effective_max_rpm * (cfg.dsg_shift_rpm_pct / 100.0);

        let gear = pkt.gear as i32;
        let in_drive_gear = (1..=10).contains(&gear);

        // ── Continuous calibration ──────────────────────────────────────────────
        // Extrapolate each gear's speed at the full redline (100% RPM) from the current
        // RPM/speed ratio. The shift-point speed is derived live elsewhere as
        // redline_speed * (shift_rpm_pct/100), so it tracks the Shift RPM slider.
        // Conditions: meaningful RPM (≥50% of shift threshold), no wheel slip (≤0.5),
        // springs loaded (≥0.1), and the car moving straight forward (velocity aligned with
        // its heading within ~5%). Once a sample at ≥80% of shift threshold is recorded the
        // gear is locked.
        if in_drive_gear && effective_max_rpm > 0.0 && kmh > 5.0 {
            let shift_threshold = effective_max_rpm * (cfg.dsg_shift_rpm_pct / 100.0);
            let rpm_pct = rpm / shift_threshold;
            let gear_idx = gear as usize;

            let no_slip = pkt.tire_slip_ratio_fl.abs() <= 0.5
                && pkt.tire_slip_ratio_fr.abs() <= 0.5
                && pkt.tire_slip_ratio_rl.abs() <= 0.5
                && pkt.tire_slip_ratio_rr.abs() <= 0.5;

            let springs_loaded = pkt.normalized_suspension_travel_fl >= 0.1
                && pkt.normalized_suspension_travel_fr >= 0.1
                && pkt.normalized_suspension_travel_rl >= 0.1
                && pkt.normalized_suspension_travel_rr >= 0.1;

            // Forward velocity must account for ≥95% of total speed — i.e. driving straight,
            // not sliding/drifting/reversing/airborne. Otherwise the road-speed magnitude
            // includes motion the wheels aren't producing and kmh/rpm is wrong.
            let speed_ms = pkt.speed.abs();
            let moving_straight = speed_ms > 0.1 && pkt.velocity_z >= 0.95 * speed_ms;

            if rpm_pct >= 0.5 && no_slip && springs_loaded && moving_straight {
                // Continuously recompute the median of the last 10 valid redline-speed estimates.
                // The gear is never "locked": a wrong value is corrected as fresh samples slide
                // the window, and the median rejects the occasional outlier packet.
                let buf = &mut self.gear_samples[gear_idx];
                buf.push_back(kmh * effective_max_rpm / rpm);
                while buf.len() > CALIB_WINDOW {
                    buf.pop_front();
                }
                self.gear_redline_speeds[gear_idx] = median(buf).floor();
            }
        }

        if !cfg.dsg_enabled {
            return;
        }

        // ── Shift execution state machine ───────────────────────────────────────
        // While a shift is in flight, send nothing until the expected gear appears or we time
        // out. The 500 ms timeout applies regardless of a transitional "N" (so a stuck N can't
        // hang the box).
        if let ShiftPhase::Shifting { expected, since } = self.phase {
            if gear == expected {
                self.phase = ShiftPhase::Idle;
                self.last_shift_done = Some(Instant::now());
            } else if since.elapsed() >= Duration::from_millis(SHIFT_CONFIRM_TIMEOUT_MS) {
                // Desync: sync to the actual gear, then hold off re-commanding long enough to
                // absorb a shift that is merely landing late (prevents a double-shift / overshoot).
                self.phase = ShiftPhase::Idle;
                self.resync_until =
                    Some(Instant::now() + Duration::from_millis(POST_TIMEOUT_COOLDOWN_MS));
                self.desync_count += 1;
                self.last_desync = Some(Instant::now());
            }
            // Otherwise (old drive gear or transitional N, within 500 ms): keep waiting.
            return;
        }

        // Idle: only act on a real drive gear.
        if !in_drive_gear || effective_max_rpm <= 0.0 {
            self.upshift_pending_since = None;
            return;
        }

        // Post-desync cooldown: observe the live gear but issue no new command yet.
        if let Some(t) = self.resync_until {
            if Instant::now() < t {
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

        let shift_threshold = effective_max_rpm * (cfg.dsg_shift_rpm_pct / 100.0);
        let desired = self.select_desired_gear(pkt, cfg, gear, effective_max_rpm);
        self.dbg_desired_gear = desired;

        let now = Instant::now();

        // ── Kickdown detection + cooldown ──────────────────────────────────────
        // Throttle only counts when the engine is actually making power (hp > 0).
        let throttle = if pkt.power > 0.0 { pkt.accel as f32 / 255.0 } else { 0.0 };
        let in_cooldown = self.kickdown_cooldown_until.map(|t| now < t).unwrap_or(false);
        self.dbg_kickdown_cooldown = in_cooldown;

        if desired < gear && throttle >= KICKDOWN_THROTTLE {
            // Full-throttle downshift to a power gear → (re)arm the cooldown.
            self.kickdown_cooldown_until =
                Some(now + Duration::from_secs_f32(cfg.dsg_kickdown_cooldown_secs.max(0.0)));
        }

        // Resolve the gear we will actually move toward this frame.
        let mut target_gear = desired;

        if target_gear > gear {
            if rpm >= shift_threshold {
                // Redline protection: at/above the shift point, upshift immediately — bypass the
                // kickdown-cooldown hold and hysteresis. (Wheelspin is already handled in
                // select_desired_gear, which holds the gear while the wheels are spinning.)
                self.upshift_pending_since = None;
            } else {
                // During kickdown cooldown, hold the lower gear "ready" — only allow the
                // upshift once we're genuinely bouncing off the ceiling.
                if in_cooldown && rpm < shift_threshold * 0.98 {
                    target_gear = gear;
                }

                // Upshift hysteresis: the request must persist for the mode's delay.
                if target_gear > gear {
                    let delay = Duration::from_millis(cfg.dsg_active_tuning().upshift_delay_ms);
                    match self.upshift_pending_since {
                        Some(t) if t.elapsed() >= delay => {}
                        Some(_) => target_gear = gear, // still waiting out the delay
                        None => {
                            self.upshift_pending_since = Some(now);
                            target_gear = gear;
                        }
                    }
                } else {
                    self.upshift_pending_since = None;
                }
            }
        } else {
            // Not upshifting → clear any pending upshift timer.
            self.upshift_pending_since = None;
        }

        target_gear = target_gear.max(1);

        // ── Command a single confirmed step toward the target ──────────────────
        if target_gear != gear {
            let step: i32 = if target_gear > gear { 1 } else { -1 };
            let expected = gear + step;
            let key_char = if step > 0 { 'e' } else { 'q' };
            if let Some(key) = char_to_key(key_char) {
                input.press(key, 10);
            }
            self.phase = ShiftPhase::Shifting { expected, since: now };
            if step > 0 {
                self.upshift_pending_since = None;
            }
        }
    }

    /// Predicted engine RPM for `gear` at the current speed, referenced to the full redline.
    /// Returns `None` for uncalibrated gears.
    fn predicted_rpm(&self, gear: i32, kmh: f32, effective_max_rpm: f32) -> Option<f32> {
        let idx = gear as usize;
        if !(1..=10).contains(&gear) {
            return None;
        }
        let redline_speed = self.gear_redline_speeds[idx];
        if redline_speed <= 0.0 {
            return None;
        }
        Some(effective_max_rpm * kmh / redline_speed)
    }

    /// Choose the ideal gear for the current driving intent using the target-RPM model.
    fn select_desired_gear(
        &self,
        pkt: &ForzaPacket,
        cfg: &AppConfig,
        current_gear: i32,
        effective_max_rpm: f32,
    ) -> i32 {
        let kmh = pkt.speed_kmh();

        // Stopped / crawling → be in 1st, ready to launch. (At a standstill every gear's
        // predicted RPM is ~0, so the closest-target search would otherwise tie toward the
        // tallest gear.)
        if kmh < STANDSTILL_KMH {
            return 1;
        }

        // Don't shift away from an uncalibrated gear — we have to stay in it to sample it.
        // Hold until it has a valid calibration value (the continuous calibration fills it in
        // once RPM/grip/heading conditions are met). Standstill (above) still forces 1st.
        if (1..=10).contains(&current_gear)
            && self.gear_redline_speeds[current_gear as usize] <= 0.0
        {
            return current_gear;
        }

        let actual_rpm = pkt.current_engine_rpm;
        let shift_threshold = effective_max_rpm * (cfg.dsg_shift_rpm_pct / 100.0);

        // Wheelspin hold: if the engine is turning well faster than the current road speed implies
        // for this gear, the wheels are spinning — road-speed-based selection is unreliable, so
        // hold the gear (prevents upshifting on spin and spurious kickdown downshifts).
        if let Some(pred_cur) = self.predicted_rpm(current_gear, kmh, effective_max_rpm) {
            if actual_rpm > pred_cur * SPIN_RPM_FACTOR {
                return current_gear;
            }
        }

        // Stable limiter upshift: once genuinely at/above the shift point (and not spinning),
        // upshift one gear. Computed from actual RPM so it doesn't flicker current↔next the way
        // the closest-target search does as a gear's predicted RPM crosses the over-rev boundary.
        if actual_rpm >= shift_threshold && (1..10).contains(&current_gear) {
            return current_gear + 1;
        }

        // Throttle only counts when the engine is actually making power (hp > 0); when coasting
        // or engine-braking a pressed pedal shouldn't drive the gear choice.
        let throttle = if pkt.power > 0.0 { pkt.accel as f32 / 255.0 } else { 0.0 };
        let brake_f = pkt.brake as f32 / 255.0;
        let tuning = cfg.dsg_active_tuning();

        let cruise = tuning.cruise_rpm_pct / 100.0;
        let brake_add = if brake_f > HARD_BRAKE_THRESHOLD {
            brake_f * (tuning.brake_downshift_pct / 100.0)
        } else {
            0.0
        };
        let target_frac =
            (cruise + (1.0 - cruise) * throttle + brake_add).clamp(cruise, POWER_HEADROOM_FRAC);
        let target_rpm = shift_threshold * target_frac;

        // Pick the calibrated gear whose predicted RPM is closest to target, excluding gears
        // that would over-rev. Ties prefer the taller gear (lower revs).
        let mut best: Option<(i32, f32)> = None;
        let mut any_calibrated = false;
        for g in 1..=10 {
            let Some(pred) = self.predicted_rpm(g, kmh, effective_max_rpm) else {
                continue;
            };
            any_calibrated = true;
            // `pred` is the RPM we'd land at after shifting into gear g. A downshift/kickdown
            // target is only valid if it leaves ≥10% of the RPM range free afterwards; higher or
            // equal gears just must not over-rev.
            let max_pred = if g < current_gear {
                shift_threshold * POWER_HEADROOM_FRAC
            } else {
                shift_threshold
            };
            if pred > max_pred {
                continue;
            }
            let dist = (pred - target_rpm).abs();
            match best {
                // Strictly closer wins; on a tie, larger g (taller, listed later) replaces.
                Some((_, bd)) if dist <= bd => best = Some((g, dist)),
                None => best = Some((g, dist)),
                _ => {}
            }
        }

        // Upshift into an *uncalibrated* next gear. The closest-target loop can only choose
        // among already-calibrated gears, so without this the box would never move up into
        // (and thus never calibrate) a higher gear. If the current gear has reached its upshift
        // target and the next gear up has no calibration yet, step into it.
        if (1..10).contains(&current_gear)
            && self.gear_redline_speeds[(current_gear + 1) as usize] <= 0.0
        {
            if let Some(cur_pred) = self.predicted_rpm(current_gear, kmh, effective_max_rpm) {
                if cur_pred >= target_rpm {
                    return current_gear + 1;
                }
            }
        }

        if let Some((g, _)) = best {
            let g = g.max(1);
            // Cap upshifts to a single gear step. Shifts execute one gear at a time anyway, and
            // this keeps the target stable/sequential instead of jumping to a far (possibly
            // mis-calibrated) gear — which previously made the target flicker e.g. 3↔7 whenever
            // the next gear's predicted RPM crossed the over-rev boundary.
            if g > current_gear + 1 {
                return current_gear + 1;
            }
            // Downshift deadzone: while cruising (not full throttle, not braking hard) hold the
            // current gear until revs drop below the deadzone — avoids busy downshifting when you
            // modulate the throttle. Kickdown / hard braking still downshift.
            if g < current_gear
                && throttle < KICKDOWN_THROTTLE
                && brake_f <= HARD_BRAKE_THRESHOLD
            {
                let deadzone_rpm = shift_threshold * (cfg.dsg_downshift_deadzone_pct / 100.0);
                if pkt.current_engine_rpm >= deadzone_rpm {
                    return current_gear;
                }
            }
            return g;
        }

        // ── Fallback before calibration: simple single-step rule. ──────────────
        if !any_calibrated {
            let rpm = pkt.current_engine_rpm;
            if rpm >= target_rpm && kmh > 1.0 {
                return current_gear + 1;
            }
            let lug_rpm = shift_threshold * (cruise * 0.5);
            if rpm < lug_rpm && current_gear > 1 {
                return current_gear - 1;
            }
        }

        current_gear
    }
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
