# Navigation

Route from the car to a clicked destination over the game's road network (phase L, decisions
D83-D85, D92). This page documents what exists; sections marked **(not built yet)** are filled
in by the later tasks (runtime L2, drawing L3, tab L5, co-op L4a).

Decisions the user settled (do not re-open): filters **Road, Highway, Dirt (= offroad), Trail,
Cross-country, Jumps** and one slider **faster roads <-> more curves** (D83); the route goes from
the car to a clicked destination, road speeds are *assumed per type* (no real speed data), the ETA
uses the same speeds (D84); jump lines are one-way take-off -> landing (D41); Tunnel counts as
asphalt (D37); D92 defaults: Road / Highway / Dirt on, Trail / Cross-country / Jumps off,
turnarounds never routable, Other and unset follow Road, **no road is one-way except jumps**.

## 1. Routing core (`src/nav/`, L1)

Pure CPU, no egui / GL / Wayland: compiles on every platform and is called from a worker thread.
*Why a top-level module and not under `maprender/`:* the router is not a renderer, it gets its own
runtime (worker thread, listener hook); `maprender` only consumes its result.

| File | What |
|---|---|
| `cfg.rs` | `RouteFilters` (the six checkboxes, `to_bits` / `from_bits`), `RoutePrefs { filters, curves }`, `NavConfig` (the persisted settings, `AppConfig.nav` comes with L5) |
| `graph.rs` | `RouteGraph::build(nav, road_types, node_positions)`, `Edge`, the snap grid |
| `cost.rs` | assumed speeds, `CostModel` (cost per arc, A* heuristic factor) |
| `snap.rs` | `RouteGraph::snap`, `Snap` |
| `search.rs` | `RouteGraph::plan` / `route`, `Route`, `RouteError`, `Endpoint` |
| `tests.rs` | synthetic-graph tests + the real-install test (skipped without an install) |

### Public API

```rust
// graph
RouteGraph::build(nav: &Nav, rt: &RoadTypes, pos: &HashMap<u32,[f32;3]>) -> RouteGraph   // Default = empty
g.node_count() / edge_count() / is_empty(); g.node_pos(n) -> [x,z,y]; g.node_id(n); g.node_index(id)
// snapping (a world position -> a point on an allowed edge)
g.snap(x, z, y: Option<f32>, filters: &RouteFilters) -> Option<Snap>      // Snap { edge, t, pos:[x,z,y], dist_m }
// search; both are Send/Sync-safe reads of an immutable graph
g.plan(car: (x, z, Option<y>), dest: (x, z), prefs: &RoutePrefs) -> Result<Route, RouteError>
g.route(from: &Snap, to: &Snap, prefs: &RoutePrefs) -> Result<Route, RouteError>
// result
Route { pts: Vec<[f32;2]>, y: Vec<f32>, seg_kind: Vec<u8>, seg_len: Vec<f32>, seg_time_s: Vec<f32>,
        dist_m, eta_s, cost }
RouteError::{ EmptyGraph, NoRoadNear(Endpoint::{Car, Destination}), Unreachable }   // Display = the tab's status text
```

The graph is built in `GameData::layers` (`maprender/data.rs`) next to the drawn roads and shared as
`MapLayers::route_graph: Arc<RouteGraph>`. Both are built from **one** `node_positions(nav, rt)`
table (nav vertices, orphans, user `points`, then `moved` overrides), so what is drawn and what is
routable cannot drift apart. *Why there:* `maprender::store` is keyed on install + override
mtime/rev, so an editor Save rebuilds the graph with no second cache and no second file watcher,
and `MapLayers::rev` tells the router when to recompute.

### Graph

* Built **by node id**, not by coordinates: polylines meet at shared ids (junctions). Nodes =
  every id with a position (38 597 on the project data: 38 473 nav + 124 user points); edges =
  each polyline's consecutive pairs minus `removed` (a pair in two polylines is one edge) plus the
  `added` links between known points (39 600 edges).
* `Edge { a, b, len, kind, curv }`: `len` is 3D when both heights are known (`mesh3d::known_y`
  rule, 0 = unknown), else 2D; `kind` = `RoadType::index` (0 = unset).
* **Directedness:** every edge has two arcs (`edge << 1 | dir`, CSR adjacency), except a Jump:
  exactly one arc, take-off -> landing (take-off = `jump_from[edge]` if it is an endpoint, else
  the polyline pair's first node / the link's `a`, the rule of the drawn jump line). Arcs carry no
  other data, so decoding the game's one-way flags later is "drop one arc", not a format change.
* **Curvature** `curv` = `(turn(i-1,i,i+1) + turn(i,i+1,i+2)) / 2 / len` for the edge at polyline
  index `i`; turns only inside the same polyline, polyline ends and added links get 0.
  *Why:* it is the road's own winding, independent of which route uses it, so the cost needs no
  per-route state (an edge-based search over directed edges would be needed otherwise).
* The snap grid (128 m cells) holds every edge except turnarounds and jumps.

### Filters (`RouteFilters::allows(kind)`)

| Edge kind | Allowed when | Why |
|---|---|---|
| Road | `road` | |
| Highway | `highway` | |
| Offroad | `dirt` | UI label "Dirt" |
| Trail | `trail` | |
| Cross-country | `cross_country` | |
| Jump | `jumps`, take-off -> landing only | D41 |
| Tunnel | `road` **or** `highway` | D37: counts as asphalt; the data does not say which kind of road the tunnel is on, so either switch lets it through |
| Other | `road` | "not classifiable" (317 edges, 6 km); treated as paved |
| Unset | `road` | a raw / partly typed override has every edge unset and must still route; unset is never a reason to exclude |
| Turnaround | never | a U-turn crossover between the carriageways of a divided road; with no one-way data both carriageways are already usable both ways, so a crossover adds nothing and would draw routes across the median (connectivity is unchanged without them: 27 547 vs 27 550 nodes). They stay in the data for a future "U-turns" rule once one-way is decoded |

**Wire form** (co-op `Dest.f`, D85): `RouteFilters::to_bits()` / `from_bits(u8)`: road 1, highway 2,
dirt 4, trail 8, cross-country 16, jumps 32, mask `0x3F` (bits 6-7 reserved, ignored on read). The
constants live in `nav/cfg.rs` (`BIT_ROAD` ... `BITS_MASK`) and are pinned by a test to the values
`coop.rs` defines (`DEST_*`); `nav` does not import `coop`. The slider travels as `c: f32`,
`RoutePrefs::curves()` clamps it to 0..1 (NaN = 0).

### Cost model (`cost.rs`)

Per metre of an arc (`s` = slider, 0 = fastest):

```
cost_per_m(kind, curv, s) = ((1-s)/speed[kind] + s/V_REF) * (1 - BETA * s * min(curv/KAPPA, 1))
```

`V_REF` = 22 m/s, `KAPPA` = 0.02 rad/m (about the 95th percentile of the island's edges),
`BETA` = 0.5. Assumed speeds (m/s): highway 40 (144 km/h), tunnel 25, road 22 (79 km/h), jump 30,
dirt 17, cross-country 12, trail 11, other / unset 16.

*Why these speeds:* no real speed data exists (D84), so plausible driving speeds that rank the types
sensibly; the ETA uses the same numbers and is shown as approximate ("~9 min").
*Why two ingredients for the slider:* (a) the speed term blends towards one uniform speed so
highways stop being attractive, (b) a bonus up to 50 % for winding edges. A curvature bonus alone
changed almost nothing (mean curvature 2.58 -> 2.78 mrad/m at `BETA` 0.6) because highways dominate
the time. Measured by the scout on 40 random pairs 4+ km apart (road+highway+tunnel+other+dirt):
`s` 0 / 0.5 / 1 gives 12.7 / 11.7 / 11.6 km, 8.1 / 8.3 / 8.8 min, mean curvature 2.4 / 2.8 / 3.1
mrad/m, highway share 30 / 19 / 9 %: at the curvy end +30 % winding for +8 % time. `BETA` is the
one tuning constant (0.8 gave 3.21 mrad/m at 8.74 min); a `const`, not UI.

### Search (`search.rs`)

* **Start and destination are edge points, not nodes:** the search is seeded with both endpoints of
  the snapped start edge at the cost of the partial edge, and ends at either endpoint of the
  destination edge plus its partial edge. The polyline starts / ends at the projection points.
  Exact for the 20 m edges and still right for the long added links (max 762 m). If both points are
  on one edge the direct piece is compared against the best detour (a long link may lose to one).
* **A\*** over node indices (binary heap, lazy deletion). Heuristic = straight 2D distance times
  `CostModel::heuristic`'s factor `((1-s)/vmax + s/V_REF) * (1 - BETA*s)`, with `vmax` the fastest
  *enabled* kind. Admissible and consistent: every arc costs at least its 2D chord times that
  factor (3D length >= 2D chord, speed <= vmax, curvature factor >= `1 - BETA*s` > 0). Dijkstra is
  the same code with `h = 0` (`search(..., astar = false)`, tests only); a test checks A* cost ==
  Dijkstra cost on ~390 random pairs over random filters and slider values.
* Two n-sized vectors are allocated per query (~0.3 MB); a reusable scratch is the next step if a
  profile ever asks for it. Measured numbers below.
* `Route` carries per-segment kind / length / time so the follower (L2) can compute the remaining
  ETA, and the drawing (L3) can split at jumps and tunnels; `y` per point is 0.0 when unknown.

### Snapping (`snap.rs`)

* Candidates: allowed edges (so a click beside a trail with trails off snaps to the road) within
  120 m, growing to 300 m when there are none; jumps and turnarounds are never targets. Beyond
  300 m: `NoRoadNear`. The caller keeps the clicked coordinates for the pin; only the route end is
  snapped.
* Score = horizontal distance + 3 * max(0, |dy| - 4) when the car's height and the edge's height at
  the projection are both known. *Why:* the racesel rule (known-height + gap) applied to overpass /
  tunnel / stacked-expressway ambiguity (Tokyo has up to 10 layers). A destination has no height
  (a 2D click has none; a click on 3D terrain gets the terrain's, which differs from a road under
  a hill or on a bridge): distance only. Ties: lowest horizontal distance.
* `ponytail:` snapping does not look at connectivity: a click next to a tiny disconnected island
  (e.g. 51-node fragments of the road-only network) is `Unreachable`, not re-snapped to the main
  network.

### Measured (project road-type data, FH6 install of 2026-10)

Graph build (`node_positions` + `RouteGraph::build`, on the `map-layers` loader thread): **~30 ms
release** (2 + 28 ms), ~140 ms debug. One route across the island (node 1 to the node farthest
away, 11.5 km straight, 21 km driven, default filters): **1.7 ms release, 11 ms debug**, so no
precomputation or contraction hierarchies are needed. The real-install test (`real_install_graph`) pins graph counts
(38 597 nodes / 39 600 edges), the per-type edge counts, connectivity (all types but turnaround:
38 594 nodes in one component; road+tunnel only: 21 719; road+tunnel+highway: 27 251), the 18 jumps
(each: one arc, out of the take-off only; take-off -> landing routable) and a timed route across
the island. It skips without an install; set `FH6_INSTALL_DIR` to the game folder.

## 2. Runtime: progress, re-route, arrival **(not built yet, L2)**

## 3. Drawing on the maps **(not built yet, L3)**

## 4. Navigation tab and Viewer destination **(not built yet, L5)**

## 5. Co-op shared destination **(wire format in L4a; adoption in L2)**

## Known limits

* **One-way roads are undecoded** (the nav file has `oneway_forward` / `deadend` / `give_way`
  property names but no decoded values, `game-data/fh6-game-files.md`): on divided highways the
  router may use the "wrong" carriageway. Accepted for v1 (D92).
* Speeds are assumed per type; the ETA is approximate.
