# Navigation

Route from the car to a clicked destination over the game's road network (phase L, decisions
D83-D85, D92). This page documents what exists; sections marked **(not built yet)** are filled
in by the later tasks (drawing L3, tab L5).

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

## 2. Runtime: progress, re-route, arrival (`src/nav/follow.rs`, `src/nav/state.rs`, L2)

Keeps a live route from the car to the destination: computed off-thread, followed per packet,
re-routed when the driver leaves it, hidden in races, following a co-op shared destination.
No drawing and no UI here: readers call `nav::view()`, the UI calls the setters.

### Threads and data flow

```
 UI thread     --set_destination / set_prefs / set_follow_shared--> Shared.inputs (+ atomic inputs_seq)
 listener      --Tracker::tick(now, sample)-----------------------> (progress, debounce, race, publish)
 listener      --Tracker::set_shared(coop dest)  (every 100 ms: destination_seq compare)
 Tracker       --Request--> `nav-route` thread --Reply--> Tracker      (stale generation dropped)
 any thread    <-- nav::view() = Shared.view (one lock + a clone)
```

* **The tracker runs on the listener thread** (`listeners/worker.rs:run`, once per loop iteration
  after the packet match, so it also runs on the 200 ms idle timeout), **not** on the UI thread.
  *Why:* the UI frame loop stops while the game covers the window, which is exactly when the HUD
  minimap is the only map in use; this is the same reason `coop.push_local` lives there. The result
  is a **process-global** (`nav::view()`), like `maprender::store`, not a `HudSnapshot` field.
* **The route is computed on the `nav-route` thread** (`ThreadPlanner`; started by the first
  destination, Condvar-woken, one request at a time, a newer request replaces one not started yet).
  *Why not on the listener thread:* the packet loop sends the gearbox's key presses and must never
  block (2-6 ms per route, more for an unreachable query over the whole island, and the graph may
  still be loading).
* **The graph** comes from `maprender::store::layers().data.route_graph` via `StoreSource`, called
  on the `nav-route` thread as soon as a destination exists (HUD-only use, no map layer on): that
  loads the layers at startup when a saved destination exists. The store's doc comment says so; the
  call costs the UI nothing (own `map-layers` loader thread, one short lock). While the graph is
  still `Loading` the worker retries every 250 ms; no install / failed load is the status
  `NoRoadData`. While a destination exists the worker looks at the store once a second; a different
  graph `Arc` (editor Save, season change, install found / gone) re-routes (`graph_changed`).
* **Per-packet cost:** with no destination one atomic load (about 5 ns, release); following a 20 km
  route about 0.8 us (windowed scan of 83 segments), plus the view lock only when something a reader
  sees changed (remaining distance at 1 m, ETA at 1 s, status, line). Measured by the ignored test
  `tick_cost` (`cargo test --release tick_cost -- --nocapture --ignored`).

### Inputs (the API the UI calls)

`nav::set_destination(Option<[x, z]>)` (this player's own), `nav::set_prefs(RoutePrefs)` (their
filters + slider), `nav::set_follow_shared(bool)`, `nav::local_destination()`. All idempotent (the
same value again is not a change; the tab may call them every frame). The shared destination is
not set by the UI: the listener thread polls `CoopReader::destination_seq()` every 100 ms and hands
a `SharedIn` to `Tracker::set_shared`. Persisting the destination is the config's business
(`NavConfig.destination`); at startup the app calls `set_destination(cfg.nav.destination)` once.

### `nav::view()` -> `NavView`

`{ rev, status, dest: Option<Dest>, line: Option<Arc<NavLine>>, remaining_m, total_m, eta_s,
local_cleared_seq }`. `NavLine { rev, pts, y, seg_kind }` is the part still to drive (`seg_kind` 7 =
jump). `Dest { x, z, source: Local | Shared { setter, hue }, prefs }`. `NavStatus`: `Idle`,
`WaitingForCar` (destination, no driving packet yet), `Routing` (a previous line may still be shown),
`Ok`, `NoRoadNear(Car | Destination)`, `Unreachable`, `NoRoadData`, `PausedRace`, `Arrived`.
`NavView::rev` changes on any published change; `NavLine::rev` only when the line itself is new
(the key for the 3D deck mesh). `local_cleared_seq` increments when a local destination was cleared
by arriving: the UI clears `AppConfig.nav.destination` when it sees it change.

### Rules

| Rule | Value | Why |
|---|---|---|
| Debounce of a destination / prefs change | 200 ms | a slider drag or two quick clicks make one request |
| Spacing of *automatic* requests (off route, graph replaced, retry) | >= 3 s | no storm if the car keeps leaving the route; a user change is not held back |
| Off route | > 50 m for >= 2 s of **packet time** | `dt_ms` summed over driving packets, each capped at 250 ms by the worker: a stalled loop or the first packet after a pause is one capped step, a paused game counts nothing |
| After a route is adopted | off-route check suppressed 3 s | the new route starts at the car's snap |
| Route starts far from the car (car in a field, snap up to 300 m) | off-route limit is 50 m + that gap until the car was first within 50 m of the route | otherwise a car 120 m from the road is "off route" at once and re-routes every 3 s forever |
| `NoRoadNear(Car)` / `Unreachable` | retried every >= 3 s **while the car has moved >= 25 m** since the request | fast travel into a field recovers, a parked car does not poll; `NoRoadNear(Destination)` / `NoRoadData` are only retried on a changed input or graph |
| Progress | windowed scan (2 back, 80 ahead), global scan if the window's nearest is > 50 m | a route passing near itself (loop, overpass) cannot make the car jump to the other pass; a shortcut / U-turn within 50 m of the route is "on route" |
| Along-route distance | monotone inside the window | a reversing car does not add distance back; only the global fallback may move it backwards |
| Remaining ETA | sum of the remaining `seg_time_s` | same assumed speeds as the route (approximate by design, D84) |
| Arrival | remaining < 25 m **and** car within 40 m of the route's end | the end is the snapped destination (it can be up to 300 m from the clicked point) |
| Drawn line | trimmed in 150 m chunks; new `Arc<NavLine>` (new `rev`) per chunk and per new route | the 3D deck mesh is rebuilt when the `Arc` changes (the `gl3d::scene::sync_race` pattern, "a few ms, once per change, never per frame"); trimming per packet would rebuild it 70x a second. 2D and 3D agree to within 150 m of tail |
| Race (race position != 0) | `PausedRace`, line hidden, nothing requested; on the way out a new route from wherever the race ended (D92) | the race is already a road-styled line in the same slot (D80), a second coloured road on top fights it, and the destination is irrelevant mid-race. A destination change during a race is held until it ends |
| Prefs-only change | old line stays (and is followed) until the new route arrives | no blinking while the slider is dragged; a new destination drops the line at once |
| Arrival, local | destination cleared (inputs + `local_cleared_seq`), `Arrived` until the next destination | D84: no prompt; this is where a later HUD notification hooks in |
| Arrival, shared | only marked arrived locally (by the shared destination's `ts`), `Arrived` with `dest` still the room's; the same destination re-sent (late-joiner resend) does not bring it back, a new one does; if a local destination exists it takes over | the destination stays for the other players; this player's route is hidden until a new one arrives |

### Tests

`follow.rs` (pure): progress / monotone, window vs global scan, the 50 m / 2 s rule incl. the 3 s
suppression and hysteresis, capped steps, paused packets, far-start gap, arrival, degenerate route,
150 m chunks. `state.rs` (a recording `FakePlanner`, explicit clock): idle asks for nothing,
waits for a car, debounce, stale reply dropped, 150 m republish with no rev per packet, off-route
re-route, 3 s spacing, `NoRoadNear(Car)` retries, error -> status, race pause / resume / in-flight
reply dropped, paused packets, local arrival, shared overrides local / falls back / opt-out,
shared arrival, prefs-only change. Plus `ThreadPlanner` with a scripted graph source (waits for a
loading graph, missing install, replaced graph, latest request wins). Real install: the L1 test
`real_install_graph` (skips without `FH6_INSTALL_DIR`). The ignored `tick_cost` prints the per-packet cost.


## 3. Drawing on the maps **(not built yet, L3)**

## 4. Navigation tab and Viewer destination **(not built yet, L5)**

## 5. Co-op shared destination (wire format in L4a; adoption in L2)

The wire format is `Control::Dest` (`docs/features/coop.md`). The receiving half lives in the
tracker: the listener thread polls `CoopReader::destination_seq()` every 100 ms (a compare under
the co-op mutex that `push_local` takes per packet anyway), and on a change hands
`destination()` as a `SharedIn` to `Tracker::set_shared` (`nav` itself does not import `coop`).

* A shared destination **overrides the local one** while `follow_shared` is on and this player has
  not arrived at it; it is routed with the **setter's** filters (`RouteFilters::from_bits(f)`,
  unknown bits ignored) and curve, so a group routes over the same roads. *Why:* D85, an incoming
  destination replaces a local one (last write wins, one slot), and no route geometry travels.
* The local destination and the user's own filters are untouched and apply again when the shared
  one is cleared (or `follow_shared` is switched off); there is no stack.
* Identity of a shared destination is the setter's `ts`: a resend of the same one (late joiner)
  changes nothing, even after the arrival.
* The setter's display name / hue are in `NavView::dest.source` for the tab and the pin ring.

## Known limits

* **One-way roads are undecoded** (the nav file has `oneway_forward` / `deadend` / `give_way`
  property names but no decoded values, `game-data/fh6-game-files.md`): on divided highways the
  router may use the "wrong" carriageway. Accepted for v1 (D92).
* Speeds are assumed per type; the ETA is approximate.
