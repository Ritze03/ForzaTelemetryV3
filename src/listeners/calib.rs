//! The RPM-calibration conditions as pure, per-packet check structs.
//!
//! Three things have to happen before the gearbox is calibrated, each with its own set of
//! conditions: capturing the max RPM (worker), engaging on the first manual upshift, and
//! sampling the per-gear redline speed (`dsg.rs`). The real logic and the Debug tab's
//! "Calibration checks" section both go through the fns here, so the display can't drift from
//! what actually gates calibration. Why one bool per condition: "calibration doesn't work for
//! this car" was undiagnosable; the Debug tab now shows exactly which check is failing.

use crate::packet::ForzaPacket;

/// Calibrate a gear only once the engine is past this fraction of the detected redline — high
/// enough that the kmh/rpm extrapolation is accurate, low enough to lock in within one pull.
pub const CALIB_RPM_FRAC: f32 = 0.60;
/// Tyre slip (abs) at or above which a gear-map sample is rejected (wheelspin corrupts kmh/rpm).
pub const CALIB_SLIP: f32 = 0.8;
/// Tyre slip (abs) above which a max-RPM capture is rejected (slip inflates RPM without speed).
pub const MAX_RPM_SLIP: f32 = 0.5;
/// Gear-map samples need at least this normalized suspension travel on every wheel.
pub const CALIB_SUSPENSION: f32 = 0.1;
/// Gear-map samples need more than this speed (km/h).
pub const CALIB_MIN_KMH: f32 = 5.0;
/// Moving straight: `velocity_z >= this * speed` (velocity aligned with heading within ~5%).
pub const CALIB_STRAIGHT_FRAC: f32 = 0.95;

fn slips(pkt: &ForzaPacket) -> [f32; 4] {
    [
        pkt.tire_slip_ratio_fl,
        pkt.tire_slip_ratio_fr,
        pkt.tire_slip_ratio_rl,
        pkt.tire_slip_ratio_rr,
    ]
}

fn suspension(pkt: &ForzaPacket) -> [f32; 4] {
    [
        pkt.normalized_suspension_travel_fl,
        pkt.normalized_suspension_travel_fr,
        pkt.normalized_suspension_travel_rl,
        pkt.normalized_suspension_travel_rr,
    ]
}

/// Raw per-packet values behind the checks, kept for the Debug tab. Wheel order FL FR RL RR.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CalibRaw {
    pub power: f32,
    pub hand_brake: u8,
    pub slip: [f32; 4],
    pub suspension: [f32; 4],
    pub kmh: f32,
    pub rpm: f32,
    /// The redline the checks were run against (0 = not detected yet).
    pub max_rpm: f32,
    /// `rpm / max_rpm`, 0 while no redline is known.
    pub rpm_ratio: f32,
    /// `velocity_z / |speed|`, 0 while standing still.
    pub straight_ratio: f32,
}

impl CalibRaw {
    pub fn of(pkt: &ForzaPacket, max_rpm: f32) -> Self {
        let rpm = pkt.current_engine_rpm;
        let speed_ms = pkt.speed.abs();
        Self {
            power: pkt.power,
            hand_brake: pkt.hand_brake,
            slip: slips(pkt),
            suspension: suspension(pkt),
            kmh: pkt.speed_kmh(),
            rpm,
            max_rpm,
            rpm_ratio: if max_rpm > 0.0 { rpm / max_rpm } else { 0.0 },
            straight_ratio: if speed_ms > 0.0 { pkt.velocity_z / speed_ms } else { 0.0 },
        }
    }
}

/// 1. Max-RPM capture: all must hold for `dynamic_max_rpm = max(dynamic_max_rpm, rpm)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaxRpmChecks {
    /// The redline is not locked yet: false once the box is engaged *and* a redline is known.
    /// Why: the whole point of the calibration procedure is a stable redline; later over-revs
    /// (limiter bounce, downshift spikes) must not move the shift point. Only Clear RPM
    /// calibration (which clears `engaged` and the redline), a car change or a loaded saved
    /// calibration changes it. "And a redline is known": a restored profile can carry
    /// `engaged` with `max_rpm == 0` (RPM cleared, gear map kept, then saved), which must
    /// still be able to capture rather than lock at 0.
    pub unlocked: bool,
    pub race_on: bool,
    pub power_positive: bool,
    pub handbrake_off: bool,
    /// `|slip| <= 0.5` per wheel (FL FR RL RR).
    pub slip_ok: [bool; 4],
}

impl MaxRpmChecks {
    pub fn eval(pkt: &ForzaPacket, engaged: bool, max_rpm: f32) -> Self {
        Self {
            unlocked: !(engaged && max_rpm > 0.0),
            race_on: pkt.is_race_on != 0,
            power_positive: pkt.power > 0.0,
            handbrake_off: pkt.hand_brake == 0,
            slip_ok: slips(pkt).map(|s| s.abs() <= MAX_RPM_SLIP),
        }
    }

    /// The real capture condition.
    pub fn all(&self) -> bool {
        // `power_positive` is deliberately NOT required: cars with a very fast rev limiter report
        // power == 0 while bouncing off the limiter, which blocked max-RPM capture for them. It
        // originally guarded against mis-shift over-revs, which the current calibration protocol
        // (engage only on a manual upshift, gear-map median) makes practically impossible.
        // Still computed for the Debug panel (informational).
        // && self.power_positive
        self.unlocked && self.race_on && self.handbrake_off && self.slip_ok.iter().all(|&b| b)
    }
}

/// The detected redline after this packet: raised to `rpm` while capture is allowed
/// (`MaxRpmChecks::all`), otherwise unchanged. The worker's only writer of `dynamic_max_rpm`
/// besides car change / clear / restore.
pub fn next_max_rpm(current: f32, pkt: &ForzaPacket, engaged: bool) -> f32 {
    if MaxRpmChecks::eval(pkt, engaged, current).all() {
        current.max(pkt.current_engine_rpm)
    } else {
        current
    }
}

/// 2. Engage (calibrated): the first manual upshift between two forward gears.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EngageChecks {
    /// Already engaged (sticky); nothing more to detect.
    pub engaged: bool,
    /// Previous forward gear seen (`DsgListener::prev_gear`, 0 = none) and the current gear.
    pub prev_gear: i32,
    pub gear: i32,
    /// `prev_gear` in 1..=9.
    pub prev_ok: bool,
    /// `gear` in 2..=10.
    pub gear_ok: bool,
    /// `gear > prev_gear`.
    pub upshift: bool,
}

impl EngageChecks {
    pub fn eval(engaged: bool, prev_gear: i32, gear: i32) -> Self {
        Self {
            engaged,
            prev_gear,
            gear,
            prev_ok: (1..=9).contains(&prev_gear),
            gear_ok: (2..=10).contains(&gear),
            upshift: gear > prev_gear,
        }
    }

    /// The real engage trigger: not engaged yet and this packet is a forward upshift.
    pub fn triggers(&self) -> bool {
        !self.engaged && self.prev_ok && self.gear_ok && self.upshift
    }
}

/// 3. Gear-map sample: all must hold for a redline-speed estimate to be recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GearMapChecks {
    pub race_on: bool,
    /// Gear 1..=10 (not N/R).
    pub in_gear: bool,
    /// A redline is known (`max_rpm > 0`).
    pub redline_known: bool,
    /// `kmh > 5`.
    pub moving: bool,
    /// `rpm >= 0.60 * max_rpm`.
    pub rpm_high: bool,
    /// `|slip| < 0.8` per wheel (FL FR RL RR).
    pub slip_ok: [bool; 4],
    /// `normalized_suspension_travel >= 0.1` per wheel.
    pub springs_ok: [bool; 4],
    /// `speed > 0.1 && velocity_z >= 0.95 * speed`.
    pub straight: bool,
}

impl GearMapChecks {
    pub fn eval(pkt: &ForzaPacket, max_rpm: f32) -> Self {
        let gear = pkt.gear as i32;
        let speed_ms = pkt.speed.abs();
        Self {
            race_on: pkt.is_race_on != 0,
            in_gear: (1..=10).contains(&gear),
            redline_known: max_rpm > 0.0,
            moving: pkt.speed_kmh() > CALIB_MIN_KMH,
            rpm_high: pkt.current_engine_rpm >= CALIB_RPM_FRAC * max_rpm,
            slip_ok: slips(pkt).map(|s| s.abs() < CALIB_SLIP),
            springs_ok: suspension(pkt).map(|t| t >= CALIB_SUSPENSION),
            straight: speed_ms > 0.1 && pkt.velocity_z >= CALIB_STRAIGHT_FRAC * speed_ms,
        }
    }

    /// The real sampling condition.
    pub fn all(&self) -> bool {
        self.race_on
            && self.in_gear
            && self.redline_known
            && self.moving
            && self.rpm_high
            && self.slip_ok.iter().all(|&b| b)
            && self.springs_ok.iter().all(|&b| b)
            && self.straight
    }
}

/// Everything the Debug tab shows about calibration for the latest packet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CalibChecks {
    pub max_rpm: MaxRpmChecks,
    pub engage: EngageChecks,
    pub gear_map: GearMapChecks,
    pub raw: CalibRaw,
}

impl CalibChecks {
    pub fn eval(pkt: &ForzaPacket, engage: EngageChecks, max_rpm: f32) -> Self {
        Self {
            max_rpm: MaxRpmChecks::eval(pkt, engage.engaged, max_rpm),
            engage,
            gear_map: GearMapChecks::eval(pkt, max_rpm),
            raw: CalibRaw::of(pkt, max_rpm),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A packet that passes every check (redline 8000, 6000 rpm, 100 km/h straight, gear 3).
    fn good() -> ForzaPacket {
        ForzaPacket {
            is_race_on: 1,
            power: 1000.0,
            hand_brake: 0,
            gear: 3,
            current_engine_rpm: 6000.0,
            speed: 100.0 / 3.6,
            velocity_z: 100.0 / 3.6,
            normalized_suspension_travel_fl: 0.5,
            normalized_suspension_travel_fr: 0.5,
            normalized_suspension_travel_rl: 0.5,
            normalized_suspension_travel_rr: 0.5,
            ..Default::default()
        }
    }

    #[test]
    fn max_rpm_passes_on_a_clean_packet() {
        assert!(MaxRpmChecks::eval(&good(), false, 0.0).all());
    }

    #[test]
    fn max_rpm_each_condition_fails_alone() {
        let mut p = good();
        p.is_race_on = 0;
        let c = MaxRpmChecks::eval(&p, false, 0.0);
        assert!(!c.race_on && !c.all());

        let mut p = good();
        p.power = 0.0; // informational only: still reported, never blocks capture
        assert!(!MaxRpmChecks::eval(&p, false, 0.0).power_positive);
        p.power = 0.01;
        assert!(MaxRpmChecks::eval(&p, false, 0.0).power_positive);

        let mut p = good();
        p.hand_brake = 1;
        let c = MaxRpmChecks::eval(&p, false, 0.0);
        assert!(!c.handbrake_off && !c.all());
    }

    #[test]
    fn max_rpm_still_captures_with_zero_power_at_the_limiter() {
        let mut p = good();
        p.power = 0.0; // fast rev limiter: power reads 0
        let c = MaxRpmChecks::eval(&p, false, 0.0);
        assert!(!c.power_positive && c.all());
        p.power = -5.0;
        assert!(MaxRpmChecks::eval(&p, false, 0.0).all());
    }

    #[test]
    fn max_rpm_slip_boundary_is_inclusive_and_per_wheel() {
        let mut p = good();
        p.tire_slip_ratio_rl = 0.5; // <= 0.5 passes
        assert!(MaxRpmChecks::eval(&p, false, 0.0).all());
        p.tire_slip_ratio_rl = -0.51; // abs
        let c = MaxRpmChecks::eval(&p, false, 0.0);
        assert_eq!(c.slip_ok, [true, true, false, true]);
        assert!(!c.all());
    }

    fn pkt_rpm(rpm: f32) -> ForzaPacket {
        ForzaPacket { current_engine_rpm: rpm, ..good() }
    }

    #[test]
    fn max_rpm_rises_until_engaged_then_is_locked() {
        // Before engagement the redline keeps climbing (and never drops).
        let mut max = next_max_rpm(0.0, &pkt_rpm(7000.0), false);
        assert_eq!(max, 7000.0);
        max = next_max_rpm(max, &pkt_rpm(8000.0), false);
        assert_eq!(max, 8000.0);
        max = next_max_rpm(max, &pkt_rpm(7500.0), false);
        assert_eq!(max, 8000.0);
        // Engaged: a clean, higher-RPM packet (limiter bounce) no longer moves it.
        assert_eq!(next_max_rpm(max, &pkt_rpm(9000.0), true), 8000.0);
        let c = MaxRpmChecks::eval(&pkt_rpm(9000.0), true, max);
        assert!(!c.unlocked && !c.all());
    }

    #[test]
    fn max_rpm_recaptures_after_clear_rpm_calibration() {
        // Clear RPM calibration = engaged false and max 0 (`worker::clear_rpm_calibration`).
        let max = next_max_rpm(0.0, &pkt_rpm(9000.0), false);
        assert_eq!(max, 9000.0);
    }

    #[test]
    fn max_rpm_engaged_without_a_redline_can_still_capture() {
        // Restored profile with max_rpm 0 (RPM cleared, gear map kept): must not lock at 0.
        assert_eq!(next_max_rpm(0.0, &pkt_rpm(8000.0), true), 8000.0);
        assert_eq!(next_max_rpm(8000.0, &pkt_rpm(9000.0), true), 8000.0);
    }

    #[test]
    fn engage_only_on_a_forward_upshift() {
        let t = |engaged, prev, gear| EngageChecks::eval(engaged, prev, gear).triggers();
        assert!(t(false, 1, 2));
        assert!(t(false, 9, 10));
        assert!(!t(true, 1, 2), "already engaged");
        assert!(!t(false, 0, 2), "no previous forward gear (spawned in gear)");
        assert!(!t(false, 10, 11), "prev 10 is out of 1..=9 and gear 11 is out of 2..=10");
        assert!(!t(false, 2, 1), "downshift");
        assert!(!t(false, 3, 3), "same gear");
        assert!(!t(false, 1, 1), "gear must be >= 2");
        assert!(!t(false, 5, 11), "gear above 10");
    }

    #[test]
    fn engage_reports_which_part_failed() {
        let c = EngageChecks::eval(false, 0, 2);
        assert!(!c.prev_ok && c.gear_ok && c.upshift);
        let c = EngageChecks::eval(false, 3, 2);
        assert!(c.prev_ok && c.gear_ok && !c.upshift);
    }

    #[test]
    fn gear_map_passes_on_a_clean_packet() {
        assert!(GearMapChecks::eval(&good(), 8000.0).all());
    }

    #[test]
    fn gear_map_gear_and_race_and_redline() {
        let mut p = good();
        p.gear = 0;
        assert!(!GearMapChecks::eval(&p, 8000.0).in_gear);
        p.gear = 11;
        assert!(!GearMapChecks::eval(&p, 8000.0).in_gear);
        p.gear = 10;
        assert!(GearMapChecks::eval(&p, 8000.0).in_gear);

        let mut p = good();
        p.is_race_on = 0;
        assert!(!GearMapChecks::eval(&p, 8000.0).all());

        let c = GearMapChecks::eval(&good(), 0.0);
        assert!(!c.redline_known && !c.all());
    }

    #[test]
    fn gear_map_speed_boundary() {
        let mut p = good();
        p.speed = 5.0 / 3.6;
        p.velocity_z = p.speed;
        assert!(!GearMapChecks::eval(&p, 8000.0).moving, "exactly 5 km/h is not > 5");
        p.speed = 5.1 / 3.6;
        p.velocity_z = p.speed;
        assert!(GearMapChecks::eval(&p, 8000.0).moving);
    }

    #[test]
    fn gear_map_rpm_fraction_boundary() {
        let mut p = good();
        p.current_engine_rpm = 4801.0; // just above 60% of 8000
        assert!(GearMapChecks::eval(&p, 8000.0).rpm_high);
        p.current_engine_rpm = 4799.0;
        assert!(!GearMapChecks::eval(&p, 8000.0).rpm_high);
    }

    #[test]
    fn gear_map_slip_boundary_is_exclusive_and_per_wheel() {
        let mut p = good();
        p.tire_slip_ratio_fr = 0.79;
        assert!(GearMapChecks::eval(&p, 8000.0).slip_ok[1]);
        p.tire_slip_ratio_fr = 0.8; // < 0.8 required
        let c = GearMapChecks::eval(&p, 8000.0);
        assert_eq!(c.slip_ok, [true, false, true, true]);
        assert!(!c.all());
        p.tire_slip_ratio_fr = -0.9;
        assert!(!GearMapChecks::eval(&p, 8000.0).slip_ok[1]);
    }

    #[test]
    fn gear_map_suspension_boundary_is_inclusive_and_per_wheel() {
        let mut p = good();
        p.normalized_suspension_travel_rr = 0.1;
        assert!(GearMapChecks::eval(&p, 8000.0).springs_ok[3]);
        p.normalized_suspension_travel_rr = 0.099;
        let c = GearMapChecks::eval(&p, 8000.0);
        assert_eq!(c.springs_ok, [true, true, true, false]);
        assert!(!c.all());
    }

    #[test]
    fn gear_map_straight_boundary() {
        let mut p = good();
        p.speed = 20.0;
        p.velocity_z = 19.1; // above 0.95 * 20
        assert!(GearMapChecks::eval(&p, 8000.0).straight);
        p.velocity_z = 18.9;
        assert!(!GearMapChecks::eval(&p, 8000.0).straight);
        p.speed = 0.05; // too slow to judge
        p.velocity_z = 0.05;
        assert!(!GearMapChecks::eval(&p, 8000.0).straight);
    }

    #[test]
    fn raw_ratios() {
        let mut p = good();
        p.velocity_z = 0.5 * p.speed;
        let r = CalibRaw::of(&p, 8000.0);
        assert!((r.rpm_ratio - 0.75).abs() < 1e-6);
        assert!((r.straight_ratio - 0.5).abs() < 1e-6);
        let r = CalibRaw::of(&ForzaPacket::default(), 0.0);
        assert_eq!((r.rpm_ratio, r.straight_ratio), (0.0, 0.0));
    }
}
