//! The routable graph: nodes (nav node ids and user points), edges (the nav polylines' consecutive
//! pairs plus the road-type file's added links) with a type, a length and a winding, and a grid
//! to snap a world position to an edge.
//!
//! Built by id, not by coordinate: polylines meet at shared node ids (junctions). Node positions
//! come from `maprender::data::node_positions`, the same function the drawn roads use, so what
//! is drawn and what is routable (moved nodes, user points) cannot drift apart.

use std::collections::{HashMap, HashSet};

use crate::gamedata::nav::Nav;
use crate::gamedata::roadtypes::{EdgeKey, RoadType, RoadTypes};
use crate::maprender::mesh3d::known_y;

/// Snap grid cell size (m): a few 20 m edges per cell, a 120 m search touches ~4 x 4 cells.
pub const GRID_CELL_M: f32 = 128.0;

/// One undirected road segment. `a -> b` is its "forward" direction (arc `edge << 1`), `b -> a`
/// the backward one (`edge << 1 | 1`); only a jump has just the forward arc, with `a` the take-off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Dense node indices.
    pub a: u32,
    pub b: u32,
    /// Length in metres: 3D when both heights are known, else 2D.
    pub len: f32,
    /// `RoadType::index`, 0 = no type.
    pub kind: u8,
    /// The road's own winding at this edge, rad per metre (see [`RouteGraph::build`]).
    pub curv: f32,
}

impl Edge {
    pub fn is_jump(&self) -> bool {
        self.kind == RoadType::Jump.index()
    }
}

/// Buckets of edge indices by 128 m cell. Turnaround and jump edges are not in it.
#[derive(Clone, Debug, Default)]
pub(super) struct SnapGrid {
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl SnapGrid {
    fn key(v: f32) -> i32 {
        (v / GRID_CELL_M).floor() as i32
    }

    /// Put edge `e` into every cell its bounding box touches (a superset of the cells it crosses;
    /// the query filters by true distance).
    fn insert(&mut self, e: u32, p: [f32; 2], q: [f32; 2]) {
        for cx in Self::key(p[0].min(q[0]))..=Self::key(p[0].max(q[0])) {
            for cz in Self::key(p[1].min(q[1]))..=Self::key(p[1].max(q[1])) {
                self.cells.entry((cx, cz)).or_default().push(e);
            }
        }
    }

    /// Edges in the cells within `r` metres of (x, z). An edge may be reported more than once.
    pub(super) fn near(&self, x: f32, z: f32, r: f32, mut f: impl FnMut(u32)) {
        for cx in Self::key(x - r)..=Self::key(x + r) {
            for cz in Self::key(z - r)..=Self::key(z + r) {
                if let Some(v) = self.cells.get(&(cx, cz)) {
                    v.iter().for_each(|&e| f(e));
                }
            }
        }
    }
}

/// The routing graph. `Default` is the empty graph (`MapLayers` is `Default`); every query on it
/// fails with `RouteError::EmptyGraph` / `NoRoadNear`.
#[derive(Clone, Default)]
pub struct RouteGraph {
    /// x, z, y per dense node index (y 0.0 = unknown, the `mesh3d::known_y` rule).
    pub(super) pos: Vec<[f32; 3]>,
    /// Stable nav / user-point id per node, ascending (so an id is found by binary search). Only
    /// the test accessors ([`RouteGraph::node_id`], [`RouteGraph::node_index`]) read it today; it
    /// is kept because a route has to be mappable back to nav ids (editor links, debugging).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) ids: Vec<u32>,
    pub(super) edges: Vec<Edge>,
    /// CSR adjacency: the outgoing arcs of node `n` are `arcs[arc_start[n]..arc_start[n + 1]]`;
    /// an arc is `edge << 1 | dir`.
    pub(super) arc_start: Vec<u32>,
    pub(super) arcs: Vec<u32>,
    pub(super) grid: SnapGrid,
}

impl std::fmt::Debug for RouteGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RouteGraph({} nodes, {} edges)", self.pos.len(), self.edges.len())
    }
}

/// Heading change (rad, 0..pi) at `b` of the 2D path `a -> b -> c`; 0 for a zero-length leg.
fn turn(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let (u, v) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
    if u[0].hypot(u[1]) < 1e-3 || v[0].hypot(v[1]) < 1e-3 {
        return 0.0;
    }
    (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1]).abs()
}

/// 3D length when both heights are known, else the 2D one.
fn edge_len(p: [f32; 3], q: [f32; 3]) -> f32 {
    let d2 = (q[0] - p[0]).hypot(q[1] - p[1]);
    match (known_y(p[2]), known_y(q[2])) {
        (Some(y0), Some(y1)) => d2.hypot(y1 - y0),
        _ => d2,
    }
}

impl RouteGraph {
    /// Build from the nav roads and the road-type data; `pos` = `maprender::data::node_positions`
    /// (id -> x, z, y). One pass, ~10 ms on the island. Rules (the same as the drawn roads,
    /// `build_roads`):
    ///
    /// 1. Game edges: each polyline's consecutive pairs, skipping `rt.removed`; a pair seen in
    ///    two polylines is one edge. Type = `rt.types` (missing = 0 = unset).
    /// 2. `rt.added` links between any two known points, type from the link (`None` = unset).
    /// 3. Directedness: every edge has two arcs, except a Jump, which has exactly one, take-off ->
    ///    landing (take-off = `rt.jump_from[edge]` if it is an endpoint, else the first node of
    ///    the polyline pair / the link's `a` - the rule of the drawn jump line). Arcs carry no
    ///    per-arc data, so a future decoded one-way flag is "drop one arc", not a format change.
    /// 4. `curv` = `(turn(i-1,i,i+1) + turn(i,i+1,i+2)) / 2 / len` for the edge at polyline index
    ///    `i`, turns taken only inside the same polyline (polyline ends and added links get 0).
    ///    *Why:* it is the road's own winding, independent of which route uses it, so the cost
    ///    needs no per-route state (an edge-based search on directed edges would be needed
    ///    otherwise).
    /// 5. The snap grid holds every edge except turnarounds and jumps.
    pub fn build(nav: &Nav, rt: &RoadTypes, pos: &HashMap<u32, [f32; 3]>) -> RouteGraph {
        let mut ids: Vec<u32> = pos.keys().copied().collect();
        ids.sort_unstable();
        let p: Vec<[f32; 3]> = ids.iter().map(|id| pos[id]).collect();
        let index = |id: u32| ids.binary_search(&id).ok().map(|i| i as u32);

        let removed: HashSet<EdgeKey> = rt.removed.iter().copied().collect();
        let jump_from: HashMap<EdgeKey, u32> = rt.jump_from.iter().copied().collect();
        let jump = RoadType::Jump.index();
        let mut edges: Vec<Edge> = Vec::with_capacity(nav.polys.iter().map(|p| p.len().saturating_sub(1)).sum::<usize>() + rt.added.len());
        let mut seen: HashSet<EdgeKey> = HashSet::new();

        // `first` = the endpoint a jump takes off from unless `jump_from` says otherwise.
        let push = |edges: &mut Vec<Edge>, first: u32, second: u32, key: EdgeKey, kind: u8, curv_turns: f32| {
            let (Some(ia), Some(ib)) = (index(first), index(second)) else { return };
            if ia == ib {
                return;
            }
            let (mut a, mut b) = (ia, ib);
            if kind == jump {
                if let Some(&s) = jump_from.get(&key) {
                    if index(s) == Some(ib) {
                        (a, b) = (ib, ia);
                    }
                }
            }
            let len = edge_len(p[a as usize], p[b as usize]);
            let curv = if len > 1e-3 { curv_turns / 2.0 / len } else { 0.0 };
            edges.push(Edge { a, b, len, kind, curv });
        };

        for pl in &nav.polys {
            for i in 0..pl.len().saturating_sub(1) {
                let key = EdgeKey::new(pl[i].id, pl[i + 1].id);
                if removed.contains(&key) || !seen.insert(key) {
                    continue;
                }
                let kind = rt.types.get(&key).map_or(0, |t| t.index());
                let at = |j: usize| index(pl[j].id).map(|n| p[n as usize]);
                let mut turns = 0.0;
                if i >= 1 {
                    if let (Some(x), Some(y), Some(z)) = (at(i - 1), at(i), at(i + 1)) {
                        turns += turn(x, y, z);
                    }
                }
                if i + 2 < pl.len() {
                    if let (Some(x), Some(y), Some(z)) = (at(i), at(i + 1), at(i + 2)) {
                        turns += turn(x, y, z);
                    }
                }
                push(&mut edges, pl[i].id, pl[i + 1].id, key, kind, turns);
            }
        }
        for l in &rt.added {
            push(&mut edges, l.a, l.b, EdgeKey::new(l.a, l.b), l.ty.map_or(0, |t| t.index()), 0.0);
        }

        // CSR adjacency + snap grid.
        let n = p.len();
        let mut count = vec![0u32; n + 1];
        for e in &edges {
            count[e.a as usize] += 1;
            if !e.is_jump() {
                count[e.b as usize] += 1;
            }
        }
        let mut arc_start = vec![0u32; n + 1];
        for i in 0..n {
            arc_start[i + 1] = arc_start[i] + count[i];
        }
        let mut fill = arc_start.clone();
        let mut arcs = vec![0u32; arc_start[n] as usize];
        let mut grid = SnapGrid::default();
        let turnaround = RoadType::Turnaround.index();
        for (i, e) in edges.iter().enumerate() {
            let i = i as u32;
            arcs[fill[e.a as usize] as usize] = i << 1;
            fill[e.a as usize] += 1;
            if !e.is_jump() {
                arcs[fill[e.b as usize] as usize] = i << 1 | 1;
                fill[e.b as usize] += 1;
                if e.kind != turnaround {
                    let (pa, pb) = (p[e.a as usize], p[e.b as usize]);
                    grid.insert(i, [pa[0], pa[1]], [pb[0], pb[1]]);
                }
            }
        }
        RouteGraph { pos: p, ids, edges, arc_start, arcs, grid }
    }

    #[cfg(test)]
    pub fn node_count(&self) -> usize {
        self.pos.len()
    }
    #[cfg(test)]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
    /// Position `[x, z, y]` of dense node `n` (y 0.0 = unknown).
    #[cfg(test)]
    pub fn node_pos(&self, n: u32) -> [f32; 3] {
        self.pos[n as usize]
    }
    /// The stable nav / user-point id of dense node `n`.
    #[cfg(test)]
    pub fn node_id(&self, n: u32) -> u32 {
        self.ids[n as usize]
    }
    /// Dense index of a nav / user-point id.
    #[cfg(test)]
    pub fn node_index(&self, id: u32) -> Option<u32> {
        self.ids.binary_search(&id).ok().map(|i| i as u32)
    }
    #[cfg(test)]
    pub fn edge(&self, e: u32) -> &Edge {
        &self.edges[e as usize]
    }
    #[cfg(test)]
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }
    /// The outgoing arcs (`edge << 1 | dir`) of node `n`.
    pub(super) fn arcs_of(&self, n: u32) -> &[u32] {
        &self.arcs[self.arc_start[n as usize] as usize..self.arc_start[n as usize + 1] as usize]
    }
    /// Where an arc leads: `(edge, from node, to node)`.
    pub(super) fn arc_ends(&self, arc: u32) -> (&Edge, u32, u32) {
        let e = &self.edges[(arc >> 1) as usize];
        if arc & 1 == 0 {
            (e, e.a, e.b)
        } else {
            (e, e.b, e.a)
        }
    }
}
