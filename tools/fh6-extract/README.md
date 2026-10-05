# fh6-extract

Research scripts that read **map imagery, roads, POIs, race starts / racing lines, speed-limit signs, terrain, car names,
area / region names and the game's map icons** from the user's own Forza Horizon 6 install. Format details, validation and unknowns:
[`docs/game-data/fh6-game-files.md`](../../docs/game-data/fh6-game-files.md),
[`docs/game-data/fh6-terrain.md`](../../docs/game-data/fh6-terrain.md) and
[`docs/game-data/fh6-cars-names-icons.md`](../../docs/game-data/fh6-cars-names-icons.md).

- **Read-only** on the game install.
- Output is Playground Games' data: write it **outside the repo** (default `./fh6-out` is relative
  to your cwd — run from a scratch dir or pass `--out`). Never commit it.
- Deps: `python3`, `numpy`, `Pillow` (BC7 icons use its `bcn` decoder; tested with 12.3.0). `scipy` is optional (speeds up `roaddist.py` and gives
  `road_dist_m` in `extract_speedsigns.py`; required only for the race-name heuristic with `--entity-model`). `lz4` (pip) is optional and
  speeds up PGZP reads; `texture2ddecoder` (pip) is an optional BC7 fallback for `extract_icons.py` if your Pillow lacks BC7.
- Every script has `--help`; none hardcodes a path (imports resolve relative to the script).

All scripts take `--media <...>/steamapps/common/ForzaHorizon6/media` (default: auto-detect via
Steam's `libraryfolders.vdf`, see `fh6common.py`) and `--out DIR` (default `./fh6-out`).

| Script | Output | Notes |
|---|---|---|
| `extract_map.py [--level 3] [--seasons Summer,Winter]` | `map_<season>_L<level>.png` + `_preview.png` | BC1 swatchbin tiles → PNG. L3 = 8192², L2 = 4096² (fast; ~25 MB PNG). |
| `decode_nav.py [--map IMG]` | `roads.json`, `roads.png`, `roads_on_map.png` | `Brio_00.nav` road graph. `--map` (optional) = full-map image from `extract_map.py` for the overlay. |
| `extract_poi.py [--entity-model ZIP]` | `pois.json` | ~5.6k POIs from `.nt`/`.tz`/`gameobjs.xml`/`.owt`/route files (+ arenas, car meets, train line). `race_pin` = race map pin (NOT the start). `--entity-model` adds the creator-dump-only categories (photo spots, time attacks, chase starts…). |
| `extract_geochunk.py [--skip-pgeo]` | `geochunk_pois.json` | exact positions: `Ribbon_00/GameObjs.xml` (793 objects: speed traps/zones, drift zones, XP boards, …) + GeoChunk0 `.pgeo` (danger signs, drift-zone posts). Takes ~1 s. |
| `extract_races.py [--entity-model ZIP]` | `races.json` | start line, 12-slot grid, heading, finish for 169 routes from the `RVAN` block of `AITracks/Route<N>.nav`; exact names (EN+DE) for 111 routes and exact types for 67 from `ObjectModelGame.zip` (`fh6careers.py`); older fallback names for the rest (more with `--entity-model`). Optional validation vs `roads.json` if it is in `--out`. |
| `extract_terrain.py [--region X0,Z0,X1,Z1] [--res 4] [--surf-res 8] [--coarse] [--no-elevation] [--no-surfaces]` | `elevation.npy/.json/.png`, `surfaces.npy/.json/.png` | terrain height raster + surface-id raster from GeoChunk0. Full island: minutes and a few GB RAM — use `--region` (world metres) for a quick test, e.g. `--region -87,1460,1413,2960`. `--coarse` = whole-island low-detail elevation (~40 s). Surface names are **ours** (`fh6surfaces.py`: confirmed in-game / seen / reasoned); the class colours only group ids for the preview. |
| `classify_roads.py [--step 4] [--jobs 8]` | `roadsurf.npz` | terrain surface id under every nav road (exact `.phys` triangle at 4 m spacing, node-height rule for bridges); needs `roads.json` **with `heights`** (current `decode_nav.py`) in `--out`. ~16 s. `build_viewer.py` turns it into paved / off-road / unknown runs (kinds from `fh6surfaces.py`). Method: [fh6-terrain.md](../../docs/game-data/fh6-terrain.md#surface-kind-paved--off-road-and-the-surface-under-the-roads). |
| `extract_racelines.py [--step 5]` | `racelines.json` | 170 racing lines from `AITracks/Route<N>.owt`, trimmed to one drive / lap (127 point-to-point + 43 circuits), with left/right track edges (half-width vector). Needs `fh6owt.py`. |
| `extract_predictions.py [--refit]` | `predictions.json` | **predicted** race type (`type_predicted`, `type_confidence`, `type_source` = exact / event / ai_family / not_a_race / copy / line / id_convention, `type_why`, `type_alternatives`) and map pin (`pin_predicted {x, z, method, expected_err_m}`) for all 170 routes. Needs `races.json`, `pois.json`, `racelines.json` in `--out` (run after the three extractors; `build_viewer.py` does that); ~20 s (terrain collision under every racing line). Prints a self-check against the marks in `data/fh6-road-types.json` (92/93, AI family alone 83/85). `--refit` re-fits the line-rule thresholds (leave-one-out accuracy, writes nothing). Methods in `racetype_method.py` / `pin_method.py`; validation and caveats: [fh6-game-files.md](../../docs/game-data/fh6-game-files.md#predicting-race-type-and-map-pin-position-validated-against-the-users-marks). |
| `fix_highways.py IN.json OUT.json [--work DIR] [--report BASE] [--dry-run] [--validate] [--no-overmarks] [--no-turnarounds] [--plot PNG [--bbox X0,Z0,X1,Z1]]` | `OUT.json` + `BASE.review.{csv,json}` | post-processes a `fh6-road-types` file (v1/v2 in, **v2** out in the editor's key order, `counts` recomputed like `exportObj()`, idempotent): highway over-marks -> `road`, crossover strips -> `turnaround`; lists needs-review edges without changing them. `--work` (default `./fh6-out-work`) needs `roads.json`, optionally `terr_e/elevation.npy` + `regions.json`; refuses a file whose nav sha1/nodes differ. `--validate` re-finds the file's own turnarounds on a copy. Keep review outputs outside the repo. Rules and why: [fh6-map-tooling.md](../../docs/game-data/fh6-map-tooling.md#fix_highwayspy--highway-clean-up). |
| `extract_speedsigns.py [--roads roads.json] [--variant-map "0=50,1=60"]` | `speedsigns.json` | 2141 speed-limit signs (1659 limit + 217 high-speed + 265 end-of-limit) from GeoChunk0 `signs_do` pgeo cells, with facing and (if `roads.json` present, scipy) road snap. `limit_kmh` stays null unless `--variant-map` — the km/h per variant is **not** in the files. |
| `extract_cars.py [--lang EN,DE\|all] [--ordinal N ...]` | `cars.json` | `CarOrdinal` → `MediaName` (671 `Cars/*.zip` central directories) → full / model name from `Data_Car.str`. ~0.5 s. |
| `extract_names.py` | `names.json`, `regions.json` | 75 landmark names + 10 map regions (outline polygon + names) in all 24 languages, stunt name tables. |
| `extract_icons.py`, then `build_icon_mapping.py OUT` | `png/**`, `icons.json`, `xml_symbols.json`; `mapping.json` | the game's map icons (1074 PNGs: `Horizon_Map.zip` BC7 swatchbins + atlas crops + 6 ext) and which icon the game draws for which map-element type; `mapping.json` = our POI category → icon (38 derived, 7 guessed, 8 none). |
| `roaddist.py` | stdout | Per-type distance to nearest road (needs `roads.json` + `pois.json` in `--out`). |
| `plot_pois.py --map IMG` | `pois_on_map.png`, `pois_dense_on_map.png` | Needs `pois.json` in `--out`. |

## Map viewer (`build_viewer.py`)

A local, interactive Leaflet map of everything above — for browsing what the install gives us before deciding what goes into the app.

```
python3 -B build_viewer.py --out /some/dir/outside/the/repo      # then open  /some/dir/outside/the/repo/index.html
```

- **Self-sufficient:** runs the extractors itself into `--work` (default `<out>-work`; steps whose output already exists are skipped, `--force` redoes
  them). First run ≈ 5 min (terrain reads the 40 GB GeoChunk0; measured 270 s with one season), later runs ≈ 10–20 s. `--no-terrain` skips elevation/surfaces,
  `--seasons Summer` builds one season only. A layer whose extractor fails is skipped with a `WARNING` line; the rest is still built.
- **Output is Playground Games' imagery/data** (~100 MB): keep it outside the repo, do not publish or commit it. The script refuses `--out`/`--work` inside the repo.
- **Opens from `file://`** (data is `<script src="data/*.js">`, no `fetch`). It needs internet for the Leaflet + markercluster CDN scripts (unpkg).
  Elevation/surface hover lookups use `DecompressionStream` (Chrome/Edge 80+, Firefox 113+, Safari 16.4+).
- **What it shows:** the game's own tile pyramid as base map (4 seasons, JPEG; zoom 0–3 = levels L0–L3, overzoom to 7), roads by surface kind (paved / off-road / unknown, click = nav class, this stretch, whole-road split, dominant ids with confirmed/reasoned status) or by nav class, regions (outline + names), landmarks (75),
  every POI category with the game's own icon where `mapping.json` has one (coloured circle otherwise; dense layers clustered and off by default), race starts (+ 12-slot grids),
  170 race lines with track edges (ribbons appear from zoom 4), speed-limit signs by variant with a heading arrow, stunt gates, surfaces raster (54 ids, our names with
  confirmed / seen / reasoned status; click a legend row to isolate one id), elevation hillshade. EN/DE (and the other 22 languages for landmark / region names) toggle;
  click = popup (type, name, x/z/y, source file, extras); the mouse readout (bottom left) shows telemetry-space x/z, terrain height and surface under the cursor;
  right-click = coordinates popup; the search box finds names or jumps to `x z`.
- Design notes: Leaflet CRS units = pixels of the 8192 map (`lat = -py`, `lng = px`), custom `scale = 2^(zoom-3)` so Leaflet zoom 3 is native 1:1 and the 1024 px game tiles
  are used as-is (`tileSize 1024`). Everything vector shares one canvas renderer (several canvases would swallow each other's clicks). Race names are English only
  (no German source in the files).

### Road editor (and race-type marking) in the viewer

The viewer has an **Editor** button (top left) for hand-classifying the nav road network and the race types, then exporting the result. The user wants the data for the app
(e.g. a "paved / off-road" lookup), and the game files do not give a trustworthy per-road type (see `docs/game-data/fh6-terrain.md`), so a human paints it.

- **Roads** have nine types (**Road, Offroad, Other, Trail, Cross-country, Tunnel, Jump line, Highway, Turnaround**; meanings and the why: [fh6-map-tooling.md](../../docs/game-data/fh6-map-tooling.md#road-types--what-the-user-means-by-them)); an edge nobody painted is **not set** (magenta, so it is easy to see what is left). An *edge* is the stretch
  between two consecutive nav nodes (39 383 of them). Modes: **Pan**, **Paint** (pick a brush, click or drag; Shift-click or the "fill" tick paints the whole road between two
  junctions; hold Space to pan while painting; brush keys below), **Connect** (zoom in until the points show, click A then B: a new link (white outline) gets the current brush type; yellow rings = dead ends, i.e. the usual gaps), **Draw** (chain free points; click an existing point to snap, Alt = no snap; Esc / right-click ends), **Split** (click an edge, drag to place the new point), **Move** (drag any point), **Flip jump**, **Delete link** (also game edges; they go to `removed`).
  Brush keys: 1-9 = Road, Offroad, Other, Trail, Cross-country, Tunnel, Jump line, Highway, Turnaround and **0 = Clear** (was 4). A "Show turnaround links" checkbox hides turnarounds. **Review spots** (checkbox + Prev / Next, key N) highlights what `fix_highways.py` could not decide (gaps / half-marked / cut-off highways, doubtful turnarounds), computed live from your current paint; **Preview** (button, key P) shows the map in the vanilla 2D look for the current state (B backdrop, T turnarounds, editing off) - details: [fh6-map-tooling.md](../../docs/game-data/fh6-map-tooling.md#review-spots-editor-live-port-of-the-review_-rules). Full v2 behaviour (heights, styles): [fh6-map-tooling.md](../../docs/game-data/fh6-map-tooling.md#editor-viewer-editor-block-of-viewer_templatehtml).
  **Prefill from surface data** sets paved -> Road and off-road -> Offroad per edge (majority of the smoothed 4 m terrain samples on that edge, from `roadsurf.npz`); unknown edges
  (no collision mesh / elevated / unidentified id) stay *not set*. It asks before overwriting existing paint. Ctrl+Z undoes (200 actions). Totals in km are shown live.
- **Races** (mode **Race types**): click a race pin (or its race line) and pick **Road / Street / Rally / Cross Country / Touge / Drag / Story / Wristband event** (or "Clear mark"). The pin takes
  that type's own game icon (Road = asphalt, Rally = mixedsurface, Cross Country = crosscountry, each in its circuit / point-to-point variant; Street, Touge, Drag have one icon; Story = the Horizon Story map marker, Wristband event = the orange wristband from the pause-menu art, since the game has no wristband map pin). **Story** = a Horizon Story pin, **Wristband event** = e.g. the "Mech My Day" showcase (DE "Armband-Event").
  **Every race starts unmarked and shows the greyed icon with a "?"** - nothing is inferred into your marks from geometry, surface or names (predictions are a separate, labelled layer, see below). Circuit vs point-to-point is exact (`.owt` header) and not marked
  by hand. **Prefill race types (exact)** (button in the editor panel) sets the type of every race whose type the game files state exactly (67 of 170: road 21, cross country 19, street 15,
  touge 5, rally 4, drag 3; source `ObjectModelGame.zip`, see `docs/game-data/fh6-game-files.md#exact-race-names-and-types-objectmodelgamezip`); the other 103 stay unmarked (the 16 scramble/trail
  routes too - a name word is not a type field). It never overwrites a different manual mark without a `confirm()`, and says how many it set. The pin tooltip and popup show the **exact race
  name** in the current language (111 routes, EN + DE; others show `Route <N>`), plus "Game files: Road (exact type from game files)" and, if your mark differs, "your mark differs".
  One pin per route (170 routes in `racelines.json`; route 99, the test route, has no POI and is placed at the start of its race line; IE routes and Horizon Chases are placed at their start line).
  **Where the pin sits:** at the in-game **map pin** where the game files give one, else on the RVAN start line. Exact map pins: 36 routes from `race_trigger_zone_rt<N>` + route 5411 from the `sidi_touge_event_5411`
  locator (37 of 170; link = the route id inside the object's name, nothing guessed). Those markers have a solid orange ring and a dashed connector to the start line; the other 133 have a dashed white ring,
  the tooltip and popup say "predicted: at start line (+-N m)" (validated: routes without a sphere have their pin at the start line; route 8007 uses the nearby special-event marker instead; the popup also gives the pin source and the pin-to-start distance). The editor panel counts "Exact map pins". Marks stay keyed by route id,
  so moving a marker never changes the autosave or the export. The three drag meets and the touge events are separate POI categories (the touge events are used as described, the drag meets are not linked).
- **Predictions** (`predictions.json`, see [fh6-game-files.md](../../docs/game-data/fh6-game-files.md#predicting-race-type-and-map-pin-position-validated-against-the-users-marks)): the race popup has three separate rows - **Game files** (exact), **Your mark**, **Predicted** (type, confidence %, source, one-line why;
  the `id_convention` source is labelled as a naming convention, not a game field). The editor panel has a **Show predicted types on unmarked pins** toggle (a faint predicted icon instead of the grey "?"; "?" stays where
  no prediction reaches 50 %), and a **Review queue** ("N to review", also on the Editor button): prev / next zoom to the route and highlight its line; the card shows the exact name, your mark, the prediction and why;
  **Accept** sets your mark to the prediction (an ordinary mark: Ctrl+Z undoes it), **Mark...** opens the type palette on the pin, **Skip** hides the entry (kept in `localStorage` key `fh6viewer.rqskip`, never in the export;
  "restore N skipped" brings them back). The queue is rebuilt from the *current* marks, in this order: (1) marked but the prediction disagrees (race-type marks before story / wristband marks), (2) unmarked, predicted
  at >= 70 % (story routes the game files state outright are not queued), (3) unmarked story routes next to a route you marked Story, (4) unmarked, low-confidence. Predictions never change your marks by themselves.
- **Project data** (`tools/fh6-extract/data/fh6-road-types.json`, committed, **v2**: ids + types, plus the user's own points / added links): the user's finished hand classification (see `docs/game-data/fh6-game-files.md#hand-classified-road-and-race-types`).
  `build_viewer.py` embeds it as `data/canon.js`; when the browser has **no saved editor work** the editor starts from it (not copied into `localStorage` until you edit, so a newer build's project data
  shows through), and the **Reset to project data** button (with `confirm()`) loads it over the current state. Saved `localStorage` work keeps priority. If its nav sha1 / node count differ from the
  install's, it is not loaded, the button is disabled and the panel says so. *Why committed:* it is the user's own work and the only copy that must never be lost; it holds no game data.
- **Autosave** to `localStorage` (guarded; `file://` may block it) after every change and restored on load; **Export** downloads `fh6-road-types.json`; **Import** reads it back
  (asks before replacing, warns if the nav sha1 / node count differ).
- **Export format** (`fh6-road-types`): **v2** is written now (`points` / `moved` / `removed` / `jump_from`, and why coordinates are allowed: [fh6-map-tooling.md](../../docs/game-data/fh6-map-tooling.md#fh6-road-types-v2)); v1 files still import. The v1 form, ids and types only with **no coordinates**, looked like this:

```json
{"format":"fh6-road-types","version":1,"nav":{"file":"Brio_00.nav","sha1":"a88c69f4...","nodes":38473},
 "types":{"1-2":"offroad","2-3":"offroad","40-41":"road"},
 "added":[{"a":25028,"b":30062,"type":"offroad"}],
 "races":{"41":"touge","51":"cross_country","30100":"drag"},
 "counts":{"km":{"road":502.2,"offroad":200.8,"other":0.1,"not_set":61.3},"edges":{"total":39383,"painted":36303,"added":1},"races":{"marked":3,"total":170}}}
```

  `types` key = `"<idA>-<idB>"` with idA < idB, both **stable nav node ids** (`a` in the node struct); only painted game edges are listed. `added` = user-made links
  between two node ids (`type` may be `null` = unset). `races` key = route id, value `road|street|rally|cross_country|touge|drag|story|wristband` (the first editor versions wrote `midnight`; Import maps it to `story`, and route 8004 to `wristband`). `counts` is informational.
  *Why ids and not coordinates:* the export must contain no game data (licensing rule), and positions are re-read from the user's own install when the file is used. Node ids are
  unique over all 38 473 nodes (checked by `decode_nav.py`), so they are a safe key; the nav sha1 pins the graph version.
- Implementation: `decode_nav.py` exports `ids` (node id per polyline vertex), `nav` {file, sha1, nodes} and `orphans` into `roads.json`; `build_viewer.py:build_roaded` writes
  `data/roaded.js` (ids + one prefill digit per edge); the editor is one self-contained block (`EDITOR`) in `viewer_template.html` drawing all edges on its own canvas layer with a
  grid index for hit tests, so painting 39 k edges stays smooth (no per-edge Leaflet layers).

## 2D and 3D previews

Local pages, same licensing rule as the viewer (game data: `--out` outside the repo, never committed; both scripts refuse an `--out` inside it). `build_viewer.py` runs both after the build
(`python -B <script> --out OUT --work WORK`; a failure is only a warning; `--no-previews` skips); they also run standalone. Why and design history: [fh6-map-tooling.md](../../docs/game-data/fh6-map-tooling.md).
Viewer data changed for them: `roaded.js` has per-vertex node `y` (0.1 m); `elevation.js` is the full 8 m raster, delta-coded per row (~4.7 MB, was 16 m).

```
python3 -B preview_2d.py --out OUT --work WORK [--road-types PATH]
python3 -B preview_3d.py --out OUT --work WORK [--media M] [--road-types PATH] [--seasons S,..] [--tex-size 2048|4096|8192]
                         [--jpeg-quality Q] [--decim 1|2|4] [--skirt 1000] [--step 5] [--lift 0.6] [--no-coarse]
```

- `--road-types` default = `data/fh6-road-types.json` (v1 or v2; point it at an editor export to preview unsaved work).
- **`preview_2d.py`** needs only `<work>/roads.json`; writes one self-contained `OUT/preview-2d.html` (~0.5 MB, no CDN), transparent and frameless (vanilla in-game look). Wheel zoom, drag pan,
  **F / 0 / double-click** fit, **B** cycles backdrop (none / checker / dark), **T** shows the turnaround links (hidden by default), URL hash `#x,z,scale[,backdrop]`, `window.__P2D`.
- **`preview_3d.py`** needs `<work>/terr_e/elevation.npy` + `roads.json` and `OUT/tiles/` (from `build_viewer.py`); writes `OUT/preview-3d.html` + `OUT/preview3d/{meta,terrain,roads,tex_<Season>}.js`
  (script-tag data, so `file://` works). three.js r147 comes from jsdelivr: **needs internet**. `--media` is only used for the coarse hole filler (`--no-coarse` interpolates holes instead;
  the coarse raster is cached in `OUT/preview3d/cache/terr_c/`, first run ~55 s). Controls: left drag pan, right / middle / Ctrl drag orbit, wheel zoom, WASD / arrows, Q/E rotate, +/- zoom, R reset.
  Panel: season, road height **Nodes (default) / Terrain (drape)**, "Never below terrain", road thickness 0-20 m (default 3), edge lines, per-type visibility, **Road colours: Type colours (default) / Map look** (see fh6-map-tooling.md, 3D preview).
  URL hash (all optional): `v=x,z,dist,yawDeg,pitchDeg`, `season=`, `exag=`, `edges=0`, `types=a,b`, `rh=node|terrain`, `above=1`, `th=<metres>`, `style=type|map`; console `window.P3D_DEBUG.check(n, mode)` self-checks road heights.
- **Testing headless:** python `playwright` is not installed; node Playwright works (chromium with `--use-angle=swiftshader --enable-unsafe-swiftshader` for WebGL). The MCP browser blocks `file:` navigation.

Library modules (imported, not run): `fh6common.py` (install detection, case-insensitive paths, `.nt` / `.tz`
readers), `pgzp.py` + `lz4b.py` (reader for the 40 GB `GeoChunk*.minizip` PGZP containers — seek-reads single
entries), `fh6str.py` (string-table reader + key hash), `fh6owt.py` (`.owt` racing-line reader + `RVAN` start/finish block), `fh6surfaces.py` (terrain
surface-id → name + kind table; each entry is marked confirmed in-game / seen / reasoned), `fh6bxml.py` (binary-XML decoder; `--entity-model`, and the plaintext `ObjectModelGame.zip`), `fh6careers.py` (exact race names + types from it), `racetype_method.py` / `pin_method.py` (predicted race type / map pin; driven by `extract_predictions.py`).
`pgzp.py` reads all four `GeoChunk*.minizip` (handles the u64-N table of GeoChunk2 and the last entry of the file).

**`--entity-model PATH`** = an *older, readable* `Stripped/EntityModel.zip`. The one in the current install is
encrypted, so the data behind it is "creator dump only" and **not loadable from a user install** — the flag exists so
the research can be re-run, not as an app feature.

Typical run:

```
python3 extract_map.py     --out /tmp/fh6 --level 2 --seasons Summer
python3 decode_nav.py      --out /tmp/fh6 --map /tmp/fh6/map_summer_L2.png
python3 extract_poi.py     --out /tmp/fh6
python3 extract_geochunk.py --out /tmp/fh6
python3 extract_races.py   --out /tmp/fh6
python3 extract_racelines.py --out /tmp/fh6
python3 extract_predictions.py --out /tmp/fh6          # needs races.json + pois.json + racelines.json in /tmp/fh6
python3 extract_speedsigns.py --out /tmp/fh6          # uses /tmp/fh6/roads.json if present
python3 extract_cars.py    --out /tmp/fh6 --lang EN,DE --ordinal 4144
python3 extract_names.py   --out /tmp/fh6
python3 extract_icons.py   --out /tmp/fh6/icons && python3 build_icon_mapping.py /tmp/fh6/icons
python3 extract_terrain.py --out /tmp/fh6 --region -87,1460,1413,2960
python3 roaddist.py        --out /tmp/fh6
python3 plot_pois.py       --out /tmp/fh6 --map /tmp/fh6/map_summer_L2_preview.png
```

Coordinates in every output are telemetry space (`PositionX`/`PositionZ` metres, `y` = height).
