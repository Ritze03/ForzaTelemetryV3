//! Which race lines to draw. The interesting mode is "current race": the telemetry has **no
//! race id** (`packet.rs` has `race_position` and lap fields, nothing that names the route),
//! so the line is *guessed*: while `race_position != 0` (the HUD's own "in a race" rule), the
//! line the car is on is the one whose centre line passes within `|half-width| + 4 m` of the
//! car with a driving direction that agrees with the car's heading (dot product > 0). Many
//! races share roads, so several lines can match; the one the car is closest to wins, and the
//! previous pick is kept while it still matches (`sticky`) so the choice does not flicker
//! between overlapping lines. Best effort, not verified against live races.

use super::cfg::{RaceCfg, RaceLineMode};
use super::data::RaceLayer;

/// Extra metres beyond a line's half-width that still count as "on" it.
pub const ON_LINE_SLACK_M: f32 = 4.0;
/// Lines nearer/near the car are recomputed when it moved this far (m).
const MOVE_M: f32 = 10.0;

/// Selection state kept by the caller (one per map): the picked line indices, plus what is
/// needed to avoid recomputing every frame.
#[derive(Default, Debug)]
pub struct RaceSel {
    /// Indices into `RaceLayer::lines` to draw.
    pub picked: Vec<usize>,
    sticky: Option<usize>,
    at: Option<(f32, f32)>,
    key: (Option<RaceLineMode>, usize, u32),
}

/// Squared distance from point p to segment a-b, and the segment's direction (unit).
fn seg_dist(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> (f32, [f32; 2], f32) {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dz * dz;
    let t = if len2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qz) = (a[0] + dx * t, a[1] + dz * t);
    let d = ((p[0] - qx).powi(2) + (p[1] - qz).powi(2)).sqrt();
    let l = len2.sqrt().max(1e-6);
    (d, [dx / l, dz / l], t)
}

/// Half-width of `line` near segment `s` (the larger of its two end nodes), metres.
fn half_width(layer: &RaceLayer, line: usize, s: usize) -> f32 {
    let h = &layer.lines[line].half;
    let m = |i: usize| h.get(i).map_or(0.0, |v| (v[0] * v[0] + v[1] * v[1]).sqrt());
    m(s).max(m(s + 1))
}

/// The lines the car at `car` (world x, z) driving along `fwd` (unit) is on, each with its
/// distance to the centre line. Public for tests.
pub fn lines_under_car(layer: &RaceLayer, car: (f32, f32), fwd: (f32, f32)) -> Vec<(usize, f32)> {
    let mut best: Vec<(usize, f32)> = Vec::new();
    layer.grid.near(car.0, car.1, 40.0, |li, s| {
        let (li, s) = (li as usize, s as usize);
        let l = &layer.lines[li];
        if s + 1 >= l.pts.len() {
            return;
        }
        let (d, dir, _) = seg_dist([car.0, car.1], l.pts[s], l.pts[s + 1]);
        if d > half_width(layer, li, s) + ON_LINE_SLACK_M || dir[0] * fwd.0 + dir[1] * fwd.1 <= 0.0 {
            return;
        }
        match best.iter_mut().find(|b| b.0 == li) {
            Some(b) => b.1 = b.1.min(d),
            None => best.push((li, d)),
        }
    });
    best
}

/// Lines within `radius` of `car`, nearest first, each with its distance.
pub fn lines_near(layer: &RaceLayer, car: (f32, f32), radius: f32) -> Vec<(usize, f32)> {
    let mut best: Vec<(usize, f32)> = Vec::new();
    layer.grid.near(car.0, car.1, radius, |li, s| {
        let (li, s) = (li as usize, s as usize);
        let l = &layer.lines[li];
        if s + 1 >= l.pts.len() {
            return;
        }
        let (d, _, _) = seg_dist([car.0, car.1], l.pts[s], l.pts[s + 1]);
        if d > radius {
            return;
        }
        match best.iter_mut().find(|b| b.0 == li) {
            Some(b) => b.1 = b.1.min(d),
            None => best.push((li, d)),
        }
    });
    best.sort_by(|a, b| a.1.total_cmp(&b.1));
    best
}

impl RaceSel {
    /// Pick the lines for this frame. `car` = world x, z; `yaw` = the car's raw heading
    /// (`pkt.yaw`, clockwise from north = +z); `in_race` = `race_position != 0`. Returns the
    /// picked indices into `layer.lines` (empty for `Off`, or `Current` outside a race). For
    /// `All` the caller draws every line in view, so the list is empty and `all` is true.
    pub fn update(&mut self, layer: &RaceLayer, cfg: &RaceCfg, car: (f32, f32), yaw: f32, in_race: bool) -> &[usize] {
        let key = (Some(cfg.mode), layer.lines.len(), cfg.radius_m.to_bits());
        let moved = self.at.is_none_or(|p| (p.0 - car.0).hypot(p.1 - car.1) >= MOVE_M);
        match cfg.mode {
            RaceLineMode::Off | RaceLineMode::All => {
                self.picked.clear();
                self.sticky = None;
                self.at = None;
            }
            RaceLineMode::Current => {
                if !in_race {
                    self.picked.clear();
                    self.sticky = None;
                } else {
                    let fwd = (yaw.sin(), yaw.cos());
                    let cands = lines_under_car(layer, car, fwd);
                    let pick = self
                        .sticky
                        .filter(|s| cands.iter().any(|c| c.0 == *s))
                        .or_else(|| cands.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|c| c.0));
                    // Keep showing the last pick for a moment when the car is momentarily off
                    // every line (a shortcut, a wide corner); a new match replaces it.
                    if let Some(p) = pick {
                        self.sticky = Some(p);
                    }
                    self.picked = self.sticky.into_iter().collect();
                }
            }
            RaceLineMode::Nearest | RaceLineMode::Near => {
                if moved || key != self.key {
                    let near = lines_near(layer, car, cfg.radius_m.max(1.0));
                    self.picked = if cfg.mode == RaceLineMode::Nearest { near.iter().take(1).map(|n| n.0).collect() } else { near.iter().map(|n| n.0).collect() };
                    self.at = Some(car);
                }
            }
        }
        self.key = key;
        &self.picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::racelines::RaceLine;

    /// A straight line along +z at x = `x`, from z = 0 to 1000, 5 m spacing, half-width 6 m.
    fn line(route: u32, x: f32, reversed: bool) -> RaceLine {
        let mut pts: Vec<[f32; 2]> = (0..=200).map(|i| [x, i as f32 * 5.0]).collect();
        if reversed {
            pts.reverse();
        }
        let n = pts.len();
        RaceLine {
            route,
            circuit: false,
            start: [pts[0][0], 0.0, pts[0][1]],
            finish: [pts[n - 1][0], 0.0, pts[n - 1][1]],
            y: vec![0.0; n],
            half: vec![[6.0, 0.0]; n],
            length_m: 1000.0,
            closed: false,
            n_sections: 1,
            bbox: [x, 0.0, x, 1000.0],
            pts,
        }
    }

    fn cfg(mode: RaceLineMode) -> RaceCfg {
        RaceCfg { mode, ..RaceCfg::default() }
    }

    #[test]
    fn current_race_picks_the_line_under_the_car_going_the_same_way() {
        // Two parallel lines 8 m apart, one driven north, one south (same road, other direction),
        // and a far one.
        let layer = RaceLayer::new(vec![line(1, 0.0, false), line(2, 8.0, true), line(3, 500.0, false)]);
        let mut sel = RaceSel::default();
        // Heading north (yaw 0 → fwd +z), at x = 1 (on line 1, 7 m from line 2).
        let got = sel.update(&layer, &cfg(RaceLineMode::Current), (1.0, 300.0), 0.0, true).to_vec();
        assert_eq!(got, vec![0]);
        // Facing south (yaw π): the reversed line 2 agrees; at x = 7 the car is 1 m from it.
        let mut sel = RaceSel::default();
        let got = sel.update(&layer, &cfg(RaceLineMode::Current), (7.0, 300.0), std::f32::consts::PI, true).to_vec();
        assert_eq!(got, vec![1]);
        // Not in a race: nothing, however well it matches.
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Current), (1.0, 300.0), 0.0, false).is_empty());
        // Too far from every line (more than half-width + 4 m): nothing.
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Current), (200.0, 300.0), 0.0, true).is_empty());
    }

    #[test]
    fn current_race_pick_sticks_while_it_still_matches() {
        // Two lines 8 m apart, same direction. Start nearer line 1, drift towards line 2 but
        // stay within line 1's corridor: the pick stays on line 1 instead of flipping.
        let layer = RaceLayer::new(vec![line(1, 0.0, false), line(2, 8.0, false)]);
        let mut sel = RaceSel::default();
        let c = cfg(RaceLineMode::Current);
        assert_eq!(sel.update(&layer, &c, (1.0, 100.0), 0.0, true), &[0]);
        assert_eq!(sel.update(&layer, &c, (6.0, 110.0), 0.0, true), &[0]);
        // Leaving line 1's corridor (6 + 4 = 10 m) hands over to line 2.
        assert_eq!(sel.update(&layer, &c, (12.0, 120.0), 0.0, true), &[1]);
        // Leaving every corridor keeps the last pick for the moment; leaving the race clears it.
        assert_eq!(sel.update(&layer, &c, (80.0, 130.0), 0.0, true), &[1]);
        assert!(sel.update(&layer, &c, (80.0, 130.0), 0.0, false).is_empty());
    }

    #[test]
    fn nearest_near_off_and_all() {
        let layer = RaceLayer::new(vec![line(1, 0.0, false), line(2, 300.0, false), line(3, 2000.0, false)]);
        let mut sel = RaceSel::default();
        assert_eq!(sel.update(&layer, &cfg(RaceLineMode::Nearest), (50.0, 500.0), 0.0, false), &[0]);
        let mut sel = RaceSel::default();
        let mut near = sel.update(&layer, &cfg(RaceLineMode::Near), (150.0, 500.0), 0.0, false).to_vec();
        near.sort();
        assert_eq!(near, vec![0, 1]); // within 1500 m: lines 1 and 2, not 3 (1850 m away)
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Off), (0.0, 0.0), 0.0, true).is_empty());
        assert!(sel.update(&layer, &cfg(RaceLineMode::All), (0.0, 0.0), 0.0, true).is_empty());
        // Nothing within a tiny radius.
        let mut sel = RaceSel::default();
        let small = RaceCfg { mode: RaceLineMode::Nearest, radius_m: 20.0, ..RaceCfg::default() };
        assert!(sel.update(&layer, &small, (150.0, 500.0), 0.0, false).is_empty());
    }
}
