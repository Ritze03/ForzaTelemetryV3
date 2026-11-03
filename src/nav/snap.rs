//! Snapping a world position to the graph: the nearest point on an allowed edge, with a height
//! penalty so a car on an overpass snaps to the overpass and not to the road under it.

use crate::maprender::mesh3d::known_y;

use super::cfg::RouteFilters;
use super::graph::RouteGraph;

/// First search radius (m). The nav's edges are ~20 m long, so a car on a road is always inside.
pub const SNAP_R_M: f32 = 120.0;
/// Fallback radius when nothing allowed is within [`SNAP_R_M`]; beyond it there is no road near.
pub const SNAP_R_MAX_M: f32 = 300.0;
/// Height gap (m) that is free: the nav's node height vs the telemetry's `position_y` (suspension,
/// slopes between 20 m nodes). Beyond it every metre costs [`DY_WEIGHT`] metres of distance.
pub const DY_FREE_M: f32 = 4.0;
pub const DY_WEIGHT: f32 = 3.0;

/// A point on an edge: where a start / destination sits on the graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    /// Edge index ([`RouteGraph::edge`]).
    pub edge: u32,
    /// 0..=1 from the edge's `a` to its `b`.
    pub t: f32,
    /// The projection point `[x, z, y]` (y 0.0 = unknown).
    pub pos: [f32; 3],
    /// Horizontal distance from the queried position to `pos` (m).
    pub dist_m: f32,
}

impl RouteGraph {
    /// The best edge point for (x, z): candidates are the allowed edges (`filters`, so a click
    /// beside a trail with trails off snaps to the road) within [`SNAP_R_M`], growing to
    /// [`SNAP_R_MAX_M`] when there are none. Jump edges and turnarounds are never targets (the grid
    /// does not hold them).
    ///
    /// Score = horizontal distance + [`DY_WEIGHT`] * max(0, |dy| - [`DY_FREE_M`]) when `y` (the
    /// car's height) and the edge's height at the projection are both known: the racesel rule
    /// applied to overpass / tunnel / stacked-expressway ambiguity (Tokyo has up to 10 layers). A
    /// destination has `y = None` (a 2D click has no height; a click on 3D terrain has the
    /// terrain's, which differs from a road under a hill or on a bridge): distance only.
    /// Ties: the lowest horizontal distance.
    pub fn snap(&self, x: f32, z: f32, y: Option<f32>, filters: &RouteFilters) -> Option<Snap> {
        let y = y.and_then(known_y);
        let mask = filters.mask();
        for r in [SNAP_R_M, SNAP_R_MAX_M] {
            let mut best: Option<(f32, Snap)> = None;
            self.grid.near(x, z, r, |ei| {
                let e = &self.edges[ei as usize];
                if mask >> e.kind & 1 == 0 {
                    return;
                }
                let (pa, pb) = (self.pos[e.a as usize], self.pos[e.b as usize]);
                let d = [pb[0] - pa[0], pb[1] - pa[1]];
                let l2 = d[0] * d[0] + d[1] * d[1];
                let t = if l2 > 1e-9 { (((x - pa[0]) * d[0] + (z - pa[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (px, pz) = (pa[0] + d[0] * t, pa[1] + d[1] * t);
                let dist = (x - px).hypot(z - pz);
                if dist > r {
                    return;
                }
                let ey = match (known_y(pa[2]), known_y(pb[2])) {
                    (Some(a), Some(b)) => Some(a + (b - a) * t),
                    _ => None,
                };
                let score = match (y, ey) {
                    (Some(cy), Some(ey)) => dist + DY_WEIGHT * ((cy - ey).abs() - DY_FREE_M).max(0.0),
                    _ => dist,
                };
                let better = match &best {
                    None => true,
                    Some((s, b)) => score < *s || (score == *s && dist < b.dist_m),
                };
                if better {
                    // ponytail: an edge with one unknown end gets height 0 (unknown) rather than a guess
                    let py = ey.unwrap_or(0.0);
                    best = Some((score, Snap { edge: ei, t, pos: [px, pz, py], dist_m: dist }));
                }
            });
            if let Some((_, s)) = best {
                return Some(s);
            }
        }
        None
    }
}
