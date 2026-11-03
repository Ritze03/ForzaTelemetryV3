//! The cost of driving an arc: assumed speed per road type (D84: there is no real speed data),
//! blended with the "faster roads <-> more curves" slider (`s`, 0 = fastest). One [`CostModel`]
//! per query.
//!
//! Per metre of an arc:
//!
//! ```text
//! cost_per_m(kind, curv, wind, s) = ((1 - s) / speed[kind] + s / V_REF)
//!                                 * (kind == Highway ? (1 - s) * HW_FAST + s * HW_AVOID : 1)
//!                                 * (1 - BETA * s * min(max(curv / KAPPA, wind / KAPPA_WIND), 1))
//! cost(arc)                 = len * cost_per_m
//! ```
//!
//! This is the *search objective*, not the ETA: [`CostModel::eta_s`] stays plain length / assumed
//! speed, so the preference factors never leak into the time shown.
//!
//! Three ingredients (D96, the user: "the highway is pretty much always the fastest mean of
//! travel; for 'Faster roads' it should be (at least almost) always used. 'More curves' isn't
//! aggressive enough, it still kinda avoids some touge routes"):
//!
//! (a) the speed term blends towards one uniform speed, so at `s = 1` highways are no faster;
//! (b) a highway factor: `HW_FAST` (0.4) at `s = 0` makes a highway metre 2.5x cheaper than its
//! speed alone says (a preference, not a speed: the ETA is unchanged), `HW_AVOID` (1.5) at
//! `s = 1` makes it 50 % dearer than any other straight road. *Why:* by assumed speed alone a
//! highway only beat a local road by 1.8x per metre, so a somewhat shorter local road often won
//! and the route skipped the highway the user expects it to take; at the curvy end highways
//! kept a share of the trips the curve bonus did not decide;
//! (c) a winding bonus of up to `BETA` (95 %) on an edge whose own curvature `curv` reaches
//! `KAPPA` (10 mrad/m, about the 80th percentile of the island's road and dirt edges; touge
//! hairpins sit at 15-50) **or** whose road winds that much around it: `wind` is the heading
//! change per metre over +-400 m of the same road (`Edge::wind`, `graph.rs`), full at
//! `KAPPA_WIND` (8 mrad/m). *Why wind:* a touge is a sustained stretch of bends with straights in
//! between; the per-edge curvature rewarded only the bends, so the route kept preferring a
//! shorter straighter road to the whole pass (D96, the user: "More Curves isnt aggressive enough,
//! since it still kinda avoids some Touge routes"). The window gives the straights between
//! hairpins the same bonus as the hairpins.
//! The measurements are in `docs/features/navigation.md` (cost section).
//!
//! All the constants are `const`s, not UI settings. A highway factor below 1 lowers the A*
//! heuristic's lower bound (it uses the cheapest *enabled* per-metre cost), so the search stays
//! exact; the test `astar_cost_equals_dijkstra_on_random_pairs` guards that.

use crate::gamedata::roadtypes::RoadType;
use crate::maprender::data::N_TYPES;

use super::cfg::RoutePrefs;

/// Speed (m/s) the slider's "curves" end blends every road type towards = the Road speed.
pub const V_REF: f32 = 22.0;
/// Curvature (rad per m) at which the winding bonus is full (about the 80th percentile of the
/// island's road and dirt edges).
pub const KAPPA: f32 = 0.01;
/// Winding of the road around an edge (rad per m, see `Edge::wind`) at which the bonus is full.
/// The measured touge sit at 7-14 mrad/m; the 3-4 mrad/m of an ordinary country road barely counts.
pub const KAPPA_WIND: f32 = 0.008;
/// The winding bonus at `s = 1` for an edge at or above [`KAPPA`] / [`KAPPA_WIND`] (cost x `1 - BETA`).
pub const BETA: f32 = 0.95;
/// Cost factor of a highway metre at `s = 0` (the "faster roads" end): the preference for the
/// highway on top of its speed. Not part of the ETA.
pub const HW_FAST: f32 = 0.4;
/// Cost factor of a highway metre at `s = 1` (the "more curves" end).
pub const HW_AVOID: f32 = 1.5;

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
    /// `(1 - s) / speed + s / V_REF` per kind, times the highway factor for highways.
    base: [f32; N_TYPES],
    /// Lower bound of the cost per metre of any allowed arc (the heuristic's factor).
    h_per_m: f32,
}

impl CostModel {
    pub fn new(prefs: &RoutePrefs) -> CostModel {
        let s = prefs.curves();
        let mask = prefs.filters.mask();
        let highway = RoadType::Highway.index() as usize;
        let mut base = [0.0; N_TYPES];
        let mut bmin = f32::INFINITY;
        for (k, b) in base.iter_mut().enumerate() {
            *b = (1.0 - s) / speed_ms(k as u8) + s / V_REF;
            if k == highway {
                *b *= (1.0 - s) * HW_FAST + s * HW_AVOID;
            }
            if mask >> k & 1 != 0 {
                bmin = bmin.min(*b);
            }
        }
        // Every arc costs at least its 2D chord times this: 3D length >= 2D chord, per-metre base
        // >= the cheapest of the *enabled* kinds, curvature factor >= 1 - BETA * s (> 0). So the
        // straight-line distance times it is an admissible and consistent A* heuristic.
        let h_per_m = if bmin.is_finite() { bmin * (1.0 - BETA * s) } else { 0.0 };
        CostModel { s, mask, base, h_per_m }
    }

    /// May an arc of this kind be driven (the filters)?
    #[inline]
    pub fn allows(&self, kind: u8) -> bool {
        self.mask >> kind & 1 != 0
    }

    /// How winding an edge counts, 0..=1: the stronger of its own curvature (against [`KAPPA`])
    /// and the road's winding around it (against [`KAPPA_WIND`]).
    #[inline]
    fn winding(curv: f32, wind: f32) -> f32 {
        (curv / KAPPA).max(wind / KAPPA_WIND).clamp(0.0, 1.0)
    }

    /// Cost per metre of an edge of `kind` with own curvature `curv` and surrounding winding `wind`
    /// (both rad per m).
    #[inline]
    pub fn per_m(&self, kind: u8, curv: f32, wind: f32) -> f32 {
        self.base[kind as usize] * (1.0 - BETA * self.s * Self::winding(curv, wind))
    }

    /// Cost of driving `len` metres of such an edge.
    #[inline]
    pub fn cost(&self, kind: u8, curv: f32, wind: f32, len: f32) -> f32 {
        len * self.per_m(kind, curv, wind)
    }

    /// Lower bound of the cost of getting `dist` metres (straight line, 2D) closer to the goal.
    #[inline]
    pub fn heuristic(&self, dist: f32) -> f32 {
        dist * self.h_per_m
    }

    /// Travel time in seconds of `len` metres of `kind` (the ETA; independent of the slider and
    /// of every preference factor).
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
    fn slider_zero_is_travel_time_with_a_highway_preference() {
        let m = CostModel::new(&prefs(0.0, RouteFilters::ALL));
        let (hw, rd) = (RoadType::Highway.index(), RoadType::Road.index());
        assert!((m.cost(rd, 0.05, 0.0, 440.0) - 440.0 / 22.0).abs() < 1e-4, "plain travel time, curvature ignored at s = 0");
        assert!((m.cost(hw, 0.05, 0.0, 400.0) - HW_FAST * 400.0 / 40.0).abs() < 1e-4, "highway: travel time x HW_FAST");
        assert!((CostModel::eta_s(hw, 400.0) - 10.0).abs() < 1e-6, "the ETA never carries the preference");
    }

    #[test]
    fn slider_one_blends_speeds_avoids_highways_and_rewards_winding() {
        let m = CostModel::new(&prefs(1.0, RouteFilters::ALL));
        let (hw, rd, dirt) = (RoadType::Highway.index(), RoadType::Road.index(), RoadType::Offroad.index());
        assert!((m.per_m(dirt, 0.0, 0.0) - m.per_m(rd, 0.0, 0.0)).abs() < 1e-7, "all non-highway types cost the same per metre when straight");
        assert!((m.per_m(hw, 0.0, 0.0) - HW_AVOID * m.per_m(rd, 0.0, 0.0)).abs() < 1e-7, "a straight highway costs HW_AVOID times a straight road");
        assert!((m.per_m(rd, KAPPA, 0.0) - m.per_m(rd, 0.0, 0.0) * (1.0 - BETA)).abs() < 1e-7, "full bonus at KAPPA");
        assert!((m.per_m(rd, 5.0 * KAPPA, 0.0) - m.per_m(rd, KAPPA, 0.0)).abs() < 1e-7, "bonus saturates");
        assert!((m.per_m(rd, 0.0, KAPPA_WIND) - m.per_m(rd, KAPPA, 0.0)).abs() < 1e-7, "the road's winding around an edge gives the same bonus, straight edge or not");
        assert!((m.per_m(rd, KAPPA, 0.5 * KAPPA_WIND) - m.per_m(rd, KAPPA, 0.0)).abs() < 1e-7, "the stronger of the two counts, they do not add");
        assert!(m.per_m(rd, 0.0, 0.5 * KAPPA_WIND) < m.per_m(rd, 0.0, 0.0) && m.per_m(rd, 0.0, 0.5 * KAPPA_WIND) > m.per_m(rd, 0.0, KAPPA_WIND), "in between is in between");
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
                    for (curv, wind) in [(0.0, 0.0), (0.005, 0.0), (0.02, 0.0), (0.3, 0.0), (0.0, 0.004), (0.0, 0.3)] {
                        assert!(m.heuristic(1.0) <= m.per_m(k, curv, wind) + 1e-9, "s {s} kind {k} curv {curv} wind {wind}");
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
