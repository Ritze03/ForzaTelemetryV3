use crate::packet::ForzaPacket;

const FULL_THROTTLE_THRESHOLD: u8 = 245;

/// Captures live power / torque / boost curves during full-throttle runs.
pub struct PowerCapture {
    /// [rpm_bucket, ps] sorted by rpm
    pub power_series: Vec<[f64; 2]>,
    /// [rpm_bucket, nm] sorted by rpm
    pub torque_series: Vec<[f64; 2]>,
    /// [rpm_bucket, psi_gauge] sorted by rpm
    pub boost_series: Vec<[f64; 2]>,

    was_full_throttle: bool,
}

impl PowerCapture {
    pub fn new() -> Self {
        Self {
            power_series: Vec::new(),
            torque_series: Vec::new(),
            boost_series: Vec::new(),
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
        self.was_full_throttle = true;

        let rpm = pkt.current_engine_rpm as f64;
        let step = step_rpm as f64;
        if step <= 0.0 || rpm <= 0.0 {
            return;
        }

        // Snap to step-aligned bucket so each bucket has one entry with the max value.
        let bucket = (rpm / step).floor() * step;

        upsert_max(&mut self.power_series, bucket, pkt.power_ps() as f64);
        upsert_max(&mut self.torque_series, bucket, pkt.torque_nm() as f64);

        let max_slip = [
            pkt.tire_combined_slip_fl, pkt.tire_combined_slip_fr,
            pkt.tire_combined_slip_rl, pkt.tire_combined_slip_rr,
        ]
        .iter()
        .map(|s| s.abs())
        .fold(0.0_f32, f32::max);
        if max_slip < 1.0 {
            upsert_max(&mut self.boost_series, bucket, pkt.boost as f64);
        }
    }
}

fn upsert_max(series: &mut Vec<[f64; 2]>, rpm: f64, val: f64) {
    if let Some(pt) = series.iter_mut().find(|p| p[0] == rpm) {
        if val > pt[1] {
            pt[1] = val;
        }
    } else {
        let pos = series.partition_point(|p| p[0] < rpm);
        series.insert(pos, [rpm, val]);
    }
}
