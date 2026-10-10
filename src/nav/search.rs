//! The route search: A* over node indices from a snapped start to a snapped destination.
//!
//! Start and destination are **edge points, not nodes** ([`Snap`]): the search is seeded with both
//! endpoints of the start edge at the cost of the partial edge, and ends at either endpoint of the
//! destination edge plus its partial edge. The polyline then starts / ends at the projection
//! points. Exact for the 20 m edges and still right for the long added links.
//!
//! Heuristic: straight-line 2D distance times [`CostModel::heuristic`]'s factor; admissible and
//! consistent (see `cost.rs`). Dijkstra is the same code with the heuristic off (the tests prove
//! both give the same cost). Measured: A* on the real island settles 12-17 k of 38 k nodes on a
//! 12 km route, a few ms in release.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::cfg::RoutePrefs;
use super::cost::CostModel;
use super::graph::RouteGraph;
use super::snap::Snap;

/// Which end of the trip a failed snap was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endpoint {
    Car,
    Destination,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RouteError {
    /// No road data at all (no game install, or the map data is still loading).
    EmptyGraph,
    /// No allowed road within [`SNAP_R_MAX_M`](super::snap::SNAP_R_MAX_M) of that end.
    NoRoadNear(Endpoint),
    /// Both ends are on the network but the filters leave no connection.
    Unreachable,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::EmptyGraph => f.write_str("no road data"),
            RouteError::NoRoadNear(Endpoint::Car) => f.write_str("no road near the car"),
            RouteError::NoRoadNear(Endpoint::Destination) => f.write_str("no road near the destination"),
            RouteError::Unreachable => f.write_str("no route with these road types"),
        }
    }
}

impl std::error::Error for RouteError {}

/// A found route, ready to draw and to follow.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    /// World x, z of the polyline: starts at the start's projection point, ends at the
    /// destination's. Consecutive points are never identical.
    pub pts: Vec<[f32; 2]>,
    /// Height per point (0.0 = unknown, the `mesh3d::known_y` rule).
    pub y: Vec<f32>,
    /// `RoadType::index` per segment (`pts.len() - 1` entries; 7 = a jump).
    pub seg_kind: Vec<u8>,
    /// Length of each segment (m, 3D where heights are known).
    pub seg_len: Vec<f32>,
    /// Assumed travel time of each segment (s).
    pub seg_time_s: Vec<f32>,
    /// Sum of `seg_len`.
    pub dist_m: f32,
    /// Sum of `seg_time_s`: the ETA at the assumed speeds (approximate by design, D84).
    pub eta_s: f32,
    /// The objective the search minimised (depends on the slider; for tests and diagnostics).
    pub cost: f32,
}

/// No predecessor.
const NONE: u32 = u32::MAX;

impl RouteGraph {
    /// Snap `car` (x, z, optional height) and `dest` (x, z) with `prefs.filters` and search.
    /// This is what the navigator's worker calls. A destination further than
    /// [`SNAP_R_MAX_M`](super::snap::SNAP_R_MAX_M) from any allowed road is
    /// `NoRoadNear(Destination)`; the caller keeps the clicked coordinates for the pin.
    pub fn plan(&self, car: (f32, f32, Option<f32>), dest: (f32, f32), prefs: &RoutePrefs) -> Result<Route, RouteError> {
        if self.is_empty() {
            return Err(RouteError::EmptyGraph);
        }
        let from = self.snap(car.0, car.1, car.2, &prefs.filters).ok_or(RouteError::NoRoadNear(Endpoint::Car))?;
        let to = self.snap(dest.0, dest.1, None, &prefs.filters).ok_or(RouteError::NoRoadNear(Endpoint::Destination))?;
        self.route(&from, &to, prefs)
    }

    /// The cheapest route between two edge points under `prefs` (A*).
    pub fn route(&self, from: &Snap, to: &Snap, prefs: &RoutePrefs) -> Result<Route, RouteError> {
        self.search(from, to, &CostModel::new(prefs), true)
    }

    /// The search itself; `astar = false` is Dijkstra (h = 0), for the tests.
    pub(super) fn search(&self, from: &Snap, to: &Snap, cm: &CostModel, astar: bool) -> Result<Route, RouteError> {
        if self.is_empty() {
            return Err(RouteError::EmptyGraph);
        }
        let n = self.pos.len();
        // ponytail: two n-sized vectors per query (~0.3 MB, a memset next to ~10 k settled nodes);
        // a reusable scratch with a generation stamp is the next step if profiles ever show it.
        let mut g = vec![f32::INFINITY; n];
        let mut prev = vec![NONE; n]; // the arc that reached the node; NONE = a seed
        let goal = [to.pos[0], to.pos[1]];
        let h = |u: u32| -> f32 {
            if !astar {
                return 0.0;
            }
            let p = self.pos[u as usize];
            cm.heuristic((p[0] - goal[0]).hypot(p[1] - goal[1]))
        };
        // min-heap on (f, g, node); f, g >= 0 so their bit patterns order like the floats
        let mut heap: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::new();

        let (fe, te) = (&self.edges[from.edge as usize], &self.edges[to.edge as usize]);
        let (f_per_m, t_per_m) = (cm.per_m(fe.kind, fe.curv, fe.wind), cm.per_m(te.kind, te.curv, te.wind));
        for (node, cost) in [(fe.a, fe.len * from.t * f_per_m), (fe.b, fe.len * (1.0 - from.t) * f_per_m)] {
            if cost < g[node as usize] {
                g[node as usize] = cost;
                heap.push(Reverse(((cost + h(node)).to_bits(), cost.to_bits(), node)));
            }
        }
        // The destination edge's two ends and the cost of the partial edge from each to the point.
        let tails = [(te.a, te.len * to.t * t_per_m), (te.b, te.len * (1.0 - to.t) * t_per_m)];

        let mut best = f32::INFINITY;
        let mut best_end = NONE;
        while let Some(Reverse((fb, gb, u))) = heap.pop() {
            if f32::from_bits(fb) >= best {
                break; // h is a lower bound of (rest + tail): nothing left can beat `best`
            }
            let gu = f32::from_bits(gb);
            if gu > g[u as usize] {
                continue; // stale entry
            }
            for (end, tail) in tails {
                if end == u && gu + tail < best {
                    best = gu + tail;
                    best_end = u;
                }
            }
            for &arc in self.arcs_of(u) {
                let (e, _, v) = self.arc_ends(arc);
                if !cm.allows(e.kind) {
                    continue;
                }
                let ng = gu + cm.cost(e.kind, e.curv, e.wind, e.len);
                if ng < g[v as usize] {
                    g[v as usize] = ng;
                    prev[v as usize] = arc;
                    heap.push(Reverse(((ng + h(v)).to_bits(), ng.to_bits(), v)));
                }
            }
        }

        // Both points on one edge: walking along it may beat any detour (e.g. a long added link).
        let direct = (from.edge == to.edge).then(|| fe.len * (to.t - from.t).abs() * f_per_m);
        if let Some(d) = direct {
            if d <= best {
                let mut b = Builder::new(from.pos);
                b.push(to.pos, fe.kind, fe.len * (to.t - from.t).abs());
                return Ok(b.finish(d));
            }
        }
        if best_end == NONE {
            return Err(RouteError::Unreachable);
        }

        // Walk the predecessors back to the seed, then lay the polyline forwards.
        let mut arcs_rev: Vec<u32> = Vec::new();
        let mut u = best_end;
        while prev[u as usize] != NONE {
            let arc = prev[u as usize];
            arcs_rev.push(arc);
            u = self.arc_ends(arc).1;
        }
        let seed = u;
        let mut b = Builder::new(from.pos);
        let seed_frac = if seed == fe.a { from.t } else { 1.0 - from.t };
        b.push(self.pos[seed as usize], fe.kind, fe.len * seed_frac);
        for &arc in arcs_rev.iter().rev() {
            let (e, _, v) = self.arc_ends(arc);
            b.push(self.pos[v as usize], e.kind, e.len);
        }
        let end_frac = if best_end == te.a { to.t } else { 1.0 - to.t };
        b.push(to.pos, te.kind, te.len * end_frac);
        Ok(b.finish(best))
    }
}

/// Lays a polyline down point by point, dropping zero-length pieces (a snap exactly on a node).
struct Builder {
    r: Route,
}

impl Builder {
    fn new(start: [f32; 3]) -> Builder {
        let r = Route { pts: vec![[start[0], start[1]]], y: vec![start[2]], seg_kind: vec![], seg_len: vec![], seg_time_s: vec![], dist_m: 0.0, eta_s: 0.0, cost: 0.0 };
        Builder { r }
    }

    fn push(&mut self, p: [f32; 3], kind: u8, len: f32) {
        let last = self.r.pts[self.r.pts.len() - 1];
        if (p[0] - last[0]).hypot(p[1] - last[1]) < 1e-3 {
            // a snap exactly on a node (or two points on top of each other): no new point, but keep the better height
            let i = self.r.y.len() - 1;
            if self.r.y[i] == 0.0 {
                self.r.y[i] = p[2];
            }
            return;
        }
        let time = CostModel::eta_s(kind, len);
        self.r.pts.push([p[0], p[1]]);
        self.r.y.push(p[2]);
        self.r.seg_kind.push(kind);
        self.r.seg_len.push(len);
        self.r.seg_time_s.push(time);
        self.r.dist_m += len;
        self.r.eta_s += time;
    }

    fn finish(mut self, cost: f32) -> Route {
        self.r.cost = cost;
        self.r
    }
}
