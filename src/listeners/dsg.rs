use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::config::AppConfig;
use crate::input::{InputSender, char_to_key};
use crate::packet::ForzaPacket;

const TRANSMISSION_DELAY_MS: u64 = 70;
const UPSHIFT_RPM_BEFORE_MAX: f32 = 500.0;
const UPSHIFT_ALLOWED_TIRE_SLIP: f32 = 1.3;
const DOWNSHIFT_RPM: f32 = 2000.0;
const DOWNSHIFT_RPM_ACCEL_FACTOR: f32 = 1000.0;
const KICKDOWN_SPEED_THRESHOLD: f32 = 20.0;
const KICKDOWN_BLOCK_SECS: u64 = 10;

pub struct DsgListener {
    pub gear_max_speeds: [f32; 11],
    accel_history: VecDeque<f32>,
    last_shift: Option<Instant>,
    kickdown_blocked_until: Option<Instant>,
    kickdown_activation_delay: Option<Instant>,
}

impl DsgListener {
    pub fn new(gear_max_speeds: [f32; 11]) -> Self {
        Self {
            gear_max_speeds,
            accel_history: VecDeque::new(),
            last_shift: None,
            kickdown_blocked_until: None,
            kickdown_activation_delay: None,
        }
    }

    pub fn update(&mut self, pkt: &ForzaPacket, cfg: &AppConfig, input: &InputSender) {
        if !cfg.dsg_enabled || pkt.is_race_on == 0 {
            return;
        }

        let gear = pkt.gear as i32;
        if gear <= 0 {
            return;
        }

        let accel_f = pkt.accel as f32 / 255.0;
        let kmh = pkt.speed_kmh();
        let rpm = pkt.current_engine_rpm;
        let max_rpm = pkt.engine_max_rpm;

        self.accel_history.push_back(accel_f);
        while self.accel_history.len() > 20 {
            self.accel_history.pop_front();
        }

        // Calibration mode: record max speed per gear, don't shift
        if cfg.dsg_calibration_mode {
            if max_rpm > 0.0 && rpm >= (max_rpm - 100.0) && gear >= 1 && gear <= 10 {
                let idx = gear as usize;
                let kmh_floor = kmh.floor();
                if kmh_floor > self.gear_max_speeds[idx] {
                    self.gear_max_speeds[idx] = kmh_floor;
                }
            }
            return;
        }

        let now = Instant::now();
        let transmission_ready = self.last_shift
            .map(|t| now.duration_since(t) >= Duration::from_millis(TRANSMISSION_DELAY_MS))
            .unwrap_or(true);

        let kickdown_blocked = self.kickdown_blocked_until
            .map(|t| now < t)
            .unwrap_or(false);

        let avg_slip = (pkt.tire_slip_ratio_fl.abs()
            + pkt.tire_slip_ratio_fr.abs()
            + pkt.tire_slip_ratio_rl.abs()
            + pkt.tire_slip_ratio_rr.abs())
            / 4.0;

        let mut target_gear = gear;

        // Upshift
        let upshift_rpm_hit = rpm > (max_rpm - UPSHIFT_RPM_BEFORE_MAX);
        let can_upshift = upshift_rpm_hit
            && avg_slip < UPSHIFT_ALLOWED_TIRE_SLIP
            && kmh > 0.0
            && transmission_ready
            && (!kickdown_blocked || self.accel_has_been_pressed(0.5));

        if can_upshift {
            target_gear = gear + 1;
        }

        // Downshift / kickdown
        let downshift_threshold = DOWNSHIFT_RPM + DOWNSHIFT_RPM_ACCEL_FACTOR * accel_f;
        if rpm < downshift_threshold && accel_f < 1.0 && gear > 1 {
            target_gear = gear - 1;
            self.kickdown_activation_delay = None;
        } else if accel_f >= 1.0 && !kickdown_blocked {
            if self.kickdown_activation_delay.is_none() {
                self.kickdown_activation_delay = Some(now);
            }
            let kickdown_ready = self
                .kickdown_activation_delay
                .map(|t| now.duration_since(t) >= Duration::from_millis(100))
                .unwrap_or(false);
            if kickdown_ready {
                let optimal = self.get_optimal_gear(kmh);
                if optimal > 0 && optimal < gear {
                    target_gear = optimal;
                    self.kickdown_blocked_until =
                        Some(now + Duration::from_secs(KICKDOWN_BLOCK_SECS));
                    self.kickdown_activation_delay = None;
                }
            }
        } else {
            self.kickdown_activation_delay = None;
        }

        // Execute shifts
        if target_gear != gear && transmission_ready {
            let diff = target_gear - gear;
            let key_char = if diff > 0 { 'e' } else { 'q' };
            let steps = diff.unsigned_abs() as usize;
            self.last_shift = Some(now);
            if let Some(key) = char_to_key(key_char) {
                for _ in 0..steps {
                    input.press(key, 10);
                }
            }
        }
    }

    fn accel_has_been_pressed(&self, min_val: f32) -> bool {
        !self.accel_history.is_empty() && self.accel_history.iter().all(|&v| v >= min_val)
    }

    fn get_optimal_gear(&self, kmh: f32) -> i32 {
        for (i, &max_speed) in self.gear_max_speeds.iter().enumerate().skip(1) {
            if max_speed > kmh + KICKDOWN_SPEED_THRESHOLD {
                return i as i32;
            }
        }
        -1
    }
}
