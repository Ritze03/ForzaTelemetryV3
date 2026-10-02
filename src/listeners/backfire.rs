use crate::config::{AppConfig, BackfireDynamicMode};
use crate::input::{char_to_key, InputSender};
use crate::packet::ForzaPacket;
use std::time::Duration;

/// How long after key-up the game may still report the synthetic accel back in
/// telemetry (empirical round-trip margin).
pub const ECHO_MS: u64 = 150;

/// Drift detection: while any wheel's slip-ratio magnitude exceeds this, the car
/// is sliding / spinning up rather than in a clean lift-off, so the pop is
/// suppressed. Gated behind `backfire_drift_detection`.
pub const DRIFT_SLIP_MAX: f32 = 1.1;

/// Packet-based hold safety: if telemetry stops before the next packet arrives,
/// the input worker auto-releases the held key after this long so it can't stick.
pub const MAX_HOLD_MS: u64 = 120;

/// Grace to add on top of the echo window when CHECKING it **from the UI thread**:
/// with a render-FPS limit active, packets sit in the drain queue for up to one
/// frame interval, so a packet generated inside the window can be drawn after it
/// expired. Single source of truth for every UI-side consumer
/// (`ForzaApp::backfire_echo_active`).
///
/// The listener thread does **not** use it: it reads the window the instant the
/// packet arrives, with no frame in between, so any grace there would only widen
/// the suppression window for no reason.
pub fn echo_grace(cfg: &AppConfig) -> Duration {
    if cfg.fps_limit_enabled {
        Duration::from_secs_f32(1.0 / cfg.fps_limit.max(1.0))
    } else {
        Duration::ZERO
    }
}

/// Max-burst-duration limiter (`backfire_limit_duration`). Pure timing logic so it
/// can be unit-tested: backfire may run continuously for `limit_ms`, then it is
/// blocked until the driver touches the throttle or downshifts.
#[derive(Default)]
pub struct BurstLimiter {
    run_start: Option<u64>,
    blocked: bool,
    last_gear: u8,
}

impl BurstLimiter {
    /// `wants`: backfire conditions hold (lift-off in RPM range) this packet.
    /// `gas`: the driver's real throttle (echo of our own key already excluded).
    /// `limit_ms`: `None` = unlimited. Returns whether backfire is allowed now.
    pub fn update(&mut self, now_ms: u64, wants: bool, gas: u8, gear: u8, limit_ms: Option<u32>) -> bool {
        if gas > 0 || gear < self.last_gear {
            self.blocked = false;
            self.run_start = None;
        }
        self.last_gear = gear;
        let Some(limit) = limit_ms else {
            self.blocked = false;
            self.run_start = None;
            return true;
        };
        if !wants {
            self.run_start = None; // run ended; a blocked state persists until re-armed
        } else if !self.blocked {
            let start = *self.run_start.get_or_insert(now_ms);
            if now_ms.saturating_sub(start) >= limit as u64 {
                self.blocked = true;
            }
        }
        !self.blocked
    }
}

pub struct BackfireListener {
    burst: BurstLimiter,
    last_backfire_rpm: f32,
    last_kmh: f32,
    holding: bool,
    pub last_min_rpm: f32,
    pub last_max_rpm: f32,
}

/// Display-only copy of the listener, published to the UI by the listener thread
/// (`listeners/worker.rs`) — the real listener lives on that thread.
#[derive(Clone, Copy, Default)]
pub struct BackfireView {
    pub last_min_rpm: f32,
    pub last_max_rpm: f32,
}

impl BackfireListener {
    /// Snapshot of what the UI shows (the live RPM window).
    pub fn view(&self) -> BackfireView {
        BackfireView {
            last_min_rpm: self.last_min_rpm,
            last_max_rpm: self.last_max_rpm,
        }
    }

    pub fn new() -> Self {
        Self {
            burst: BurstLimiter::default(),
            last_backfire_rpm: 0.0,
            last_kmh: 9999.0,
            holding: false,
            last_min_rpm: 0.0,
            last_max_rpm: 0.0,
        }
    }

    pub fn update(&mut self, pkt: &ForzaPacket, cfg: &AppConfig, input: &InputSender, pps: f32) {
        // Packet-based hold: this packet is the "next frame", so release the key
        // held on the previous trigger now — BEFORE any early return, so ending a
        // race / disabling / standstill never leaves W stuck down.
        if self.holding {
            input.release();
            self.holding = false;
        }

        if !cfg.backfire_enabled || pkt.is_race_on == 0 {
            return;
        }

        // Dynamic duration: hold the key for one game frame, derived from the current
        // packet rate (the game emits one packet per frame). Clamped so a stalled/slow
        // feed can't produce an absurd length; falls back to the fixed value if the rate
        // isn't usable yet.
        let press_ms = if cfg.backfire_dynamic_duration && pps >= 1.0 {
            (1000.0 / pps).round().clamp(4.0, 40.0) as u64
        } else {
            cfg.backfire_accel_time_ms
        };

        let kmh = pkt.speed_kmh();
        let rpm = pkt.current_engine_rpm;

        // Below 1 km/h counts as standstill — telemetry speed is rarely exactly 0.0.
        if cfg.backfire_disable_standstill && kmh < 1.0 {
            return;
        }

        let (min_rpm, max_rpm) = if cfg.backfire_dynamic_rpm {
            (
                pkt.engine_max_rpm * (cfg.backfire_dynamic_min_pct / 100.0),
                pkt.engine_max_rpm * (cfg.backfire_dynamic_max_pct / 100.0),
            )
        } else {
            (cfg.backfire_min_rpm, cfg.backfire_max_rpm)
        };

        self.last_min_rpm = min_rpm;
        self.last_max_rpm = max_rpm;

        let in_rpm_range = (rpm >= min_rpm && rpm <= max_rpm) || cfg.backfire_test_mode;
        let rpm_delta_ok = (self.last_backfire_rpm - rpm).abs() >= cfg.backfire_interval_rpm;
        let off_throttle = pkt.accel == 0;
        let no_brake = pkt.brake == 0 || kmh == 0.0;
        let not_accelerating = self.last_kmh >= kmh;

        // Drift detection: a slide/wheelspin (any wheel's slip-ratio magnitude past
        // the threshold) isn't a clean lift-off, so never pop while it's happening.
        let max_slip_ratio = pkt
            .tire_slip_ratio_fl
            .abs()
            .max(pkt.tire_slip_ratio_fr.abs())
            .max(pkt.tire_slip_ratio_rl.abs())
            .max(pkt.tire_slip_ratio_rr.abs());
        let no_drift = !cfg.backfire_drift_detection || max_slip_ratio <= DRIFT_SLIP_MAX;

        // Max burst duration. Our own pop's echo shows up as accel and must neither
        // count as throttle (re-arm) nor end the run, so it is masked here.
        let echo = input.synthetic_active(Duration::ZERO);
        let burst_ok = self.burst.update(
            pkt.timestamp_ms as u64,
            (off_throttle || echo) && no_brake && in_rpm_range,
            if echo { 0 } else { pkt.accel },
            pkt.gear,
            cfg.backfire_limit_duration.then_some(cfg.backfire_max_duration_ms),
        );

        if burst_ok && off_throttle && no_brake && in_rpm_range && rpm_delta_ok && not_accelerating && no_drift {
            self.last_backfire_rpm = rpm;
            if let Some(key) = char_to_key('w') {
                if cfg.backfire_dynamic_duration
                    && cfg.backfire_dynamic_mode == BackfireDynamicMode::PacketBased
                {
                    // Hold W until the NEXT packet arrives (released at the top of the
                    // next update) — an exact one-frame tap. MAX_HOLD_MS bounds a stuck
                    // key if telemetry stops before the next packet.
                    input.hold_tracked(key, MAX_HOLD_MS, ECHO_MS);
                    self.holding = true;
                } else {
                    // Tracked: the input worker anchors the echo window at the ACTUAL
                    // key emission (see input::EchoWindow), so queued presses ahead of
                    // this one can't erode it.
                    input.press_tracked(key, press_ms, 0, ECHO_MS);
                }
            }
        } else if !(off_throttle && no_brake && in_rpm_range)
            // No grace: this runs on the listener thread, one step after the packet
            // arrived — see `echo_grace`.
            && !echo
        {
            // Don't react to our OWN echo: the fake accel makes off_throttle false
            // for a few frames, and zeroing last_backfire_rpm here would make
            // rpm_delta_ok trivially true right after — machine-gunning pops and
            // defeating the backfire_interval_rpm spacing entirely.
            self.last_backfire_rpm = 0.0;
        }

        self.last_kmh = kmh;
    }
}

#[cfg(test)]
mod tests {
    use super::BurstLimiter;

    const L: Option<u32> = Some(1000);

    #[test]
    fn unlimited_when_disabled() {
        let mut b = BurstLimiter::default();
        for t in 0..20 {
            assert!(b.update(t * 1000, true, 0, 3, None));
        }
    }

    #[test]
    fn stops_after_limit() {
        let mut b = BurstLimiter::default();
        assert!(b.update(0, true, 0, 3, L));
        assert!(b.update(999, true, 0, 3, L));
        assert!(!b.update(1000, true, 0, 3, L));
    }

    #[test]
    fn stays_stopped_while_rpm_in_range() {
        let mut b = BurstLimiter::default();
        b.update(0, true, 0, 3, L);
        assert!(!b.update(1500, true, 0, 3, L));
        assert!(!b.update(5000, true, 0, 3, L));
        // leaving and re-entering the range alone does not re-arm
        assert!(!b.update(5100, false, 0, 3, L));
        assert!(!b.update(5200, true, 0, 3, L));
    }

    #[test]
    fn rearms_on_gas() {
        let mut b = BurstLimiter::default();
        b.update(0, true, 0, 3, L);
        assert!(!b.update(1500, true, 0, 3, L));
        assert!(b.update(1600, false, 1, 3, L));
        // timer restarts from zero at the next burst
        assert!(b.update(2000, true, 0, 3, L));
        assert!(b.update(2900, true, 0, 3, L));
        assert!(!b.update(3000, true, 0, 3, L));
    }

    #[test]
    fn rearms_on_downshift() {
        let mut b = BurstLimiter::default();
        b.update(0, true, 0, 4, L);
        assert!(!b.update(1500, true, 0, 4, L));
        assert!(b.update(1600, true, 0, 3, L));
    }

    #[test]
    fn upshift_does_not_rearm() {
        let mut b = BurstLimiter::default();
        b.update(0, true, 0, 3, L);
        assert!(!b.update(1500, true, 0, 3, L));
        assert!(!b.update(1600, true, 0, 4, L));
    }
}
