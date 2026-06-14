use crate::packet::ForzaPacket;

/// Tracks automatic 0→100 and 100→200 km/h sprint times.
#[derive(Default)]
pub struct SprintTimer {
    // 0→100
    pub zero_to_hundred: Option<f32>,
    z_start_ms: Option<u32>,

    // 100→200
    pub hundred_to_two: Option<f32>,
    h_start_ms: Option<u32>,

    last_speed_kmh: f32,
}

impl SprintTimer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn update(&mut self, pkt: &ForzaPacket) {
        if pkt.is_race_on == 0 {
            return;
        }

        let speed = pkt.speed_kmh();
        let ts = pkt.timestamp_ms;

        // 0→100: arm when below 5 km/h, trigger crossing 100
        if self.last_speed_kmh < 5.0 && speed >= 5.0 {
            self.z_start_ms = Some(ts);
            self.zero_to_hundred = None;
        }
        if let Some(start) = self.z_start_ms {
            if self.zero_to_hundred.is_none() && speed >= 100.0 {
                self.zero_to_hundred = Some(ts_diff_secs(start, ts));
                self.z_start_ms = None;
            }
        }

        // 100→200: arm when crossing 100, trigger crossing 200
        if self.last_speed_kmh < 100.0 && speed >= 100.0 {
            self.h_start_ms = Some(ts);
            self.hundred_to_two = None;
        }
        if let Some(start) = self.h_start_ms {
            if self.hundred_to_two.is_none() && speed >= 200.0 {
                self.hundred_to_two = Some(ts_diff_secs(start, ts));
                self.h_start_ms = None;
            }
        }

        self.last_speed_kmh = speed;
    }
}

fn ts_diff_secs(start: u32, end: u32) -> f32 {
    // Handle timestamp overflow (wraps at u32::MAX ms ≈ 49 days)
    let diff = end.wrapping_sub(start);
    diff as f32 / 1000.0
}
