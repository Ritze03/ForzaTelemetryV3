# Navigation

Route from the car to a clicked destination over the game's road network (phase L, decisions
D83-D85, D92). This page documents what exists.

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
| `cfg.rs` | `RouteFilters` (the six checkboxes, `to_bits` / `from_bits`), `RoutePrefs { filters, curves }`, `NavConfig` (the persisted settings, `AppConfig.nav`) |
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
cost_per_m(kind, curv, s) = ((1-s)/speed[kind] + s/V_REF)
                          * (kind == Highway ? (1-s)*HW_FAST + s*HW_AVOID : 1)
                          * (1 - BETA * s * min(curv/KAPPA, 1))
```

`V_REF` = 22 m/s, `KAPPA` = 0.01 rad/m (about the 80th percentile of the island's road and dirt
edges), `BETA` = 0.9, `HW_FAST` = 0.4, `HW_AVOID` = 1.5. Assumed speeds (m/s): highway 40
(144 km/h), tunnel 25, road 22 (79 km/h), jump 30, dirt 17, cross-country 12, trail 11, other /
unset 16. **This is the search objective only:** `eta_s` is plain length / assumed speed, so the
preference factors never leak into the time shown (a test pins it).

*Why these speeds:* no real speed data exists (D84), so plausible driving speeds that rank the types
sensibly; the ETA uses the same numbers and is shown as approximate ("~9 min").

*Why three ingredients (D96, retuned 2026-10-10):* the user: "The highway is pretty much always the
fastest mean of travel. For the PREFERENCE 'Faster Roads', it should be (at least almost) always
used. More Curves isn't aggressive enough, since it still kinda avoids some Touge routes."

* **(a) Speed blend** (unchanged): towards one uniform speed, so at `s = 1` a highway is no faster.
* **(b) Highway factor** `(1-s)*HW_FAST + s*HW_AVOID`. *Why:* by assumed speed alone a highway beats
  a local road only 1.8x per metre, so a somewhat shorter local road often won and the route skipped
  the highway. `HW_FAST` = 0.4 makes a highway metre 2.5x cheaper than its speed says (a
  *preference*, not a speed: the ETA stays honest, which is why the shown ETA of an `s = 0` route is
  now a little above the true optimum). `HW_AVOID` = 1.5 makes it 50 % dearer than any other
  straight road at `s = 1`, so the curvy end does not pick up highway on trips the bonus left
  undecided. **No short-trip threshold:** the factor is multiplicative, so a highway is taken only
  while the road-equivalent length of the highway route (ramps full price, highway metres at 0.22)
  beats the local alternative; a trip with no highway nearby cannot detour absurdly to reach one
  (short trips of 0.3-2 km: 2.65 -> 2.72 km mean route, 14.0 -> 14.6 % highway).
* **(c) Winding bonus** up to 90 % (was 50 %) from 10 mrad/m (was 20) up. *Why:* a hairpin edge is
  15-50 mrad/m (p90 15.6, p95 23.9, p99 47.6; p75 6.8), so the old 50 % at 20 mrad/m left a 2x
  shorter straight road competitive with a hairpin pass.

**Measured** (`cargo test --release pref_eval -- --ignored --nocapture`, FH6 install of 2026-10,
project road-type data, default filters, 80 fixed-seed node pairs 3-25 km apart; "of near-800 / near-1500"
= trips with a highway edge within 800 m / 1.5 km of both ends that drive >= 500 m of highway; 14 and
36 such trips; `wind` = summed heading change per metre of the route polyline, `tight` = share of its
length turning faster than 20 mrad/m):

| `s` | km before -> after | ETA min | highway share | hw used, near-800 | near-1500 | wind mrad/m | tight % |
|---|---|---|---|---|---|---|---|
| 0 | 12.74 -> 14.83 | 8.31 -> 8.82 | 27.8 -> 40.9 % | 12 -> 13 of 14 | 24 -> 27 of 36 | 3.68 -> 3.43 | 3.5 -> 3.2 |
| 0.25 | 12.05 -> 12.87 | 8.41 -> 8.32 | 18.6 -> 29.2 % | 12 -> 12 | 22 -> 25 | 3.95 -> 3.71 | 3.7 -> 3.6 |
| 0.5 | 11.81 -> 11.79 | 8.57 -> 8.65 | 14.1 -> 13.5 % | 11 -> 10 | 19 -> 18 | 4.20 -> 4.45 | 4.1 -> 4.4 |
| 0.75 | 11.73 -> 11.79 | 8.70 -> 9.07 | 12.2 -> 7.4 % | 10 -> 8 | 18 -> 15 | 4.38 -> 4.81 | 4.4 -> 5.0 |
| 1 | 11.75 -> 11.99 | 9.03 -> 9.47 | 8.3 -> 4.7 % | 9 -> 8 | 16 -> 14 | 4.70 -> 5.08 | 4.9 -> 5.3 |

So at `s = 0` the highway share rises by a third and 13 of 14 trips with a highway at both ends use it
(the remaining one has no highway leading its way); at `s = 1` the route winds +48 % more
than `s = 0` (was +28 %) for +9 % ETA (the island crossing, node 1 to the farthest node: 21.3 -> 23.0 km /
11.8 -> 12.2 min at `s = 0`, 16.7 -> 17.0 km / 13.4 -> 13.8 min at `s = 1`).
*Why `HW_FAST` = 0.4 and not stronger:* sweeping it (1.0 / 0.5 / 0.4 / 0.25 / 0.1) gave
24 / 27 / 27 / 29 / 32 of the 36 near-1500 trips on a highway, but also mean routes of 12.7 / 14.1 / 14.8 /
15.8 / 17.7 km (ETA 8.3 / 8.6 / 8.8 / 9.2 / 9.9 min), with 0.1 sending a 10 km trip over 36 km; the
remaining trips have no highway that leads their way, so more discount only buys detours. 0.4 is the
knee: the user asked for "(at least almost) always", not "at any price".

**Touge** (hairpin / mountain-pass roads): `chains()` in `nav/tests.rs` cuts the network at junctions
into road chains; the touge set is the 20 most winding chains >= 1 km with >= 60 m of up and down
(winding 6.9-13.9 mrad/m, e.g. a 5.1 km pass with 444 m of relief). End-to-end between the chain's two
ends (`touge_trips`), 5 of the 20 are contested (`s = 0` drives around them instead). Before: `s = 0.5`
took 1 of those 5 and `s = 1` took 2; after: `s = 0.5` takes 2 and `s = 1` takes 3, including the 5.1 km,
444 m pass (it was not taken at all before; drawn as a hairpin ladder in red against the blue `s = 0`
road around it). The other 2 have a 0.41 km shortcut / a parallel, equally curvy road, which is
correct. Trips starting and ending 0.5-2 km beyond a touge's
ends (`touge_uptake`, 152 trips over the 20 chains) drive it at `s` 0 / 0.5 / 1: 8 / 9 / 11 before,
8 / 10 / 15 after. *The ceiling:* `BETA` 0.95 with `KAPPA` 0.005 only reached 16 of 152, and adding a
climb-grade term to the curvature (tried at weight 0.1) 15 against 13: most trips simply have no touge on any
route near their line, so the curve knob saturates; more aggressive constants buy ETA, not touge.

### Search (`search.rs`)

* **Start and destination are edge points, not nodes:** the search is seeded with both endpoints of
  the snapped start edge at the cost of the partial edge, and ends at either endpoint of the
  destination edge plus its partial edge. The polyline starts / ends at the projection points.
  Exact for the 20 m edges and still right for the long added links (max 762 m). If both points are
  on one edge the direct piece is compared against the best detour (a long link may lose to one).
* **A\*** over node indices (binary heap, lazy deletion). Heuristic = straight 2D distance times
  `CostModel::heuristic`'s factor `min over enabled kinds of base[kind] * (1 - BETA*s)` (`base` =
  the speed blend times the highway factor). Admissible and consistent: every arc costs at least its
  2D chord times that factor (3D length >= 2D chord, `base` >= its minimum, curvature factor >=
  `1 - BETA*s` > 0); the highway discount lowers the bound, which is why it is `min`, not a speed.
  The weaker bound (2.5x at `s = 0`, 5x at `s = 1`) did not slow the search measurably. Dijkstra is
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
away, 11.5 km straight, 21-23 km driven, default filters): **1.3-1.7 ms release, 11 ms debug** (the D96
retune of the cost did not change it: 1.39 ms before, 1.28 ms after, at `s` 0), so no
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
The ignored route-preference evaluation (D96, need `FH6_INSTALL_DIR`; run with `cargo test --release
<name> -- --ignored --nocapture`): `pref_eval` (the table in the cost section), `touge_chains` (edge
winding percentiles and the most winding chains), `touge_trips`, `touge_uptake`, and `touge_png`
(`PNG_DIR=<dir>` draws the contested touge trips, blue = `s` 0, red = `s` 1, the chain in yellow).


## 3. Drawing on the maps (`maprender/`, `hud/`, `overlay/`, `ui/map_scene.rs`, L3)

The route is drawn on **all three maps** (HUD Minimap, Dashboard map, Map-tab Viewer) in **all view
modes** (Flat, Tilted, 3D) as a **road of its own**, like the race road (D80, D84): a casing (the
`Road` type's casing colour and `casing_px`) under a fill in the route colour, opaque, round ends,
width = the roads' width rule x `style::NAV_ROUTE_WIDTH` (1.5, a little wider than a highway, so it
covers the road it runs on and that road's casing shows as its edge) x `NavRouteCfg::width`. The
destination is a **pin** (egui, in every mode).

*Why a road of its own and not recoloured nav roads:* the same reason as the race road (D80): the
route is its own polyline with its own heights, it can leave the road network at the ends (the
snap, up to 300 m), and the road mesh is per type and rebuilt on every editor Save. *Why 1.5 =
the race road's width:* they never show together (the route is hidden in a race, D92).

### How the route reaches each map

| Map | Thread | Read |
|---|---|---|
| HUD Minimap | overlay thread | `overlay::render::Renderer::frame_at` calls `nav_fn()` (= `nav::view()`; the PNG harness swaps in a synthetic one, like `layers_fn`) once per frame **only while the minimap shows the route** (`minimap_on && map_layers.nav_route.on`), then `Hud::set_nav` -> `MapAnim::set_nav`. **No `HudSnapshot` field** (it would have to be forwarded by the UI thread, which stops while the game covers the window). |
| Dashboard map, Viewer | UI thread | `ui::map_scene::draw` calls `nav::view()` itself (one lock and an `Arc` clone per frame). |

Both then filter the view the same way (`paint2d::route_line` / `route_dest`): nothing when
`nav_route.on` is off on that map, nothing in a race (`PausedRace`, where the runtime already sends
no line: the belt to its braces) or after `Arrived`; the **pin** also stands while there is no route
yet or it failed (`WaitingForCar`, `Routing`, `Unreachable`, `NoRoadNear`, `NoRoadData`: it says
where the click went) but not when `Idle`. `Routing` keeps drawing the previous line (the runtime
still carries it). The route needs **no `MapLayers`**: with every layer switched off (or still
loading) `paint2d::draw_layers_or_route(.., None, ..)` draws the route alone.

### 2D and tilted (`paint2d.rs`)

`draw_race_lines`' road branch was factored into `draw_road_polylines(cx, cfg, factor, taper,
&[RoadItem])` (every casing under every fill, tapered pieces, round ends); the race road and
`draw_nav_route` both call it. `LayerCtx::nav: Option<&NavLine>` carries the line, `Parts::nav` the
switch (`ALL` has it; **`OVER_3D` does not**: in the 3D view the GL scene draws the route; while the
scene is not `Ready`, the underlay frames use `Parts::ALL` and draw it flat, like the roads).
**Layer order (bottom to top):** roads, jump lines, race lines, **navigation route**, POIs, then
trails, teammates, the own arrow, waypoints, **the destination pin**, compass. *Why this order:*
the route is a road, so it lies over the roads and the race lines; the POIs and markers stay
readable above it, and the car arrow is not covered by its own route. **"Race road only" (D82)
does not hide the route** (it hides the road layer, and the route is not part of it).

**Jumps (`seg_kind` 7):** the line is split at them; the road runs on each side are separate
polylines, and the jump stretch is a **dashed take-off -> landing line** with a casing, like the jump
lines (4 / 3 design px on / off, solid below 0.02 px/m), width factor `style::NAV_JUMP_WIDTH` (1.0).
*Why:* the gap is 130-760 m of air; a deck laid over it would look like a road that is not there.

### 3D (`gl3d/`)

* **Mesh:** `RoadMesh::nav_route(pts, y, seg_kind, terrain)` = the race road's builder
  (`race_road_layer`: 8 m samples, deck, mitred joins, round caps at the ends, **stretches 4 m or
  more under the terrain in the tunnel slot**) per run between jumps, and each jump stretch as a jump
  line (the jump slot: a taut string over the ground, no deck across the gap). `y` 0 = unknown = the
  terrain. Heights are the route's own (the graph's node heights).
* **Scene:** `Scene3d::route: Option<Route3d { line, mesh, cfg }>`, independent of the race lines and
  the in-race focus. The GPU copy (`Gl3d::nav`, `sync_nav`) is replaced when the mesh `Arc` changes,
  under the same "one heavy upload per callback" rule as the race mesh.
* **Pass:** after the roads and the race lines, before the trails: `draw_roads` with `roads::nav_table`
  (every slot in the route colour, the `Road` casing, `NAV_ROUTE_WIDTH` x `width`; the jump slot
  dashed) and a depth bias a step above the race road's (`NAV_BIAS` 0.0043, fill +0.00008; trails
  0.0044). **Depth-tested like the race road:** hills and decks in front hide it, only the tunnel
  stretches go through (last, without the depth test, at the tunnel alpha).
* **Build thread:** the mesh is built **on the calling thread** (UI / overlay thread) by
  `Route3d::new`, once per `NavLine::rev` and terrain (one process-wide single-slot cache,
  shared by the HUD and the Dashboard, behind a mutex that is held during the build so two maps
  do not build the same line twice). *Why not off-thread like `store::race_mesh`:* measured on the
  real install, release (`gl3d::tests::real_install_nav_route_mesh_cost`): the island-crossing
  route (node 1 to the farthest node, 21.3 km, 3 180 samples, 26 k triangles, 347 KB) builds in
  **0.51 ms** (6.2 ms debug), an 11 km route 0.28 ms, a lone jump 0.04 ms; a cache hit costs 60 ns.
  It happens a new route or every 150 m chunk (every few seconds of driving), never per frame. An
  off-thread build (thread, generation counter, the previous line shown for a frame after a change)
  pays at 270 ms (all 170 race lines), not at half a millisecond. A panic in the builder is caught
  (no route, not a dead overlay thread).
* **Pin in 3D:** egui, over the scene, its tip placed by the 3D camera at the **terrain's height**
  at the clicked point (`MapCanvas::to_screen` -> `Camera::project`), the same call the waypoint
  uses. *Why egui and not GL:* a pin is a screen-space marker (upright, constant size) and nothing
  should hide it behind a ridge: it is where you are going.

### The pin (`hud::map_shared::draw_destination`)

A teardrop with its tip on the point, head 13 px above it (design px x `s`), the route colour with
a white centre and a black outline, the distance from the car above it. A destination a teammate
shared (D85) has a **ring in the setter's hue** round the head. *Why the fill is always the route
colour and the hue only a ring (the D85 "your call"):* the pin and the route read as one thing,
and the ring says whose it is without making a green pin lead a fuchsia route. Off the map it is a
dot on the edge (the circle's edge on the round Minimap) with the distance placed 16 px inwards of
it (like an off-map teammate; above the dot it was cut in half on a left / right edge), like the
waypoint (whose off-map label moved inwards the same way). The pin's distance is the straight line
from the car, not the route's remaining distance (that is on the tab).

### Config (`MapLayerConfig::nav_route: NavRouteCfg`, per map)

| Field | Default | Meaning |
|---|---|---|
| `on` | `true` | draw the route and the pin on this map |
| `color` | `#d946ef` (fuchsia) | fill of the route and the pin. *Why:* used by no road type that is drawn (the turnaround has it but is never drawn, D52) and clear of the race colours (orange, pink), road `#38bdf8`, highway, offroad, tunnel and the jump line |
| `width` | `1.0` | factor on the route's width, clamped to 0.5..3 (`width_factor`) |

HUD: `overlay.map_layers.nav_route`; Dashboard map and Viewer share `minimap_layers.nav_route`
(D73); `OverlayConfig::effective` copies `map_layers` wholesale under "Use Dashboard map settings".
`serde(default)`: a config from before has none and gets the defaults (on, fuchsia). It is also a
`LayerCategory::NavRoute` for "Copy to ..." (`MapLayerConfig::copy_category`). The **Navigation route**
card (On, Colour, Width, Copy to...) is in `maprender::ui::layers_ui` on the Minimap and Dashboard
map & Viewer pages (L5).

### Tests

`paint2d`: route drawn as a road over the roads (casing, fill, round ends, width rule, config colour
and width, HUD fade alpha), not in `Parts::OVER_3D`, hidden with `on = false`, drawn without layer
data, drawn with "Race road only", tapered in a tilted view, jump stretches dashed with no road over
the gap, degenerate lines; `route_line` / `route_dest` per `NavStatus`. `mesh3d`: tunnel / jump /
cap structure of the route mesh. `map_shared`: the pin projects through the camera onto the terrain
in 3D. `gl3d` (headless GL, `#[ignore]`, `cargo test gl3d -- --ignored`, parallel-safe since #229): over the
road it runs on, with race road only, through a hill (tunnel), a dashed jump, hidden behind a hill
and under a deck, mesh rebuilt only when `rev` changes and uploaded once (and unaffected by a colour
change), in all three GL flavours (`suite`), real-install routes (`gl3d_real_install_nav_route`,
`real_install_nav_route_mesh_cost`, need `FH6_INSTALL_DIR`). HUD PNG harness (`cargo test
render_3d_states -- --ignored`): route + pin in 3D, shared ring, tilted, flat, hidden in a race and
when switched off, and through `Renderer::frame_at` with `nav_fn`.

## 4. Navigation tab and Viewer destination (`src/ui/nav_tab.rs`, `src/ui/map_tab.rs`, L5)

`Tab::Navigation` sits between Map and Power Curve (icon `icons::NAVIGATION`, fa-location-arrow
`U+F124`, checked present in the bundled Nerd Font; title "Navigation"). No Mini-Settings page.

### Layout (Panes rule)

Two panes with an 8 px gap (`nav_tab::split`): the **left pane** is `min(360 px, 40 % of the tab)`
wide, one vertical `ScrollArea` of four `theme::card`s (spacing zeroed, the card owns the gap), clipped
to its rect; the **right pane** is the map, filling the rest, also clipped. (`theme::columns` is
equal halves only, hence the manual rect pair.) No helper text under options: explanations are
tooltips (D21).

1. **ROUTE**: status line (dot colour + text, `status_text`: *Click the map to set a destination* /
   *Waiting for the car's position* / *Calculating…* / *On route* / *No road near the car* / *No road near
   the destination* / *No route with these road types* / *No road data (needs your Forza Horizon 6
   install)* / *Paused during a race* / *Arrived*), *Set by <name>* with the setter's colour dot while a
   shared destination is navigated, **Distance** and **Time** (remaining; `~12.4 km` / `~7.7 mi`
   when the app's `use_mph` is on, `~9 min`, `~1 h 05 min`; `–` without a route), and **Clear route**
   (`danger_button`, enabled with a destination; **Clear for everyone** while a shared one is active,
   tooltip says so). A saved destination the navigator has not published yet reads *Calculating…*
   (`PaneIn::pending`), not the idle hint.
2. **ROAD TYPES**: Road, Highway, Dirt, Trail, Cross-country, Jumps (`RouteFilters`). Tooltips carry
   the detail: Road also covers roads of another or unknown type, tunnels are driven when Road *or*
   Highway is on, turnaround crossovers are never used; Jumps are one-way and risky.
3. **PREFERENCE**: one slider, end labels *Faster roads* / *More curves*, no numbers (tooltip says how
   the route is chosen). Stored as `curves` 0..1.
4. **CO-OP**: *Share my destination* and *Follow shared destinations* (the two `NavConfig` switches),
   and a line with who set the room's destination (*Shared destination set by <name>* / *you* /
   *No shared destination*). Greyed with a status line *Not in a co-op session* outside a session.

**Filters while following a teammate's destination** (D85) are **read-only and show the setter's**
(`PaneIn::followed()` = the navigator's destination is `Shared` and was not set by this player; the
cards then draw `dest.prefs`, the same filters / slider the navigator routes with). Own settings are
untouched underneath and are shown again when the shared destination ends. *Why read-only rather
than "edit and ignore":* the route really is computed with the setter's values; showing editable
controls that do nothing would lie. A destination this player shared themselves comes back from the
room as "shared" too, but it is theirs (`RoomInfo::mine`), so their controls stay editable.

### The map pane

`map_tab::map_pane(ui, app, rect, MapPane)` is the Map tab viewer's code made reusable (input, scene
via `map_scene::draw`, clicks, buttons); the viewer calls it with `MapPane::Viewer`, this tab with
`MapPane::Navigation` and its own `MapTabState` (`app.nav_ui.map`: pan / zoom / race selection, not
shared with the viewer). **It draws with the Dashboard map & Viewer settings** (D73: layers, view
mode incl. 3D, north-up, allow pan and zoom): the tab has none of its own, and its **Settings**
button opens that page of the Map tab. *Why:* one map look everywhere, and the user wanted every map
setting on the Map tab (D79). Pan / zoom is the Viewer's (drag, wheel, *Follow car* while manual).
A faint hint pill *Click the map to set a destination* shows at the bottom while there is none (not
on a map narrower than 520 px).

### Clicks: how they are disambiguated

One pure function, `map_tab::click_action(pane, click, shift, armed, in_session)`:

| | Navigation tab | Viewer |
|---|---|---|
| left click | **set destination** | waypoint (only in a co-op session); **set destination** if Shift is held or the *Set destination* button is armed |
| right click | nothing | clear waypoint (in a session); while armed: just disarm |
| *Clear route* button | (the ROUTE card's) | shown while there is a destination; reads *Clear for everyone* (with a tooltip) while the destination is the room's, as on the tab |
| Esc | - | disarms |

The destination point is `map_scene::pick(&cam, pos)` (2D / tilted / 3D terrain ray-march), the same
as a waypoint. A drag is a pan, not a click (egui). Armed state is `MapTabState::dest_armed` (viewer
only, not saved); armed shows a primary-coloured button and a crosshair cursor. The Dashboard map
widget has no click handling for destinations (D84 names the tab and the Viewer only).

### Config and the bridge to the navigator

`AppConfig.nav: NavConfig` (`serde(default)`; lenient parse resets only the bad field, e.g.
`nav.curves`; in `EXPORT_EXCLUDE`: the destination is personal, so presets / exports never carry it,
and a profile switch keeps the live destination but takes the profile's filters).

| key | default |
|---|---|
| `nav.filters.{road, highway, dirt}` | `true` |
| `nav.filters.{trail, cross_country, jumps}` | `false` (D92) |
| `nav.curves` | `0.0` (fastest roads) |
| `nav.destination` | `null` (world `[x, z]`; **saved**, so a trip survives a restart) |
| `nav.share_destination`, `nav.follow_shared` | `true`, `true` |

`nav_tab::sync` runs once per frame in `ForzaApp::update` before any tab draws:

1. **Arrival:** `take_arrival(seen, view.local_cleared_seq, nav::local_destination(), &mut cfg.nav.destination)`
   (also run by `take_arrival_now` in `on_exit` before the final save: an arrival while the game
   covered the window has had no UI frame to take it over):
   when the navigator counted an arrival, the saved destination is cleared **once** (the counter
   moves once per arrival; `seen` starts at 0 and the counter at 0, so a destination loaded at
   startup is never cleared by the first frame). If the navigator holds a destination again (a click
   in the same frame as the arrival) the saved one stays. This is how the destination does not come
   back after a restart.
2. **Push:** `diff(pushed, wanted(cfg))` hands the navigator only what changed (`set_destination`,
   `set_prefs`, `set_follow_shared`; all of it on the first frame, the saved destination included).
   Pushing only on change (instead of every frame) avoids re-setting a destination the navigator has
   just cleared on arrival, in the gap before step 1 sees it. A NaN slider is clamped so it never
   looks "changed".

`nav_tab::set_destination(app, pos)` (a click) and `clear_destination(app)` (the Clear buttons) write
the config, push at once and do the co-op part below.

### Co-op behaviour as built (D85, D92)

The decision is the pure `room_op(event, nav_cfg, ctx) -> Option<RoomOp>` (tested as a table; the
adapter `room_event` fills `RoomCtx` from `CoopState::destination()` / `my_id()` / `nav::view()` and
calls `CoopState::set_destination` / `clear_destination`, which do nothing outside a session):

* **Set** (a click): sent to the room when *Share my destination* is on, with the player colour
  (`coop_hue`), `filters.to_bits()` and `curves`.
* **A teammate cleared this player's shared destination** ("Clear for everyone"): the own saved copy
  (the destination the share came from) is cleared too (`cleared_by_teammate`, from the room's
  destination last frame and `CoopState::cleared_by()`, the tombstone's setter). *Why:* without it the
  setter's navigator fell back to its local copy of the same point, so "Clear for everyone" left the
  setter's route standing (found live in QC). Not when this player took it back (sharing off: the
  clearer is them) or the session ended (no tombstone).
* **Clear**: clears the room's when the destination being cleared is a shared one: followed (anyone's,
  also with sharing off: anyone may clear it for everyone), or this player's own while sharing. A
  teammate's destination that is not followed (`follow_shared` off) is left alone.
* **Share switched on** with a destination: sends it. **Switched off**: takes this player's own out of
  the room (a teammate's stays).
* **Filters / slider changed** while this player's own destination is in the room: sent again (the
  navigator routes a shared destination, even one's own, with the room's copy of the filters, so
  without the resend the new filters would not apply). Debounced to the pointer release
  (`NavState::resend`), so a slider drag sends one message.
* **Receiving** needs no UI: the listener thread hands the room's destination to the tracker (section
  5); the tab only displays it. A teammate's destination overrides this player's own while
  *Follow shared destinations* is on; the own one and the own filters are back when it ends.
* **Joined** with a destination already set (set before the session, or saved from the last run):
  sent once per session, 3 s (`JOIN_GRACE`) after the session is up and this player is in the roster,
  **only if the room has no destination by then** (`JoinWatch` / `join_due`, `RoomEvent::Joined`).
  *Why the wait and the condition:* a late joiner receives the room's destination right after the
  handshake; a saved destination from yesterday must not replace the group's current one (one slot,
  last write wins), and the joiner then follows the room's instead. A host starting a session with a
  destination shares it (the room is empty). A reconnect does not repeat it.
* A click while a teammate's destination is followed and sharing is off sets
  the own destination but the teammate's keeps winning (one slot, last write wins; Clear first).

Tests: `ui::nav_tab::tests` (bridge: startup does not clear, arrival clears once, same-frame click is
kept, only changed inputs are pushed; `room_op` table incl. outside a session and the join share;
`join_due` once per session after the grace; `cleared_by_teammate`; formatting; the split;
the four cards stay inside the pane at 240 / 280 / 360 px in English and German over every status incl.
a shared destination with a very long name; read-only followed filters), `ui::map_tab::tests`
(`clicks_on_the_maps_do_what_each_map_promises`, `viewer_controls_stay_inside_the_tab` with the new
buttons), `config::tests` (NavConfig defaults, partial / bad nav, not exported, profile keeps the
destination), `maprender::ui::tests` (Copy to… incl. the Navigation route card).

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
