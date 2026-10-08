//! Render-ready map layer data: roads as chains per type, POIs with a cull grid, race lines
//! with a segment grid. Pure CPU, no egui; built once on the `map-layers` thread (`store`) and
//! shared as `Arc`s with both maps.
//!
//! All coordinates are world metres (telemetry space: x east, z north). Heights ride along
//! (`Chain::y`, `Poi::y`, `RaceLine::y`) for phase K's 3D scene; the 2D renderer ignores them.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::gamedata::icons::{PoiIcons, RaceClass};
use crate::gamedata::nav::Nav;
use crate::gamedata::poi::{Poi, PoiKind, Pois};
use crate::gamedata::racelines::{self, RaceLine};
use crate::gamedata::roadtypes::{Current, EdgeKey, RoadType, RoadTypes};

/// Number of road type slots: 0 = edge without a type, 1..=9 = [`RoadType::index`].
pub const N_TYPES: usize = 10;
/// Race lines are decimated to a point every this many metres (the reader's `step_m`).
pub const RACE_STEP_M: f32 = 5.0;
/// POIs further than this from the origin are off the map (the 11 parking areas at z ≈ 18 km).
pub const POI_LIMIT_M: f32 = 12_000.0;

// ── roads ────────────────────────────────────────────────────────────────────────────────────

/// A run of consecutive same-type edges of one nav polyline, so dashes run on across nodes.
#[derive(Clone, Debug)]
pub struct Chain {
    pub pts: Vec<[f32; 2]>,
    /// Height per vertex (phase K: bridges, tunnels). Unread by the 2D renderer.
    #[allow(dead_code)]
    pub y: Vec<f32>,
    /// `[min_x, min_z, max_x, max_z]`.
    pub bbox: [f32; 4],
}

impl Chain {
    pub fn new(pts: Vec<[f32; 2]>, y: Vec<f32>) -> Chain {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for p in &pts {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        Chain { pts, y, bbox: b }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RoadLayer {
    /// Chains by type slot ([`N_TYPES`]). The turnaround slot is filled but never drawn (D52).
    pub by_type: [Vec<Chain>; N_TYPES],
    /// Jump edges: take-off `x, z, y` → landing `x, z, y`.
    pub jumps: Vec<[f32; 6]>,
    /// Edges per type slot (consecutive polyline edges plus added links), for diagnostics.
    pub edge_counts: [u32; N_TYPES],
}

impl RoadLayer {
    pub fn chains(&self) -> usize {
        self.by_type.iter().map(Vec::len).sum()
    }
    pub fn vertices(&self) -> usize {
        self.by_type.iter().flatten().map(|c| c.pts.len()).sum()
    }
}

/// Port of `preview_2d.py:build`. Positions come from the nav polylines, overridden by the road-type
/// file's `moved` / `points`. Per nav polyline the consecutive edges are walked: removed ones
/// break the chain, the type comes from `rt.types` (missing = slot 0), and consecutive
/// same-type edges are chained. Jump edges become [`RoadLayer::jumps`] (take-off from
/// `rt.jump_from`, else the first node). `rt.added` links follow as single segments.
///
/// Why not `mapedit::data::road_graph`: it rounds to 0.1 m, drops the polyline order (so no
/// chains) and has no `jump_from`.
pub fn build_roads(nav: &Nav, rt: &RoadTypes) -> RoadLayer {
    // id → x, z, height
    let mut pos: HashMap<u32, [f32; 3]> = HashMap::new();
    for pl in &nav.polys {
        for v in pl {
            pos.insert(v.id, [v.x, v.z, v.y]);
        }
    }
    for &(id, x, z) in &nav.orphans {
        pos.insert(id, [x, z, 0.0]);
    }
    for src in [&rt.points, &rt.moved] {
        for (&id, p) in src {
            pos.insert(id, [p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    let removed: HashSet<EdgeKey> = rt.removed.iter().copied().collect();
    let jump_from: HashMap<EdgeKey, u32> = rt.jump_from.iter().copied().collect();
    let jump_slot = RoadType::Jump.index() as usize;

    let mut out = RoadLayer::default();
    let add_jump = |out: &mut RoadLayer, a: u32, b: u32, k: EdgeKey| {
        let s = jump_from.get(&k).copied().unwrap_or(a);
        let e = if s == a { b } else { a };
        if let (Some(ps), Some(pe)) = (pos.get(&s), pos.get(&e)) {
            out.jumps.push([ps[0], ps[1], ps[2], pe[0], pe[1], pe[2]]);
        }
    };
    let flush = |out: &mut RoadLayer, cur: &mut Vec<[f32; 3]>, slot: usize| {
        if cur.len() >= 2 && slot < N_TYPES {
            let pts = cur.iter().map(|p| [p[0], p[1]]).collect();
            let y = cur.iter().map(|p| p[2]).collect();
            out.by_type[slot].push(Chain::new(pts, y));
        }
        cur.clear();
    };

    for pl in &nav.polys {
        let mut cur: Vec<[f32; 3]> = Vec::new();
        let mut cur_slot = usize::MAX;
        for w in pl.windows(2) {
            let (a, b) = (w[0].id, w[1].id);
            let k = EdgeKey::new(a, b);
            if removed.contains(&k) {
                flush(&mut out, &mut cur, cur_slot);
                continue;
            }
            let slot = rt.types.get(&k).map_or(0, |t| t.index() as usize);
            out.edge_counts[slot] += 1;
            if slot == jump_slot {
                flush(&mut out, &mut cur, cur_slot);
                add_jump(&mut out, a, b, k);
                continue;
            }
            if slot != cur_slot {
                flush(&mut out, &mut cur, cur_slot);
                cur_slot = slot;
            }
            if cur.is_empty() {
                cur.push(pos[&a]);
            }
            cur.push(pos[&b]);
        }
        flush(&mut out, &mut cur, cur_slot);
    }
    for l in &rt.added {
        let (Some(&pa), Some(&pb)) = (pos.get(&l.a), pos.get(&l.b)) else { continue };
        let slot = l.ty.map_or(0, |t| t.index() as usize);
        out.edge_counts[slot] += 1;
        if slot == jump_slot {
            add_jump(&mut out, l.a, l.b, EdgeKey::new(l.a, l.b));
        } else {
            out.by_type[slot].push(Chain::new(vec![[pa[0], pa[1]], [pb[0], pb[1]]], vec![pa[2], pb[2]]));
        }
    }
    out
}

// ── cell grid (POIs) ─────────────────────────────────────────────────────────────────────────

/// Items bucketed by cell, to cull a point layer by a world box without scanning it.
#[derive(Clone, Debug, Default)]
pub struct CellGrid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl CellGrid {
    pub fn new(cell: f32) -> CellGrid {
        CellGrid { cell, cells: HashMap::new() }
    }
    fn key(&self, x: f32, z: f32) -> (i32, i32) {
        ((x / self.cell).floor() as i32, (z / self.cell).floor() as i32)
    }
    pub fn insert(&mut self, i: u32, x: f32, z: f32) {
        let k = self.key(x, z);
        self.cells.entry(k).or_default().push(i);
    }
    /// Indices in the cells that overlap `bbox` (a superset of those inside it).
    pub fn query(&self, bbox: &[f32; 4], mut f: impl FnMut(u32)) {
        let (a, b) = (self.key(bbox[0], bbox[1]), self.key(bbox[2], bbox[3]));
        // A box with more cells than the grid holds is cheaper to scan flat.
        let span = (b.0 - a.0 + 1) as i64 * (b.1 - a.1 + 1) as i64;
        if span > self.cells.len() as i64 * 2 {
            for (k, v) in &self.cells {
                if k.0 >= a.0 && k.0 <= b.0 && k.1 >= a.1 && k.1 <= b.1 {
                    v.iter().for_each(|&i| f(i));
                }
            }
            return;
        }
        for cx in a.0..=b.0 {
            for cz in a.1..=b.1 {
                if let Some(v) = self.cells.get(&(cx, cz)) {
                    v.iter().for_each(|&i| f(i));
                }
            }
        }
    }
}

// ── POIs ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct PoiLayer {
    pub items: Vec<Poi>,
    /// `style::POI_CATS` index per item ([`NO_CAT`] = kind the renderer has no category for).
    pub cat: Vec<u8>,
    pub grid: CellGrid,
    /// The treasure chests (`TreasureChest` + `TreasureChestBoard`), kept apart so the one that
    /// is current this week can be picked ([`PoiLayer::current_chest`]).
    pub chests: Pois,
}

pub const NO_CAT: u8 = u8::MAX;

impl PoiLayer {
    /// Off-map items (`|x|` or `|z|` beyond [`POI_LIMIT_M`], the parking areas at z ≈ 18 km)
    /// and kinds without a category are dropped.
    pub fn from_pois(pois: &Pois) -> PoiLayer {
        Self::from_items(pois.items.iter().cloned())
    }

    pub fn from_items(items: impl IntoIterator<Item = Poi>) -> PoiLayer {
        let by_kind: HashMap<PoiKind, u8> = super::style::POI_CATS.iter().enumerate().filter_map(|(i, c)| c.kind.map(|k| (k, i as u8))).collect();
        let mut l = PoiLayer { items: Vec::new(), cat: Vec::new(), grid: CellGrid::new(250.0), chests: Pois::default() };
        for p in items {
            if !(p.x.abs() <= POI_LIMIT_M && p.z.abs() <= POI_LIMIT_M) {
                continue;
            }
            if matches!(p.kind, PoiKind::TreasureChest | PoiKind::TreasureChestBoard) {
                l.chests.items.push(p.clone());
            }
            let Some(&cat) = by_kind.get(&p.kind) else { continue };
            l.grid.insert(l.items.len() as u32, p.x, p.z);
            l.cat.push(cat);
            l.items.push(p);
        }
        l
    }
}

impl PoiLayer {
    /// The treasure chest that is current in game week `week` (`week_index_now()`), by the
    /// inferred weekly rule of `Pois::current_treasure_chest` (unverified, see
    /// `docs/features/minimap.md`). Cheap (a few dozen chests), so the renderer asks every frame
    /// and picks up the Thursday 14:30 UTC rollover without any timer.
    pub fn current_chest(&self, week: i64) -> Option<&Poi> {
        self.chests.current_treasure_chest(week)
    }
}

// ── race lines ───────────────────────────────────────────────────────────────────────────────

/// Segment grid over all race lines: 100 m cells, entries `(line index, first point of the
/// segment)`. Built once; serves "the line the car is on" and "lines near the car".
#[derive(Clone, Debug, Default)]
pub struct SegGrid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<(u16, u32)>>,
}

impl SegGrid {
    pub fn build(lines: &[RaceLine], cell: f32) -> SegGrid {
        let mut g = SegGrid { cell, cells: HashMap::new() };
        for (li, l) in lines.iter().enumerate() {
            for s in 0..l.pts.len().saturating_sub(1) {
                let (a, b) = (l.pts[s], l.pts[s + 1]);
                let (c0, c1) = (g.key(a[0].min(b[0]), a[1].min(b[1])), g.key(a[0].max(b[0]), a[1].max(b[1])));
                for cx in c0.0..=c1.0 {
                    for cz in c0.1..=c1.1 {
                        g.cells.entry((cx, cz)).or_default().push((li as u16, s as u32));
                    }
                }
            }
        }
        g
    }
    fn key(&self, x: f32, z: f32) -> (i32, i32) {
        ((x / self.cell).floor() as i32, (z / self.cell).floor() as i32)
    }
    /// Segments in the cells within `r` metres of (x, z).
    pub fn near(&self, x: f32, z: f32, r: f32, mut f: impl FnMut(u16, u32)) {
        let (a, b) = (self.key(x - r, z - r), self.key(x + r, z + r));
        for cx in a.0..=b.0 {
            for cz in a.1..=b.1 {
                if let Some(v) = self.cells.get(&(cx, cz)) {
                    v.iter().for_each(|&(l, s)| f(l, s));
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RaceLayer {
    pub lines: Vec<RaceLine>,
    pub grid: SegGrid,
}

impl RaceLayer {
    pub fn new(lines: Vec<RaceLine>) -> RaceLayer {
        let grid = SegGrid::build(&lines, 100.0);
        RaceLayer { lines, grid }
    }
}

// ── everything ───────────────────────────────────────────────────────────────────────────────

/// What the maps share. Cheap to clone (three `Arc`s).
#[derive(Clone, Debug, Default)]
pub struct MapLayers {
    /// Increments on every rebuild (compare to know when to drop caches).
    pub rev: u64,
    pub roads: Arc<RoadLayer>,
    pub pois: Arc<PoiLayer>,
    pub races: Arc<RaceLayer>,
    /// The game's POI icons (CPU pixels, read once on the loader thread so both maps get the same
    /// data; each map uploads them into its own egui context, see `maprender::icontex`). `None`
    /// when they could not be read: the maps draw shape markers.
    pub icons: Option<Arc<PoiIcons>>,
    /// Race class per route id for the race pin icons: the user's race marks (`RoadTypes::races`)
    /// combined with circuit / sprint of the route's line. Routes without a mark are absent.
    pub race_class: Arc<HashMap<u32, RaceClass>>,
    /// Why the project road data is used instead of the user's saved file, if so
    /// (`roadtypes::Current::note`).
    pub note: Option<String>,
}

/// The race class a route's pin icon is drawn with, from the user's mark (`road`, `street`,
/// `rally`, `cross_country`, `touge`, `drag`; `story` / `wristband` have no class icon) and
/// whether the route is a circuit. The mapping `rally` -> mixed surface is a guess: the marks are
/// the user's own hand classification (`docs/game-data/fh6-map-tooling.md`), not the game's.
pub fn race_class_of(mark: &str, circuit: bool) -> Option<RaceClass> {
    let pick = |p2p, circ| Some(if circuit { circ } else { p2p });
    match mark {
        "road" => pick(RaceClass::AsphaltP2p, RaceClass::AsphaltCircuit),
        "rally" => pick(RaceClass::MixedsurfaceP2p, RaceClass::MixedsurfaceCircuit),
        "cross_country" => pick(RaceClass::CrosscountryP2p, RaceClass::CrosscountryCircuit),
        "street" => Some(RaceClass::Streetracing),
        "touge" => Some(RaceClass::Touge),
        "drag" => Some(RaceClass::Dragracing),
        _ => None,
    }
}

/// [`race_class_of`] for every marked route that has a race line.
fn race_classes(races: &RaceLayer, marks: &[(String, serde_json::Value)]) -> HashMap<u32, RaceClass> {
    let circuit: HashMap<u32, bool> = races.lines.iter().map(|l| (l.route, l.circuit)).collect();
    marks
        .iter()
        .filter_map(|(k, v)| {
            let route: u32 = k.parse().ok()?;
            Some((route, race_class_of(v.as_str()?, *circuit.get(&route)?)?))
        })
        .collect()
}

/// The parts that only change when the install does.
pub struct GameData {
    pub nav: Nav,
    pub pois: Arc<PoiLayer>,
    pub races: Arc<RaceLayer>,
    pub icons: Option<Arc<PoiIcons>>,
    /// Sources the readers skipped (a game update that moved a file costs one category); logged.
    pub skipped: Vec<String>,
}

impl GameData {
    /// Read nav, POIs and race lines from `<install>/media`. Only the nav is required; a POI /
    /// race-line failure leaves that layer empty and is listed in `skipped`.
    pub fn load(media: &Path) -> Result<GameData, String> {
        // The icons (~25 ms) and the danger signs (0.13-0.5 s, they live in the 40 GB GeoChunk0)
        // are read on their own threads next to the nav, so they add nothing to the first load.
        let (icons, danger, core) = std::thread::scope(|s| {
            let icons = s.spawn(|| PoiIcons::load(media));
            let danger = s.spawn(|| Pois::load_danger_signs(media));
            let core = Self::load_core(media);
            (icons.join(), danger.join(), core)
        });
        let (nav, mut pois, races, mut skipped) = core?;
        match danger {
            Ok(Ok(signs)) => pois.items.extend(signs),
            Ok(Err(e)) => skipped.push(format!("danger signs: {e}")),
            Err(_) => skipped.push("danger signs: the reader panicked".into()),
        }
        let icons = match icons {
            Ok(Ok(i)) => {
                skipped.extend(i.skipped.iter().map(|s| format!("icon {s}")));
                Some(Arc::new(i))
            }
            Ok(Err(e)) => {
                skipped.push(format!("POI icons: {e}"));
                None
            }
            Err(_) => {
                skipped.push("POI icons: the reader panicked".into());
                None
            }
        };
        Ok(GameData { nav, pois: Arc::new(PoiLayer::from_pois(&pois)), races: Arc::new(races), icons, skipped })
    }

    /// Nav, the POIs of `Pois::load` and the race lines, with what the readers skipped.
    #[allow(clippy::type_complexity)]
    fn load_core(media: &Path) -> Result<(Nav, Pois, RaceLayer, Vec<String>), String> {
        let nav = Nav::load(media)?;
        let mut skipped = Vec::new();
        let pois = match Pois::load(media) {
            Ok(p) => {
                skipped.extend(p.skipped.iter().cloned());
                p
            }
            Err(e) => {
                skipped.push(format!("POIs: {e}"));
                Pois::default()
            }
        };
        let races = match racelines::load_all(media, RACE_STEP_M) {
            Ok(r) => {
                skipped.extend(r.skipped.iter().cloned());
                RaceLayer::new(r.lines)
            }
            Err(e) => {
                skipped.push(format!("race lines: {e}"));
                RaceLayer::default()
            }
        };
        Ok((nav, pois, races, skipped))
    }

    /// Roads for the road-type data `cur` (the user's saved file or the project's).
    pub fn layers(&self, cur: &Current, rev: u64) -> MapLayers {
        MapLayers {
            rev,
            roads: Arc::new(build_roads(&self.nav, &cur.types)),
            pois: self.pois.clone(),
            races: self.races.clone(),
            icons: self.icons.clone(),
            race_class: Arc::new(race_classes(&self.races, &cur.types.races)),
            note: cur.note.clone(),
        }
    }
}

impl MapLayers {
    /// A small hand-made layer set around the world origin (a road cross of every type, a few
    /// POIs, one circuit): for the HUD's PNG harness and tests, no install needed.
    #[cfg(test)]
    pub fn synthetic() -> MapLayers {
        let mut roads = RoadLayer::default();
        let types = [
            RoadType::Road,
            RoadType::Highway,
            RoadType::Offroad,
            RoadType::Other,
            RoadType::Trail,
            RoadType::Crosscountry,
            RoadType::Tunnel,
        ];
        for (i, t) in types.iter().enumerate() {
            let z = -300.0 + 100.0 * i as f32;
            let pts = vec![[-600.0, z], [-200.0, z + 30.0], [200.0, z - 30.0], [600.0, z]];
            let y = vec![0.0; pts.len()];
            roads.by_type[t.index() as usize].push(Chain::new(pts, y));
            roads.edge_counts[t.index() as usize] += 3;
        }
        roads.jumps.push([-100.0, 450.0, 0.0, 100.0, 450.0, 0.0]);
        let poi = |kind, x, z| Poi { kind, x, z, y: 0.0, name: String::new(), n: 0, gate: None };
        let pois = PoiLayer::from_items([
            poi(PoiKind::House, -300.0, -100.0),
            poi(PoiKind::FastTravel, 300.0, 100.0),
            poi(PoiKind::CarMeet, 0.0, 250.0),
            poi(PoiKind::SpeedTrap, 450.0, -250.0),
            poi(PoiKind::BarnFind, -450.0, 250.0),
        ]);
        let ring: Vec<[f32; 2]> = (0..=64).map(|i| {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            [400.0 * a.cos(), 300.0 * a.sin()]
        }).collect();
        let n = ring.len();
        let line = RaceLine {
            route: 1,
            circuit: true,
            start: [ring[0][0], 0.0, ring[0][1]],
            finish: [ring[0][0], 0.0, ring[0][1]],
            y: vec![0.0; n],
            half: vec![[6.0, 0.0]; n],
            length_m: 1750.0,
            closed: true,
            n_sections: 1,
            bbox: [-400.0, -300.0, 400.0, 300.0],
            pts: ring,
        };
        MapLayers { rev: 1, roads: Arc::new(roads), pois: Arc::new(pois), races: Arc::new(RaceLayer::new(vec![line])), ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::nav::NavVert;
    use crate::gamedata::roadtypes::AddedLink;

    fn v(id: u32, x: f32, z: f32, y: f32) -> NavVert {
        NavVert { id, x, z, y }
    }

    fn nav(polys: Vec<Vec<NavVert>>) -> Nav {
        Nav { sha1: String::new(), nodes: 0, cls: vec![0; polys.len()], hi: vec![0; polys.len()], polys, orphans: vec![(900, 5.0, 5.0)] }
    }

    fn rt() -> RoadTypes {
        RoadTypes::raw()
    }

    #[test]
    fn build_roads_chains_same_type_runs_and_splits_on_type_change_and_removal() {
        // One polyline 1-2-3-4-5-6: road, road, highway, highway, road. Edge 3-4 is removed later.
        let n = nav(vec![vec![v(1, 0.0, 0.0, 10.0), v(2, 10.0, 0.0, 11.0), v(3, 20.0, 0.0, 12.0), v(4, 30.0, 0.0, 13.0), v(5, 40.0, 0.0, 14.0), v(6, 50.0, 0.0, 15.0)]]);
        let mut r = rt();
        for (a, b, t) in [(1, 2, RoadType::Road), (2, 3, RoadType::Road), (3, 4, RoadType::Highway), (4, 5, RoadType::Highway), (5, 6, RoadType::Road)] {
            r.types.insert(EdgeKey::new(a, b), t);
        }
        let l = build_roads(&n, &r);
        let road = RoadType::Road.index() as usize;
        let hw = RoadType::Highway.index() as usize;
        assert_eq!(l.by_type[road].len(), 2);
        assert_eq!(l.by_type[road][0].pts, vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]]);
        assert_eq!(l.by_type[road][0].y, vec![10.0, 11.0, 12.0]); // heights kept for phase K
        assert_eq!(l.by_type[road][0].bbox, [0.0, 0.0, 20.0, 0.0]);
        assert_eq!(l.by_type[hw].len(), 1);
        assert_eq!(l.by_type[hw][0].pts.len(), 3);
        assert_eq!((l.edge_counts[road], l.edge_counts[hw]), (3, 2));
        // Removing the highway's first edge starts the chain at node 4.
        r.removed.push(EdgeKey::new(3, 4));
        let l = build_roads(&n, &r);
        assert_eq!(l.by_type[hw].len(), 1);
        assert_eq!(l.by_type[hw][0].pts, vec![[30.0, 0.0], [40.0, 0.0]]);
        assert_eq!(l.edge_counts[hw], 1);
    }

    #[test]
    fn build_roads_untyped_edges_land_in_slot_zero() {
        let n = nav(vec![vec![v(1, 0.0, 0.0, 0.0), v(2, 10.0, 0.0, 0.0)]]);
        let l = build_roads(&n, &rt());
        assert_eq!(l.by_type[0].len(), 1);
        assert_eq!(l.edge_counts[0], 1);
        assert_eq!(l.chains(), 1);
        assert_eq!(l.vertices(), 2);
    }

    #[test]
    fn build_roads_moved_nodes_jumps_and_added_links() {
        let n = nav(vec![vec![v(1, 0.0, 0.0, 0.0), v(2, 10.0, 0.0, 0.0), v(3, 20.0, 0.0, 0.0)]]);
        let mut r = rt();
        r.types.insert(EdgeKey::new(1, 2), RoadType::Road);
        r.types.insert(EdgeKey::new(2, 3), RoadType::Jump);
        // Node 2 moved; the jump takes off at node 3 (not the polyline's first node of the edge).
        r.moved.insert(2, [10.0, 7.0, 3.0]);
        r.jump_from.push((EdgeKey::new(2, 3), 3));
        // A user point and two added links: a typed one and one landing on an unknown node.
        r.points.insert(1_000_001, [50.0, 50.0, 9.0]);
        r.added.push(AddedLink { a: 3, b: 1_000_001, ty: Some(RoadType::Crosscountry) });
        r.added.push(AddedLink { a: 3, b: 424_242, ty: Some(RoadType::Road) });
        let l = build_roads(&n, &r);
        let road = RoadType::Road.index() as usize;
        assert_eq!(l.by_type[road].len(), 1);
        assert_eq!(l.by_type[road][0].pts, vec![[0.0, 0.0], [10.0, 7.0]]); // moved node honoured
        assert_eq!(l.jumps, vec![[20.0, 0.0, 0.0, 10.0, 7.0, 3.0]]); // from node 3 to node 2
        let cc = RoadType::Crosscountry.index() as usize;
        assert_eq!(l.by_type[cc].len(), 1);
        assert_eq!(l.by_type[cc][0].pts, vec![[20.0, 0.0], [50.0, 50.0]]);
        assert_eq!(l.by_type[cc][0].y, vec![0.0, 9.0]);
        assert_eq!(l.by_type[road].len(), 1); // the link to the unknown node was skipped
        assert_eq!(l.edge_counts[cc], 1);
    }

    #[test]
    fn jump_without_jump_from_takes_off_at_the_first_node() {
        let n = nav(vec![vec![v(1, 0.0, 0.0, 5.0), v(2, 100.0, 0.0, 0.0)]]);
        let mut r = rt();
        r.types.insert(EdgeKey::new(1, 2), RoadType::Jump);
        let l = build_roads(&n, &r);
        assert_eq!(l.jumps, vec![[0.0, 0.0, 5.0, 100.0, 0.0, 0.0]]);
        assert_eq!(l.chains(), 0);
    }

    #[test]
    fn cell_grid_returns_a_superset_of_the_box() {
        let mut g = CellGrid::new(250.0);
        let pts = [(10.0, 10.0), (260.0, 10.0), (-300.0, -300.0), (5000.0, 5000.0), (600.0, -40.0)];
        for (i, p) in pts.iter().enumerate() {
            g.insert(i as u32, p.0, p.1);
        }
        let mut got = Vec::new();
        g.query(&[0.0, 0.0, 300.0, 100.0], |i| got.push(i));
        got.sort();
        assert_eq!(got, vec![0, 1]);
        let mut all = Vec::new();
        g.query(&[-1e6, -1e6, 1e6, 1e6], |i| all.push(i)); // flat-scan path
        all.sort();
        assert_eq!(all, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn poi_layer_drops_off_map_and_unmapped_kinds() {
        let p = |kind, x, z| Poi { kind, x, z, y: 0.0, name: String::new(), n: 0, gate: None };
        let l = PoiLayer::from_items([
            p(PoiKind::House, 100.0, 200.0),
            p(PoiKind::Parking, 3000.0, 18_300.0), // off the map
            p(PoiKind::Parking, 3000.0, 4000.0),
            p(PoiKind::House, f32::NAN, 0.0),
        ]);
        assert_eq!(l.items.len(), 2);
        assert!(l.cat.iter().all(|&c| c != NO_CAT));
        let mut got = 0;
        l.grid.query(&[-1e4, -1e4, 1e4, 1e4], |_| got += 1);
        assert_eq!(got, 2);
    }

    #[test]
    fn danger_signs_have_a_category_and_chests_are_kept_for_the_week_pick() {
        let p = |kind, name: &str, x| Poi { kind, x, z: 0.0, y: 0.0, name: name.into(), n: 0, gate: None };
        let l = PoiLayer::from_items([
            p(PoiKind::DangerSign, "bm_01", 10.0),
            p(PoiKind::TreasureChest, "DISCOUNT_BOARD_TREASURE_CHEST_015", -50.0),
            p(PoiKind::TreasureChestBoard, "treasure_chest_board_016", 50.0),
            p(PoiKind::TreasureChest, "DISCOUNT_BOARD_TREASURE_CHEST_017", 99_999.0), // off the map
        ]);
        let cats: Vec<&str> = l.cat.iter().map(|&c| super::super::style::POI_CATS[c as usize].id).collect();
        assert_eq!(cats, ["danger_sign", "treasure_chest", "treasure_chest_board"]);
        assert_eq!(l.chests.items.len(), 2);
        // chest number = week - 53; the board wins a tie, a week past the newest chest keeps the newest.
        assert_eq!(l.current_chest(68).map(|c| c.x), Some(-50.0));
        assert_eq!(l.current_chest(69).map(|c| c.x), Some(50.0));
        assert_eq!(l.current_chest(80).map(|c| c.x), Some(50.0));
        assert!(l.current_chest(10).is_none());
        // The weekly rollover: the Thursday 14:30 UTC the chest numbers move on (week 68 -> 69).
        use crate::gamedata::poi::{week_index_at, WEEK_EPOCH};
        let thu = WEEK_EPOCH + 69 * 604_800;
        assert_eq!((week_index_at(thu - 1), week_index_at(thu)), (68, 69));
        assert_eq!((l.current_chest(week_index_at(thu - 1)).map(|c| c.x), l.current_chest(week_index_at(thu)).map(|c| c.x)), (Some(-50.0), Some(50.0)));
    }

    #[test]
    fn race_class_follows_the_users_mark_and_the_routes_shape() {
        use RaceClass::*;
        assert_eq!(race_class_of("road", false), Some(AsphaltP2p));
        assert_eq!(race_class_of("road", true), Some(AsphaltCircuit));
        assert_eq!(race_class_of("rally", true), Some(MixedsurfaceCircuit));
        assert_eq!(race_class_of("cross_country", false), Some(CrosscountryP2p));
        assert_eq!((race_class_of("street", true), race_class_of("touge", false), race_class_of("drag", false)), (Some(Streetracing), Some(Touge), Some(Dragracing)));
        assert_eq!((race_class_of("story", false), race_class_of("wristband", true), race_class_of("", true)), (None, None, None));
        // Per route: marks of routes without a line, unmarked routes and non-string marks are absent.
        let layers = MapLayers::synthetic(); // one circuit, route 1
        let marks = vec![
            ("1".to_string(), serde_json::json!("road")),
            ("2".to_string(), serde_json::json!("road")),
            ("x".to_string(), serde_json::json!("road")),
            ("1000".to_string(), serde_json::json!(5)),
        ];
        assert_eq!(race_classes(&layers.races, &marks), HashMap::from([(1, AsphaltCircuit)]));
    }

    /// The numbers of the design scout on the real install, with the project road-type data
    /// (a user's saved file would change them, so it is not consulted).
    #[test]
    fn real_install_layers() {
        let Some(media) = crate::gamedata::install::find_media(None) else {
            eprintln!("SKIP real_install_layers: FH6 install not found");
            return;
        };
        let g = GameData::load(&media).expect("game data");
        let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), Path::new("/nonexistent/road-types.json"), &g.nav);
        let l = g.layers(&cur, 1);
        let by = |t: RoadType| l.roads.edge_counts[t.index() as usize];
        assert_eq!(
            [by(RoadType::Road), by(RoadType::Offroad), by(RoadType::Other), by(RoadType::Trail), by(RoadType::Crosscountry), by(RoadType::Tunnel), by(RoadType::Highway), by(RoadType::Turnaround), by(RoadType::Jump)],
            [21_701, 7_237, 317, 3_943, 144, 904, 4_996, 340, 18]
        );
        assert_eq!(l.roads.edge_counts[0], 0, "untyped edges");
        assert_eq!(l.roads.jumps.len(), 18);
        assert_eq!(l.roads.chains(), 1798);
        assert!(l.roads.vertices() > 40_000);
        // Every vertex is finite and on the map.
        for c in l.roads.by_type.iter().flatten() {
            assert!(c.pts.len() == c.y.len() && c.pts.iter().all(|p| p[0].abs() < 12_000.0 && p[1].abs() < 12_000.0));
        }
        // POIs: the off-map parking areas (z ≈ 18 km) are gone, the rest is there.
        assert!(l.pois.items.len() > 4000, "{}", l.pois.items.len());
        assert!(l.pois.items.iter().all(|p| p.x.abs() <= POI_LIMIT_M && p.z.abs() <= POI_LIMIT_M));
        let parking = l.pois.items.iter().filter(|p| p.kind == PoiKind::Parking).count();
        assert_eq!(parking, 2664 - 11);
        // The danger signs of the GeoChunk and the game's icons came with the load.
        assert_eq!(l.pois.items.iter().filter(|p| p.kind == PoiKind::DangerSign).count(), 15);
        let icons = l.icons.as_ref().expect("POI icons");
        assert!(icons.uv.contains_key(&PoiKind::DangerSign) && icons.uv.contains_key(&PoiKind::TreasureChest));
        assert!(l.pois.current_chest(crate::gamedata::poi::week_index_now()).is_some(), "a current chest this week");
        // Race lines: 170 routes, 43 circuits, ~171 k points at 5 m.
        assert_eq!(l.races.lines.len(), 170);
        assert_eq!(l.races.lines.iter().filter(|r| r.circuit).count(), 43);
        let pts: usize = l.races.lines.iter().map(|r| r.pts.len()).sum();
        assert!((pts as i64 - 171_564).abs() < 200, "{pts}");
        // The grid answers: the first race line's own start is found next to itself.
        let r0 = &l.races.lines[0];
        let mut hit = false;
        l.races.grid.near(r0.pts[0][0], r0.pts[0][1], 5.0, |li, _| hit |= li == 0);
        assert!(hit);
    }

    /// Release-mode numbers for the docs: `cargo test --release bench_ -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_build_on_the_real_install() {
        let Some(media) = crate::gamedata::install::find_media(None) else { return };
        let t = std::time::Instant::now();
        let g = GameData::load(&media).expect("game data");
        let t_game = t.elapsed();
        let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), Path::new("/nonexistent"), &g.nav);
        let t = std::time::Instant::now();
        let l = g.layers(&cur, 1);
        eprintln!("GameData::load {t_game:?}; roads rebuild {:?} ({} chains, {} vertices)", t.elapsed(), l.roads.chains(), l.roads.vertices());
    }

    #[test]
    fn synthetic_layers_are_consistent() {
        let m = MapLayers::synthetic();
        assert!(m.roads.chains() >= 7 && !m.roads.jumps.is_empty());
        assert_eq!(m.pois.items.len(), 5);
        assert_eq!(m.races.lines.len(), 1);
        let mut hit = 0;
        m.races.grid.near(400.0, 0.0, 10.0, |_, _| hit += 1);
        assert!(hit > 0);
    }
}
