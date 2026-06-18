use crate::config::AppConfig;
use crate::input::{char_to_key, InputSender};
use crate::packet::ForzaPacket;

pub struct BackfireListener {
    last_backfire_rpm: f32,
    last_kmh: f32,
    pub last_min_rpm: f32,
    pub last_max_rpm: f32,
}

impl BackfireListener {
    pub fn new() -> Self {
        Self {
            last_backfire_rpm: 0.0,
            last_kmh: 9999.0,
            last_min_rpm: 0.0,
            last_max_rpm: 0.0,
        }
    }

    pub fn update(&mut self, pkt: &ForzaPacket, cfg: &AppConfig, input: &InputSender) {
        if !cfg.backfire_enabled || pkt.is_race_on == 0 {
            return;
        }

        let kmh = pkt.speed_kmh();
        let rpm = pkt.current_engine_rpm;

        let (min_rpm, max_rpm) = if cfg.backfire_dynamic_rpm {
            (pkt.engine_max_rpm * 0.6, pkt.engine_max_rpm - 500.0)
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
                input.press(key, cfg.backfire_accel_time_ms);
            }
        } else if !(off_throttle && no_brake && in_rpm_range) {
            self.last_backfire_rpm = 0.0;
        }

        self.last_kmh = kmh;
    }
}
