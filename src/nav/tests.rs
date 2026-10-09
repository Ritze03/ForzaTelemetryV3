//! Tests of the routing core on synthetic graphs, plus the real install (skipped without one).

use std::collections::HashMap;

use crate::gamedata::nav::{Nav, NavVert};
use crate::gamedata::roadtypes::{AddedLink, EdgeKey, RoadType, RoadTypes};
use crate::maprender::data::{build_roads, node_positions};

use super::cost::CostModel;
use super::*;

fn v(id: u32, x: f32, z: f32, y: f32) -> NavVert {
    NavVert { id, x, z, y }
}

fn nav_of(polys: Vec<Vec<NavVert>>) -> Nav {
    Nav { sha1: String::new(), nodes: 0, cls: vec![0; polys.len()], hi: vec![0; polys.len()], polys, orphans: vec![] }
}

fn build(nav: &Nav, rt: &RoadTypes) -> RouteGraph {
    RouteGraph::build(nav, rt, &node_positions(nav, rt))
}

/// A straight polyline along x with 100 m between the nodes `ids`, from `x0`.
fn line(ids: &[u32], x0: f32, z: f32) -> Vec<NavVert> {
    ids.iter().enumerate().map(|(i, &id)| v(id, x0 + 100.0 * i as f32, z, 0.0)).collect()
}

fn typed(rt: &mut RoadTypes, edges: &[(u32, u32, RoadType)]) {
    for &(a, b, t) in edges {
        rt.types.insert(EdgeKey::new(a, b), t);
    }
}

fn prefs(filters: RouteFilters, curves: f32) -> RoutePrefs {
    RoutePrefs { filters, curves }
}

fn road_prefs() -> RoutePrefs {
    prefs(RouteFilters::default(), 0.0)
}

// ── build ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn build_honours_removed_moved_points_added_and_jump_from() {
    let nav = nav_of(vec![vec![v(1, 0.0, 0.0, 0.0), v(2, 10.0, 0.0, 0.0), v(3, 20.0, 0.0, 0.0), v(4, 30.0, 0.0, 0.0)]]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Road), (2, 3, RoadType::Jump), (3, 4, RoadType::Highway)]);
    rt.removed.push(EdgeKey::new(3, 4));
    rt.moved.insert(2, [10.0, 40.0, 5.0]);
    rt.jump_from.push((EdgeKey::new(2, 3), 3)); // takes off at 3, lands at 2
    rt.points.insert(1_000_001, [50.0, 50.0, 9.0]);
    rt.added.push(AddedLink { a: 3, b: 1_000_001, ty: Some(RoadType::Crosscountry) });
    rt.added.push(AddedLink { a: 3, b: 424_242, ty: Some(RoadType::Road) }); // unknown node: skipped
    rt.added.push(AddedLink { a: 1, b: 1_000_001, ty: None }); // untyped

    let g = build(&nav, &rt);
    assert_eq!(g.node_count(), 5, "4 nav nodes + 1 user point");
    assert_eq!(g.edge_count(), 4, "1-2, 2-3, the two good links (3-4 removed, 424242 unknown)");
    let n = |id| g.node_index(id).unwrap();
    assert_eq!(g.node_pos(n(2)), [10.0, 40.0, 5.0], "moved node");
    assert_eq!(g.node_pos(n(1_000_001)), [50.0, 50.0, 9.0], "user point");
    // the jump: take-off (a) = node 3, landing (b) = node 2; exactly one arc
    let j = g.edges().iter().find(|e| e.is_jump()).unwrap();
    assert_eq!((g.node_id(j.a), g.node_id(j.b)), (3, 2));
    assert_eq!(g.arcs_of(n(3)).iter().filter(|&&a| g.edge(a >> 1).is_jump()).count(), 1);
    assert_eq!(g.arcs_of(n(2)).iter().filter(|&&a| g.edge(a >> 1).is_jump()).count(), 0, "no arc out of the landing");
    // lengths: the jump is 3D (heights 0 -> unknown, so 2D here); 1-2 uses the moved node
    let e12 = g.edges().iter().find(|e| e.kind == RoadType::Road.index()).unwrap();
    assert!((e12.len - 41.231).abs() < 0.01, "{}", e12.len);
    // the untyped link has kind 0
    assert!(g.edges().iter().any(|e| e.kind == 0));
    // A turnaround is in the graph but not in the snap grid.
    let mut rt2 = rt.clone();
    typed(&mut rt2, &[(1, 2, RoadType::Turnaround)]);
    let g2 = build(&nav, &rt2);
    assert_eq!(g2.edge_count(), 4);
    assert!(g2.snap(5.0, 20.0, None, &RouteFilters::ALL).is_some_and(|s| g2.edge(s.edge).kind != RoadType::Turnaround.index()));
}

/// The graph and the drawn roads take their node positions from the same function.
#[test]
fn graph_and_roads_share_node_positions() {
    let nav = nav_of(vec![vec![v(1, 0.0, 0.0, 3.0), v(2, 10.0, 0.0, 4.0), v(3, 20.0, 0.0, 5.0)]]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Road), (2, 3, RoadType::Road)]);
    rt.moved.insert(2, [11.0, 6.0, 4.5]);
    rt.points.insert(1_000_000, [30.0, 0.0, 6.0]);
    rt.added.push(AddedLink { a: 3, b: 1_000_000, ty: Some(RoadType::Road) });
    let g = build(&nav, &rt);
    let roads = build_roads(&nav, &rt);
    let nodes: Vec<[f32; 3]> = (0..g.node_count() as u32).map(|n| g.node_pos(n)).collect();
    let chains = roads.by_type[RoadType::Road.index() as usize].iter().collect::<Vec<_>>();
    assert_eq!(chains.len(), 2); // the polyline run and the added link
    for c in chains {
        for (p, &y) in c.pts.iter().zip(&c.y) {
            assert!(nodes.contains(&[p[0], p[1], y]), "{p:?} {y}");
        }
    }
}

#[test]
fn a_pair_in_two_polylines_is_one_edge() {
    let nav = nav_of(vec![line(&[1, 2, 3], 0.0, 0.0), line(&[2, 3, 4], 100.0, 0.0)]);
    assert_eq!(build(&nav, &RoadTypes::raw()).edge_count(), 3);
}

#[test]
fn curvature_is_the_roads_own_winding() {
    // 90 degree corner at node 2 inside one polyline; polyline ends and added links get 0.
    let nav = nav_of(vec![vec![v(1, 0.0, 0.0, 0.0), v(2, 10.0, 0.0, 0.0), v(3, 10.0, 10.0, 0.0), v(4, 10.0, 20.0, 0.0)]]);
    let g = build(&nav, &RoadTypes::raw());
    let curv = |a, b| g.edges().iter().find(|e| EdgeKey::new(g.node_id(e.a), g.node_id(e.b)) == EdgeKey::new(a, b)).unwrap().curv;
    let quarter = std::f32::consts::FRAC_PI_2;
    assert!((curv(1, 2) - quarter / 2.0 / 10.0).abs() < 1e-5);
    assert!((curv(2, 3) - quarter / 2.0 / 10.0).abs() < 1e-5);
    assert_eq!(curv(3, 4), 0.0);
}

// ── filters and directedness in the search ───────────────────────────────────────────────────

/// Offroad - T - Offroad in a row; `dirt` is always on so both ends can be snapped. Is the far end
/// reachable? The expected side is written out from the design table, not from `allows`.
#[test]
fn filters_prune_edges_per_the_table() {
    let kinds: [(Option<RoadType>, fn(&RouteFilters) -> bool); 10] = [
        (Some(RoadType::Road), |f| f.road),
        (Some(RoadType::Highway), |f| f.highway),
        (Some(RoadType::Offroad), |_| true),
        (Some(RoadType::Trail), |f| f.trail),
        (Some(RoadType::Crosscountry), |f| f.cross_country),
        (Some(RoadType::Tunnel), |f| f.road || f.highway),
        (Some(RoadType::Other), |f| f.road),
        (None, |f| f.road),
        (Some(RoadType::Turnaround), |_| false),
        (Some(RoadType::Jump), |f| f.jumps),
    ];
    let nav = nav_of(vec![line(&[1, 2, 3, 4], 0.0, 0.0)]);
    for (mid, expect) in kinds {
        let mut rt = RoadTypes::raw();
        typed(&mut rt, &[(1, 2, RoadType::Offroad), (3, 4, RoadType::Offroad)]);
        if let Some(t) = mid {
            typed(&mut rt, &[(2, 3, t)]);
        }
        let g = build(&nav, &rt);
        for bits in 0..64u8 {
            let f = RouteFilters { dirt: true, ..RouteFilters::from_bits(bits) };
            let r = g.plan((10.0, 2.0, None), (290.0, 2.0), &prefs(f, 0.0));
            assert_eq!(r.is_ok(), expect(&f), "{mid:?} with {f:?}: {r:?}");
        }
    }
}

#[test]
fn jump_is_one_way_take_off_to_landing() {
    // Two roads joined only by a jump 2 -> 3 (take-off 2).
    let nav = nav_of(vec![line(&[1, 2], 0.0, 0.0), line(&[3, 4], 300.0, 0.0)]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Road), (3, 4, RoadType::Road)]);
    rt.added.push(AddedLink { a: 2, b: 3, ty: Some(RoadType::Jump) });
    let g = build(&nav, &rt);
    let on = prefs(RouteFilters::ALL, 0.0);
    let fwd = g.plan((10.0, 3.0, None), (390.0, 3.0), &on).expect("take-off -> landing");
    assert!(fwd.seg_kind.contains(&RoadType::Jump.index()));
    assert_eq!(g.plan((390.0, 3.0, None), (10.0, 3.0), &on), Err(RouteError::Unreachable), "the way back must not use the jump");
    // with the Jumps filter off neither direction works
    let off = prefs(RouteFilters { jumps: false, ..RouteFilters::ALL }, 0.0);
    assert_eq!(g.plan((10.0, 3.0, None), (390.0, 3.0), &off), Err(RouteError::Unreachable));
    // a jump is never a snap target
    assert!(g.snap(150.0, 0.0, None, &RouteFilters::ALL).is_some_and(|s| !g.edge(s.edge).is_jump()));
    // jump_from decides the direction: reversed take-off flips it
    let mut rt2 = rt.clone();
    rt2.jump_from.push((EdgeKey::new(2, 3), 3));
    let g2 = build(&nav, &rt2);
    assert_eq!(g2.plan((10.0, 3.0, None), (390.0, 3.0), &on), Err(RouteError::Unreachable));
    assert!(g2.plan((390.0, 3.0, None), (10.0, 3.0), &on).is_ok());
}

#[test]
fn turnaround_is_never_routed() {
    let nav = nav_of(vec![line(&[1, 2], 0.0, 0.0), line(&[3, 4], 200.0, 0.0)]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Road), (3, 4, RoadType::Road)]);
    rt.added.push(AddedLink { a: 2, b: 3, ty: Some(RoadType::Turnaround) });
    assert_eq!(build(&nav, &rt).plan((10.0, 3.0, None), (290.0, 3.0), &prefs(RouteFilters::ALL, 0.0)), Err(RouteError::Unreachable));
    // an untyped link does connect them (unset follows Road)
    rt.added[0].ty = None;
    assert!(build(&nav, &rt).plan((10.0, 3.0, None), (290.0, 3.0), &road_prefs()).is_ok());
}

// ── the slider ───────────────────────────────────────────────────────────────────────────────

/// Node 1 (0,0) to node 2 (2000,0): one straight 2000 m highway edge, or a 3.1 km zig-zag road
/// (sharp corners, curvature above KAPPA on every edge).
fn two_routes() -> RouteGraph {
    let mut zig = vec![v(1, 0.0, 0.0, 0.0)];
    for i in 1..40u32 {
        zig.push(v(100 + i, 50.0 * i as f32, if i % 2 == 1 { 60.0 } else { 0.0 }, 0.0));
    }
    zig.push(v(2, 2000.0, 0.0, 0.0));
    let nav = nav_of(vec![vec![v(1, 0.0, 0.0, 0.0), v(2, 2000.0, 0.0, 0.0)], zig]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Highway)]);
    let ids: Vec<u32> = nav.polys[1].iter().map(|p| p.id).collect();
    for w in ids.windows(2) {
        typed(&mut rt, &[(w[0], w[1], RoadType::Road)]);
    }
    build(&nav, &rt)
}

#[test]
fn slider_picks_the_highway_or_the_winding_road() {
    let g = two_routes();
    let all = RouteFilters::default();
    let fast = g.plan((-5.0, 0.0, None), (2005.0, 0.0), &prefs(all, 0.0)).unwrap();
    assert!(fast.seg_kind.iter().all(|&k| k == RoadType::Highway.index() || k == 0), "{:?}", fast.seg_kind);
    let curvy = g.plan((-5.0, 0.0, None), (2005.0, 0.0), &prefs(all, 1.0)).unwrap();
    let road = RoadType::Road.index();
    assert!(curvy.seg_kind.iter().filter(|&&k| k == road).count() >= 38, "{:?}", curvy.seg_kind);
    assert!(curvy.dist_m > 1.4 * fast.dist_m && curvy.eta_s > 2.0 * fast.eta_s, "the price of winding: {} m vs {} m", curvy.dist_m, fast.dist_m);
    // with the highway filter off, even s = 0 takes the winding road
    let nohw = g.plan((-5.0, 0.0, None), (2005.0, 0.0), &prefs(RouteFilters { highway: false, ..all }, 0.0)).unwrap();
    assert!(nohw.seg_kind.iter().filter(|&&k| k == road).count() >= 38);
}

// ── snapping ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn overpass_snaps_by_height() {
    // an x road at height 10 and a z road at height 30 cross at the origin without a shared node
    let nav = nav_of(vec![vec![v(1, -100.0, 0.0, 10.0), v(2, 100.0, 0.0, 10.0)], vec![v(3, 0.0, -100.0, 30.0), v(4, 0.0, 100.0, 30.0)]]);
    let g = build(&nav, &RoadTypes::raw());
    let f = RouteFilters::default();
    let ids = |s: Snap| {
        let e = g.edge(s.edge);
        (g.node_id(e.a), g.node_id(e.b))
    };
    // 2 m from the x road, 10 m from the z road
    assert_eq!(ids(g.snap(10.0, 2.0, None, &f).unwrap()), (1, 2), "no height: the nearest");
    assert_eq!(ids(g.snap(10.0, 2.0, Some(29.0), &f).unwrap()), (3, 4), "car on the upper road");
    assert_eq!(ids(g.snap(10.0, 2.0, Some(10.5), &f).unwrap()), (1, 2), "car on the lower road");
    // within the free gap the distance still decides
    assert_eq!(ids(g.snap(10.0, 2.0, Some(14.0), &f).unwrap()), (1, 2));
    // unknown (0) heights do not count
    assert_eq!(ids(g.snap(10.0, 2.0, Some(0.0), &f).unwrap()), (1, 2));
    let s = g.snap(10.0, 2.0, Some(29.0), &f).unwrap();
    assert!((s.pos[2] - 30.0).abs() < 1e-4 && (s.dist_m - 10.0).abs() < 1e-3 && (s.pos[0] - 0.0).abs() < 1e-4 && (s.pos[1] - 2.0).abs() < 1e-3, "{s:?}");
}

#[test]
fn snap_skips_disallowed_types_and_grows_its_radius() {
    let nav = nav_of(vec![line(&[1, 2], 0.0, 0.0), line(&[3, 4], 0.0, 100.0), line(&[5, 6], 0.0, 1000.0)]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Trail), (3, 4, RoadType::Road), (5, 6, RoadType::Road)]);
    let g = build(&nav, &rt);
    let edge_ids = |s: Snap| g.node_id(g.edge(s.edge).a);
    // 5 m from the trail, 95 m from the road
    assert_eq!(edge_ids(g.snap(50.0, 5.0, None, &RouteFilters { trail: true, ..Default::default() }).unwrap()), 1);
    assert_eq!(edge_ids(g.snap(50.0, 5.0, None, &RouteFilters::default()).unwrap()), 3, "trails off: the road");
    // nothing within 120 m but a road at 250 m: found by the second ring
    let far = g.snap(50.0, 750.0, None, &RouteFilters::default()).unwrap();
    assert_eq!(edge_ids(far), 5);
    assert!((far.dist_m - 250.0).abs() < 1e-3);
    // beyond 300 m: nothing
    assert!(g.snap(50.0, 600.0, None, &RouteFilters::NONE).is_none());
    assert!(g.snap(5000.0, 5000.0, None, &RouteFilters::ALL).is_none());
}

fn assert_pts(r: &Route, want: &[[f32; 2]]) {
    assert_eq!(r.pts.len(), want.len(), "{:?}", r.pts);
    for (p, w) in r.pts.iter().zip(want) {
        assert!((p[0] - w[0]).abs() < 1e-3 && (p[1] - w[1]).abs() < 1e-3, "{:?} vs {want:?}", r.pts);
    }
}

#[test]
fn start_and_destination_sit_mid_edge() {
    let nav = nav_of(vec![line(&[1, 2, 3, 4], 0.0, 0.0)]); // nodes at x = 0, 100, 200, 300
    let g = build(&nav, &RoadTypes::raw());
    let p = road_prefs();
    // 130 -> 170 on edge 2-3 would be same-edge; use 30 on edge 1-2 and 270 on 3-4... first: 30 -> 170
    let r = g.plan((30.0, 5.0, None), (170.0, -5.0), &p).unwrap();
    assert_pts(&r, &[[30.0, 0.0], [100.0, 0.0], [170.0, 0.0]]);
    assert!((r.dist_m - 140.0).abs() < 1e-3);
    assert_eq!(r.seg_kind.len(), 2);
    // against the polyline order, ending on the edge before the start's
    let r = g.plan((130.0, 3.0, None), (40.0, 3.0), &p).unwrap();
    assert_pts(&r, &[[130.0, 0.0], [100.0, 0.0], [40.0, 0.0]]);
    assert!((r.dist_m - 90.0).abs() < 1e-3);
    // both on the same edge, either order: the direct piece, no detour through the nodes
    let r = g.plan((20.0, 3.0, None), (60.0, 3.0), &p).unwrap();
    assert_pts(&r, &[[20.0, 0.0], [60.0, 0.0]]);
    assert!((r.dist_m - 40.0).abs() < 1e-3);
    let r = g.plan((60.0, 3.0, None), (20.0, 3.0), &p).unwrap();
    assert_pts(&r, &[[60.0, 0.0], [20.0, 0.0]]);
    // exactly on a node: no duplicate points
    let r = g.plan((100.0, 0.0, None), (200.0, 0.0), &p).unwrap();
    assert!(r.pts.windows(2).all(|w| w[0] != w[1]), "{:?}", r.pts);
    assert!((r.dist_m - 100.0).abs() < 1e-3);
    // the same point twice: a one-point route of length 0
    let r = g.plan((50.0, 2.0, None), (50.0, -2.0), &p).unwrap();
    assert_eq!(r.pts.len(), 1);
    assert_eq!(r.dist_m, 0.0);
}

#[test]
fn eta_uses_the_assumed_speeds() {
    let nav = nav_of(vec![line(&[1, 2, 3], 0.0, 0.0)]);
    let mut rt = RoadTypes::raw();
    typed(&mut rt, &[(1, 2, RoadType::Highway), (2, 3, RoadType::Road)]);
    let r = build(&nav, &rt).plan((0.0, 1.0, None), (200.0, 1.0), &road_prefs()).unwrap();
    assert!((r.eta_s - (100.0 / 40.0 + 100.0 / 22.0)).abs() < 1e-3, "{}", r.eta_s);
    assert!((r.seg_time_s.iter().sum::<f32>() - r.eta_s).abs() < 1e-4);
    assert!((r.seg_len.iter().sum::<f32>() - r.dist_m).abs() < 1e-4);
}

#[test]
fn heights_ride_along_and_make_the_length_3d() {
    let nav = nav_of(vec![vec![v(1, 0.0, 0.0, 10.0), v(2, 30.0, 0.0, 50.0), v(3, 60.0, 0.0, 0.0)]]);
    let g = build(&nav, &RoadTypes::raw());
    assert!((g.edge(0).len - 50.0).abs() < 1e-3, "30 m flat + 40 m up = 50 m");
    assert!((g.edge(1).len - 30.0).abs() < 1e-3, "an unknown height (0) means 2D");
    let r = g.plan((0.0, 0.0, None), (60.0, 0.0), &road_prefs()).unwrap();
    assert_eq!(r.y, vec![10.0, 50.0, 0.0]);
}

// ── errors ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn errors() {
    let p = road_prefs();
    assert_eq!(RouteGraph::default().plan((0.0, 0.0, None), (1.0, 1.0), &p), Err(RouteError::EmptyGraph));
    assert_eq!(format!("{}", RouteError::Unreachable), "no route with these road types");
    let nav = nav_of(vec![line(&[1, 2], 0.0, 0.0), line(&[3, 4], 0.0, 1000.0)]);
    let g = build(&nav, &RoadTypes::raw());
    assert_eq!(g.plan((5000.0, 0.0, None), (50.0, 0.0), &p), Err(RouteError::NoRoadNear(Endpoint::Car)));
    assert_eq!(g.plan((50.0, 0.0, None), (5000.0, 0.0), &p), Err(RouteError::NoRoadNear(Endpoint::Destination)));
    assert_eq!(g.plan((50.0, 0.0, None), (50.0, 1000.0), &p), Err(RouteError::Unreachable), "two islands");
    // every road filtered out: nothing to snap to
    assert_eq!(g.plan((50.0, 0.0, None), (50.0, 1.0), &prefs(RouteFilters::NONE, 0.0)), Err(RouteError::NoRoadNear(Endpoint::Car)));
}

// ── A* == Dijkstra ───────────────────────────────────────────────────────────────────────────

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// A 16 x 16 lattice (45 m, jittered) of row and column polylines with random types, heights
/// (some unknown), a few removals, a few typed added links including jumps.
fn lattice(rng: &mut Rng) -> RouteGraph {
    const N: usize = 16;
    let id = |r: usize, c: usize| 1 + (r * N + c) as u32;
    let mut pts = vec![];
    for r in 0..N {
        for c in 0..N {
            let y = if rng.below(20) == 0 { 0.0 } else { 100.0 + 30.0 * rng.f() };
            pts.push(v(id(r, c), c as f32 * 45.0 + 14.0 * rng.f(), r as f32 * 45.0 + 14.0 * rng.f(), y));
        }
    }
    let mut polys = vec![];
    for r in 0..N {
        polys.push((0..N).map(|c| pts[r * N + c]).collect());
    }
    for c in 0..N {
        polys.push((0..N).map(|r| pts[r * N + c]).collect());
    }
    let nav = nav_of(polys);
    let mut rt = RoadTypes::raw();
    let pool = [
        Some(RoadType::Road),
        Some(RoadType::Road),
        Some(RoadType::Road),
        Some(RoadType::Highway),
        Some(RoadType::Highway),
        Some(RoadType::Offroad),
        Some(RoadType::Offroad),
        Some(RoadType::Trail),
        Some(RoadType::Crosscountry),
        Some(RoadType::Tunnel),
        Some(RoadType::Other),
        Some(RoadType::Turnaround),
        None,
    ];
    for e in nav.edges() {
        match pool[rng.below(pool.len())] {
            Some(t) => {
                rt.types.insert(EdgeKey::new(e.0, e.1), t);
            }
            None => {}
        }
        if rng.below(60) == 0 {
            rt.removed.push(EdgeKey::new(e.0, e.1));
        }
    }
    for k in 0..8 {
        let (a, b) = (1 + rng.below(N * N) as u32, 1 + rng.below(N * N) as u32);
        let ty = [RoadType::Jump, RoadType::Road, RoadType::Offroad, RoadType::Crosscountry][k % 4];
        if k % 4 == 0 {
            rt.jump_from.push((EdgeKey::new(a, b), if rng.below(2) == 0 { a } else { b }));
        }
        rt.added.push(AddedLink { a, b, ty: Some(ty) });
    }
    build(&nav, &rt)
}

#[test]
fn astar_cost_equals_dijkstra_on_random_pairs() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut ok, mut err, mut pairs) = (0, 0, 0);
    for _ in 0..4 {
        let g = lattice(&mut rng);
        for i in 0..100 {
            let filters = if i % 5 == 0 { RouteFilters::ALL } else { RouteFilters::from_bits(rng.below(64) as u8) };
            let curves = [0.0, 0.25, 0.5, 1.0][rng.below(4)];
            let (q0, q1) = ([rng.f() * 720.0, rng.f() * 720.0], [rng.f() * 720.0, rng.f() * 720.0]);
            let y0 = (rng.below(3) != 0).then(|| 100.0 + 30.0 * rng.f());
            let (Some(a), Some(b)) = (g.snap(q0[0], q0[1], y0, &filters), g.snap(q1[0], q1[1], None, &filters)) else { continue };
            pairs += 1;
            let cm = CostModel::new(&prefs(filters, curves));
            let (astar, dijkstra) = (g.search(&a, &b, &cm, true), g.search(&a, &b, &cm, false));
            match (astar, dijkstra) {
                (Ok(x), Ok(y)) => {
                    ok += 1;
                    assert!((x.cost - y.cost).abs() <= 1e-3 * y.cost.max(1.0), "A* {} vs Dijkstra {} ({filters:?}, s {curves})", x.cost, y.cost);
                    assert!(x.dist_m >= (a.pos[0] - b.pos[0]).hypot(a.pos[1] - b.pos[1]) - 1e-2, "shorter than the straight line");
                    assert_eq!(x.pts.first().map(|p| [p[0], p[1]]), Some([a.pos[0], a.pos[1]]));
                    assert_eq!(x.pts.last().map(|p| [p[0], p[1]]), Some([b.pos[0], b.pos[1]]));
                    assert_eq!(x.seg_kind.len() + 1, x.pts.len());
                    // no turnaround, and no disallowed type, in the route
                    assert!(x.seg_kind.iter().all(|&k| filters.allows(k)), "{:?} with {filters:?}", x.seg_kind);
                }
                (Err(x), Err(y)) => {
                    err += 1;
                    assert_eq!(x, y);
                }
                (x, y) => panic!("A* {x:?} vs Dijkstra {y:?}"),
            }
        }
    }
    eprintln!("A* == Dijkstra: {pairs} pairs, {ok} routes, {err} unreachable");
    assert!(ok >= 100 && err >= 5, "the fixture must exercise both outcomes: {ok} routes, {err} errors");
}

// ── the real install ─────────────────────────────────────────────────────────────────────────

/// Connected-component sizes (nodes) over the arcs of edges `allow` lets through; largest first.
fn components(g: &RouteGraph, allow: impl Fn(u8) -> bool) -> Vec<usize> {
    // arcs are directed (jumps): use the union-find of undirected connectivity, which equals the
    // component structure for everything but jumps
    let mut uf: Vec<u32> = (0..g.node_count() as u32).collect();
    fn find(uf: &mut [u32], mut x: u32) -> u32 {
        while uf[x as usize] != x {
            uf[x as usize] = uf[uf[x as usize] as usize];
            x = uf[x as usize];
        }
        x
    }
    for e in g.edges().iter().filter(|e| allow(e.kind)) {
        let (a, b) = (find(&mut uf, e.a), find(&mut uf, e.b));
        uf[a as usize] = b;
    }
    let mut size: HashMap<u32, usize> = HashMap::new();
    for n in 0..g.node_count() as u32 {
        *size.entry(find(&mut uf, n)).or_default() += 1;
    }
    let mut v: Vec<usize> = size.into_values().collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}

/// A snap exactly on `node`, on an allowed non-jump edge next to it.
fn snap_at(g: &RouteGraph, node: u32, filters: &RouteFilters) -> Option<Snap> {
    let e = g.arcs_of(node).iter().map(|&a| a >> 1).find(|&e| {
        let k = g.edge(e).kind;
        filters.allows(k) && k != RoadType::Jump.index()
    })?;
    let t = if g.edge(e).a == node { 0.0 } else { 1.0 };
    Some(Snap { edge: e, t, pos: g.node_pos(node), dist_m: 0.0 })
}

#[test]
fn real_install_graph() {
    let Some(media) = crate::gamedata::install::find_media(None) else {
        eprintln!("SKIP real_install_graph: FH6 install not found (set FH6_INSTALL_DIR)");
        return;
    };
    let nav = Nav::load(&media).expect("nav");
    // The project road-type data, not a user's saved file (it would change the numbers).
    let rt = RoadTypes::project();
    let t = std::time::Instant::now();
    let pos = node_positions(&nav, &rt);
    let t_pos = t.elapsed();
    let t = std::time::Instant::now();
    let g = RouteGraph::build(&nav, &rt, &pos);
    let t_build = t.elapsed();
    eprintln!("graph: {} nodes, {} edges; node_positions {t_pos:?}, build {t_build:?}", g.node_count(), g.edge_count());
    assert_eq!((g.node_count(), g.edge_count()), (38_597, 39_600));
    let count = |t: RoadType| g.edges().iter().filter(|e| e.kind == t.index()).count();
    assert_eq!([count(RoadType::Road), count(RoadType::Offroad), count(RoadType::Highway), count(RoadType::Trail), count(RoadType::Tunnel), count(RoadType::Turnaround), count(RoadType::Other), count(RoadType::Crosscountry), count(RoadType::Jump)], [21_701, 7_237, 4_996, 3_943, 904, 340, 317, 144, 18]);
    assert!(t_build.as_millis() < if cfg!(debug_assertions) { 3000 } else { 300 }, "{t_build:?}");

    // Connectivity (the scout's numbers; jumps are one-way and leave out of these unions).
    let ti = |t: RoadType| t.index();
    let not_turn = |k: u8| k != ti(RoadType::Turnaround);
    assert_eq!(components(&g, not_turn)[0], 38_594);
    let rt_only = components(&g, |k| k == ti(RoadType::Road) || k == ti(RoadType::Tunnel));
    eprintln!("road+tunnel components: {:?}", &rt_only[..6]);
    assert_eq!(rt_only[0], 21_719);
    let rth = components(&g, |k| k == ti(RoadType::Road) || k == ti(RoadType::Tunnel) || k == ti(RoadType::Highway));
    assert_eq!(rth[0], 27_251);
    let with_other = components(&g, |k| RouteFilters { highway: false, dirt: false, ..RouteFilters::default() }.allows(k));
    eprintln!("default 'road' switch only (road+tunnel+other+unset): largest component {}", with_other[0]);

    // The 18 jumps: one arc each, out of the take-off only; routable forward, never backward.
    let jumps: Vec<u32> = (0..g.edge_count() as u32).filter(|&e| g.edge(e).is_jump()).collect();
    assert_eq!(jumps.len(), 18);
    let all = prefs(RouteFilters::ALL, 0.0);
    let no_jumps = prefs(RouteFilters { jumps: false, ..RouteFilters::ALL }, 0.0);
    let mut used = 0;
    for &je in &jumps {
        let e = *g.edge(je);
        assert!(g.arcs_of(e.a).contains(&(je << 1)) && !g.arcs_of(e.b).contains(&(je << 1 | 1)) && !g.arcs_of(e.b).contains(&(je << 1)));
        let (from, to) = (snap_at(&g, e.a, &all.filters).unwrap(), snap_at(&g, e.b, &all.filters).unwrap());
        let fwd = g.route(&from, &to, &all).expect("take-off -> landing");
        let back = g.route(&to, &from, &all).expect("the island is connected without the jump");
        // the way back never flies THIS jump (another one may help): no jump segment from the landing to the take-off
        let (pa, pb) = ([g.node_pos(e.a)[0], g.node_pos(e.a)[1]], [g.node_pos(e.b)[0], g.node_pos(e.b)[1]]);
        let flies_back = back.pts.windows(2).zip(&back.seg_kind).any(|(w, &k)| k == RoadType::Jump.index() && w[0] == pb && w[1] == pa);
        assert!(!flies_back, "jump {je} flown backwards");
        if fwd.seg_kind.contains(&RoadType::Jump.index()) {
            used += 1;
        }
        let nj = g.route(&from, &to, &no_jumps).unwrap();
        assert!(nj.cost >= fwd.cost - 1e-2, "allowing jumps never costs more");
    }
    eprintln!("jumps: {used} of 18 take-off -> landing routes use the jump at slider 0");
    assert!(used >= 1, "a jump must pay off somewhere");

    // One route across the island, timed.
    let n0 = g.node_index(1).unwrap();
    let p0 = g.node_pos(n0);
    let far = (0..g.node_count() as u32).max_by(|&a, &b| {
        let d = |n: u32| (g.node_pos(n)[0] - p0[0]).hypot(g.node_pos(n)[1] - p0[1]);
        d(a).total_cmp(&d(b))
    });
    let far = far.unwrap();
    let pf = g.node_pos(far);
    let straight = (pf[0] - p0[0]).hypot(pf[1] - p0[1]);
    let p = road_prefs();
    let mut best = std::time::Duration::MAX;
    let mut route = None;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let r = g.plan((p0[0], p0[1], Some(p0[2])), (pf[0], pf[1]), &p);
        best = best.min(t.elapsed());
        route = Some(r);
    }
    let r = route.unwrap().expect("a route across the island");
    eprintln!("across the island: straight {straight:.0} m, route {:.0} m, {} points, ETA {:.1} min, best of 5: {best:?} ({})", r.dist_m, r.pts.len(), r.eta_s / 60.0, if cfg!(debug_assertions) { "debug" } else { "release" });
    assert!(r.dist_m >= straight);
    assert!(best.as_millis() < if cfg!(debug_assertions) { 2000 } else { 150 }, "{best:?}");
    for curves in [0.5, 1.0] {
        let t = std::time::Instant::now();
        let c = g.plan((p0[0], p0[1], Some(p0[2])), (pf[0], pf[1]), &prefs(p.filters, curves)).unwrap();
        eprintln!("  curves {curves}: {:.0} m, ETA {:.1} min, {:?}", c.dist_m, c.eta_s / 60.0, t.elapsed());
    }
}
