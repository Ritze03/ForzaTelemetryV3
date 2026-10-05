# FH6 map tooling — road editor v2, 2D preview, 3D preview (standalone + live in the editor)

Phase-H tooling in `tools/fh6-extract/` (usage and flags: [its README](../../tools/fh6-extract/README.md)). This page holds the
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
- The canonical `tools/fh6-extract/data/fh6-road-types.json` is the committed copy and is **v2** since 2026-10-05 (the user's editor export, then run through `fix_highways.py`): ids plus the user's 42 points (ids >= 1000000), 116 added links (incl. the jump links with `jump_from`), 1 removed edge and 94 of 170 race marks (refreshed 2026-10-06 = user export (7) + `fix_highways.py` + restored race marks). Game edges by type: road 21747, offroad 11115, highway 4979, tunnel 904, other 317, turnaround 317 (km: road 426.3, offroad 207.1, highway 103.4, tunnel 18.1, other 6, turnaround 6.8, cross-country 2.4, jump 6.8). Committing the coordinates is fine (D48: the user's own work). In the browser the editor's localStorage autosave keeps the old state until "Reset to project data" (see [hand-classified road and race types](fh6-game-files.md#hand-classified-road-and-race-types)).

## `fix_highways.py` — highway clean-up

`fix_highways.py IN.json OUT.json [--work DIR] [--report BASE] [--dry-run] [--validate] [--no-overmarks] [--no-turnarounds] [--plot PNG [--bbox X0,Z0,X1,Z1]]`. Reads v1/v2, writes v2 (editor key order, `counts` as `exportObj()`), idempotent, refuses a different nav sha1/nodes. Review CSV/JSON/PNG go outside the repo.

**Why:** the editor brush also painted roads stacked under/over a highway, and hand-marking hundreds of crossovers was not wanted.

- **Over-marks (highway -> road):** `OVR_SPILL` = nav road with highway share <= 30 % (tunnel edges excluded); `OVR_ISLAND` = highway piece joined to no other highway/tunnel/turnaround/added link, <= 1500 m, >= 80 % of nodes within 3 m of terrain.
  *Why no "stacked" rule (2D overlap + height gap):* Tokyo City has real double-deck expressways (nav roads #800/#801, ~9 m apart, lower deck on the ground, each with its own crossovers) and interchanges up to 10 layers; a stack rule would delete real lower decks. Nav `cls` (4/5 direction, 6 ramps), `hi` and terrain height do not separate streets from highways either. Highway marks are nearly always whole nav roads (121/138 polylines 100 % highway), so per-nav-road topology is the reliable signal.
- **Turnarounds:** nav polyline of 1 edge (<= 30 m) or 2 edges (<= 40 m) typed road/highway/unset, both ends degree-3 nodes whose other two edges are highway and ~collinear (>= 130 deg), strip crossing the carriageway (25-155 deg), end heights within 4 m, ends not joined by <= 150 m of highway. *Why:* the game stores each crossover as its own 2-node road record between two carriageway nodes. Calibrated on the user's 12 hand marks (12/12 re-found with `--validate`).
- **Needs review (nothing changed):** `REVIEW_HIGHWAY_GAP` (road edges inside mostly-highway nav roads, likely under-marks), `REVIEW_CUT_OFF_PIECE`, `REVIEW_HALF_MARKED`, `REVIEW_TURNAROUND` (strip-like links failing a test, e.g. crossovers inside tunnels).
- **First run (2026-10-05):** 32 edges / 0.63 km highway -> road (all Tokyo City), 305 new turnarounds, 76 review rows. **Second run (on export (7), 2026-10-06):** 31 highway -> road, 305 turnarounds, 80 review rows = `REVIEW_TURNAROUND` 36, `REVIEW_HIGHWAY_GAP` 28, `REVIEW_HALF_MARKED` 9, `REVIEW_CUT_OFF_PIECE` 7.
- **Fixing the review rows:** the editor's **Review spots** (below) runs these rules live, so the user can walk the list and fix it by hand.

## Editor (viewer, `EDITOR` block of `viewer_template.html`)

- **Modes:** Pan, Paint, Connect, Draw, Split, Move, Flip jump, Delete link, Race types. **Brush keys:** 1 Road, 2 Offroad, 3 Other, 4 Trail, 5 Cross-country, 6 Tunnel, 7 Jump line, 8 Highway, 9 Turnaround, **0 Clear**
  (key 4 used to be Clear; it moved to 0). One undo stack (Ctrl+Z) covers all modes.
- **Draw** (D42): click chains free points; clicking an existing point snaps and connects (**Alt** = no snap); ends with Esc, right-click or a click on the last point; a lone point is dropped.
- **Split** (D36): click an edge, a point appears at the projected click position, drag to place it; one undo step; both halves keep the type.
- **Move:** drag any point. A game node goes to `moved`, a user point to `points`; dragging it back onto the original position drops the override.
- **Flip jump** reverses a jump's direction (D41: jumps are one-way, drawn as take-off to landing arrows).
- **Delete link** also removes game edges (they go to `removed`); user points left without a link vanish.
- **Heights (D40):** new / moved points take the **bilinear height on the 8 m elevation grid**. If both neighbours are >= 3 m off the terrain (bridge / tunnel), or the edge is a tunnel, or there is no terrain,
  the height is the **linear interpolation between the neighbours**; a tunnel point with a single neighbour copies its height (drawn tunnels stay level); at a junction the two most opposite neighbours are used.
  *Why:* the height map gives the ground under a bridge or the hill above a tunnel, which is the wrong height for the road itself.
- **Styles:** trail dashed, tunnel dark casing + pale core, cross-country violet, jump white arrows, highway amber wide with casing, turnaround fuchsia dotted, user links white outline
  (no longer dashed: dash now means Trail), user points lilac, moved game nodes an orange ring. **"Show turnaround links"** checkbox hides them from drawing *and* hit-testing; picking the Turnaround brush re-shows them.
  **Prefill from surface data** only touches Road / Offroad. In edit modes popups/labels no longer swallow clicks.
- `elevation.js` (viewer data) is the full 8 m raster, delta-coded per row (`grid.delta=1`, ~4.7 MB; was 16 m) so the editor's height lookups match the previews. `roaded.js` carries the node height `y` (0.1 m) per vertex.

### Review spots (editor, live port of the `REVIEW_*` rules)

- **UI:** editor panel, "Review spots" card (under "Show turnaround links"): checkbox **Highlight review spots**, a category filter (All / Highway gap / Cut-off highway piece / Half-marked road / Turnaround? / Isolated highway piece), a counter ("N spots - spot i of N"), **Prev / Next** (keys **N** / **Shift+N** while it is on) and a card with the category, a one-line reason (EN + DE), link count, length, nav road and centre x, z. Next / Prev recompute, then pan + zoom to the spot (max zoom 5.25); clicking the card re-centres. Highlight = pulsing red-orange glow with a bright core on every suspect link; the current spot is bigger, yellow-white, and ringed with a dashed white circle. Code: `rv*` functions + `drawHalo()` in the `EDITOR` block; test hook `FH6V.ed.review`.
- **Rules (ported 1:1 from `fix_highways.py`, same thresholds; computed from the *current* editor state):** per nav road (polyline of `roads.json` = `RE.ids[q]`; links typed highway vs. non-highway-non-tunnel): highway share <= 30 % is the script's own `OVR_SPILL` (not a spot); 30-70 % -> `half` on its highway links; >= 70 % -> `gap` on its Road / Other / unset links. Highway-only islands (no tunnel / crossover / added link touching) <= 1500 m: other highway links on the same nav road -> `cutoff`; else, unless >= 80 % of >= 3 nodes with terrain are within 3 m of the ground (the script's `OVR_ISLAND`), -> `island` (`REVIEW_ISLAND`; no rows on the current data). 1-2-link nav roads between two highway nodes failing the crossover test (> 30 / 40 m, branching middle node, ends > 4 m apart in height, an end that is not a straight 2-link highway carriageway node (angle >= 130 deg, crossed at 25-155 deg), ends joined by <= 150 m of highway) -> `turn`, with every failed test in the reason.
- **Spots:** links of one category that share a point are one spot. Order = (category, nav road, smallest point id), which is stable under edits: fix a spot, it vanishes, **Next** goes on with what followed it.
- **Why live + in JS:** the script's CSV/JSON is stale after the first paint stroke; the user asked to *see the spots to fix them*, so the list has to shrink as they work (recomputed 400 ms after each edit; ~15 ms for the whole map). The editor already holds every edge with its live type, node heights and the nav-road membership (`roaded.js` ids), so no new data was needed. **Why a halo above the road canvas:** under it the dark casing of neighbouring highways hid the highlight at crossovers.
- **Counts on the canonical data (2026-10-06):** 37 spots = gap 6 (28 links), cut-off 1 (7), half-marked 2 (9), turnaround 28 (36), island 0 - the same link counts as the script's 80 review rows. The tunnel crossovers (3955,-1937), (3192,-2162), (1694,-2807), (1421,-3260), (3405,932) are all in the `turn` spots.

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
- **Road thickness** (0-20 m, default 3): the user wanted floating roads (bridges) to read as decks, not paper-thin. A child wall mesh (left/right walls + underside) is extruded down in the vertex shader by a uniform,
  so the slider is live without a rebuild. Walls are the road colour x 0.72, depth-tested.
- **Look:** ribbons keep a screen-space minimum width; asphalt `#34373d`, highway `#2b2e34` 12 m wide, optional bright edge lines; turnaround magenta dashed (off by default); tunnel and jump off by default.
- **Known limits:** ~39 stretches sink 50-100 m below the terrain in node mode: they are **unmarked tunnels** (e.g. x 2956 z 1051, x -4229 z -5248). Fix: mark them Tunnel in the editor, or enable "Never below terrain".
  152 bridge stretches float, which is correct. The 16 m mesh stair-steps at coasts.

**Road colours (`style=type|map` hash, panel dropdown):** switching only swaps uniforms/material flags, so it is instant. *Type colours* (default): unset red #ff1a1a, highway amber #fbbf24 (18 m + casing), road #38bdf8, offroad #ff8c1a, other #4ade80, trail #facc15 dashed, cross-country #a78bfa, tunnel #e2e8f0 drawn on top (depth test off), jump #f43f5e dashed (no direction arrows: `jump_from` is not read in 3D), turnaround #d946ef dashed; every type is visible. *Map look*: the earlier style and defaults (turnaround / tunnel / jump off). Unpainted edges are their own `unset` type in `preview_3d.py` (drawn like Other in Map look). *Why:* the user wants to QC their marking, so the preview must show the individual road types.

## Why previews are local pages built next to the viewer (D39)

They contain game data, so they cannot ship; `build_viewer.py` therefore runs `preview_2d.py` and `preview_3d.py` after the build into the same out dir (failures are warnings, `--no-previews` skips). Both previews also run standalone.
