//! Which race lines to draw. The interesting mode is "current race": the telemetry has **no
//! race id** (`packet.rs` has `race_position` and lap fields, nothing that names the route),
//! so the line is *guessed*: while `race_position != 0` (the HUD's own "in a race" rule), the
//! line the car is on is the one whose centre line passes within `|half-width| + 4 m` of the
//! car with a driving direction that agrees with the car's heading (dot product > 0). Many
//! races share roads, so several lines can match; the one the car is closest to wins, and the
//! previous pick is kept while it still matches (`sticky`) so the choice does not flicker
//! between overlapping lines. Best effort, not verified against live races.
//!
//! **In-race focus (D66).** The same selection also says which *roads* belong to the race, so the
//! renderer can mute or hide the rest (`cfg::RaceFocusCfg`). The focus is on only when the mode is
//! `Current`, the car is in a race and a line is picked; otherwise [`RaceSel::focus_line`] is
//! `None` and every road is drawn normally (a failed guess must never blank the map).
//! [`RoadFocus`] cuts each road chain into runs along / away from the picked line's corridor
//! (the line's `half` width + [`FOCUS_SLACK_M`]), computed **once per picked line** and cached in
//! the `RaceSel`, which is why `update` returns the selector itself: the renderer reads both the
//! picked lines and this cache from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::cfg::{RaceCfg, RaceLineMode};
use super::data::{MapLayers, RaceLayer, RoadLayer, N_TYPES};
use super::view::bbox_hits;
use crate::gamedata::racelines::RaceLine;
use crate::gamedata::roadtypes::RoadType;

/// Extra metres beyond a line's half-width that still count as "on" it.
pub const ON_LINE_SLACK_M: f32 = 4.0;
/// Lines nearer/near the car are recomputed when it moved this far (m).
const MOVE_M: f32 = 10.0;

/// Selection state kept by the caller (one per map): the picked line indices, plus what is
/// needed to avoid recomputing every frame, plus the in-race focus cache.
#[derive(Default, Debug)]
pub struct RaceSel {
    /// Indices into `RaceLayer::lines` to draw.
    pub picked: Vec<usize>,
    sticky: Option<usize>,
    at: Option<(f32, f32)>,
    key: (Option<RaceLineMode>, usize, u32),
    /// The in-race focus applies: `Current` mode, in a race, a line picked.
    focus_on: bool,
    /// The road focus of the picked line. A `Mutex` (not a `RefCell`) so the selector can be
    /// shared by `&` with the renderer and still be a `static` in tests.
    cache: Mutex<Option<(FocusKey, Arc<RoadFocus>)>>,
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
    line_half(&layer.lines[line], s)
}

fn line_half(l: &RaceLine, s: usize) -> f32 {
    let m = |i: usize| l.half.get(i).map_or(0.0, |v| (v[0] * v[0] + v[1] * v[1]).sqrt());
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
    /// The picked indices into `RaceLayer::lines` (empty for `Off`, or `Current` outside a
    /// race). For `All` the caller draws every line in view, so the list is empty.
    pub fn picked(&self) -> &[usize] {
        &self.picked
    }

    /// The race line the in-race focus is about, if it is on: `Current` mode, in a race, and a
    /// line picked. `None` = draw everything normally.
    pub fn focus_line(&self) -> Option<usize> {
        self.focus_on.then(|| self.picked.first().copied()).flatten()
    }

    /// Which roads belong to the focus line (cached per picked line + road data; the first call
    /// after a change builds it, see [`RoadFocus::build`]). `None` without a focus line.
    pub fn road_focus(&self, layers: &MapLayers) -> Option<Arc<RoadFocus>> {
        let li = self.focus_line()?;
        let line = layers.races.lines.get(li)?;
        let key: FocusKey = (layers.rev, Arc::as_ptr(&layers.roads) as usize, Arc::as_ptr(&layers.races) as usize, li);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, f)) = cache.as_ref() {
            if *k == key {
                return Some(f.clone());
            }
        }
        let f = Arc::new(RoadFocus::build(&layers.roads, line));
        *cache = Some((key, f.clone()));
        Some(f)
    }

    /// A selector with a fixed pick, for tests of the renderer.
    #[cfg(test)]
    pub fn fixed(picked: Vec<usize>, focus: bool) -> RaceSel {
        RaceSel { picked, focus_on: focus, ..Default::default() }
    }

    /// Pick the lines for this frame. `car` = world x, z; `yaw` = the car's raw heading
    /// (`pkt.yaw`, clockwise from north = +z); `in_race` = `race_position != 0`. Returns the
    /// selector itself (read the picks with [`picked`](Self::picked)); the renderer takes it
    /// whole for the in-race focus.
    pub fn update(&mut self, layer: &RaceLayer, cfg: &RaceCfg, car: (f32, f32), yaw: f32, in_race: bool) -> &RaceSel {
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
        self.focus_on = cfg.mode == RaceLineMode::Current && in_race && !self.picked.is_empty();
        self
    }
}

// ── in-race focus (D66) ──────────────────────────────────────────────────────────────────────

/// Metres beyond the race line's half-width that still count as "on the race" for a road.
pub const FOCUS_SLACK_M: f32 = 8.0;
/// A road segment is sampled every this many metres (ends included) when judging it.
const FOCUS_SAMPLE_M: f32 = 10.0;
/// Cell size of the corridor lookup, metres.
const CORRIDOR_CELL_M: f32 = 32.0;

/// The race line's corridor: its segments, each widened by its half-width plus
/// [`FOCUS_SLACK_M`]. A small grid of the **one** selected line (the store's `SegGrid` holds all
/// 170 lines in 100 m cells, so asking it "is this point near line N" would wade through the
/// others); a segment is registered in every cell its widened box touches, so a point only has to
/// look into its own cell.
struct Corridor {
    segs: Vec<([f32; 2], [f32; 2], f32)>,
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// Widest tolerance, to reject far chains by their box.
    max_tol: f32,
}

impl Corridor {
    fn new(l: &RaceLine) -> Corridor {
        let n = l.pts.len();
        let mut c = Corridor { segs: Vec::new(), cells: HashMap::new(), max_tol: 0.0 };
        if n < 2 {
            return c;
        }
        // A closed circuit's last point does not repeat the first: add the closing segment.
        let count = if l.closed && l.pts[0] != l.pts[n - 1] { n } else { n - 1 };
        let key = |x: f32, z: f32| ((x / CORRIDOR_CELL_M).floor() as i32, (z / CORRIDOR_CELL_M).floor() as i32);
        for s in 0..count {
            let (a, b) = (l.pts[s], l.pts[(s + 1) % n]);
            let tol = line_half(l, s) + FOCUS_SLACK_M;
            c.max_tol = c.max_tol.max(tol);
            let (k0, k1) = (key(a[0].min(b[0]) - tol, a[1].min(b[1]) - tol), key(a[0].max(b[0]) + tol, a[1].max(b[1]) + tol));
            let id = c.segs.len() as u32;
            c.segs.push((a, b, tol));
            for cx in k0.0..=k1.0 {
                for cz in k0.1..=k1.1 {
                    c.cells.entry((cx, cz)).or_default().push(id);
                }
            }
        }
        c
    }

    fn hit(&self, p: [f32; 2]) -> bool {
        let k = ((p[0] / CORRIDOR_CELL_M).floor() as i32, (p[1] / CORRIDOR_CELL_M).floor() as i32);
        self.cells.get(&k).is_some_and(|v| {
            v.iter().any(|&i| {
                let (a, b, tol) = self.segs[i as usize];
                seg_dist(p, a, b).0 <= tol
            })
        })
    }

    /// Is the road segment a-b along the corridor: more than half of its samples (every
    /// [`FOCUS_SAMPLE_M`], both ends included) inside. So a road that runs with the race line
    /// is relevant and one that merely crosses it is not, except for its short pieces near the
    /// crossing (which the race line covers anyway).
    fn segment(&self, a: [f32; 2], b: [f32; 2]) -> bool {
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        let n = ((len / FOCUS_SAMPLE_M).ceil() as usize).max(1);
        let hits = (0..=n)
            .filter(|&i| {
                let t = i as f32 / n as f32;
                self.hit([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
            })
            .count();
        hits * 2 > n + 1
    }
}

/// A stretch `a..=b` of points of one road chain that is wholly relevant or wholly not.
#[derive(Clone, Copy, Debug)]
pub struct Run {
    pub chain: u32,
    pub a: u32,
    pub b: u32,
    pub relevant: bool,
    /// `[min_x, min_z, max_x, max_z]` of the run's points.
    pub bbox: [f32; 4],
}

/// Which roads belong to the selected race line: every road chain cut into [`Run`]s of relevant /
/// other segments, and the same flag per jump line. Built once per (road data, selected line) in
/// [`RaceSel::road_focus`], never per frame.
#[derive(Debug, Default)]
pub struct RoadFocus {
    /// Per road type slot, in chain order (a chain's runs are consecutive and cover all its
    /// segments). The turnaround slot is left empty (never drawn).
    pub runs: [Vec<Run>; N_TYPES],
    /// Per `RoadLayer::jumps` entry: both ends on the corridor.
    pub jumps: Vec<bool>,
    /// Road segments looked at / found relevant (diagnostics, tests).
    pub segments: usize,
    pub relevant_segments: usize,
}

impl RoadFocus {
    pub fn build(roads: &RoadLayer, line: &RaceLine) -> RoadFocus {
        let cor = Corridor::new(line);
        let mut f = RoadFocus::default();
        let t = cor.max_tol;
        let lb = [line.bbox[0] - t, line.bbox[1] - t, line.bbox[2] + t, line.bbox[3] + t];
        let turnaround = RoadType::Turnaround.index() as usize;
        for (slot, chains) in roads.by_type.iter().enumerate() {
            if slot == turnaround {
                continue;
            }
            for (ci, ch) in chains.iter().enumerate() {
                let nseg = ch.pts.len().saturating_sub(1);
                if nseg == 0 {
                    continue;
                }
                f.segments += nseg;
                if cor.segs.is_empty() || !bbox_hits(&ch.bbox, &lb) {
                    f.runs[slot].push(Run { chain: ci as u32, a: 0, b: nseg as u32, relevant: false, bbox: ch.bbox });
                    continue;
                }
                let flush = |f: &mut RoadFocus, from: usize, to: usize, relevant: bool| {
                    let mut bb = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
                    for p in &ch.pts[from..=to] {
                        bb = [bb[0].min(p[0]), bb[1].min(p[1]), bb[2].max(p[0]), bb[3].max(p[1])];
                    }
                    f.runs[slot].push(Run { chain: ci as u32, a: from as u32, b: to as u32, relevant, bbox: bb });
                    if relevant {
                        f.relevant_segments += to - from;
                    }
                };
                let (mut start, mut cur) = (0usize, cor.segment(ch.pts[0], ch.pts[1]));
                for s in 1..nseg {
                    let r = cor.segment(ch.pts[s], ch.pts[s + 1]);
                    if r != cur {
                        flush(&mut f, start, s, cur);
                        (start, cur) = (s, r);
                    }
                }
                flush(&mut f, start, nseg, cur);
            }
        }
        f.jumps = roads.jumps.iter().map(|j| cor.hit([j[0], j[1]]) && cor.hit([j[3], j[4]])).collect();
        f
    }
}

/// What [`RaceSel::road_focus`] cached for: the road data (rev and the `Arc`'s identity, so a
/// rebuild is noticed), the race layer and the line.
type FocusKey = (u64, usize, usize, usize);

/// A straight line along +z at x = `x`, from z = 0 to 1000, 5 m spacing, half-width 6 m (tests,
/// here and in `paint2d`).
#[cfg(test)]
pub(crate) fn test_line(route: u32, x: f32, reversed: bool) -> RaceLine {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maprender::data::Chain;
    use super::test_line as line;

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
        let got = sel.update(&layer, &cfg(RaceLineMode::Current), (1.0, 300.0), 0.0, true).picked().to_vec();
        assert_eq!(got, vec![0]);
        // Facing south (yaw π): the reversed line 2 agrees; at x = 7 the car is 1 m from it.
        let mut sel = RaceSel::default();
        let got = sel.update(&layer, &cfg(RaceLineMode::Current), (7.0, 300.0), std::f32::consts::PI, true).picked().to_vec();
        assert_eq!(got, vec![1]);
        // Not in a race: nothing, however well it matches.
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Current), (1.0, 300.0), 0.0, false).picked().is_empty());
        // Too far from every line (more than half-width + 4 m): nothing.
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Current), (200.0, 300.0), 0.0, true).picked().is_empty());
    }

    #[test]
    fn current_race_pick_sticks_while_it_still_matches() {
        // Two lines 8 m apart, same direction. Start nearer line 1, drift towards line 2 but
        // stay within line 1's corridor: the pick stays on line 1 instead of flipping.
        let layer = RaceLayer::new(vec![line(1, 0.0, false), line(2, 8.0, false)]);
        let mut sel = RaceSel::default();
        let c = cfg(RaceLineMode::Current);
        assert_eq!(sel.update(&layer, &c, (1.0, 100.0), 0.0, true).picked(), &[0]);
        assert_eq!(sel.update(&layer, &c, (6.0, 110.0), 0.0, true).picked(), &[0]);
        // Leaving line 1's corridor (6 + 4 = 10 m) hands over to line 2.
        assert_eq!(sel.update(&layer, &c, (12.0, 120.0), 0.0, true).picked(), &[1]);
        // Leaving every corridor keeps the last pick for the moment; leaving the race clears it.
        assert_eq!(sel.update(&layer, &c, (80.0, 130.0), 0.0, true).picked(), &[1]);
        assert!(sel.update(&layer, &c, (80.0, 130.0), 0.0, false).picked().is_empty());
    }

    #[test]
    fn nearest_near_off_and_all() {
        let layer = RaceLayer::new(vec![line(1, 0.0, false), line(2, 300.0, false), line(3, 2000.0, false)]);
        let mut sel = RaceSel::default();
        assert_eq!(sel.update(&layer, &cfg(RaceLineMode::Nearest), (50.0, 500.0), 0.0, false).picked(), &[0]);
        let mut sel = RaceSel::default();
        let mut near = sel.update(&layer, &cfg(RaceLineMode::Near), (150.0, 500.0), 0.0, false).picked().to_vec();
        near.sort();
        assert_eq!(near, vec![0, 1]); // within 1500 m: lines 1 and 2, not 3 (1850 m away)
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Off), (0.0, 0.0), 0.0, true).picked().is_empty());
        assert!(sel.update(&layer, &cfg(RaceLineMode::All), (0.0, 0.0), 0.0, true).picked().is_empty());
        // Nothing within a tiny radius.
        let mut sel = RaceSel::default();
        let small = RaceCfg { mode: RaceLineMode::Nearest, radius_m: 20.0, ..RaceCfg::default() };
        assert!(sel.update(&layer, &small, (150.0, 500.0), 0.0, false).picked().is_empty());
    }

    fn road(pts: &[[f32; 2]]) -> Chain {
        Chain::new(pts.to_vec(), vec![0.0; pts.len()])
    }

    /// Roads around a straight race line at x = 0 (z 0..1000, half-width 6, corridor 14 m).
    fn roads_for_focus() -> RoadLayer {
        let mut r = RoadLayer::default();
        let slot = RoadType::Road.index() as usize;
        // 0: runs along the line 10 m beside it (inside); 1: 30 m beside it (outside);
        // 2: crosses it at z = 500 with nodes 20 m apart around the crossing.
        r.by_type[slot].push(road(&[[10.0, 100.0], [10.0, 300.0], [10.0, 600.0]]));
        r.by_type[slot].push(road(&[[30.0, 100.0], [30.0, 600.0]]));
        r.by_type[slot].push(road(&[[-300.0, 500.0], [-100.0, 500.0], [-10.0, 500.0], [10.0, 500.0], [100.0, 500.0], [300.0, 500.0]]));
        // Far from the line altogether (skipped by the box test), and a turnaround (never classified).
        r.by_type[slot].push(road(&[[5000.0, 0.0], [5000.0, 100.0]]));
        r.by_type[RoadType::Turnaround.index() as usize].push(road(&[[0.0, 0.0], [0.0, 100.0]]));
        // Jump lines: take-off and landing both on the corridor / one end off it.
        r.jumps.push([0.0, 700.0, 0.0, 0.0, 740.0, 0.0]);
        r.jumps.push([0.0, 700.0, 0.0, 200.0, 740.0, 0.0]);
        r
    }

    fn relevance(f: &RoadFocus, slot: RoadType, chain: u32) -> Vec<(u32, u32, bool)> {
        f.runs[slot.index() as usize].iter().filter(|r| r.chain == chain).map(|r| (r.a, r.b, r.relevant)).collect()
    }

    #[test]
    fn corridor_keeps_roads_along_the_line_and_a_crossing_only_near_the_crossing() {
        let roads = roads_for_focus();
        let f = RoadFocus::build(&roads, &line(1, 0.0, false));
        let t = RoadType::Road;
        assert_eq!(relevance(&f, t, 0), vec![(0, 2, true)], "10 m beside a 6 m wide line (14 m corridor)");
        assert_eq!(relevance(&f, t, 1), vec![(0, 1, false)], "30 m beside it");
        // The crossing road: relevant only for the short piece across the line.
        assert_eq!(relevance(&f, t, 2), vec![(0, 2, false), (2, 3, true), (3, 5, false)]);
        assert_eq!(relevance(&f, t, 3), vec![(0, 1, false)], "far chain: one other run");
        assert!(f.runs[RoadType::Turnaround.index() as usize].is_empty());
        assert_eq!(f.jumps, vec![true, false]);
        // Runs of a chain tile its segments and carry the right box.
        let r = f.runs[t.index() as usize].iter().find(|r| r.chain == 2 && r.relevant).unwrap();
        assert_eq!(r.bbox, [-10.0, 500.0, 10.0, 500.0]);
        assert_eq!((f.segments, f.relevant_segments), (2 + 1 + 5 + 1, 2 + 1));
    }

    #[test]
    fn corridor_follows_the_half_width_of_the_line() {
        let roads = roads_for_focus();
        let mut wide = line(1, 0.0, false);
        wide.half = vec![[25.0, 0.0]; wide.pts.len()]; // corridor 33 m: the 30 m road is in now
        let f = RoadFocus::build(&roads, &wide);
        assert_eq!(relevance(&f, RoadType::Road, 1), vec![(0, 1, true)]);
        let mut thin = line(1, 0.0, false);
        thin.half = vec![[0.0, 0.0]; thin.pts.len()]; // corridor = the 8 m slack: the 10 m road is out
        let f = RoadFocus::build(&roads, &thin);
        assert_eq!(relevance(&f, RoadType::Road, 0), vec![(0, 2, false)]);
    }

    #[test]
    fn focus_is_on_only_in_a_race_with_a_picked_line_in_current_mode() {
        let layer = RaceLayer::new(vec![line(1, 0.0, false)]);
        let on = |mode, in_race| {
            let mut sel = RaceSel::default();
            sel.update(&layer, &cfg(mode), (1.0, 300.0), 0.0, in_race).focus_line()
        };
        assert_eq!(on(RaceLineMode::Current, true), Some(0));
        assert_eq!(on(RaceLineMode::Current, false), None, "not in a race");
        assert_eq!(on(RaceLineMode::Nearest, true), None, "only the detected race counts");
        assert_eq!(on(RaceLineMode::All, true), None);
        // In a race but no line under the car (detection unsure): no focus.
        let mut sel = RaceSel::default();
        assert_eq!(sel.update(&layer, &cfg(RaceLineMode::Current), (300.0, 300.0), 0.0, true).focus_line(), None);
        // The focus ends with the race.
        let mut sel = RaceSel::default();
        assert!(sel.update(&layer, &cfg(RaceLineMode::Current), (1.0, 300.0), 0.0, true).focus_line().is_some());
        assert!(sel.update(&layer, &cfg(RaceLineMode::Current), (1.0, 300.0), 0.0, false).focus_line().is_none());
    }

    /// Real data: time and share of the in-race focus for the longest route (5555, 85 km) and the
    /// median line. `cargo test --release real_install_focus -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_install_focus_cost() {
        use crate::gamedata::roadtypes::RoadTypes;
        let Some(media) = crate::gamedata::install::find_media(None) else { return };
        let g = crate::maprender::data::GameData::load(&media).expect("game data");
        let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), std::path::Path::new("/nonexistent"), &g.nav);
        let layers = g.layers(&cur, 1);
        let all = RoadFocus::build(&layers.roads, &layers.races.lines[0]); // warm up
        let total = all.segments;
        let report = |label: &str, li: usize| {
            let l = &layers.races.lines[li];
            let t0 = std::time::Instant::now();
            let mut f = RoadFocus::build(&layers.roads, l);
            for _ in 0..4 {
                f = RoadFocus::build(&layers.roads, l);
            }
            let ms = t0.elapsed().as_secs_f64() * 1e3 / 5.0;
            let runs: usize = f.runs.iter().map(Vec::len).sum();
            eprintln!(
                "{label}: route {} ({} pts, {:.1} km): focus {ms:.1} ms, {} of {total} road segments relevant ({:.2} %), {runs} runs, {} of {} jumps",
                l.route,
                l.pts.len(),
                l.length_m / 1000.0,
                f.relevant_segments,
                100.0 * f.relevant_segments as f64 / total as f64,
                f.jumps.iter().filter(|j| **j).count(),
                f.jumps.len(),
            );
        };
        let longest = (0..layers.races.lines.len()).max_by(|&a, &b| layers.races.lines[a].length_m.total_cmp(&layers.races.lines[b].length_m)).unwrap();
        report("longest", longest);
        let mut by_len: Vec<usize> = (0..layers.races.lines.len()).collect();
        by_len.sort_by(|&a, &b| layers.races.lines[a].length_m.total_cmp(&layers.races.lines[b].length_m));
        report("median", by_len[by_len.len() / 2]);
        report("shortest", by_len[0]);
    }

    #[test]
    fn road_focus_is_built_once_per_line_and_road_data() {
        let races = Arc::new(RaceLayer::new(vec![line(1, 0.0, false), line(2, 500.0, false)]));
        let layers = MapLayers { rev: 1, roads: Arc::new(roads_for_focus()), races, ..Default::default() };
        let mut sel = RaceSel::default();
        let c = cfg(RaceLineMode::Current);
        sel.update(&layers.races, &c, (1.0, 300.0), 0.0, true);
        let (a, b) = (sel.road_focus(&layers).unwrap(), sel.road_focus(&layers).unwrap());
        assert!(Arc::ptr_eq(&a, &b), "second call reuses the cache");
        // The car moves along the same line: still cached.
        sel.update(&layers.races, &c, (1.0, 340.0), 0.0, true);
        assert!(Arc::ptr_eq(&a, &sel.road_focus(&layers).unwrap()));
        // New road data (a road-type edit rebuilds the roads): recomputed.
        let rebuilt = MapLayers { rev: 2, roads: Arc::new(roads_for_focus()), ..layers.clone() };
        assert!(!Arc::ptr_eq(&a, &sel.road_focus(&rebuilt).unwrap()));
        // Another line: recomputed, and its corridor is elsewhere.
        let mut sel = RaceSel::default();
        sel.update(&layers.races, &c, (501.0, 300.0), 0.0, true);
        let other = sel.road_focus(&layers).unwrap();
        assert!(other.runs[RoadType::Road.index() as usize].iter().all(|r| !r.relevant));
        // No focus, no data.
        assert!(RaceSel::default().road_focus(&layers).is_none());
    }
}
