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

- **Roads** have three types: **Road**, **Offroad**, **Other**; an edge nobody painted is **not set** (magenta, so it is easy to see what is left). An *edge* is the stretch
  between two consecutive nav nodes (39 383 of them). Modes: **Pan**, **Paint** (pick a brush, click or drag; Shift-click or the "fill" tick paints the whole road between two
  junctions; hold Space to pan while painting; keys 1-4 = Road / Offroad / Other / Clear), **Connect** (zoom in until the nodes show, click node A then node B: a new *dashed* link
  gets the current brush type; yellow rings = dead ends, i.e. the usual gaps), **Delete link** (only user-added links; game roads cannot be deleted).
  **Prefill from surface data** sets paved -> Road and off-road -> Offroad per edge (majority of the smoothed 4 m terrain samples on that edge, from `roadsurf.npz`); unknown edges
  (no collision mesh / elevated / unidentified id) stay *not set*. It asks before overwriting existing paint. Ctrl+Z undoes (200 actions). Totals in km are shown live.
- **Races** (mode **Race types**): click a race pin (or its race line) and pick **Road / Street / Rally / Cross Country / Touge / Drag / Midnight Battle** (or "Clear mark"). The pin takes
  that type's own game icon (Road = asphalt, Rally = mixedsurface, Cross Country = crosscountry, each in its circuit / point-to-point variant; Street, Touge, Drag, Midnight Battle have one icon).
  **Every race starts unmarked and shows the greyed icon with a "?"** - nothing is inferred from geometry, surface or names. Circuit vs point-to-point is exact (`.owt` header) and not marked
  by hand. **Prefill race types (exact)** (button in the editor panel) sets the type of every race whose type the game files state exactly (67 of 170: road 21, cross country 19, street 15,
  touge 5, rally 4, drag 3; source `ObjectModelGame.zip`, see `docs/game-data/fh6-game-files.md#exact-race-names-and-types-objectmodelgamezip`); the other 103 stay unmarked (the 16 scramble/trail
  routes too - a name word is not a type field). It never overwrites a different manual mark without a `confirm()`, and says how many it set. The pin tooltip and popup show the **exact race
  name** in the current language (111 routes, EN + DE; others show `Route <N>`), plus "Game files: Road (exact type from game files)" and, if your mark differs, "your mark differs".
  One pin per route (170 routes in `racelines.json`; route 99, the test route, has no POI and is placed at the start of its race line; IE routes and Horizon Chases are placed at their start line).
  **Where the pin sits:** at the in-game **map pin** where the game files give one, else on the RVAN start line. Exact map pins: 36 routes from `race_trigger_zone_rt<N>` + route 5411 from the `sidi_touge_event_5411`
  locator (37 of 170; link = the route id inside the object's name, nothing guessed). Those markers have a solid orange ring and a dashed connector to the start line; the other 133 have a dashed white ring,
  the tooltip says "start line (no map pin known)" and the popup says so too (the popup also gives the pin source and the pin-to-start distance). The editor panel counts "Exact map pins". Marks stay keyed by route id,
  so moving a marker never changes the autosave or the export. The three drag meets and the touge events are separate POI categories (the touge events are used as described, the drag meets are not linked).
- **Autosave** to `localStorage` (guarded; `file://` may block it) after every change and restored on load; **Export** downloads `fh6-road-types.json`; **Import** reads it back
  (asks before replacing, warns if the nav sha1 / node count differ).
- **Export format** (`fh6-road-types`, version 1, ~0.8 MB): ids and types only, **no coordinates**.

```json
{"format":"fh6-road-types","version":1,"nav":{"file":"Brio_00.nav","sha1":"a88c69f4...","nodes":38473},
 "types":{"1-2":"offroad","2-3":"offroad","40-41":"road"},
 "added":[{"a":25028,"b":30062,"type":"offroad"}],
 "races":{"41":"touge","51":"cross_country","30100":"drag"},
 "counts":{"km":{"road":502.2,"offroad":200.8,"other":0.1,"not_set":61.3},"edges":{"total":39383,"painted":36303,"added":1},"races":{"marked":3,"total":170}}}
```

  `types` key = `"<idA>-<idB>"` with idA < idB, both **stable nav node ids** (`a` in the node struct); only painted game edges are listed. `added` = user-made links
  between two node ids (`type` may be `null` = unset). `races` key = route id, value `road|street|rally|cross_country|touge|drag|midnight`. `counts` is informational.
  *Why ids and not coordinates:* the export must contain no game data (licensing rule), and positions are re-read from the user's own install when the file is used. Node ids are
  unique over all 38 473 nodes (checked by `decode_nav.py`), so they are a safe key; the nav sha1 pins the graph version.
- Implementation: `decode_nav.py` exports `ids` (node id per polyline vertex), `nav` {file, sha1, nodes} and `orphans` into `roads.json`; `build_viewer.py:build_roaded` writes
  `data/roaded.js` (ids + one prefill digit per edge); the editor is one self-contained block (`EDITOR`) in `viewer_template.html` drawing all edges on its own canvas layer with a
  grid index for hit tests, so painting 39 k edges stays smooth (no per-edge Leaflet layers).

Library modules (imported, not run): `fh6common.py` (install detection, case-insensitive paths, `.nt` / `.tz`
readers), `pgzp.py` + `lz4b.py` (reader for the 40 GB `GeoChunk*.minizip` PGZP containers — seek-reads single
entries), `fh6str.py` (string-table reader + key hash), `fh6owt.py` (`.owt` racing-line reader + `RVAN` start/finish block), `fh6surfaces.py` (terrain
surface-id → name + kind table; each entry is marked confirmed in-game / seen / reasoned), `fh6bxml.py` (binary-XML decoder; `--entity-model`, and the plaintext `ObjectModelGame.zip`), `fh6careers.py` (exact race names + types from it).
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
python3 extract_speedsigns.py --out /tmp/fh6          # uses /tmp/fh6/roads.json if present
python3 extract_cars.py    --out /tmp/fh6 --lang EN,DE --ordinal 4144
python3 extract_names.py   --out /tmp/fh6
python3 extract_icons.py   --out /tmp/fh6/icons && python3 build_icon_mapping.py /tmp/fh6/icons
python3 extract_terrain.py --out /tmp/fh6 --region -87,1460,1413,2960
python3 roaddist.py        --out /tmp/fh6
python3 plot_pois.py       --out /tmp/fh6 --map /tmp/fh6/map_summer_L2_preview.png
```

Coordinates in every output are telemetry space (`PositionX`/`PositionZ` metres, `y` = height).
