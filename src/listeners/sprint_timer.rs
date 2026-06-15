use crate::packet::ForzaPacket;

/// Tracks automatic sprint times: 0→100, 100→200, 200→300, 300→400, 400→500 km/h.
/// All splits are always captured; which ones are displayed depends on the active GameMode.
#[derive(Default)]
pub struct SprintTimer {
    pub zero_to_hundred:  Option<f32>,
    z_start_ms:           Option<u32>,

    pub hundred_to_two:   Option<f32>,
    h_start_ms:           Option<u32>,

    pub two_to_three:     Option<f32>,
    t_start_ms:           Option<u32>,

    pub three_to_four:    Option<f32>,
    th_start_ms:          Option<u32>,

    pub four_to_five:     Option<f32>,
    f_start_ms:           Option<u32>,

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

        // 0 → 100: arm below 5 km/h, trigger at 100
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

        // 100 → 200: arm at 100, trigger at 200
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

        // 200 → 300: arm at 200, trigger at 300
        if self.last_speed_kmh < 200.0 && speed >= 200.0 {
            self.t_start_ms = Some(ts);
            self.two_to_three = None;
        }
        if let Some(start) = self.t_start_ms {
            if self.two_to_three.is_none() && speed >= 300.0 {
                self.two_to_three = Some(ts_diff_secs(start, ts));
                self.t_start_ms = None;
            }
        }

        // 300 → 400: arm at 300, trigger at 400
        if self.last_speed_kmh < 300.0 && speed >= 300.0 {
            self.th_start_ms = Some(ts);
            self.three_to_four = None;
        }
        if let Some(start) = self.th_start_ms {
            if self.three_to_four.is_none() && speed >= 400.0 {
                self.three_to_four = Some(ts_diff_secs(start, ts));
                self.th_start_ms = None;
            }
        }

        // 400 → 500: arm at 400, trigger at 500
        if self.last_speed_kmh < 400.0 && speed >= 400.0 {
            self.f_start_ms = Some(ts);
            self.four_to_five = None;
        }
        if let Some(start) = self.f_start_ms {
            if self.four_to_five.is_none() && speed >= 500.0 {
                self.four_to_five = Some(ts_diff_secs(start, ts));
                self.f_start_ms = None;
            }
        }

        self.last_speed_kmh = speed;
    }
}

fn ts_diff_secs(start: u32, end: u32) -> f32 {
    let diff = end.wrapping_sub(start);
    diff as f32 / 1000.0
}
