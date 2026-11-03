use crate::packet::ForzaPacket;

const FULL_THROTTLE_THRESHOLD: u8 = 245;

/// Boost (PSI above atmospheric) a car must exceed for it to count as forced-induction.
///
/// Why 1.0 and not "anything above 0": the packet's `Boost` is manifold pressure relative to
/// atmosphere, so a naturally-aspirated engine sits in vacuum (negative) off-throttle and
/// creeps up to roughly 0 at wide-open throttle — never meaningfully above it. A turbo or
/// supercharger goes several PSI positive under load. The old 0.05 PSI cutoff left no margin
/// for an NA car's WOT reading hovering a hair above zero; 1 PSI is well clear of that and still
/// far below any real boost level.
pub const FI_BOOST_THRESHOLD_PSI: f64 = 1.0;

/// True if any point in the `[rpm, psi]` series shows real forced-induction boost.
pub fn has_fi_boost(series: &[[f64; 2]]) -> bool {
    series.iter().any(|&[_, psi]| psi > FI_BOOST_THRESHOLD_PSI)
}

/// The one forced-induction visibility rule every boost plot shares (Power Curve tab, Dashboard
/// Power Graph's boost line, Dashboard Boost Graph).
///
/// - `detection` OFF → always show boost (no filtering).
/// - `detection` ON  → show only if one of the `series` actually being plotted shows real boost,
///   or — with `save_state` ON — boost was latched for the current car (survives "Clear live").
pub fn boost_visible(detection: bool, save_state: bool, fi_latched: bool, series: &[&[[f64; 2]]]) -> bool {
    !detection || series.iter().any(|s| has_fi_boost(s)) || (save_state && fi_latched)
}

#[derive(Clone, Default)]
pub struct PowerCurveSnapshot {
    pub power_series: Vec<[f64; 2]>,
    pub torque_series: Vec<[f64; 2]>,
    pub boost_series: Vec<[f64; 2]>,
}

/// Captures live power / torque / boost curves during full-throttle runs.
pub struct PowerCapture {
    /// [rpm_bucket, ps] sorted by rpm
    pub power_series: Vec<[f64; 2]>,
    /// [rpm_bucket, nm] sorted by rpm
    pub torque_series: Vec<[f64; 2]>,
    /// [rpm_bucket, psi_gauge] sorted by rpm
    pub boost_series: Vec<[f64; 2]>,

    was_full_throttle: bool,
    /// Forced induction seen for the current car (any race-on packet with boost above
    /// [`FI_BOOST_THRESHOLD_PSI`], throttle/speed not required). Survives [`clear`](Self::clear)
    /// — that's what "Save Forced Induction State" relies on — and resets on car change.
    fi_detected: bool,
}

impl PowerCapture {
    pub fn new() -> Self {
        Self {
            power_series: Vec::new(),
            torque_series: Vec::new(),
            boost_series: Vec::new(),
            was_full_throttle: false,
            fi_detected: false,
        }
    }

    pub fn on_car_changed(&mut self) {
        self.clear();
        self.fi_detected = false;
    }

    /// Forced induction latched for the current car (see the field doc).
    pub fn fi_detected(&self) -> bool {
        self.fi_detected
    }

    pub fn clear(&mut self) {
        self.power_series.clear();
        self.torque_series.clear();
        self.boost_series.clear();
        self.was_full_throttle = false;
    }

    pub fn snapshot(&self) -> PowerCurveSnapshot {
        PowerCurveSnapshot {
            power_series: self.power_series.clone(),
            torque_series: self.torque_series.clone(),
            boost_series: self.boost_series.clone(),
        }
    }

    pub fn update(&mut self, pkt: &ForzaPacket, step_rpm: f32) {
        if pkt.is_race_on == 0 {
            return;
        }
        if pkt.boost as f64 > FI_BOOST_THRESHOLD_PSI {
            self.fi_detected = true;
        }
        // Skip stationary — neutral-revving produces falsely high power figures
        if pkt.speed < 0.1 {
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

        upsert_max(&mut self.boost_series, bucket, pkt.boost as f64);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A moving, full-throttle, race-on packet at `rpm` with `boost` PSI.
    fn pkt(rpm: f32, boost: f32) -> ForzaPacket {
        ForzaPacket {
            is_race_on: 1,
            speed: 30.0,
            accel: 255,
            current_engine_rpm: rpm,
            boost,
            ..Default::default()
        }
    }

    fn feed(cap: &mut PowerCapture, samples: &[(f32, f32)]) {
        for &(rpm, boost) in samples {
            cap.update(&pkt(rpm, boost), 100.0);
        }
    }

    /// Visibility with detection ON + save-state ON, judged on the live series.
    fn visible(cap: &PowerCapture) -> bool {
        boost_visible(true, true, cap.fi_detected(), &[&cap.boost_series])
    }

    #[test]
    fn na_car_vacuum_is_not_forced_induction() {
        let mut cap = PowerCapture::new();
        // Idle-ish vacuum up to near-atmospheric at wide-open throttle, incl. tiny positive noise.
        feed(&mut cap, &[(1500.0, -12.0), (3000.0, -4.0), (5000.0, -0.5), (6500.0, 0.0), (7000.0, 0.3)]);
        assert!(!cap.boost_series.is_empty());
        assert!(!has_fi_boost(&cap.boost_series));
        assert!(!cap.fi_detected());
        assert!(!visible(&cap));
    }

    #[test]
    fn turbo_spool_is_forced_induction() {
        let mut cap = PowerCapture::new();
        feed(&mut cap, &[(2000.0, -3.0), (3000.0, 0.5), (4000.0, 6.0), (5000.0, 10.0)]);
        assert!(has_fi_boost(&cap.boost_series));
        assert!(cap.fi_detected());
        assert!(visible(&cap));
    }

    #[test]
    fn latch_survives_clear_but_not_car_change() {
        let mut cap = PowerCapture::new();
        feed(&mut cap, &[(4000.0, 10.0)]);
        cap.clear();
        assert!(cap.boost_series.is_empty());
        assert!(cap.fi_detected(), "Clear live keeps the FI latch");
        assert!(visible(&cap), "save-state keeps the graph after Clear live");
        assert!(!boost_visible(true, false, cap.fi_detected(), &[&cap.boost_series]),
            "without save-state, an empty capture hides it");

        cap.on_car_changed();
        assert!(!cap.fi_detected(), "car change resets detection");
        feed(&mut cap, &[(3000.0, -6.0), (6000.0, -0.2)]);
        assert!(!visible(&cap), "next (NA) car does not inherit the previous car's boost");
    }

    #[test]
    fn detection_off_always_shows() {
        assert!(boost_visible(false, false, false, &[&[]]));
        assert!(boost_visible(false, true, false, &[&[[3000.0, -5.0]]]));
    }

    #[test]
    fn latch_ignores_throttle_and_speed() {
        let mut cap = PowerCapture::new();
        // Part-throttle boost while cruising isn't captured into the curve but still latches FI.
        cap.update(&ForzaPacket { accel: 100, ..pkt(3500.0, 4.0) }, 100.0);
        assert!(cap.boost_series.is_empty());
        assert!(cap.fi_detected());

        // Same while stationary (e.g. a supercharger revved in neutral).
        let mut cap = PowerCapture::new();
        cap.update(&ForzaPacket { speed: 0.0, ..pkt(5000.0, 3.0) }, 100.0);
        assert!(cap.boost_series.is_empty());
        assert!(cap.fi_detected());
    }
}
