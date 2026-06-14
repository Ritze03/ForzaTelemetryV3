use crate::packet::ForzaPacket;

const FULL_THROTTLE_THRESHOLD: u8 = 245;

/// Captures live power / torque / boost curves during full-throttle runs.
pub struct PowerCapture {
    /// [rpm, ps] sorted by rpm
    pub power_series: Vec<[f64; 2]>,
    /// [rpm, nm] sorted by rpm
    pub torque_series: Vec<[f64; 2]>,
    /// [rpm, psi_gauge] sorted by rpm
    pub boost_series: Vec<[f64; 2]>,

    last_captured_rpm: f32,
    was_full_throttle: bool,
}

impl PowerCapture {
    pub fn new() -> Self {
        Self {
            power_series: Vec::new(),
            torque_series: Vec::new(),
            boost_series: Vec::new(),
            last_captured_rpm: 0.0,
            was_full_throttle: false,
        }
    }

    pub fn on_car_changed(&mut self) {
        self.clear();
    }

    pub fn clear(&mut self) {
        self.power_series.clear();
        self.torque_series.clear();
        self.boost_series.clear();
        self.last_captured_rpm = 0.0;
        self.was_full_throttle = false;
    }

    pub fn update(&mut self, pkt: &ForzaPacket, step_rpm: f32) {
        if pkt.is_race_on == 0 {
            return;
        }

        let full_throttle = pkt.accel >= FULL_THROTTLE_THRESHOLD;

        if !full_throttle {
            self.was_full_throttle = false;
            return;
        }

        if !self.was_full_throttle {
            // Fresh throttle press — anchor the first capture threshold
            self.last_captured_rpm = pkt.current_engine_rpm - step_rpm;
            self.was_full_throttle = true;
        }

        let rpm = pkt.current_engine_rpm;
        if rpm < self.last_captured_rpm + step_rpm {
            return;
        }

        let rpm64 = rpm as f64;
        let ps = pkt.power_ps() as f64;
        let nm = pkt.torque_nm() as f64;
        let boost = pkt.boost as f64;

        upsert_max(&mut self.power_series, rpm64, ps);
        upsert_max(&mut self.torque_series, rpm64, nm);
        upsert_max(&mut self.boost_series, rpm64, boost);

        self.last_captured_rpm = rpm;
    }
}

fn upsert_max(series: &mut Vec<[f64; 2]>, rpm: f64, val: f64) {
    if let Some(pt) = series.iter_mut().find(|p| p[0] == rpm) {
        if val > pt[1] {
            pt[1] = val;
        }
    } else {
        // Insert in sorted order
        let pos = series.partition_point(|p| p[0] < rpm);
        series.insert(pos, [rpm, val]);
    }
}
