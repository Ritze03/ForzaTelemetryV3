use crate::config::AppConfig;
use crate::input::{char_to_key, InputSender};
use crate::packet::ForzaPacket;
use std::time::{Duration, Instant};

pub struct BackfireListener {
    last_backfire_rpm: f32,
    last_kmh: f32,
    pub last_min_rpm: f32,
    pub last_max_rpm: f32,
    active_until: Option<Instant>,
}

impl BackfireListener {
    pub fn new() -> Self {
        Self {
            last_backfire_rpm: 0.0,
            last_kmh: 9999.0,
            last_min_rpm: 0.0,
            last_max_rpm: 0.0,
            active_until: None,
        }
    }

    /// True while the synthetic 'W' keypress is (or was very recently) held, meaning the game
    /// may still be reporting an artificial pkt.accel from it in telemetry.
    pub fn is_active(&self) -> bool {
        match self.active_until {
            Some(deadline) => Instant::now() < deadline,
            None => false,
        }
    }

    pub fn update(&mut self, pkt: &ForzaPacket, cfg: &AppConfig, input: &InputSender) {
        if !cfg.backfire_enabled || pkt.is_race_on == 0 {
            return;
        }

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

        if off_throttle && no_brake && in_rpm_range && rpm_delta_ok && not_accelerating {
            self.last_backfire_rpm = rpm;
            if let Some(key) = char_to_key('w') {
                input.press(key, cfg.backfire_accel_time_ms, 0);
                // +150ms margin: the game/telemetry round-trip can still report the
                // artificial accel value for a few frames after the key is released.
                self.active_until = Some(
                    Instant::now()
                        + Duration::from_millis(cfg.backfire_accel_time_ms)
                        + Duration::from_millis(150),
                );
            }
        } else if !(off_throttle && no_brake && in_rpm_range) {
            self.last_backfire_rpm = 0.0;
        }

        self.last_kmh = kmh;
    }
}
