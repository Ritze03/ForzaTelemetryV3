use crate::packet::ForzaPacket;

/// Seconds between two packet timestamps (`wrapping_sub`, like the sprint timer, so a `u32`
/// wrap of the game clock doesn't break a run).
///
/// *Why packet timestamps and not `Instant::now()` at drain time:* the UI drains the packet
/// queue once per frame, so a UI stall (a frame that takes 160 ms) delivers a batch of packets
/// at once and wall-clock timing at drain time is off by up to the stall. The game's own clock
/// in each packet is exact regardless of when the UI got round to it.
fn ts_diff_secs(start: u32, end: u32) -> f32 {
    end.wrapping_sub(start) as f32 / 1000.0
}

/// Configurable acceleration test (e.g. 0→100 or 80→120 km/h).
#[derive(Default)]
pub struct AccelTest {
    pub result_secs: Option<f32>,
    pub running: bool,
    pub progress: f32,
    pub current_g: f32,
    /// `pkt.timestamp_ms` of the packet the run started on.
    start_time: Option<u32>,
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
                self.start_time = Some(pkt.timestamp_ms);
                self.result_secs = None;
            }
        } else {
            let range = (self.end_speed - self.start_speed).max(1.0);
            self.progress = ((speed - self.start_speed) / range).clamp(0.0, 1.0);
            self.current_g = pkt.acceleration_z / 9.81;

            if speed >= self.end_speed {
                if let Some(start) = self.start_time.take() {
                    self.result_secs = Some(ts_diff_secs(start, pkt.timestamp_ms));
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
    start_time: Option<u32>,
    start_speed: f32,
    end_speed: f32,
    last_speed: f32,
    /// `pkt.timestamp_ms` since when the car has been re-accelerating (dynamic-mode abort).
    accel_start: Option<u32>,
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
                self.start_time = Some(pkt.timestamp_ms);
                self.result_secs = None;
                self.accel_start = None;
            }
        } else {
            let range = (self.start_speed - self.end_speed).max(1.0);
            self.progress = ((self.start_speed - speed) / range).clamp(0.0, 1.0);
            self.current_g = -pkt.acceleration_z / 9.81;

            if speed <= self.end_speed {
                if let Some(start) = self.start_time.take() {
                    self.result_secs = Some(ts_diff_secs(start, pkt.timestamp_ms));
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
                        self.accel_start = Some(pkt.timestamp_ms);
                    } else if self.accel_start
                        .map(|t| pkt.timestamp_ms.wrapping_sub(t) > 500)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A running car: `speed` in km/h at game-clock `ts` ms.
    fn pkt(ts: u32, kmh: f32) -> ForzaPacket {
        ForzaPacket { is_race_on: 1, timestamp_ms: ts, speed: kmh / 3.6, ..Default::default() }
    }

    #[test]
    fn accel_time_comes_from_packet_timestamps_not_drain_time() {
        // All packets are fed back-to-back (a UI stall drains them in one go), but the game
        // clock says the run took 4.2 s.
        let mut t = AccelTest::default();
        t.update(&pkt(1_000, 10.0), 0.0, 100.0);
        assert!(t.running);
        t.update(&pkt(3_000, 60.0), 0.0, 100.0);
        t.update(&pkt(5_200, 101.0), 0.0, 100.0);
        assert!(!t.running);
        assert_eq!(t.result_secs, Some(4.2));
    }

    #[test]
    fn accel_time_survives_timestamp_wrap() {
        let mut t = AccelTest::default();
        t.update(&pkt(u32::MAX - 999, 10.0), 0.0, 100.0);
        t.update(&pkt(1_000, 101.0), 0.0, 100.0);
        assert_eq!(t.result_secs, Some(2.0));
    }

    #[test]
    fn decel_time_comes_from_packet_timestamps() {
        let mut t = DecelTest::default();
        t.update(&pkt(0, 90.0), 100.0, 10.0);
        t.update(&pkt(1_000, 100.0), 100.0, 10.0); // crosses start upward: arms
        assert!(t.running);
        t.update(&pkt(4_500, 50.0), 100.0, 10.0);
        t.update(&pkt(7_300, 9.0), 100.0, 10.0);
        assert!(!t.running);
        assert_eq!(t.result_secs, Some(6.3));
    }

    #[test]
    fn dynamic_decel_abort_uses_the_game_clock() {
        let mut t = DecelTest { dynamic_mode: true, ..Default::default() };
        t.update(&pkt(0, 150.0), 100.0, 10.0);
        t.update(&pkt(100, 140.0), 100.0, 10.0); // slowing above start: arms
        assert!(t.running);
        // Re-accelerating, 400 ms of game clock: still running no matter how fast we drain.
        t.update(&pkt(200, 141.0), 100.0, 10.0);
        t.update(&pkt(600, 142.0), 100.0, 10.0);
        assert!(t.running);
        // 700 ms of game clock since the re-acceleration started: abort.
        t.update(&pkt(900, 143.0), 100.0, 10.0);
        assert!(!t.running);
    }
}
