//! The cost of driving an arc: assumed speed per road type (D84: there is no real speed data),
//! blended with the "faster roads <-> more curves" slider. One [`CostModel`] per query.
//!
//! Per metre of an arc (`s` = the slider, 0 = fastest):
//!
//! ```text
//! cost_per_m(kind, curv, s) = ((1 - s) / speed[kind] + s / V_REF) * (1 - BETA * s * min(curv / KAPPA, 1))
//! cost(arc)                 = len * cost_per_m
//! ```
//!
//! At `s = 0` that is plain travel time. Two ingredients make the other end "more curves":
//! (a) the speed term blends towards one uniform speed, so highways stop being attractive;
//! (b) a bonus of up to `BETA` (50 %) for winding edges. *Why both:* measured on 40 random pairs
//! 4+ km apart, a curvature bonus alone (no speed blending) changed almost nothing (mean
//! curvature 2.58 -> 2.78 mrad/m) because highways dominate the time; with both, `s = 1` gives
//! +30 % winding for +8 % travel time and a highway share of 9 % instead of 30 %. `BETA` is the
//! one tuning constant (0.8 gave 3.21 mrad/m at 8.74 min); it is a `const`, not a UI setting.

use crate::gamedata::roadtypes::RoadType;
use crate::maprender::data::N_TYPES;

use super::cfg::RoutePrefs;

/// Speed (m/s) the slider's "curves" end blends every road type towards = the Road speed.
pub const V_REF: f32 = 22.0;
/// Curvature (rad per m) at which the winding bonus is full (about the 95th percentile of the
/// island's edges).
pub const KAPPA: f32 = 0.02;
/// The winding bonus at `s = 1` for an edge at or above [`KAPPA`] (cost x `1 - BETA`).
pub const BETA: f32 = 0.5;

/// Assumed speed (m/s) of a kind (`RoadType::index`, 0 = no type). *Why:* no real speed data
/// exists, so these are plausible driving speeds that rank the types sensibly: highway 144 km/h,
/// tunnel 90, road 79, jump 108 (a flight, faster than any road: with the Jumps filter on the
/// router happily takes it), cross-country 43, dirt 61, trail 40, unclassified roads 58. The same
/// numbers give the ETA, shown as approximate.
pub fn speed_ms(kind: u8) -> f32 {
    match RoadType::from_index(kind) {
        None => 16.0,
        Some(RoadType::Highway) => 40.0,
        Some(RoadType::Tunnel) => 25.0,
        Some(RoadType::Road) => 22.0,
        Some(RoadType::Other) => 16.0,
        Some(RoadType::Offroad) => 17.0,
        Some(RoadType::Trail) => 11.0,
        Some(RoadType::Crosscountry) => 12.0,
        Some(RoadType::Jump) => 30.0,
        // never routable; a finite value so nothing divides by zero
        Some(RoadType::Turnaround) => 16.0,
    }
}

/// Per-query cost table. Cheap to build (ten floats).
#[derive(Clone, Debug)]
pub struct CostModel {
    s: f32,
    mask: u16,
    /// `(1 - s) / speed + s / V_REF` per kind.
    base: [f32; N_TYPES],
    /// Lower bound of the cost per metre of any allowed arc (the heuristic's factor).
    h_per_m: f32,
}

impl CostModel {
    pub fn new(prefs: &RoutePrefs) -> CostModel {
        let s = prefs.curves();
        let mask = prefs.filters.mask();
        let mut base = [0.0; N_TYPES];
        let mut vmax = 0.0f32;
        for (k, b) in base.iter_mut().enumerate() {
            *b = (1.0 - s) / speed_ms(k as u8) + s / V_REF;
            if mask >> k & 1 != 0 {
                vmax = vmax.max(speed_ms(k as u8));
            }
        }
        // Every arc costs at least its 2D chord times this: 3D length >= 2D chord, speed <= vmax
        // (of the *enabled* kinds), curvature factor >= 1 - BETA * s (> 0). So the straight-line
        // distance times it is an admissible and consistent A* heuristic.
        let h_per_m = if vmax > 0.0 { ((1.0 - s) / vmax + s / V_REF) * (1.0 - BETA * s) } else { 0.0 };
        CostModel { s, mask, base, h_per_m }
    }

    /// May an arc of this kind be driven (the filters)?
    #[inline]
    pub fn allows(&self, kind: u8) -> bool {
        self.mask >> kind & 1 != 0
    }

    /// Cost per metre of an edge of `kind` with winding `curv` (rad per m).
    #[inline]
    pub fn per_m(&self, kind: u8, curv: f32) -> f32 {
        self.base[kind as usize] * (1.0 - BETA * self.s * (curv / KAPPA).clamp(0.0, 1.0))
    }

    /// Cost of driving `len` metres of such an edge.
    #[inline]
    pub fn cost(&self, kind: u8, curv: f32, len: f32) -> f32 {
        len * self.per_m(kind, curv)
    }

    /// Lower bound of the cost of getting `dist` metres (straight line, 2D) closer to the goal.
    #[inline]
    pub fn heuristic(&self, dist: f32) -> f32 {
        dist * self.h_per_m
    }

    /// Travel time in seconds of `len` metres of `kind` (the ETA; independent of the slider).
    #[inline]
    pub fn eta_s(kind: u8, len: f32) -> f32 {
        len / speed_ms(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::cfg::RouteFilters;

    fn prefs(curves: f32, filters: RouteFilters) -> RoutePrefs {
        RoutePrefs { filters, curves }
    }

    #[test]
    fn slider_zero_is_travel_time() {
        let m = CostModel::new(&prefs(0.0, RouteFilters::ALL));
        let hw = RoadType::Highway.index();
        assert!((m.cost(hw, 0.05, 400.0) - 400.0 / 40.0).abs() < 1e-4, "curvature is ignored at s = 0");
        assert!((CostModel::eta_s(hw, 400.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn slider_one_blends_speeds_and_rewards_winding() {
        let m = CostModel::new(&prefs(1.0, RouteFilters::ALL));
        let (hw, rd) = (RoadType::Highway.index(), RoadType::Road.index());
        assert!((m.per_m(hw, 0.0) - m.per_m(rd, 0.0)).abs() < 1e-7, "all types cost the same per metre when straight");
        assert!((m.per_m(rd, KAPPA) - m.per_m(rd, 0.0) * (1.0 - BETA)).abs() < 1e-7, "full bonus at KAPPA");
        assert!((m.per_m(rd, 5.0 * KAPPA) - m.per_m(rd, KAPPA)).abs() < 1e-7, "bonus saturates");
    }

    /// The heuristic factor never exceeds the cost per metre of any allowed arc (admissibility).
    #[test]
    fn heuristic_is_a_lower_bound() {
        for s in [0.0, 0.25, 0.5, 1.0] {
            for filters in [RouteFilters::ALL, RouteFilters::default(), RouteFilters { highway: false, ..RouteFilters::ALL }] {
                let m = CostModel::new(&prefs(s, filters));
                for k in 0..N_TYPES as u8 {
                    if !m.allows(k) {
                        continue;
                    }
                    for curv in [0.0, 0.005, 0.02, 0.3] {
                        assert!(m.heuristic(1.0) <= m.per_m(k, curv) + 1e-9, "s {s} kind {k} curv {curv}");
                    }
                }
            }
        }
        assert_eq!(CostModel::new(&prefs(0.5, RouteFilters::NONE)).heuristic(100.0), 0.0);
    }

    #[test]
    fn disallowed_kinds_follow_the_filters() {
        let m = CostModel::new(&prefs(0.0, RouteFilters::default()));
        assert!(m.allows(0) && m.allows(RoadType::Tunnel.index()) && m.allows(RoadType::Offroad.index()));
        assert!(!m.allows(RoadType::Trail.index()) && !m.allows(RoadType::Jump.index()) && !m.allows(RoadType::Turnaround.index()));
    }
}
