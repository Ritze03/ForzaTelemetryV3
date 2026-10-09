//! Which race lines to draw. The interesting mode is "current race": the telemetry has **no
//! race id** (`packet.rs` has `race_position` and lap fields, nothing that names the route),
//! so the route is *inferred* while `race_position != 0` (the HUD's own "in a race" rule).
//!
//! **Candidate routes (D76).** Many routes share their first stretch of road and fork later, so
//! committing to one route at once would draw a possibly wrong finish. Instead the selector keeps
//! the **set of routes consistent with where the car has driven** ([`Cand`]: line + the car's
//! progress along it in metres of arc length). A route stays a candidate while the car is within
//! `|half-width| + 4 m` of its centre line, driving the way it runs, and its progress is
//! plausible; it is dropped after [`DROP_M`] of driving off it (or while another candidate is
//! clearly closer). What is **drawn** is the part all candidates agree on ([`Span`]): from where
//! they all coincide behind the car up to the fork ahead, with no finish mark. Once one candidate
//! is left (or the rest are the same geometry to the finish) the whole route is drawn as before.
//! Details and the numbers: `docs/features/minimap.md`, "Race lines and the current race".
//! Once certain, the route is **locked** for the rest of the race (through lap wraps; given up only
//! after [`LOCK_DROP_M`] off it, a race end or a mode change).
//!
//! **Race road (D80).** With `RouteStyle::Road` the focus also carries what is drawn of the line as
//! a [`RaceRoad`] (points, heights, colour): the 3D scene builds its race road mesh from it.
//!
//! **In-race focus (D66).** The same selection also says which *roads* belong to the race, so the
//! renderer can mute or hide the rest (`cfg::RaceFocusCfg`). The focus is on only when the mode is
//! `Current`, the car is in a race and a line is picked; otherwise [`RaceSel::focus_line`] is
//! `None` and every road is drawn normally (a failed guess must never blank the map).
//! [`RoadFocus`] cuts each road chain into runs along / away from the corridor of **what is
//! drawn** (the line's `half` width + [`FOCUS_SLACK_M`]; the shared prefix while the route is
//! uncertain, the whole line once it is certain), computed **once per drawn extent** and cached in
//! the `RaceSel`, which is why `update` returns the selector itself: the renderer reads both the
//! picked lines and this cache from it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::cfg::{RaceCfg, RaceLineMode, Rgb, RouteStyle};
use super::data::{MapLayers, RaceLayer, RoadLayer, N_TYPES};
use super::mesh3d::known_y;
use super::view::bbox_hits;
use crate::gamedata::racelines::RaceLine;
use crate::gamedata::roadtypes::RoadType;

/// Extra metres beyond a line's half-width that still count as "on" it.
pub const ON_LINE_SLACK_M: f32 = 4.0;
/// Lines nearer/near the car are recomputed when it moved this far (m).
const MOVE_M: f32 = 10.0;

/// A route stays a candidate through this much driving off it (m) before it is dropped. Short
/// enough that the true route is certain soon after a fork, long enough for a shortcut or a wide
/// corner.
pub const DROP_M: f32 = 15.0;
/// A candidate this much farther from the car than the nearest one counts as "off" (m): on a
/// fork the branch the car did not take falls out of the corridor late (the corridor is wide), but
/// it is clearly the farther one much sooner.
pub const DOMINATED_M: f32 = 8.0;
/// Heading agreement (cosine) a route needs to be *picked up* (the first match, or a new match
/// after losing every candidate). Tracking an existing candidate only needs `> 0`, so a drift or a
/// wide corner does not lose it; a crossing road at a right angle must not start one.
pub const ACQUIRE_COS: f32 = 0.5;
/// How far the car's progress along a candidate may jump between two frames (m): back (a rewind)
/// and forward (a dropped-packet gap, a fast stretch).
const WINDOW_BACK_M: f32 = 300.0;
const WINDOW_AHEAD_M: f32 = 300.0;
/// Two routes are "the same road" while their centre lines are this close (m).
pub const SAME_TOL_M: f32 = 4.0;
/// Spacing of the comparison samples along the reference route (m).
const WALK_STEP_M: f32 = 4.0;
/// How far ahead of its last position a route is searched for the match of a sample (m).
const WALK_LOOK_M: f32 = 40.0;
/// Two routes that coincide to the end are "the same" when the longer one has at most this much
/// left (m): duplicates with a slightly different finish line.
pub const FINISH_TOL_M: f32 = 30.0;
/// A route the selector was certain of stays the race's route until the car has driven this far
/// off it (m). *Why (the user, 2026-10-09):* on lap 2 of a circuit whose start a sprint shares,
/// the selection "just threw away that information and just started fresh … it should stay with
/// its decision". A lap wrap, the start area or a short excursion never resets it; only the race
/// ending (`race_position` 0), a mode change or this much driving elsewhere does.
pub const LOCK_DROP_M: f32 = 200.0;

/// One route the car may be on: the line and the car's progress `s` along it (metres from the
/// line's first point; closed circuits wrap), and how far the car has driven since it last matched.
#[derive(Clone, Copy, Debug)]
struct Cand {
    line: usize,
    s: f32,
    off_m: f32,
}

/// What of the picked line is drawn: the arc-length range `s0..s1` (m from the first point;
/// closed circuits may run past either end and wrap) and which end marks belong with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub s0: f32,
    pub s1: f32,
    /// The range begins at the route's start: draw the start mark.
    pub start: bool,
}

/// What the candidates add up to: the line to draw (`None` span = the whole line, both marks, as
/// before) and what it was computed for.
#[derive(Debug)]
struct Shown {
    line: usize,
    span: Option<Span>,
    /// The candidate set (line indices, sorted) and the layer this was computed for.
    key: (Vec<usize>, usize),
}

/// Selection state kept by the caller (one per map): the picked line indices, plus what is
/// needed to avoid recomputing every frame, plus the in-race focus cache.
#[derive(Default, Debug)]
pub struct RaceSel {
    /// Indices into `RaceLayer::lines` to draw.
    pub picked: Vec<usize>,
    /// The routes the car may be on (`Current` mode, in a race), sorted by line.
    cands: Vec<Cand>,
    last_car: Option<(f32, f32)>,
    shown: Option<Shown>,
    /// Bumped when the drawn line or extent changes (the focus cache key).
    shown_gen: u32,
    at: Option<(f32, f32)>,
    key: (Option<RaceLineMode>, usize, u32),
    /// The in-race focus applies: `Current` mode, in a race, a line picked.
    focus_on: bool,
    /// The road focus of the drawn extent. A `Mutex` (not a `RefCell`) so the selector can be
    /// shared by `&` with the renderer and still be a `static` in tests.
    cache: Mutex<Option<(FocusKey, Arc<RoadFocus>)>>,
    /// The race road's colours (circuit, sprint) when race lines are drawn as roads (D80,
    /// `RouteStyle::Road`), from the config of the last `update`; `None` = drawn as a line.
    route_cols: Option<(Rgb, Rgb)>,
    /// The route the selector is certain of (D80 fix): kept for the rest of the race, through
    /// lap wraps and past routes that share its start (see [`LOCK_DROP_M`]).
    locked: Option<usize>,
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
    half_at(&layer.lines[line].half, s)
}

fn half_at(half: &[[f32; 2]], s: usize) -> f32 {
    let m = |i: usize| half.get(i).map_or(0.0, |v| (v[0] * v[0] + v[1] * v[1]).sqrt());
    m(s).max(m(s + 1))
}

// ── arc-length view of a line ────────────────────────────────────────────────────────────────

/// One race line by arc length `s` (metres from its first point). A closed circuit wraps: `s` may
/// be any real number and the closing segment is part of the loop.
pub struct Poly<'a> {
    pts: &'a [[f32; 2]],
    half: &'a [[f32; 2]],
    y: &'a [f32],
    cum: &'a [f32],
    closed: bool,
    /// Vertices of the loop / line (a closed line whose last point repeats the first drops it).
    m: usize,
    total: f32,
}

impl<'a> Poly<'a> {
    pub fn new(layer: &'a RaceLayer, li: usize) -> Poly<'a> {
        let l = &layer.lines[li];
        let cum = &layer.cum[li][..];
        let n = l.pts.len();
        let closed = l.closed && n >= 3;
        let dup = closed && (l.pts[0][0] - l.pts[n - 1][0]).hypot(l.pts[0][1] - l.pts[n - 1][1]) < 0.5;
        let m = if dup { n - 1 } else { n };
        let total = if m < 2 {
            0.0
        } else if closed {
            cum[m - 1] + (l.pts[m - 1][0] - l.pts[0][0]).hypot(l.pts[m - 1][1] - l.pts[0][1])
        } else {
            cum[n - 1]
        };
        Poly { pts: &l.pts, half: &l.half, y: &l.y, cum, closed, m, total }
    }

    fn norm(&self, s: f32) -> f32 {
        if self.closed {
            s.rem_euclid(self.total.max(1e-3))
        } else {
            s.clamp(0.0, self.total)
        }
    }

    /// Height at arc length `s` (`0` when the line has none).
    fn y_at(&self, s: f32) -> f32 {
        let s = self.norm(s);
        let j = self.cum[..self.m].partition_point(|&c| c <= s).saturating_sub(1).min(self.m - 1);
        let yv = |i: usize| self.y.get(i % self.m).copied().unwrap_or(0.0);
        let end = if j + 1 < self.m { self.cum[j + 1] } else { self.total };
        let len = end - self.cum[j];
        let t = if len > 1e-6 { ((s - self.cum[j]) / len).clamp(0.0, 1.0) } else { 0.0 };
        if !self.closed && j + 1 >= self.m {
            return yv(j);
        }
        yv(j) + (yv(j + 1) - yv(j)) * t
    }

    /// Position and half-width vector at arc length `s`.
    pub fn at(&self, s: f32) -> ([f32; 2], [f32; 2]) {
        let hv = |i: usize| self.half.get(i).copied().unwrap_or_default();
        if self.m < 2 {
            return (self.pts.first().copied().unwrap_or_default(), hv(0));
        }
        let s = self.norm(s);
        let j = self.cum[..self.m].partition_point(|&c| c <= s).saturating_sub(1).min(self.m - 1);
        if !self.closed && j + 1 >= self.m {
            return (self.pts[j], hv(j));
        }
        let (j2, end) = if j + 1 < self.m { (j + 1, self.cum[j + 1]) } else { (0, self.total) };
        let len = end - self.cum[j];
        let t = if len > 1e-6 { ((s - self.cum[j]) / len).clamp(0.0, 1.0) } else { 0.0 };
        let (a, b, ha, hb) = (self.pts[j], self.pts[j2], hv(j), hv(j2));
        ([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], [ha[0] + (hb[0] - ha[0]) * t, ha[1] + (hb[1] - ha[1]) * t])
    }

    /// Nearest point of the line to `p` among the segments that overlap the arc range `lo..hi`:
    /// `(distance, arc length of that point)`. `(f32::MAX, lo)` if there is none.
    fn nearest_in(&self, p: [f32; 2], lo: f32, hi: f32) -> (f32, f32) {
        let mut best = (f32::MAX, lo);
        if self.m < 2 {
            return best;
        }
        let m = self.m as i64;
        let (lo, hi) = if self.closed {
            (lo, hi.min(lo + self.total))
        } else {
            let lo = lo.clamp(0.0, self.total);
            (lo, hi.clamp(lo, self.total))
        };
        let (w0, lo_in) = if self.closed {
            let w = (lo / self.total).floor();
            (w as i64, lo - w * self.total)
        } else {
            (0, lo)
        };
        let jmax = if self.closed { self.m - 1 } else { self.m - 2 };
        let j0 = self.cum[..self.m].partition_point(|&c| c <= lo_in).saturating_sub(1).min(jmax);
        let mut k = w0 * m + j0 as i64;
        loop {
            let (w, j) = (k.div_euclid(m) as f32, k.rem_euclid(m) as usize);
            if !self.closed && j >= self.m - 1 {
                break;
            }
            let a0 = w * self.total + self.cum[j];
            if a0 > hi {
                break;
            }
            let a1 = w * self.total + if j + 1 < self.m { self.cum[j + 1] } else { self.total };
            let (d, _, t) = seg_dist(p, self.pts[j], self.pts[(j + 1) % self.m]);
            if d < best.0 {
                best = (d, a0 + t * (a1 - a0));
            }
            k += 1;
        }
        best
    }

    /// The part `s0..s1` of the line as points and half-width vectors (ends interpolated). A
    /// closed circuit wraps past either end, at most one lap.
    pub fn slice(&self, s0: f32, s1: f32) -> (Vec<[f32; 2]>, Vec<[f32; 2]>, Vec<f32>) {
        let (mut pts, mut half, mut ys) = (Vec::new(), Vec::new(), Vec::new());
        if self.m < 2 {
            return (pts, half, ys);
        }
        let (s0, s1) = if self.closed { (s0, s1.min(s0 + self.total)) } else { (s0.clamp(0.0, self.total), s1.clamp(0.0, self.total)) };
        if s1 < s0 {
            return (pts, half, ys);
        }
        let mut push = |(p, h): ([f32; 2], [f32; 2]), y: f32| {
            pts.push(p);
            half.push(h);
            ys.push(y);
        };
        push(self.at(s0), self.y_at(s0));
        let m = self.m as i64;
        let (w0, lo_in) = if self.closed {
            let w = (s0 / self.total).floor();
            (w as i64, s0 - w * self.total)
        } else {
            (0, s0)
        };
        let mut k = w0 * m + self.cum[..self.m].partition_point(|&c| c <= lo_in) as i64;
        loop {
            let (w, j) = (k.div_euclid(m), k.rem_euclid(m) as usize);
            if !self.closed && w > 0 {
                break;
            }
            let arc = w as f32 * self.total + self.cum[j];
            if arc >= s1 {
                break;
            }
            push((self.pts[j], self.half.get(j).copied().unwrap_or_default()), self.y.get(j).copied().unwrap_or(0.0));
            k += 1;
        }
        push(self.at(s1), self.y_at(s1));
        (pts, half, ys)
    }
}

/// One other route in a [`walk`]: its line and the arc position its pointer has reached.
struct Other<'a> {
    poly: Poly<'a>,
    ci: f32,
}

/// Walk the reference route from arc `s_ref` in direction `dir` (+1 / -1), up to `max_len` metres,
/// and stop where any of `others` is no longer within [`SAME_TOL_M`] of it (each other route is
/// followed by a pointer that only moves along, so a route that comes back past the same road
/// later is not confused). Returns the metres walked and whether the walk got to `max_len`.
fn walk(r: &Poly, s_ref: f32, others: &mut [Other], dir: f32, max_len: f32) -> (f32, bool) {
    let (mut good, mut d) = (0.0f32, 0.0f32);
    loop {
        let p = r.at(s_ref + dir * d).0;
        for o in others.iter_mut() {
            let (lo, hi) = if dir > 0.0 { (o.ci - 10.0, o.ci + WALK_LOOK_M) } else { (o.ci - WALK_LOOK_M, o.ci + 10.0) };
            let (dist, arc) = o.poly.nearest_in(p, lo, hi);
            if dist > SAME_TOL_M {
                return (good, false);
            }
            o.ci = if dir > 0.0 { o.ci.max(arc) } else { o.ci.min(arc) };
        }
        good = d;
        if d >= max_len {
            return (good, true);
        }
        d = (d + WALK_STEP_M).min(max_len);
    }
}

/// What to draw for the candidate set `cands` (sorted by line, at least one): see [`Shown`].
/// The reference route is the shortest; every other candidate is compared with it, forward from
/// the car to the first fork (or the end of the reference), backward to the first difference (or
/// the start). The forward walk reaching the end with every other route also at its end (within
/// [`FINISH_TOL_M`]) means they are the same road to the finish: certain enough, the lowest line
/// index is drawn whole.
fn compute_shown(layer: &RaceLayer, cands: &[Cand]) -> Shown {
    let key = (cands.iter().map(|c| c.line).collect::<Vec<_>>(), layer.lines.as_ptr() as usize);
    if cands.len() == 1 {
        return Shown { line: cands[0].line, span: None, key };
    }
    let ri = (0..cands.len()).min_by(|&a, &b| Poly::new(layer, cands[a].line).total.total_cmp(&Poly::new(layer, cands[b].line).total)).unwrap_or(0);
    let r = Poly::new(layer, cands[ri].line);
    let s_r = r.norm(cands[ri].s);
    let others = || -> Vec<Other> { cands.iter().enumerate().filter(|(i, _)| *i != ri).map(|(_, c)| Other { poly: Poly::new(layer, c.line), ci: c.s }).collect() };
    let mut o = others();
    let max_fwd = if r.closed { r.total } else { r.total - s_r };
    let (fwd, complete) = walk(&r, s_r, &mut o, 1.0, max_fwd);
    let all_end = o.iter().all(|x| x.poly.closed || x.poly.total - x.ci <= FINISH_TOL_M);
    if complete && all_end {
        return Shown { line: cands[0].line, span: None, key };
    }
    let mut o = others();
    let max_back = if r.closed { (r.total - fwd).max(0.0) } else { s_r };
    let (back, _) = walk(&r, s_r, &mut o, -1.0, max_back);
    let (s0, s1) = (s_r - back, s_r + fwd);
    // The start mark belongs with the range when it reaches the route's first point (to a step).
    let start = if r.closed { ((s0 - WALK_STEP_M) / r.total).ceil() * r.total <= s1 } else { s0 <= WALK_STEP_M };
    let s0 = if !r.closed && start { 0.0 } else { s0 };
    Shown { line: cands[ri].line, span: Some(Span { s0, s1, start }), key }
}

/// One line segment near the car that agrees with its heading.
struct Hit {
    line: usize,
    arc: f32,
    d: f32,
    cos: f32,
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
    /// race). For `All` the caller draws every line in view, so the list is empty. In `Current`
    /// mode it is the one line to draw: the whole route once it is certain, otherwise the
    /// reference of the candidates, of which only [`span`](Self::span) is drawn.
    pub fn picked(&self) -> &[usize] {
        &self.picked
    }

    /// What of picked line `line` to draw: `None` = all of it with both marks (a certain route, or
    /// any mode but the uncertain `Current`); `Some` = only that part, no finish mark.
    pub fn span(&self, line: usize) -> Option<Span> {
        self.shown.as_ref().filter(|s| s.line == line).and_then(|s| s.span)
    }

    /// How many routes the car may be on right now (0 outside a race or with no match).
    #[cfg(test)]
    pub fn candidates(&self) -> usize {
        self.cands.len()
    }

    /// The race line the in-race focus is about, if it is on: `Current` mode, in a race, and a
    /// line picked. `None` = draw everything normally.
    pub fn focus_line(&self) -> Option<usize> {
        self.focus_on.then(|| self.picked.first().copied()).flatten()
    }

    /// Which roads belong to what is drawn of the focus line (cached per drawn extent + road
    /// data; the first call after a change builds it, see [`RoadFocus::build`]). `None` without a
    /// focus line. While the route is uncertain this is the corridor of the shared prefix only.
    pub fn road_focus(&self, layers: &MapLayers) -> Option<Arc<RoadFocus>> {
        let li = self.focus_line()?;
        let line = layers.races.lines.get(li)?;
        let colour = self.route_cols.map(|(c, s)| if line.circuit { c } else { s });
        let key: FocusKey = (layers.rev, Arc::as_ptr(&layers.roads) as usize, Arc::as_ptr(&layers.races) as usize, li, self.shown_gen, colour.map(|c| c.0));
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((k, f)) = cache.as_ref() {
            if *k == key {
                return Some(f.clone());
            }
        }
        let mut f = match self.span(li) {
            Some(sp) if li < layers.races.cum.len() => {
                let (pts, half, y) = Poly::new(&layers.races, li).slice(sp.s0, sp.s1);
                let mut f = RoadFocus::build_pts(&layers.roads, &pts, &half, &y, false);
                f.race = colour.map(|color| RaceRoad { pts, y, closed: false, color });
                f
            }
            _ => {
                let mut f = RoadFocus::build(&layers.roads, line);
                f.race = colour.map(|color| RaceRoad { pts: line.pts.clone(), y: line.y.clone(), closed: line.closed, color });
                f
            }
        };
        f.race = f.race.take().filter(|r| r.pts.len() >= 2);
        let f = Arc::new(f);
        *cache = Some((key, f.clone()));
        Some(f)
    }

    /// A selector with a fixed pick, for tests of the renderer.
    #[cfg(test)]
    pub fn fixed(picked: Vec<usize>, focus: bool) -> RaceSel {
        RaceSel { picked, focus_on: focus, ..Default::default() }
    }

    /// Forget the candidates and what was drawn for them (the race ended, the mode changed).
    fn reset_current(&mut self) {
        self.cands.clear();
        self.locked = None;
        self.last_car = None;
        if self.shown.take().is_some() {
            self.shown_gen = self.shown_gen.wrapping_add(1);
        }
    }

    /// Follow the candidates one frame (see the module docs for the rules).
    fn track(&mut self, layer: &RaceLayer, car: (f32, f32), yaw: f32) {
        let fwd = (yaw.sin(), yaw.cos());
        let travelled = self.last_car.map_or(0.0, |p| (p.0 - car.0).hypot(p.1 - car.1));
        self.last_car = Some(car);
        if let Some(li) = self.locked {
            self.track_locked(layer, li, car, fwd, travelled);
            return;
        }
        // Every segment near the car that runs the way the car faces and has it inside its corridor.
        let mut hits: Vec<Hit> = Vec::new();
        layer.grid.near(car.0, car.1, 40.0, |li, s| {
            let (li, s) = (li as usize, s as usize);
            let l = &layer.lines[li];
            if s + 1 >= l.pts.len() {
                return;
            }
            let (d, dir, t) = seg_dist([car.0, car.1], l.pts[s], l.pts[s + 1]);
            let cos = dir[0] * fwd.0 + dir[1] * fwd.1;
            if d > half_width(layer, li, s) + ON_LINE_SLACK_M || cos <= 0.0 {
                return;
            }
            let c = &layer.cum[li];
            hits.push(Hit { line: li, arc: c[s] + t * (c[s + 1] - c[s]), d, cos });
        });
        // Where each candidate stands now: its best hit within the progress window.
        let mut on: Vec<Option<(f32, f32)>> = Vec::with_capacity(self.cands.len());
        for c in &self.cands {
            let poly = Poly::new(layer, c.line);
            let delta = |arc: f32| {
                let d = arc - c.s;
                // A circuit wraps even when its line does not close within CLOSED_M (the progress jumps
                // from its end to its start at the line): the lap-2 reset the user saw.
                if poly.closed || layer.lines[c.line].circuit {
                    (d + poly.total / 2.0).rem_euclid(poly.total.max(1e-3)) - poly.total / 2.0
                } else {
                    d
                }
            };
            let best = hits
                .iter()
                .filter(|h| h.line == c.line && (-WINDOW_BACK_M..=WINDOW_AHEAD_M).contains(&delta(h.arc)))
                .min_by(|a, b| (a.d + 0.02 * delta(a.arc).abs()).total_cmp(&(b.d + 0.02 * delta(b.arc).abs())));
            on.push(best.map(|h| (h.arc, h.d)));
        }
        // A candidate far behind the nearest one is as good as off it.
        let nearest = on.iter().flatten().map(|o| o.1).fold(f32::MAX, f32::min);
        for o in on.iter_mut() {
            if o.is_some_and(|(_, d)| d > nearest + DOMINATED_M) {
                *o = None;
            }
        }
        for (c, o) in self.cands.iter_mut().zip(&on) {
            match o {
                Some((arc, _)) => (c.s, c.off_m) = (*arc, 0.0),
                None => c.off_m += travelled,
            }
        }
        if on.iter().any(Option::is_some) {
            self.cands.retain(|c| c.off_m <= DROP_M);
        } else {
            // On none of them. A new match (a heading that agrees well) replaces the set, as a
            // new match replaced the single sticky pick before; otherwise keep them for the
            // moment (a shortcut, a wide corner, a spin).
            let mut fresh: Vec<Cand> = Vec::new();
            for h in hits.iter().filter(|h| h.cos > ACQUIRE_COS) {
                match fresh.iter_mut().find(|c| c.line == h.line) {
                    Some(c) if h.d < c.off_m => (c.s, c.off_m) = (h.arc, h.d),
                    Some(_) => {}
                    // `off_m` carries the distance until the candidate is finished below.
                    None => fresh.push(Cand { line: h.line, s: h.arc, off_m: h.d }),
                }
            }
            if !fresh.is_empty() {
                fresh.iter_mut().for_each(|c| c.off_m = 0.0);
                fresh.sort_by_key(|c| c.line);
                self.cands = fresh;
            }
        }
    }

    /// Follow the locked route one frame: its nearest stretch under the car (any progress: a lap
    /// wraps from the end to the start, and a circuit whose line does not close exactly jumps
    /// there), heading agreeing. Unlocked only after [`LOCK_DROP_M`] driven off it; then the usual
    /// candidate rules apply again (it stays the pick until another route matches).
    fn track_locked(&mut self, layer: &RaceLayer, li: usize, car: (f32, f32), fwd: (f32, f32), travelled: f32) {
        let mut best: Option<(f32, f32)> = None;
        layer.grid.near(car.0, car.1, 40.0, |l, s| {
            let (l, s) = (l as usize, s as usize);
            let line = &layer.lines[l];
            if l != li || s + 1 >= line.pts.len() {
                return;
            }
            let (d, dir, t) = seg_dist([car.0, car.1], line.pts[s], line.pts[s + 1]);
            if d > half_width(layer, l, s) + ON_LINE_SLACK_M || dir[0] * fwd.0 + dir[1] * fwd.1 <= 0.0 {
                return;
            }
            let c = &layer.cum[l];
            if best.is_none_or(|b| d < b.1) {
                best = Some((c[s] + t * (c[s + 1] - c[s]), d));
            }
        });
        if self.cands.len() != 1 || self.cands[0].line != li {
            self.cands = vec![Cand { line: li, s: 0.0, off_m: 0.0 }];
        }
        let c = &mut self.cands[0];
        match best {
            Some((arc, _)) => (c.s, c.off_m) = (arc, 0.0),
            None => c.off_m += travelled,
        }
        if c.off_m > LOCK_DROP_M {
            // Back to candidate tracking: the route stays the pick until another one matches.
            self.locked = None;
        }
    }

    /// Recompute what is drawn when the candidate set changed.
    fn refresh_shown(&mut self, layer: &RaceLayer) {
        if self.cands.is_empty() {
            self.picked.clear();
            if self.shown.take().is_some() {
                self.shown_gen = self.shown_gen.wrapping_add(1);
            }
            return;
        }
        let key = (self.cands.iter().map(|c| c.line).collect::<Vec<_>>(), layer.lines.as_ptr() as usize);
        if self.shown.as_ref().is_none_or(|s| s.key != key) {
            let new = compute_shown(layer, &self.cands);
            if self.shown.as_ref().is_none_or(|s| s.line != new.line || s.span != new.span) {
                self.shown_gen = self.shown_gen.wrapping_add(1);
            }
            // Certain (one route, or the rest the same road to the finish): lock it for the race.
            // Only when the set changes: a lock given up after LOCK_DROP_M stays given up until
            // the candidates move on.
            if new.span.is_none() {
                self.locked = Some(new.line);
                self.cands.retain(|c| c.line == new.line);
            }
            self.shown = Some(new);
        }
        self.picked = self.shown.iter().map(|s| s.line).collect();
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
                self.reset_current();
                self.picked.clear();
                self.at = None;
            }
            RaceLineMode::Current => {
                // New race data (a rebuild): the line indices mean something else now.
                if self.shown.as_ref().is_some_and(|s| s.key.1 != layer.lines.as_ptr() as usize) || self.cands.iter().any(|c| c.line >= layer.lines.len()) {
                    self.reset_current();
                }
                if !in_race {
                    self.reset_current();
                    self.picked.clear();
                } else {
                    self.track(layer, car, yaw);
                    self.refresh_shown(layer);
                }
            }
            RaceLineMode::Nearest | RaceLineMode::Near => {
                self.reset_current();
                if moved || key != self.key {
                    let near = lines_near(layer, car, cfg.radius_m.max(1.0));
                    self.picked = if cfg.mode == RaceLineMode::Nearest { near.iter().take(1).map(|n| n.0).collect() } else { near.iter().map(|n| n.0).collect() };
                    self.at = Some(car);
                }
            }
        }
        self.key = key;
        self.focus_on = cfg.mode == RaceLineMode::Current && in_race && !self.picked.is_empty();
        self.route_cols = (cfg.route == RouteStyle::Road).then_some((cfg.circuit_color, cfg.sprint_color));
        self
    }
}

// ── in-race focus (D66) ──────────────────────────────────────────────────────────────────────

/// Metres beyond the race line's half-width that still count as "on the race" for a road.
pub const FOCUS_SLACK_M: f32 = 8.0;
/// A road segment is sampled every this many metres (ends included) when judging it.
const FOCUS_SAMPLE_M: f32 = 10.0;
/// A road counts as running along the race line when its heading is within this cosine of the
/// line's (about 20 degrees, either sense): side roads at 30 degrees or more do not.
pub const ALIGN_COS: f32 = 0.94;
/// A road only belongs to the race when its height is within this of the race line's (m): an
/// overpass or underpass crossing the route lies in the corridor horizontally but is another road.
pub const HEIGHT_TOL_M: f32 = 6.0;
/// Cell size of the corridor lookup, metres.
const CORRIDOR_CELL_M: f32 = 32.0;

/// The race line's corridor: its segments, each widened by its half-width plus
/// [`FOCUS_SLACK_M`]. A small grid of the **one** selected line (the store's `SegGrid` holds all
/// 170 lines in 100 m cells, so asking it "is this point near line N" would wade through the
/// others); a segment is registered in every cell its widened box touches, so a point only has to
/// look into its own cell.
struct Corridor {
    /// Per segment: its ends, its tolerance, and the line's heights at its ends (`NaN` = unknown).
    segs: Vec<([f32; 2], [f32; 2], f32, [f32; 2])>,
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// Widest tolerance, to reject far chains by their box.
    max_tol: f32,
}

impl Corridor {
    fn new(pts: &[[f32; 2]], half: &[[f32; 2]], y: &[f32], closed: bool) -> Corridor {
        let n = pts.len();
        let mut c = Corridor { segs: Vec::new(), cells: HashMap::new(), max_tol: 0.0 };
        if n < 2 {
            return c;
        }
        // A closed circuit's last point does not repeat the first: add the closing segment.
        let count = if closed && pts[0] != pts[n - 1] { n } else { n - 1 };
        let key = |x: f32, z: f32| ((x / CORRIDOR_CELL_M).floor() as i32, (z / CORRIDOR_CELL_M).floor() as i32);
        for s in 0..count {
            let (a, b) = (pts[s], pts[(s + 1) % n]);
            let tol = half_at(half, s) + FOCUS_SLACK_M;
            c.max_tol = c.max_tol.max(tol);
            let (k0, k1) = (key(a[0].min(b[0]) - tol, a[1].min(b[1]) - tol), key(a[0].max(b[0]) + tol, a[1].max(b[1]) + tol));
            let id = c.segs.len() as u32;
            let h = |i: usize| y.get(i).copied().and_then(known_y).unwrap_or(f32::NAN);
            c.segs.push((a, b, tol, [h(s), h((s + 1) % n)]));
            for cx in k0.0..=k1.0 {
                for cz in k0.1..=k1.1 {
                    c.cells.entry((cx, cz)).or_default().push(id);
                }
            }
        }
        c
    }

    /// Is `p` inside the corridor, on a stretch that runs the way `dir` (unit; either sense) does
    /// (`|cos| >= `[`ALIGN_COS`]), or any way when `dir` is `None`, and (when `y`, the height of
    /// the point, and the line's are both known) at the line's height, within [`HEIGHT_TOL_M`]?
    fn hit(&self, p: [f32; 2], dir: Option<[f32; 2]>, y: Option<f32>) -> bool {
        let k = ((p[0] / CORRIDOR_CELL_M).floor() as i32, (p[1] / CORRIDOR_CELL_M).floor() as i32);
        self.cells.get(&k).is_some_and(|v| {
            v.iter().any(|&i| {
                let (a, b, tol, ys) = self.segs[i as usize];
                let (d, sd, t) = seg_dist(p, a, b);
                let ly = ys[0] + (ys[1] - ys[0]) * t; // NaN when either end is unknown
                d <= tol
                    && dir.is_none_or(|u| (sd[0] * u[0] + sd[1] * u[1]).abs() >= ALIGN_COS)
                    && y.is_none_or(|y| ly.is_nan() || (y - ly).abs() <= HEIGHT_TOL_M)
            })
        })
    }

    /// Is the road segment a-b **along** the corridor: more than half of its samples (every
    /// [`FOCUS_SAMPLE_M`], both ends included) inside it, on a stretch of the race line that runs
    /// the same way. So a road that runs with the race line is relevant and one that merely
    /// crosses it or joins it from the side (the first segment out of a junction lies within the
    /// corridor too, but at an angle) is not, not even for its short piece at the junction (the
    /// race line covers that anyway).
    fn segment(&self, a: [f32; 2], b: [f32; 2], ys: Option<[f32; 2]>) -> bool {
        let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dz);
        let dir = (len > 1e-3).then(|| [dx / len, dz / len]);
        let n = ((len / FOCUS_SAMPLE_M).ceil() as usize).max(1);
        let hits = (0..=n)
            .filter(|&i| {
                let t = i as f32 / n as f32;
                self.hit([a[0] + dx * t, a[1] + dz * t], dir, ys.map(|y| y[0] + (y[1] - y[0]) * t))
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
    /// D80: what is drawn of the race line as a road of its own (`RouteStyle::Road`), for the 3D
    /// scene, which gets the race only through this focus (`gl3d::Focus3d`); `None` for
    /// `RouteStyle::Line`. Built with the focus, so it follows the drawn extent (D76).
    pub race: Option<RaceRoad>,
}

/// The race line drawn as a road (D80): the drawn extent's points and heights and its colour.
#[derive(Clone, Debug, PartialEq)]
pub struct RaceRoad {
    pub pts: Vec<[f32; 2]>,
    /// The line's own height per point (the AI's driving line: on the road, in the tunnel,
    /// across the field), which the 3D deck follows.
    pub y: Vec<f32>,
    /// A whole closed circuit: the road closes on itself (no ends).
    pub closed: bool,
    /// The race colour (`RaceCfg::circuit_color` / `sprint_color`), drawn opaque.
    pub color: Rgb,
}

impl RoadFocus {
    /// The roads along the whole of `line`.
    pub fn build(roads: &RoadLayer, line: &RaceLine) -> RoadFocus {
        Self::build_pts(roads, &line.pts, &line.half, &line.y, line.closed)
    }

    /// The roads along a polyline with a half-width vector per point (a whole line, or the part of
    /// it that is drawn while the route is still uncertain, `Poly::slice`).
    pub fn build_pts(roads: &RoadLayer, pts: &[[f32; 2]], half: &[[f32; 2]], y: &[f32], closed: bool) -> RoadFocus {
        let cor = Corridor::new(pts, half, y, closed);
        let mut f = RoadFocus::default();
        let t = cor.max_tol;
        let bb = pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
        let lb = [bb[0] - t, bb[1] - t, bb[2] + t, bb[3] + t];
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
                // Node heights of the road (0 = unknown, the 3D mesh then takes the terrain: no judgement).
                let ys = |s: usize| match (ch.y.get(s).copied().and_then(known_y), ch.y.get(s + 1).copied().and_then(known_y)) {
                    (Some(a), Some(b)) => Some([a, b]),
                    _ => None,
                };
                let (mut start, mut cur) = (0usize, cor.segment(ch.pts[0], ch.pts[1], ys(0)));
                for s in 1..nseg {
                    let r = cor.segment(ch.pts[s], ch.pts[s + 1], ys(s));
                    if r != cur {
                        flush(&mut f, start, s, cur);
                        (start, cur) = (s, r);
                    }
                }
                flush(&mut f, start, nseg, cur);
            }
        }
        f.jumps = roads.jumps.iter().map(|j| cor.hit([j[0], j[1]], None, known_y(j[2])) && cor.hit([j[3], j[4]], None, known_y(j[5]))).collect();
        f
    }
}

/// What [`RaceSel::road_focus`] cached for: the road data (rev and the `Arc`'s identity, so a
/// rebuild is noticed), the race layer and the line.
type FocusKey = (u64, usize, usize, usize, u32, Option<[u8; 3]>);

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
        // The crossing road runs across the line, not along it: not relevant, not even at the crossing.
        assert_eq!(relevance(&f, t, 2), vec![(0, 5, false)]);
        assert_eq!(relevance(&f, t, 3), vec![(0, 1, false)], "far chain: one other run");
        assert!(f.runs[RoadType::Turnaround.index() as usize].is_empty());
        assert_eq!(f.jumps, vec![true, false]);
        // Runs of a chain tile its segments and carry the right box.
        let r = f.runs[t.index() as usize].iter().find(|r| r.chain == 0 && r.relevant).unwrap();
        assert_eq!(r.bbox, [10.0, 100.0, 10.0, 600.0]);
        assert_eq!((f.segments, f.relevant_segments), (2 + 1 + 5 + 1, 2));
    }

    /// Side roads out of a junction on the route are not "along" it, however close their first
    /// segment is (the user: hidden roads still showed the first node of every joining road).
    #[test]
    fn side_roads_joining_the_route_are_not_relevant_even_at_the_junction() {
        let mut r = RoadLayer::default();
        let slot = RoadType::Road.index() as usize;
        // 0: the route's own road, nodes 20 m apart, straight through both junctions.
        r.by_type[slot].push(road(&(0..=50).map(|i| [0.0, i as f32 * 20.0]).collect::<Vec<_>>()));
        // 1: a side road at 90 degrees from the route at z = 300, nodes 20 m apart.
        r.by_type[slot].push(road(&[[0.0, 300.0], [20.0, 300.0], [40.0, 300.0], [60.0, 300.0]]));
        // 2: one at 30 degrees from the route at z = 500, 10 m nodes (so several lie inside the corridor).
        r.by_type[slot].push(road(&(0..=6).map(|i| [i as f32 * 5.0, 500.0 + i as f32 * 8.66]).collect::<Vec<_>>()));
        // 3: a frontage road running parallel 10 m beside the route.
        r.by_type[slot].push(road(&[[10.0, 600.0], [10.0, 700.0], [10.0, 800.0]]));
        // 4: a road that leaves the route at 15 degrees (a slip road merging).
        r.by_type[slot].push(road(&(0..=6).map(|i| [i as f32 * 2.59, 800.0 + i as f32 * 9.66]).collect::<Vec<_>>()));
        let f = RoadFocus::build(&r, &line(1, 0.0, false));
        let t = RoadType::Road;
        assert_eq!(relevance(&f, t, 0), vec![(0, 50, true)], "the route itself, through the junctions");
        assert_eq!(relevance(&f, t, 1), vec![(0, 3, false)], "perpendicular side road: no stub");
        assert_eq!(relevance(&f, t, 2), vec![(0, 6, false)], "30 degree side road: no stub");
        assert_eq!(relevance(&f, t, 3), vec![(0, 2, true)], "a frontage road 10 m beside the route is along it: kept");
        assert!(relevance(&f, t, 4).iter().any(|r| r.2), "a 15 degree merging road counts as along the route");
    }

    /// An overpass above the route lies in its corridor horizontally but is another road: the
    /// relevance test is 3D (the user's screenshot: short highway pieces floating over the race).
    #[test]
    fn a_road_far_above_or_below_the_route_is_not_along_it() {
        let mut l = line(1, 0.0, false);
        l.y = vec![50.0; l.pts.len()];
        let mut r = RoadLayer::default();
        let slot = RoadType::Road.index() as usize;
        let chain = |pts: &[[f32; 2]], y: f32| Chain::new(pts.to_vec(), vec![y; pts.len()]);
        // 0: on the route at its height. 1: the same line 8 m higher (an overpass that runs with the
        // route for a stretch, so heading and distance alone would keep it). 2: a shallow crossing
        // 8 m above (10 degrees). 3: 5 m higher (a ramp / a hill: within the tolerance). 4: 8 m
        // lower (an underpass). 5: height unknown (0), judged by the corridor alone.
        let along: Vec<[f32; 2]> = (0..=10).map(|i| [3.0, 100.0 + i as f32 * 20.0]).collect();
        r.by_type[slot].push(chain(&along, 50.0));
        r.by_type[slot].push(chain(&along, 58.0));
        r.by_type[slot].push(chain(&(0..=10).map(|i| [-20.0 + i as f32 * 4.0, 400.0 + i as f32 * 22.7]).collect::<Vec<_>>(), 58.0));
        r.by_type[slot].push(chain(&along, 55.0));
        r.by_type[slot].push(chain(&along, 42.0));
        r.by_type[slot].push(chain(&along, 0.0));
        let f = RoadFocus::build(&r, &l);
        let t = RoadType::Road;
        assert_eq!(relevance(&f, t, 0), vec![(0, 10, true)], "same height");
        assert_eq!(relevance(&f, t, 1), vec![(0, 10, false)], "8 m above");
        assert_eq!(relevance(&f, t, 2), vec![(0, 10, false)], "shallow crossing 8 m above");
        assert_eq!(relevance(&f, t, 3), vec![(0, 10, true)], "5 m above is within the tolerance");
        assert_eq!(relevance(&f, t, 4), vec![(0, 10, false)], "8 m below");
        assert_eq!(relevance(&f, t, 5), vec![(0, 10, true)], "no height: corridor only");
        // The overpass crossing at 10 degrees at the route's own height would count as along it.
        let mut r2 = RoadLayer::default();
        r2.by_type[slot].push(chain(&(0..=10).map(|i| [-20.0 + i as f32 * 4.0, 400.0 + i as f32 * 22.7]).collect::<Vec<_>>(), 50.0));
        assert!(relevance(&RoadFocus::build(&r2, &l), t, 0).iter().any(|r| r.2));
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

    // ── candidate routes (D76) ───────────────────────────────────────────────────────────────

    /// A route through `corners`, resampled every 5 m, half-width 6 m. A circuit gets its first
    /// point as its last.
    fn route(id: u32, corners: &[[f32; 2]], circuit: bool) -> RaceLine {
        let mut pts: Vec<[f32; 2]> = vec![corners[0]];
        for w in corners.windows(2) {
            let (a, b) = (w[0], w[1]);
            let n = ((b[0] - a[0]).hypot(b[1] - a[1]) / 5.0).ceil().max(1.0) as usize;
            for i in 1..=n {
                let t = i as f32 / n as f32;
                pts.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            }
        }
        let n = pts.len();
        let len: f32 = pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        let bb = pts.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
        RaceLine {
            route: id,
            circuit,
            start: [pts[0][0], 0.0, pts[0][1]],
            finish: [pts[n - 1][0], 0.0, pts[n - 1][1]],
            y: vec![0.0; n],
            half: vec![[6.0, 0.0]; n],
            length_m: f64::from(len),
            closed: circuit,
            n_sections: 1,
            bbox: bb,
            pts,
        }
    }

    /// The user's example: routes 1 (A B C D ...) and 2 (A B C H ...) share the first 400 m
    /// (index 0 and 1), then 1 goes on straight and 2 veers off to the east.
    fn fork_layer() -> RaceLayer {
        RaceLayer::new(vec![
            route(1, &[[0.0, 0.0], [0.0, 400.0], [0.0, 900.0]], false),
            route(2, &[[0.0, 0.0], [0.0, 400.0], [250.0, 800.0]], false),
        ])
    }

    /// Drive along route `li` of `layer` from arc `from` to `to`, every 2 m, `lateral` m to the
    /// right of the line; `f(sel, s)` after each update.
    fn drive_line(sel: &mut RaceSel, layer: &RaceLayer, li: usize, from: f32, to: f32, lateral: f32, mut f: impl FnMut(&RaceSel, f32)) {
        let poly = Poly::new(layer, li);
        let c = cfg(RaceLineMode::Current);
        let mut s = from;
        while s <= to {
            let (p, q) = (poly.at(s).0, poly.at(s + 1.0).0);
            let (dx, dz) = (q[0] - p[0], q[1] - p[1]);
            let l = dx.hypot(dz).max(1e-6);
            // Right of the heading (clockwise from +z): (dz, -dx).
            let car = (p[0] + dz / l * lateral, p[1] - dx / l * lateral);
            sel.update(layer, &c, car, dx.atan2(dz), true);
            f(sel, s);
            s += 2.0;
        }
    }

    #[test]
    fn a_fork_shows_only_the_shared_part_until_the_route_is_certain() {
        let layer = fork_layer();
        for (taken, other) in [(0usize, 1usize), (1, 0)] {
            let mut sel = RaceSel::default();
            // Before the fork: both routes are candidates, only the shared 400 m is drawn.
            drive_line(&mut sel, &layer, taken, 20.0, 300.0, 1.0, |sel, _| {
                assert_eq!(sel.candidates(), 2);
                let line = sel.picked()[0];
                let sp = sel.span(line).expect("a partial span while uncertain");
                assert!(sp.start, "the shared part begins at the common start");
                assert!(sp.s0 <= 0.5 && (sp.s1 - 400.0).abs() <= 6.0, "{sp:?}: up to the fork at 400 m");
            });
            // Past the fork: the other branch falls out; certain once only one is left.
            let mut certain_at = None;
            drive_line(&mut sel, &layer, taken, 300.0, 600.0, 1.0, |sel, s| {
                if certain_at.is_none() && sel.candidates() == 1 {
                    certain_at = Some(s);
                    assert!(sel.span(sel.picked()[0]).is_none(), "whole route with both marks");
                }
            });
            let at = certain_at.expect("certain after the fork");
            assert!(at > 400.0 && at < 460.0, "certain {} m after the fork", at - 400.0);
            assert_eq!(layer.lines[sel.picked()[0]].route as usize, taken + 1, "the route taken ({other} is gone)");
        }
    }

    /// D80: the focus carries the race road of what is drawn (the shared part while uncertain,
    /// the whole route once certain) in the route's colour, and none for `RouteStyle::Line`.
    #[test]
    fn the_focus_carries_the_race_road_of_the_drawn_extent() {
        let layer = fork_layer();
        let layers = MapLayers { rev: 1, roads: Arc::new(RoadLayer::default()), races: Arc::new(RaceLayer::new(layer.lines.clone())), ..Default::default() };
        let mut sel = RaceSel::default();
        drive_line(&mut sel, &layers.races, 0, 20.0, 200.0, 0.0, |_, _| {});
        let f = sel.road_focus(&layers).expect("focus");
        let r = f.race.as_ref().expect("a race road");
        let len: f32 = r.pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum();
        assert!((len - 400.0).abs() < 8.0 && !r.closed && r.y.len() == r.pts.len(), "the shared 400 m: {len}");
        assert_eq!(r.color, RaceCfg::default().sprint_color);
        // Certain on route 1: the whole line.
        drive_line(&mut sel, &layers.races, 0, 202.0, 520.0, 0.0, |_, _| {});
        let f = sel.road_focus(&layers).expect("focus");
        assert_eq!(f.race.as_ref().map(|r| r.pts.len()), Some(layers.races.lines[0].pts.len()));
        // The thin line instead: no race road.
        let line_cfg = RaceCfg { mode: RaceLineMode::Current, route: RouteStyle::Line, ..RaceCfg::default() };
        sel.update(&layers.races, &line_cfg, Poly::new(&layers.races, 0).at(530.0).0.into(), 0.0, true);
        assert!(sel.road_focus(&layers).expect("focus").race.is_none());
    }

    #[test]
    fn while_uncertain_the_focus_corridor_is_the_shared_part_only() {
        let layer = fork_layer();
        let layers = MapLayers { rev: 1, roads: Arc::new(RoadLayer::default()), races: Arc::new(RaceLayer::new(layer.lines.clone())), ..Default::default() };
        let mut roads = RoadLayer::default();
        // One road along the shared part, one along route 1 past the fork.
        roads.by_type[RoadType::Road.index() as usize].push(road(&[[0.0, 100.0], [0.0, 300.0]]));
        roads.by_type[RoadType::Road.index() as usize].push(road(&[[0.0, 600.0], [0.0, 800.0]]));
        let layers = MapLayers { roads: Arc::new(roads), ..layers };
        let mut sel = RaceSel::default();
        drive_line(&mut sel, &layers.races, 0, 20.0, 200.0, 0.0, |_, _| {});
        let f = sel.road_focus(&layers).expect("focus");
        let slot = RoadType::Road.index() as usize;
        assert!(f.runs[slot].iter().any(|r| r.chain == 0 && r.relevant));
        assert!(f.runs[slot].iter().all(|r| r.chain != 1 || !r.relevant), "beyond the fork is not relevant yet");
        // Still uncertain further along the shared part: cached, not rebuilt.
        drive_line(&mut sel, &layers.races, 0, 202.0, 300.0, 0.0, |_, _| {});
        assert!(Arc::ptr_eq(&f, &sel.road_focus(&layers).unwrap()));
        // Past the fork onto route 1: the whole route is the corridor now.
        drive_line(&mut sel, &layers.races, 0, 302.0, 520.0, 0.0, |_, _| {});
        let f2 = sel.road_focus(&layers).expect("focus");
        assert!(!Arc::ptr_eq(&f, &f2));
        assert!(f2.runs[slot].iter().any(|r| r.chain == 1 && r.relevant));
    }

    #[test]
    fn a_route_the_car_leaves_is_dropped_but_never_the_last_one() {
        // Two parallel routes 40 m apart, the car is on the first, then wanders off both.
        let layer = RaceLayer::new(vec![route(1, &[[0.0, 0.0], [0.0, 600.0]], false), route(2, &[[40.0, 0.0], [40.0, 600.0]], false)]);
        let mut sel = RaceSel::default();
        let c = cfg(RaceLineMode::Current);
        // Starts on the left of route 1 only (route 2 is 40 m away: not a candidate at all).
        sel.update(&layer, &c, (1.0, 100.0), 0.0, true);
        assert_eq!((sel.candidates(), sel.picked()), (1, &[0][..]));
        // Off both for a long way (a shortcut across the verge): the last pick is kept.
        for z in (110..250).step_by(2) {
            sel.update(&layer, &c, (20.0, z as f32), 0.0, true);
        }
        assert_eq!(sel.picked(), &[0]);
        // Route 1 was certain: it is locked, so a short visit to route 2 keeps it.
        sel.update(&layer, &c, (41.0, 252.0), 0.0, true);
        assert_eq!(sel.picked(), &[0]);
        // Off route 1 for more than LOCK_DROP_M: unlocked, but still the last pick ...
        for z in (254..320).step_by(2) {
            sel.update(&layer, &c, (20.0, z as f32), 0.0, true);
        }
        assert_eq!(sel.picked(), &[0]);
        // ... until the car is on route 2: it replaces the set.
        sel.update(&layer, &c, (41.0, 322.0), 0.0, true);
        assert_eq!(sel.picked(), &[1]);
    }

    #[test]
    fn two_routes_that_are_one_road_to_the_finish_count_as_certain() {
        // Same road, the second one's racing line 1.5 m to the side and its finish 12 m earlier.
        let layer = RaceLayer::new(vec![route(1, &[[0.0, 0.0], [0.0, 500.0]], false), route(2, &[[1.5, 0.0], [1.5, 488.0]], false)]);
        let mut sel = RaceSel::default();
        drive_line(&mut sel, &layer, 0, 10.0, 200.0, 0.0, |sel, _| {
            // Certain at once: locked onto the one that stands for both.
            assert_eq!(sel.candidates(), 1);
            assert_eq!(sel.span(sel.picked()[0]), None, "drawn whole, finish included");
            assert_eq!(sel.picked(), &[0], "the lowest line index stands for both");
        });
        // A route that ends well before the other is not the same: shown up to its end only.
        let layer = RaceLayer::new(vec![route(1, &[[0.0, 0.0], [0.0, 500.0]], false), route(2, &[[0.0, 0.0], [0.0, 300.0]], false)]);
        let mut sel = RaceSel::default();
        drive_line(&mut sel, &layer, 0, 10.0, 100.0, 0.0, |sel, _| {
            let sp = sel.span(sel.picked()[0]).expect("partial");
            assert!((sp.s1 - 300.0).abs() < 6.0 && sp.start, "{sp:?}");
        });
    }

    #[test]
    fn circuits_wrap_and_are_tracked_across_the_start() {
        let sq = [[0.0, 0.0], [200.0, 0.0], [200.0, 200.0], [0.0, 200.0], [0.0, 0.0]];
        let layer = RaceLayer::new(vec![route(1, &sq, true)]);
        let mut sel = RaceSel::default();
        // Start mid-way, drive one and a half laps: the single candidate never gets lost.
        let total = Poly::new(&layer, 0).total;
        drive_line(&mut sel, &layer, 0, 300.0, 300.0 + total * 1.5, 0.0, |sel, _| {
            assert_eq!((sel.candidates(), sel.picked()), (1, &[0][..]));
            assert_eq!(sel.span(0), None);
        });
        // Two circuits that share the first half of the lap and then split: partial span that
        // runs across the start (wraps) while the car is on the shared stretch just before it.
        let a = route(1, &[[0.0, 0.0], [200.0, 0.0], [200.0, 200.0], [0.0, 200.0], [0.0, 0.0]], true);
        let b = route(2, &[[0.0, 0.0], [200.0, 0.0], [200.0, -200.0], [0.0, -200.0], [0.0, 0.0]], true);
        let layer = RaceLayer::new(vec![a, b]);
        let mut sel = RaceSel::default();
        drive_line(&mut sel, &layer, 0, 20.0, 120.0, 0.0, |sel, _| {
            assert_eq!(sel.candidates(), 2);
            let sp = sel.span(sel.picked()[0]).expect("partial");
            assert!(sp.s1 <= 205.0 && sp.s1 >= 195.0, "{sp:?}: up to the corner where they split");
        });
        // Past the split, on route 1: certain, the whole closed circuit.
        drive_line(&mut sel, &layer, 0, 122.0, 400.0, 0.0, |_, _| {});
        assert_eq!((sel.candidates(), sel.span(0)), (1, None));
    }

    /// The user's lap-2 bug: a circuit whose start and first stretch a sprint shares. Lap 1 makes
    /// the circuit certain; crossing the line into lap 2 (back on the shared stretch) must keep it,
    /// with the sprint not added again; the race ending resets it. Also with a circuit line that
    /// does not close exactly (the car's progress jumps from its end to its start).
    #[test]
    fn a_certain_circuit_stays_locked_through_the_next_lap() {
        let sq = [[0.0, 0.0], [200.0, 0.0], [200.0, 200.0], [0.0, 200.0], [0.0, 0.0]];
        let open_circuit = {
            let mut l = route(1, &sq, true);
            let n = l.pts.len();
            l.pts.truncate(n - 2); // ends 10 m short of its start
            l.y.truncate(n - 2);
            l.half.truncate(n - 2);
            l.closed = false;
            l
        };
        for circuit in [route(1, &sq, true), open_circuit] {
            let sprint = route(2, &[[0.0, 0.0], [200.0, 0.0], [600.0, 0.0]], false);
            let layer = RaceLayer::new(vec![circuit, sprint]);
            let total = Poly::new(&layer, 0).total;
            let mut sel = RaceSel::default();
            // Lap 1, on the shared stretch: both routes.
            drive_line(&mut sel, &layer, 0, 4.0, 150.0, 0.0, |sel, _| assert_eq!(sel.candidates(), 2));
            // Round the first corner: the circuit is certain.
            drive_line(&mut sel, &layer, 0, 152.0, total - 1.0, 0.0, |_, _| {});
            assert_eq!((sel.candidates(), sel.picked(), sel.span(0)), (1, &[0][..], None));
            // Lap 2: across the line and along the stretch the sprint shares.
            drive_line(&mut sel, &layer, 0, 1.0, 190.0, 0.0, |sel, s| {
                assert_eq!((sel.candidates(), sel.picked(), sel.span(0)), (1, &[0][..], None), "lap 2 at {s} m");
            });
            // The race ends: forgotten; the next race starts afresh with both.
            let c = cfg(RaceLineMode::Current);
            sel.update(&layer, &c, (100.0, 0.0), std::f32::consts::FRAC_PI_2, false);
            assert_eq!((sel.candidates(), sel.picked().len()), (0, 0));
            sel.update(&layer, &c, (20.0, 0.0), std::f32::consts::FRAC_PI_2, true);
            assert_eq!(sel.candidates(), 2);
        }
    }

    #[test]
    fn candidates_are_forgotten_when_the_race_ends() {
        let layer = fork_layer();
        let mut sel = RaceSel::default();
        let c = cfg(RaceLineMode::Current);
        drive_line(&mut sel, &layer, 0, 20.0, 100.0, 0.0, |_, _| {});
        assert_eq!(sel.candidates(), 2);
        sel.update(&layer, &c, (0.0, 110.0), 0.0, false);
        assert_eq!((sel.candidates(), sel.picked().len(), sel.focus_line()), (0, 0, None));
        // The next race starts afresh: both routes again.
        sel.update(&layer, &c, (0.0, 20.0), 0.0, true);
        assert_eq!(sel.candidates(), 2);
    }

    fn real_layers() -> Option<MapLayers> {
        use crate::gamedata::roadtypes::RoadTypes;
        let media = crate::gamedata::install::find_media(None)?;
        let g = crate::maprender::data::GameData::load(&media).expect("game data");
        let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), std::path::Path::new("/nonexistent"), &g.nav);
        Some(g.layers(&cur, 1))
    }

    /// Real data: drive every one of the 170 routes from its start to its finish through the
    /// whole layer and report how soon the route is certain, and whether it ends on the right
    /// line. `cargo test --release real_install_drive_every_route -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_install_drive_every_route() {
        let Some(layers) = real_layers() else { return };
        // Driving the line exactly, then wobbling +-3.5 m about it (a human driver).
        for wobble in [0.0f32, 3.5] {
            eprintln!("--- wobble {wobble} m");
            drive_every_route(&layers, wobble);
        }
    }

    fn drive_every_route(layers: &MapLayers, wobble: f32) {
        let layer = &*layers.races;
        let c = cfg(RaceLineMode::Current);
        let (mut updates, mut time) = (0usize, std::time::Duration::ZERO);
        let mut worst = std::time::Duration::ZERO;
        let mut rows = Vec::new();
        // Metres driven past the fork (the end of the shared part) until a candidate was dropped.
        let mut after_fork: Vec<f32> = Vec::new();
        for a in 0..layer.lines.len() {
            let poly = Poly::new(layer, a);
            let mut sel = RaceSel::default();
            let (mut first_n, mut certain_at, mut max_n) = (0usize, None, 0usize);
            let mut s = 1.0f32;
            let end = if layer.lines[a].closed { poly.total } else { poly.total - 1.0 };
            let mut wrong = false;
            let mut log: Vec<(f32, Vec<u32>)> = Vec::new();
            while s < end {
                let (p, q) = (poly.at(s).0, poly.at(s + 1.0).0);
                let (dx, dz) = (q[0] - p[0], q[1] - p[1]);
                let before = sel.shown.as_ref().map(|sh| (sh.line, sh.span, sh.key.0.len()));
                let l = dx.hypot(dz).max(1e-6);
                let off = wobble * (s / 23.0).sin();
                let car = (p[0] + dz / l * off, p[1] - dx / l * off);
                let t0 = std::time::Instant::now();
                sel.update(layer, &c, car, dx.atan2(dz), true);
                let dt = t0.elapsed();
                if let Some((bl, Some(sp), bn)) = before {
                    if sel.cands.len() < bn && !sel.cands.is_empty() {
                        let fork = Poly::new(layer, bl).at(sp.s1).0;
                        let (d, arc) = poly.nearest_in(fork, s - 1500.0, s + 10.0);
                        if d < 10.0 {
                            after_fork.push(s - arc);
                        }
                    }
                }
                updates += 1;
                time += dt;
                worst = worst.max(dt);
                if first_n == 0 {
                    first_n = sel.candidates();
                }
                let set: Vec<u32> = sel.cands.iter().map(|c| layer.lines[c.line].route).collect();
                if log.last().is_none_or(|l| l.1 != set) {
                    log.push((s, set));
                }
                max_n = max_n.max(sel.candidates());
                if certain_at.is_none() && sel.candidates() > 0 && sel.span(sel.picked()[0]).is_none() {
                    certain_at = Some(s);
                    // Right line, or an equal one (same geometry: a duplicate).
                    let pl = sel.picked()[0];
                    wrong = pl != a && !(layer.lines[pl].pts.len().abs_diff(layer.lines[a].pts.len()) < 8);
                }
                s += 2.0;
            }
            let fin = sel.picked().first().copied();
            rows.push((a, layer.lines[a].route, poly.total, first_n, max_n, certain_at, fin, wrong, sel.candidates(), log));
        }
        let n = rows.len();
        let certain = rows.iter().filter(|r| r.5.is_some()).count();
        let at_start = rows.iter().filter(|r| r.5.is_some_and(|c| c < 30.0)).count();
        eprintln!("{n} routes driven; {certain} certain at some point; {at_start} certain within the first 30 m; avg update {:.1} us, worst {:.0} us over {updates} updates", time.as_secs_f64() * 1e6 / updates as f64, worst.as_secs_f64() * 1e6);
        let mut by_c: Vec<_> = rows.iter().filter_map(|r| r.5.map(|c| (c, r))).collect();
        by_c.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (c, r) in by_c.iter().rev().take(14) {
            eprintln!("  route {:>5} ({:.1} km): {} candidates at start, certain after {c:.0} m (of {:.0}), final line {:?}{}\n      candidate history: {:?}", r.1, r.2 / 1000.0, r.3, r.2, r.6.map(|l| layer.lines[l].route), if r.7 { "  WRONG" } else { "" }, r.9.iter().take(8).map(|(s, v)| (*s as i32, v.clone())).collect::<Vec<_>>());
        }
        let never: Vec<_> = rows.iter().filter(|r| r.5.is_none()).map(|r| (r.1, r.3, r.8)).collect();
        eprintln!("never certain ({}): {never:?}", never.len());
        let wrong: Vec<_> = rows.iter().filter(|r| r.7).map(|r| (r.1, r.6.map(|l| layer.lines[l].route))).collect();
        eprintln!("certain on a different, non-identical line ({}): {wrong:?}", wrong.len());
        after_fork.sort_by(f32::total_cmp);
        if !after_fork.is_empty() {
            let q = |f: f32| after_fork[((after_fork.len() - 1) as f32 * f) as usize];
            eprintln!("{} drops after a fork: metres past the fork: min {:.0}, median {:.0}, p90 {:.0}, max {:.0}", after_fork.len(), q(0.0), q(0.5), q(0.9), q(1.0));
        }
        let multi = rows.iter().filter(|r| r.3 > 1).count();
        eprintln!("{multi} routes started with more than one candidate; max candidates seen: {}", rows.iter().map(|r| r.4).max().unwrap_or(0));
    }
}
