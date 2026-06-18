use std::time::{Duration, Instant};

use crate::packet::ForzaPacket;

/// Configurable acceleration test (e.g. 0→100 or 80→120 km/h).
#[derive(Default)]
pub struct AccelTest {
    pub result_secs: Option<f32>,
    pub running: bool,
    pub progress: f32,
    pub current_g: f32,
    start_time: Option<Instant>,
    start_speed: f32,
    end_speed: f32,
}

impl AccelTest {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn update(&mut self, pkt: &ForzaPacket, start_kmh: f32, end_kmh: f32) {
        if pkt.is_race_on == 0 {
            return;
        }

        let speed = pkt.speed_kmh();

        if !self.running {
            if self.start_speed != start_kmh || self.end_speed != end_kmh {
                self.start_speed = start_kmh;
                self.end_speed = end_kmh;
                self.result_secs = None;
                self.progress = 0.0;
            }
            if speed >= start_kmh && speed < end_kmh {
                self.running = true;
                self.start_time = Some(Instant::now());
                self.result_secs = None;
            }
        } else {
            let range = (self.end_speed - self.start_speed).max(1.0);
            self.progress = ((speed - self.start_speed) / range).clamp(0.0, 1.0);
            self.current_g = pkt.acceleration_z / 9.81;

            if speed >= self.end_speed {
                if let Some(start) = self.start_time.take() {
                    self.result_secs = Some(start.elapsed().as_secs_f32());
                }
                self.running = false;
                self.progress = 1.0;
            }

            if speed < self.start_speed - 5.0 {
                self.running = false;
                self.progress = 0.0;
            }
        }
    }
}

/// Configurable braking/deceleration test.
/// Dynamic mode: starts on any speed decrease above the start threshold;
/// aborts if the car re-accelerates for more than 500 ms or exceeds the
/// run-start speed by more than 5 km/h (matches V2.0 behaviour).
#[derive(Default)]
pub struct DecelTest {
    pub result_secs: Option<f32>,
    pub running: bool,
    pub progress: f32,
    pub current_g: f32,
    pub dynamic_mode: bool,
    pub dynamic_start: f32,
    start_time: Option<Instant>,
    start_speed: f32,
    end_speed: f32,
    last_speed: f32,
    accel_start: Option<Instant>,
}

impl DecelTest {
    pub fn reset(&mut self) {
        let dynamic = self.dynamic_mode;
        *self = Self::default();
        self.dynamic_mode = dynamic;
    }

    pub fn update(&mut self, pkt: &ForzaPacket, start_kmh: f32, end_kmh: f32) {
        if pkt.is_race_on == 0 {
            return;
        }

        let speed = pkt.speed_kmh();

        if self.start_speed != start_kmh || self.end_speed != end_kmh {
            self.start_speed = start_kmh;
            self.end_speed = end_kmh;
            self.result_secs = None;
            self.progress = 0.0;
        }

        if !self.running {
            let arm = if self.dynamic_mode {
                // Any deceleration while above the configured start speed
                speed > self.start_speed && self.last_speed > speed
            } else {
                speed >= self.start_speed && self.last_speed < self.start_speed
            };

            if arm {
                self.running = true;
                self.dynamic_start = speed;
                self.start_time = Some(Instant::now());
                self.result_secs = None;
                self.accel_start = None;
            }
        } else {
            let range = (self.start_speed - self.end_speed).max(1.0);
            self.progress = ((self.start_speed - speed) / range).clamp(0.0, 1.0);
            self.current_g = -pkt.acceleration_z / 9.81;

            if speed <= self.end_speed {
                if let Some(start) = self.start_time.take() {
                    self.result_secs = Some(start.elapsed().as_secs_f32());
                }
                self.running = false;
                self.progress = 1.0;
            }

            if self.dynamic_mode {
                // Abort: speed jumped more than 5 km/h above where the run started
                if speed > self.dynamic_start + 5.0 {
                    self.running = false;
                    self.progress = 0.0;
                }
                // Abort: re-accelerating for more than 500 ms
                if speed > self.last_speed {
                    if self.accel_start.is_none() {
                        self.accel_start = Some(Instant::now());
                    } else if self.accel_start
                        .map(|t| t.elapsed() > Duration::from_millis(500))
                        .unwrap_or(false)
                    {
                        self.running = false;
                        self.progress = 0.0;
                    }
                } else {
                    self.accel_start = None;
                }
            } else if speed > self.start_speed + 5.0 {
                self.running = false;
                self.progress = 0.0;
            }
        }

        self.last_speed = speed;
    }
}

pub struct PerfTest {
    pub accel: AccelTest,
    pub decel: DecelTest,
}

impl PerfTest {
    pub fn new() -> Self {
        Self {
            accel: AccelTest::default(),
            decel: DecelTest::default(),
        }
    }

    pub fn update(
        &mut self,
        pkt: &ForzaPacket,
        accel_start: f32,
        accel_end: f32,
        decel_start: f32,
        decel_end: f32,
    ) {
        self.accel.update(pkt, accel_start, accel_end);
        self.decel.update(pkt, decel_start, decel_end);
    }

    pub fn reset(&mut self) {
        self.accel.reset();
        self.decel.reset();
    }
}
