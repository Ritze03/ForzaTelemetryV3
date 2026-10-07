# FH6 map tooling — road editor v2, 2D preview, 3D preview (standalone + live in the editor)

Phase-H tooling in `tools/fh6-extract/` (usage and flags: [its README](../../tools/fh6-extract/README.md)); the editor and 3D pages themselves live in `assets/editor/` (shared with the app, see "One copy of the pages" below). This page holds the
**formats and the why**. Everything that contains game data (viewer, both previews) is generated into a directory **outside the repo**
(e.g. `/home/mo/fh6-viewer/`) and never committed or published.

## Road types — what the user means by them

| Type | Meaning (user's words / intent) |
|---|---|
| Road / Offroad / Other | the original three (paved / dirt / not classifiable) |
| **Trail** | small trails: in the game these are trails, not dirt roads (drawn dashed) |
| **Cross-country** | user-drawn lines for things that are not in the game, for routing |
| **Tunnel** | counts as asphalt for routing; there are no dirt tunnels. Also drives the height rule below |
| **Jump line** | one-way (take-off to landing); routing may opt in |
| **Highway** | motorway / expressway, marked separately from Road (drawn wider in the 2D preview) |
| **Turnaround** | a link that exists only so the AI can get back onto the right road. User: "many roads are cross-connected, but it isn't like that in the game. It's probably just for the AI driving, so it can easily get back to the right road again… mark them… so later we can use them for navigation, but hide them from the actual in-game map." |

Routing is **not built** (D47); the types are stored for it. Highway and Turnaround were added without a version bump (still `version: 2`).

## `fh6-road-types` v2

v1 (ids + three types, no coordinates) still imports; `build_viewer.py:build_canon` and both previews accept v1 and v2.

```json
{"format":"fh6-road-types","version":2,
 "nav":{"file":"Brio_00.nav","sha1":"...","nodes":38473},
 "types":{"<min>-<max>":"road|highway|offroad|other|trail|crosscountry|tunnel|jump|turnaround"},
 "added":[{"a":id,"b":id,"type":"..."}],
 "points":{"<id>":[x,z,y]},
 "moved":{"<id>":[x,z,y]},
 "removed":["<a>-<b>"],
 "jump_from":{"<a>-<b>":<take-off point id>},
 "races":{"<route id>":"road|street|rally|cross_country|touge|drag|story|wristband"},
 "counts":{}}
```

- `types` = **game edges only** (key smaller id first; unlisted = not set). `added` = links between *any* two points (game node or user point), `a < b`, `type` may be `null` (unset; the 2D preview draws it grey dashed).
- `points` = user-created points, ids >= 1 000 000 (game node ids stay below, so no collision). `moved` = position overrides of game nodes (only those whose position differs from the nav file).
  `removed` = game edges that were split or deleted. `jump_from` = per jump edge the take-off end (missing: the smaller id); the other end is the landing.
- World metres (x east, z north, y up; the roads.json / telemetry space), rounded to 0.01 m. `counts` is informational and ignored on import.
- **Split** of game edge a-b at new point p is stored as `removed:["a-b"]` + added `a-p` and `p-b`, both with the old type.
- **Why coordinates are allowed now:** v1 had none so the committed file held no game data. In v2 the coordinates are *user-placed* points (the user's own work, not read from the game),
  game nodes are still ids only, so the licensing rule holds (D48). Moved game nodes store the user's new position, not the original.
- The canonical **`assets/map/fh6-road-types.json`** (the *project file*; moved from `tools/fh6-extract/data/` in I25, D55: the app embeds it with `include_str!`, so it lives with the other app assets) is the committed copy and is **v2** since 2026-10-05 (the user's editor export, then run through `fix_highways.py`; latest refresh = user export (12), commit `198c13e`). Verified contents: 39 382 typed game edges (road 21 670, offroad 7 236, highway 4 979, trail 3 936, tunnel 904, turnaround 340, other 317; cross-country exists only as added links), **124 user points** (ids >= 1 000 000), **218 added links** (cross-country 144, road 31, jump 18, highway 17, trail 7, offroad 1), **1 moved** node, **1 removed** edge, **18 `jump_from`** entries and **94 race marks** (of 170 races: rally 21, road 21, cross-country 19, street 17, story 6, touge 5, drag 3, wristband 2). Network totals (`counts.km`): road 424.9 km, offroad 138.7, highway 103.4, trail 69.4, tunnel 18.1, cross-country 8, jump 7.3, turnaround 7.2, other 6. Committing the coordinates is fine (D48: the user's own work). In the browser the editor's localStorage autosave keeps the old state until "Reset to project data" (see [hand-classified road and race types](fh6-game-files.md#hand-classified-road-and-race-types)).
- **File layout: one entry per line.** The canonical file is stored one entry per line, and the editor's Export (`exportText()` in `assets/editor/index.html`) writes exactly the same layout as `fix_highways.py` (`write_v2`), byte for byte, so replacing the file with a fresh export keeps pull-request diffs down to the entries that changed. *Why:* contributions come in as pull requests; the editor used to export one 900 kB line, which made every PR a whole-file diff. Import is plain JSON and still accepts old single-line files. Key order inside `types` / `jump_from` follows the editor's edge order (game edges, then added links), so a file written from a long session may be re-ordered once on the first re-export.

### Rust reader and the "current" road types (`src/gamedata/roadtypes.rs`, I25)

The app reads the format in Rust (`RoadTypes::parse`, lenient like the editor's `importObj`: unknown type names, bad keys and out-of-range ids are skipped and counted in `warnings` / `skipped`; only malformed JSON or a wrong `format` / `version` is an error) and writes it back with `RoadTypes::to_json_string()`, which reproduces `fix_highways.py write_v2` / the editor's `exportText` line for line. A unit test parses the embedded project file and writes it back **byte-identical**, so the one-entry-per-line layout is pinned by a test. Entry order (types, added, jump_from, races) is kept, so those are `Vec`s / order lists, not hash maps. The `races` block is an **opaque passthrough** (`Vec<(String, serde_json::Value)>`, never interpreted): the file holds 94 marks and no load → save may lose them. `counts` is kept as the raw JSON text (informational, not recomputed). Number printing follows JS (`10`, not `10.0`).

Three states of the data, all in `roadtypes.rs`:

| Name | What | API |
|---|---|---|
| **project** | the committed file, embedded (`include_str!("../../assets/map/fh6-road-types.json")`) | `RoadTypes::project()` |
| **raw** (D57) | the bare nav graph: every edge unset, no points / links / moves / removals / race marks, no `nav` block | `RoadTypes::raw()` |
| **current** (D57) | project, **replaced wholesale** by the user's override when that is valid | `RoadTypes::current(&override_path(), &nav)` -> `Current { types, source, note, project_updated_since_save }` |

The override is `<app_data_dir>/map_editor/fh6-road-types.user.json` (`override_path()`), written by the editor's Save (I26b).

**The override replaces the project file wholesale; there is no per-entry merge (D60, user decision 2026-10-07).** *Why:* the editor always saves the *complete* state (all typed edges, the full `removed` list). "This edge is now unset" and "this added link was deleted" are expressed by **absence**, which a per-entry overlay cannot represent: the project's entry would resurrect. An overlay would need tombstones, i.e. new format surface for little gain. Rules:

1. No override file: current = project.
2. Override present, parses, `version` 1 or 2, and its `nav` block matches the installed nav (SHA-1 and node count, the editor's `navOk`; a file with no `nav` block counts as matching): current = override.
3. Override unparsable or for another nav: **ignored, never deleted or modified**; current = project; `Current.note` explains why in plain words (Setup shows it, I27).
4. The project file's own `nav` differs from the installed one (game update): still used, with a note.
5. **Drawback:** users who saved an override do not get later project improvements. Mitigation: Save stamps `"based_on": "<sha1 of the embedded project file>"` into the override (`RoadTypes::project_sha1()`; the writer emits it on its own line after `nav`), and `Current.project_updated_since_save` is true when the override's `based_on` differs (a missing `based_on` counts as false) so Setup can offer "Reset to project data".

`RoadTypes::validate_for_save(text, &nav)` is the Save endpoint's check: parse + a `nav` block that matches + no skipped entries.

## `fix_highways.py` — highway clean-up

`fix_highways.py IN.json OUT.json [--work DIR] [--report BASE] [--dry-run] [--validate] [--no-overmarks] [--no-turnarounds] [--plot PNG [--bbox X0,Z0,X1,Z1]]`. Reads v1/v2, writes v2 (editor key order, `counts` as `exportObj()`), idempotent, refuses a different nav sha1/nodes. Review CSV/JSON/PNG go outside the repo.

**Why:** the editor brush also painted roads stacked under/over a highway, and hand-marking hundreds of crossovers was not wanted.

- **Over-marks (highway -> road):** `OVR_SPILL` = nav road with highway share <= 30 % (tunnel edges excluded); `OVR_ISLAND` = highway piece joined to no other highway/tunnel/turnaround/added link, <= 1500 m, >= 80 % of nodes within 3 m of terrain.
  *Why no "stacked" rule (2D overlap + height gap):* Tokyo City has real double-deck expressways (nav roads #800/#801, ~9 m apart, lower deck on the ground, each with its own crossovers) and interchanges up to 10 layers; a stack rule would delete real lower decks. Nav `cls` (4/5 direction, 6 ramps), `hi` and terrain height do not separate streets from highways either. Highway marks are nearly always whole nav roads (121/138 polylines 100 % highway), so per-nav-road topology is the reliable signal.
- **Turnarounds:** nav polyline of 1 edge (<= 30 m) or 2 edges (<= 40 m) typed road/highway/unset, both ends degree-3 nodes whose other two edges are highway and ~collinear (>= 130 deg), strip crossing the carriageway (25-155 deg), end heights within 4 m, ends not joined by <= 150 m of highway. *Why:* the game stores each crossover as its own 2-node road record between two carriageway nodes. Calibrated on the user's 12 hand marks (12/12 re-found with `--validate`).
- **Short links (`TURN_SHORT_LINK`, runs after the crossover rule, 2026-10-06):** any edge <= 30 m whose both ends also touch an edge typed highway/tunnel -> turnaround, if it is a 1-edge nav polyline of its own (old type road/unset/highway/other) or, inside a longer polyline, every other edge at both ends is highway/tunnel/turnaround (road/unset/other only; so a ramp merely touching the highway at one end stays). *Why:* the user wanted every point-to-point link from carriageway to carriageway marked and a simple rule is easy to explain; the strict crossover tests (collinear carriageways, crossing angle, height, loop, tunnel ends) rejected 22 such 12-25 m links (plus 1 inside a 34-edge polyline). Short *highway* edges inside carriageway polylines (~4800) are the carriageway itself and are never touched. Run on (8): 23 more turnarounds (340 total).
- **Needs review (nothing changed):** `REVIEW_HIGHWAY_GAP` (road edges inside mostly-highway nav roads, likely under-marks), `REVIEW_CUT_OFF_PIECE`, `REVIEW_HALF_MARKED`, `REVIEW_TURNAROUND` (strip-like links failing a test, e.g. crossovers inside tunnels).
- **First run (2026-10-05):** 32 edges / 0.63 km highway -> road (all Tokyo City), 305 new turnarounds, 76 review rows. **Second run (on export (7), 2026-10-06):** 31 highway -> road, 305 turnarounds, 80 review rows = `REVIEW_TURNAROUND` 36, `REVIEW_HIGHWAY_GAP` 28, `REVIEW_HALF_MARKED` 9, `REVIEW_CUT_OFF_PIECE` 7.
- **Fixing the review rows:** the editor's **Review spots** (below) runs these rules live, so the user can walk the list and fix it by hand.

## Editor (viewer, `EDITOR` block of `assets/editor/index.html`)

- **Modes:** Pan, Paint, Connect, Draw, Split, Move, Flip jump, Delete link, Race types. **Brush keys:** 1 Road, 2 Offroad, 3 Other, 4 Trail, 5 Cross-country, 6 Tunnel, 7 Jump line, 8 Highway, 9 Turnaround, **0 Clear**
  (key 4 used to be Clear; it moved to 0). One undo stack (Ctrl+Z) covers all modes.
- **Draw** (D42): click chains free points; clicking an existing point snaps and connects (**Alt** = no snap); ends with Esc, right-click or a click on the last point; a lone point is dropped.
- **Split** (D36): click an edge, a point appears at the projected click position, drag to place it; one undo step; both halves keep the type.
- **Move:** drag any point. A game node goes to `moved`, a user point to `points`; dragging it back onto the original position drops the override.
- **Flip jump** reverses a jump's direction (D41: jumps are one-way, drawn as take-off to landing arrows).
- **Delete link** also removes game edges (they go to `removed`); user points left without a link vanish.
- **Heights (D40):** new / moved points take the **bilinear height on the 8 m elevation grid**. If both neighbours are >= 3 m off the terrain (bridge / tunnel), or the edge is a tunnel, or there is no terrain,
  the height is the **linear interpolation between the neighbours**; a tunnel point with a single neighbour copies its height (drawn tunnels stay level); at a junction the two most opposite neighbours are used.
  **Cross-country exception:** a point whose live links are **all** cross-country always takes the terrain height (never the bridge interpolation; `autoY(..., xc)` / `onlyXC`), and `snapXC()` re-derives that height for such *user* points on import and once the height grid has loaded (only if it differs by > 0.5 m; not autosaved by itself, but the next export is clean). On the current data it changed 0 points (46 cross-country edges, all already on the terrain). *Why:* the user asked: "Especially for the cross-country type road markings, make sure that for their height, they use the actual terrain data, so they are not like stuck in the ground and stuff, and this can be by default for all the cross-country Markings."
  *Why:* the height map gives the ground under a bridge or the hill above a tunnel, which is the wrong height for the road itself.
- **Styles:** trail dashed, tunnel dark casing + pale core, cross-country violet, jump white arrows, highway amber wide with casing, turnaround fuchsia dotted, user links white outline
  (no longer dashed: dash now means Trail), user points lilac, moved game nodes an orange ring. **"Show turnaround links"** checkbox hides them from drawing *and* hit-testing; picking the Turnaround brush re-shows them.
  **Prefill from surface data** only touches Road / Offroad. In edit modes popups/labels no longer swallow clicks.
- `elevation.js` (viewer data) is the full 8 m raster, delta-coded per row (`grid.delta=1`, ~4.7 MB; was 16 m) so the editor's height lookups match the previews. `roaded.js` carries the node height `y` (0.1 m) per vertex.

### One copy of the pages, embedded libraries, app mode (I26c; plan D50, D57-D60)

- **Location:** the editor page is **`assets/editor/index.html`** (was `tools/fh6-extract/viewer_template.html`), the 3D page **`assets/editor/preview-3d.html`** (was `tools/fh6-extract/preview_3d.html`), libraries in **`assets/editor/lib/`**
  (Leaflet 1.9.4 + `images/`, Leaflet.markercluster 1.5.3, three.js r147 global build `three.min.js`, `LICENSES.md` with the BSD-2 / MIT texts). Both tools read these files: `build_viewer.py` substitutes `<!--@DATA_SCRIPTS@-->` with the `<script src="data/*.js">` tags and copies `lib/` to `<out>/lib/`;
  `preview_3d.py` copies `preview-3d.html` and `lib/`. Usage of the Python build is unchanged (`--out` outside the repo, opens from `file://`); it now works **offline**.
  The app's local server (I26b) serves the very same two files from memory with the same marker substitution, so there is exactly one HTML to maintain.
  *Why embedded (D58):* the app must work without internet and from a fixed local port; the libraries are MIT / BSD-2 so committing them is fine (game data stays out of the repo). three.js stays on r147 because the global `three.min.js` build was dropped from npm in r160.
- **App mode** = `FH6.app` exists: `const APP = (window.FH6 && FH6.app) || null`. The server adds `data/app.js` = `(window.FH6=window.FH6||{}).app={save:"save",project:"project.json"};` (relative URLs, so they resolve under the server's token path prefix; absent in the Python build, which behaves as before). In app mode:
  - **Save** button (next to Export / Import, `#ed_save`) and **Ctrl+S** `POST save` with the v2 file (`exportText()`, `Content-Type: application/json`); the reply is `{"ok":true}` or `{"error":"..."}` (shown in the status line, `s_saved` / the error). A `dirty` flag (set by every edit, cleared on Save) drives a `beforeunload` warning.
    The page never writes `based_on`; the server stamps it. *Why Save replaces the file (D60):* the saved file is the user's project data wholesale, not a merge; Export / Import (a file download) still exist.
  - **No localStorage autosave** (`save()` only sets `dirty`; the restore reads no autosave). The editor starts from `FH6.canon` = the server's *current* file. *Why:* the server file is the truth, and a localhost origin changes with every port / session, so a browser autosave would be unreachable or stale.
  - **Reset to project data** fetches `APP.project` (`project.json` = the file embedded in the app) instead of using `FH6.canon`, because `FH6.canon` is the *current* file there (it is the project file only in the Python build).
  - **Race marks pass through:** the app build has no race lines (`HAS_RC` false), so marks cannot be matched to routes. `importObj` keeps `o.races` verbatim in `passRaces` and `exportObj` writes them back (sorted numerically like the live marks), so Save keeps all 94 marks of the project file. *Why:* without it the first Save would silently wipe them (data loss). Resetting the editor clears `passRaces` only when race lines exist.
  - **Hidden by absent data, no code switch:** POI / name / race-line / sign / surface layers (`if (D.x)` guards), the race mode and queue (`HAS_RC`), and **Prefill from surface data** (needs `RE.pre`, which the app's `roaded.js` does not carry, D57). The **elevation hillshade** layer needs `D.elevation.img`; the app sends only the height grid (`D.elevation.grid`), so the layer is guarded (`D.elevation && D.elevation.img`) and the grid-based height lookups still work.
- **3D texture:** `setSeason` loads `preview3d/tex_<Season>.jpg` as a plain image URL when the page is served over http (no base64, no 6 MB script parse) and falls back to the data URI in `preview3d/tex_<Season>.js` otherwise. A `file://` page uses the `.js` straight away (a `file://` image is cross-origin for WebGL, and the Python build writes no `.jpg`).
- Both pages carry `<link rel="icon" href="data:,">` so the browser's `/favicon.ico` request (outside the server's token prefix) is never made.

### Rust generators and the local server (I26a / I26b; plan D54, D56, D59, D60)

**Rust generators (`src/mapedit/data.rs`, I26a).** Pure functions that write every data file the editor and 3D pages load, from the user's install, in the same byte format as `build_viewer.py` / `preview_3d.py` (`write_js` wrapper `(window.FH6=window.FH6||{}).<name>=<compact JSON>;\n`, `P3D` for the 3D files; pure ASCII). Output contains game data: only served locally or written to the app data folder, never committed.

| served path | producer | content |
|---|---|---|
| `data/meta.js` | `meta_js` | `{calib, seasons:[installed], cats:{}, icons:{}, built}` |
| `data/roads.js` | `roads_js(&Nav)` | `{cls, lines}`, 0.1 m, no `surf` |
| `data/roaded.js` | `roaded_js(&Nav)` | `{nav, ids, y, orphans}`, no `pre` |
| `data/canon.js` | `canon_js(&RoadTypes)` | the current v2 object, compact |
| `data/elevation.js` | `elevation_js` | `{grid:{x0,z1,res,w,h,delta:1,data}}`, data = base64(zlib(i16 LE decimetres, delta-coded per row)), no `img/zmin/zmax/ramp` |
| `preview3d/meta.js` | `p3d_meta_js` | `{seasons, default, tex_size, built}` |
| `preview3d/terrain.js` | `p3d_terrain_js` | 16 m mesh `q=(h+5)/0.025+1` u16, 0 = no terrain; `nx=1327 nz=1376 mx0=-11748 mz0=10730 step=16` |
| `preview3d/roads.js` | `p3d_roads_js(&Nav,&RoadTypes)` | edge list: `nodes` f32 [x,z,y], `edges` u32 [a,b,typeIdx], `tn`, `km`, `lift` 0.6, `step` 5 |
| `preview3d/tex_<Season>.jpg` | `tex_jpeg` | 4096², level 2, JPEG q82 (a real `.jpg`, not a base64 `tex_*.js`) |
| `tiles/<Season>/<z>/<x>/<y>.jpg` | `tile_jpeg` | 1024² JPEG q80, entry `z-y-x` (Leaflet x = column, y = row) |

`EditorData::build(media, &RoadTypes, progress)` builds the text files once (~0.5-1 s release); `resolve(path)` serves them and generates images on demand, caching under `<app_data_dir>/map_editor/cache/{tiles/<Season>/<z>/<x>/<y>.jpg, tex_<Season>.jpg}` (regenerated if the season zip is newer). **Why `canon.js` and `preview3d/roads.js` are separate:** road types are the only input that changes at runtime, so `set_road_types` regenerates just those two (~45 ms) on Save. **Why the texture is a `.jpg`:** 33 % smaller and no 6 MB script parse. **Parity with the Python:** `roads`, `roaded`, `elevation`, `canon` and the 3D edge list are value-identical; the 3D terrain agrees on header and meshed mask exactly and 99.94 % of cells are within 1 m — hole fill (coarse raster, then iterative neighbour averaging) and sea skirt (chamfer distance transform) are not bit-exact with scipy, affecting only the 3D mesh. Unknown type names are skipped (edge unset), not normalised to `other` as the Python did.

**Local server (`src/mapedit/server.rs`, I26b; D56).** `MapServer::start(ctx, media, StartFrom::{Raw|Current}, events)` binds, then builds `EditorData` on a `mapedit-build` thread **before** anything is opened (so the first page load isn't blocked: until then every data request gets 503); when done it sends `MapEvent::Ready{url}` and the app opens it with `ctx.open_url` (`ForzaApp::poll_map_editor`). `StartFrom::Raw` = the bare nav graph (D57), `Current` = `RoadTypes::current(override_path(), &nav)`; Save writes the override either way. A hand-rolled `std::net::TcpListener` HTTP/1.1 server (one thread per connection, `Connection: close`; no new crate: `tiny_http` is not in the tree and the needs are tiny). `Drop` sets a stop flag and connects to itself to wake `accept` (the `network.rs` `NetworkHandle` pattern, but `accept` has no read timeout to poll with). The page and libraries are **embedded and served from memory, never written to disk** (D59): a copy on disk would only go stale, and nothing can use it (`file://` pages can't call the server's Save).

*Endpoints*, all under `/<token>/`: `GET` `` (= `index.html`, the marker-substituted editor page), `preview-3d.html`, `lib/*` (incl. `lib/images/*.png`), `data/{meta,roads,roaded,canon,elevation}.js`, `data/app.js` (`FH6.app`, see the I26c section), `project.json` (the embedded project file), `preview3d/{meta,terrain,roads}.js`, `preview3d/tex_<Season>.jpg`, `tiles/<Season>/<z>/<x>/<y>.jpg`; `POST save`. Anything else is 404, `GET save` too.

*Security: the server exposes the user's files and any web page can reach `127.0.0.1`.* Each check and why:

| Check | Why |
|---|---|
| bind `127.0.0.1` only | not reachable from the LAN (and no Windows firewall prompt) |
| token = 16 random bytes (hex, `getrandom`) as the **first path segment**; wrong / missing = empty `404`; constant-time compare; new every session | other web pages can't guess it, so can't reach a handler. A *path prefix* (not a query) because the pages use only relative URLs: every request, incl. the Save POST, carries it with no HTML change |
| `Host` must equal `127.0.0.1:<port>` (exactly one), else `400` | DNS rebinding: a page on `evil.example` re-resolved to 127.0.0.1 sends its own host name |
| `POST` needs `Content-Type: application/json` (`415`) and, if `Origin` is sent, our own (`403`) | a cross-site page can't send a JSON POST without a preflight; the origin check covers the rest |
| `OPTIONS` and all other methods: `405`, **no CORS headers anywhere** | never answering a preflight is what keeps foreign pages out |
| responses carry a CSP (`default-src 'self'; script-src/style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-src 'self'; worker-src blob:`), `X-Frame-Options: SAMEORIGIN` (the editor frames the 3D page), `nosniff`, `Referrer-Policy: no-referrer` | the pages use inline scripts / styles and `blob:` URLs; this also forbids any accidental external load; no-referrer so the token never leaves in a header |
| URLs never touch the filesystem: a static table (`include_bytes!`) + strictly parsed generated paths (installed season, `z <= 3`, plain digits), no percent-decoding | no traversal surface at all (`../`, `%2e%2e`, absolute paths are just unknown paths) |
| head <= 16 KB (`431`), body <= 16 MB (`413`, refused before reading), `Content-Length` required (`411`, no chunked), 10 s read timeout / deadline | a local process or page can't tie the server up with garbage |
| the token / URL is never logged | it is the secret |

`Cache-Control`: `no-store` for pages and data (a Save changes `canon.js`), `private, max-age=3600` for the JPEGs (the imagery never changes in a session) and `lib/`.

**Sticky port (D59).** The port is remembered in `<app_data_dir>/map_editor/port` (plain text, not `config.rs`) and tried first, else any free port. *Why:* a new port is a new browser origin, so the editor's `localStorage` (view, season, layers, language, ...) would start empty every session. The token still changes every session, which is fine: it is part of the path, not the origin.

**Save flow.** `POST save` -> size / header checks above -> `RoadTypes::validate_for_save(text, &nav)` (400 + `{"error"}` on a bad format / version / types, another game's nav, or skipped entries; nothing is written) -> **stamp `based_on`** = `RoadTypes::project_sha1()` (D60: the override replaces the project file wholesale, `based_on` says which project file it grew from so Setup can notice a later project update; the page never writes it, a client-sent value is replaced) -> `to_json_string()` -> atomic write (unique temp file + rename, so a crash never leaves half a file) to `override_path()` = `<app_data_dir>/map_editor/fh6-road-types.user.json` (**user data**: never in a cache folder, never deleted by "rebuild") -> `EditorData::set_road_types` (regenerates `canon.js` + `preview3d/roads.js`, so a reload of the page shows the saved state) -> the shared `Current` is replaced (`MapServer::current()`) -> `MapEvent::Saved{edges, points, bytes}` + `ctx.request_repaint()` -> `{"ok":true,"bytes":N}`. **The state is updated inside the server thread before the reply**, so "the app picks it up immediately" holds even when the egui frame loop is stopped (hidden window, see the threading notes in `docs/architecture/overview.md`); the UI only sees the event next frame. Saves are serialised. `Saved.edges` = typed game edges + user links.

**Tests.** `src/mapedit/server.rs` runs the HTTP layer against a fake backend over real sockets (token, Host, Origin, content type, OPTIONS, traversal, oversize, malformed heads, 503-while-preparing, Save with the project file keeping 94 races + `based_on`, invalid Save writes nothing, sticky port reuse). `real_install_serves_the_editor` (`#[ignore]`) starts the real server on the real install: `FORZA_DATA_DIR=<scratch> MAPEDIT_SERVE_SECS=600 cargo test --release real_install_serves -- --ignored --nocapture` prints `MAPEDIT_URL=...` for a browser check (**never** point `FORZA_DATA_DIR` at the real app data). Measured on the dev machine (release): cold ~3.0 s to `Ready` (empty cache), warm ~0.56 s.

### Review spots (editor, live port of the `REVIEW_*` rules)

- **UI:** editor panel, "Review spots" card (under "Show turnaround links"): checkbox **Highlight review spots**, a category filter (All / Highway gap / Cut-off highway piece / Half-marked road / Turnaround? / Isolated highway piece / Open end - near another road (likely unconnected) / Open end - dead end), a counter ("N spots - spot i of N"), **Prev / Next** (keys **N** / **Shift+N** while it is on) and a card with the category, a one-line reason (EN + DE), link count, length, nav road and centre x, z. Next / Prev recompute, then pan + zoom to the spot (max zoom 5.25); clicking the card re-centres. Highlight = pulsing red-orange glow with a bright core on every suspect link; the current spot is bigger, yellow-white, and ringed with a dashed white circle. Code: `rv*` functions + `drawHalo()` in the `EDITOR` block; test hook `FH6V.ed.review`. For the "near another road" category the editor also draws a thin dashed line to the suggested target and the card has a **Connect to suggested** button (key **C** while review is on; see the open-ends bullet below).
- **Unmarked (no type)** category: every live link with no type set (game edges without a `types` entry, `added` links with type null), clustered into spots by shared points; independent of the other categories (a gap link can also be unmarked). Card shows link count, total length, nav road, x / z. Drops live when the link is painted (project data currently: 2 edges, 0.0 km).
- **Rules (ported 1:1 from `fix_highways.py`, same thresholds; computed from the *current* editor state):** per nav road (polyline of `roads.json` = `RE.ids[q]`; links typed highway vs. non-highway-non-tunnel): highway share <= 30 % is the script's own `OVR_SPILL` (not a spot); 30-70 % -> `half` on its highway links; >= 70 % -> `gap` on its Road / Other / unset links. Highway-only islands (no tunnel / crossover / added link touching) <= 1500 m: other highway links on the same nav road -> `cutoff`; else, unless >= 80 % of >= 3 nodes with terrain are within 3 m of the ground (the script's `OVR_ISLAND`), -> `island` (`REVIEW_ISLAND`; no rows on the current data). 1-2-link nav roads between two highway nodes failing the crossover test (> 30 / 40 m, branching middle node, ends > 4 m apart in height, an end that is not a straight 2-link highway carriageway node (angle >= 130 deg, crossed at 25-155 deg), ends joined by <= 150 m of highway) -> `turn`, with every failed test in the reason.
- **Spots:** links of one category that share a point are one spot. Order = (category, nav road, smallest point id), which is stable under edits: fix a spot, it vanishes, **Next** goes on with what followed it.
- **Why live + in JS:** the script's CSV/JSON is stale after the first paint stroke; the user asked to *see the spots to fix them*, so the list has to shrink as they work (recomputed 400 ms after each edit; ~15 ms for the whole map). The editor already holds every edge with its live type, node heights and the nav-road membership (`roaded.js` ids), so no new data was needed. **Why a halo above the road canvas:** under it the dark casing of neighbouring highways hid the highlight at crossovers.
- **Counts on the canonical data (2026-10-06):** 37 spots = gap 6 (28 links), cut-off 1 (7), half-marked 2 (9), turnaround 28 (36), island 0 - the same link counts as the script's 80 review rows. The tunnel crossovers (3955,-1937), (3192,-2162), (1694,-2807), (1421,-3260), (3405,932) are all in the `turn` spots.
- **Open-ends categories (`opennear` / `open`, editor-only, no `fix_highways.py` counterpart):** every point with exactly ONE live link (any type; a turnaround counts as a link). Spots are per point (key = category, nav road of its link, point id), unlike the other categories, which are per link.
  - `opennear` "Open end - near another road (likely unconnected)": a link of another road passes within 25 m horizontally and 4 m vertically (a bridge above is not suggested), and is not part of the open end's own neighbourhood (anything reachable along links within 100 m of path, so a spur next to its own junction is not flagged). Candidate links exclude jump, turnaround and tunnel. Target = the nearest such link's endpoint if within 3 m of it, else a split point inside the link. The editor draws a thin dashed line to it; **Connect to suggested** (button / key **C**) adds a link of the open end's own type (splitting the target link first if needed) as ONE undoable commit.
  - `open` "Open end - dead end": all other open ends (cul-de-sacs, trail ends, map edge; jump-line ends are naturally open and kept here).
  - Constants: `RVP.OPEN_NEAR_M`=25, `OPEN_DZ`=4, `OPEN_OWN_M`=100, `OPEN_SNAP_M`=3.
  - **Why:** the user asked to "highlight all open ends of roads so I can easily check them and connect them properly". The nav graph is one connected component with only ~83 open ends, many genuine, so likely missing connections are surfaced first by checking for another road nearby that is NOT already reachable within 100 m (path distance, because component membership can't tell anything apart in a single-component graph). Same-height crossings without a shared node are rare (1 on 2026-10-06; 495 more are bridges > 4 m apart), so there is no separate crossing detector.
  - **Counts 2026-10-06 (project data):** 6 likely unconnected (1 of them < 5 m), 77 dead ends. ~6 ms for the whole review recompute.

### Preview mode (editor, key **P** / "Preview" button)

- **What:** the editor map switches to the **vanilla 2D map look** (`preview_2d.html` styles, widths, draw order and wBase formula) for the *current, live edited* state. Map tiles are removed, every map pane (POIs, labels, regions, race lines, review halo), the Layers panel, the editor box and the coords box are hidden; only the road canvas, the zoom buttons, a hint line and the **Exit preview** button (key **P** or **Esc**) remain. **B** cycles the backdrop none (transparent = the browser's white) / checker / dark (default, remembered), **T** shows the turnaround links (hidden by default like in the 2D preview). Turnarounds are hidden, user-added links look like game links (no white outline), no node dots.
- **Editing is disabled** (pointer handlers, Ctrl+Z, brush keys return early; only pan / zoom). Exit restores the previous state exactly: editor open or closed, panels, base map. If the editor was closed, preview opens it internally (the road canvas lives with it) and closes it again on exit.
- **Why a style swap in the editor and not an embedded `preview-2d.html`:** the editor's canvas already draws every edge with its live type; a second renderer would need the export re-built on every edit. The cost is that the styles are duplicated from `preview_2d.html` (`PVS` in `drawPv()`): keep them in sync.
- **Live 3D preview:** see the next section (key **G** / "3D" button, next to Preview).

### 3D preview (editor, key **G** / "3D" button)

- **What:** a full-window overlay with `preview-3d.html` in an `<iframe src="preview-3d.html#live=1&season=...">` showing the *current, live edited* state in 3D: every edge's type, new / moved points (at their stored heights), added and removed links. The button sits right of Preview (`#p3toggle`); **G** toggles, **Esc** or the "Back to editor (Esc)" button (parent overlay and iframe panel) closes. The editor's other key handlers (brushes, P, N, Space, Ctrl+Z) are off while it is open. Needs `preview-3d.html` + `preview3d/` next to `index.html` (built by `build_viewer.py`; without them the iframe shows a browser error page and the editor is unaffected).
- **Protocol (postMessage, `'*'` target - file:// origins are opaque):** iframe -> editor `{p3d:'ready'}` (terrain is up), `{p3d:'built', rev, ms, edges, km}`, `{p3d:'close'}`, `{p3d:'error', msg}`; editor -> iframe `{p3d:'roads', rev, nodes, edges, ids, tn, view}`, `{p3d:'view', view}`, `{p3d:'focus'}`. Both sides check `ev.source` (the iframe window / `window.parent`). The road payload is the **edge list**: `nodes` Float32Array `[x, z, y]*N` (every editor node; y NaN = none -> terrain), `edges` Uint32Array `[a, b, typeIdx]*M` (alive edges only, `a`/`b` = node indices), `ids` Int32Array (node ids, for debug / tests), `tn` = type names by index (the editor's order, 0 = `unset`), `view` = `{x, z, dist}` (the 2D map centre in world metres and the camera distance showing the same ground height at 45 deg fov; tilted 55 deg, yaw 0) or `null`. Jump direction is not sent (3D draws no arrows).
- **Why iframe + postMessage:** between two `file://` pages localStorage sharing differs per browser and `fetch()` is blocked, but `postMessage` into an embedded iframe works everywhere. **Why the road geometry is built in JS:** the editor holds the live state only in JS; baking roads in Python (the old design) made a live view impossible. The page now turns an edge list into ribbons itself (chain edges of one type through degree-2 nodes into runs -> resample every 5 m -> terrain y from the page's own mesh triangles + node y by linear interpolation between the edge's two nodes -> the same ribbon + wall buffers as before), so Road y, thickness, colours, per-type checkboxes and the km legend all keep working.
- **Unified with the standalone page (one code path):** `preview_3d.py` no longer resamples; `preview3d/roads.js` is the same kind of edge list (`nodes`, `edges`, `tn`, `lift`, `step`, `km`; deflate + base64, 0.57 MB instead of 3+ MB) and the standalone page calls the same `setRoads()`. `P3D_DEBUG.check(n, mode)` still self-checks (now against the edge list: node y must equal the interpolation of the edge's two nodes, recomputed independently of the resampler; terrain y = mesh triangle height + lift); `preview_3d.py` checks the written `roads.js` round-trips and prints the node-height diagnostics.
- **Re-open behaviour:** the iframe is created on the first open and kept alive (terrain + ~6 MB texture are not reloaded); each open re-sends the road data only if an FNV hash over the arrays changed, and the camera only if the 2D map view changed since the last open (so the 3D camera stays where you left it). The per-type visibility the user set in the 3D panel survives a rebuild; the legend's km are recomputed (sum of the edges' 2D length = the editor's stats).
- **Cost (39 498 edges, 195 700 road samples):** rebuild ~70-110 ms in the page (chain + resample + buffers, measured in headless Chromium; once 2.8 s while the machine was loaded), first open ~1.3-1.7 s incl. loading the iframe (terrain decode + texture). Test hooks: `P3D_DEBUG.liveInfo()`, `.edgeAt(x, z, r)` (nearest edge + type), `.vertexNear(x, z)`, `.nodeById(id)`, `.src()`, `.km()`.

## 2D preview (`preview-2d.html`)

A vanilla-in-game-look road map: transparent html/body/canvas, frameless (D34), so it can sit over anything. One self-contained file (~0.5 MB, no CDN).

- Source: `<work>/roads.json` + road types. Refuses `--out` inside the repo, exits on unknown type names; a null/missing `added` type draws as unset.
- Widths are **uniform**: the nav road class does not correlate with importance, so it cannot drive widths. Highway is the one exception (1.6x), from the user's own marking.
- Styles: road `#f2f4f7` + dark casing, highway `#fff6dd`, offroad sand, other blue-grey, trail dashed sand, tunnel ghost dashes, cross-country dotted pale green, jump orange arrow, unset grey dashed.
  **Turnaround is counted but not drawn** (it is not on the in-game map); key **T** toggles it for debugging.
- The page self-checks drawn counts against the generator's counts (`window.__P2D.counts` vs `.expected`).

## 3D preview (`preview-3d.html`)

- **Mesh:** the 8 m elevation raster decimated 2x = 16 m vertices, heights uint16 in 0.025 m steps from -5 m (0 = no terrain).
  Interior holes (9121 px, incl. one missing 512 m cell) are filled from the coarse `uberheightfield` (`extract_terrain.py --coarse`, cached under `<out>/preview3d/cache/terr_c/`, ~55 s on the first run).
  *Why:* interpolating a 512 m hole across rugged terrain gives a flat patch. Open sea beyond the data gets a smooth 1000 m skirt sloping down to y = 40, so the mesh has neither cliff nor pit.
- **Texture:** the game's level-3 season tiles stitched to 4096² per season (5.4 m/texel; 8192 is available but untested). Only the selected season is loaded.
- **Road height — design history.** D35/D43 first decided that roads **drape on the terrain height and never use node height** ("so it just looks more polished"). After seeing it the user reversed this (2026-10-04):
  "a lot of stuff that I didn't expect to look so broken… switch it to use the actual node data… implement something so I can switch in between the two." So now:
  **Road y = Nodes** (default: roads.json node heights, linear per edge; v2 points/moved use their own y) or **Terrain (drape)** (exact rendered-triangle height + 0.6 m), plus **Never below terrain** (max of both).
  The switch is a shader uniform, so it is instant; both heights are computed per road sample in the page (`resampleRuns`: terrain y from the mesh triangles, node y interpolated between the edge's two nodes) from the edge list in `roads.js` - see the editor's 3D preview above.
  **Cross-country is always draped:** `resampleRuns(..., drape)` sets node y = terrain y for type `crosscountry`, so it follows the ground in every Road-y mode (Nodes mode used to cut through hills, up to 4.8 m on the current data, because user points are only placed at terrain height every few tens of metres). `P3D_DEBUG.check` expects terrain y for crosscountry in node mode. Other types are unchanged.
  **Jump lines are a taut string:** each `jump` edge is its own run (not chained through degree-2 points), resampled every 2 m, and `tautString()` sets its height to the **upper convex hull** of the two end points (node y + lift, raised to terrain + lift if a point sits underground) and every sample's terrain + lift: straight where it clears the ground, resting on the terrain where the ground rises above the chord (e.g. a cliff edge before the drop). The result goes into BOTH heights, so all three Road-y modes show the same line. On the current data: 17 jump edges, 8 of them bend > 0.5 m, max 11.3 m above the straight chord (edge ending at x 1587.7 z 4473.3). `P3D_DEBUG.check` checks jump rows for y >= terrain + lift, end points = their node y, and concavity against the neighbours. *Why:* the user: "make sure that jump drawing ALWAYS behaves like this: Like a string wrapped around the terrain of its two connecting points" (a straight chord cut through the cliff edge and vanished into hills).
- **Road thickness** (0-20 m, default 3): the user wanted floating roads (bridges) to read as decks, not paper-thin. A child wall mesh (left/right walls + underside) is extruded down in the vertex shader by a uniform,
  so the slider is live without a rebuild. Walls are the road colour x 0.72, depth-tested.
- **Look:** ribbons keep a screen-space minimum width; asphalt `#34373d`, highway `#2b2e34` 12 m wide, optional bright edge lines; turnaround magenta dashed (off by default); tunnel and jump off by default.
- **Known limits:** ~39 stretches sink 50-100 m below the terrain in node mode: they are **unmarked tunnels** (e.g. x 2956 z 1051, x -4229 z -5248). Fix: mark them Tunnel in the editor, or enable "Never below terrain".
  152 bridge stretches float, which is correct. The 16 m mesh stair-steps at coasts.

**Road colours (`style=type|map` hash, panel dropdown):** switching only swaps uniforms/material flags, so it is instant. *Type colours* (default): unset red #ff1a1a, highway amber #fbbf24 (18 m + casing), road #38bdf8, offroad #ff8c1a, other #4ade80, trail #facc15 dashed, cross-country #a78bfa, tunnel #e2e8f0 drawn on top (depth test off), jump #f43f5e dashed (no direction arrows: `jump_from` is not read in 3D; height = taut string over the terrain, see Road height), turnaround #d946ef dashed; every type is visible. *Map look*: the earlier style and defaults (turnaround / tunnel / jump off). Unpainted edges are their own `unset` type in `preview_3d.py` (drawn like Other in Map look). *Why:* the user wants to QC their marking, so the preview must show the individual road types.

## Why previews are local pages built next to the viewer (D39)

They contain game data, so they cannot ship; `build_viewer.py` therefore runs `preview_2d.py` and `preview_3d.py` after the build into the same out dir (failures are warnings, `--no-previews` skips). Both previews also run standalone.
