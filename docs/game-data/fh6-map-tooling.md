# FH6 map tooling — road editor v2, 2D preview, 3D preview

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
- The canonical `tools/fh6-extract/data/fh6-road-types.json` is the committed copy (see [hand-classified road and race types](fh6-game-files.md#hand-classified-road-and-race-types)).

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
  The switch is a shader uniform, so it is instant; `roads.js` carries terrain y in `xyzs` and node y as per-type `yn` (uint16, 0.025 m, hmin -5).
- **Road thickness** (0-20 m, default 3): the user wanted floating roads (bridges) to read as decks, not paper-thin. A child wall mesh (left/right walls + underside) is extruded down in the vertex shader by a uniform,
  so the slider is live without a rebuild. Walls are the road colour x 0.72, depth-tested.
- **Look:** ribbons keep a screen-space minimum width; asphalt `#34373d`, highway `#2b2e34` 12 m wide, optional bright edge lines; turnaround magenta dashed (off by default); tunnel and jump off by default.
- **Known limits:** ~39 stretches sink 50-100 m below the terrain in node mode: they are **unmarked tunnels** (e.g. x 2956 z 1051, x -4229 z -5248). Fix: mark them Tunnel in the editor, or enable "Never below terrain".
  152 bridge stretches float, which is correct. The 16 m mesh stair-steps at coasts.

## Why previews are local pages built next to the viewer (D39)

They contain game data, so they cannot ship; `build_viewer.py` therefore runs `preview_2d.py` and `preview_3d.py` after the build into the same out dir (failures are warnings, `--no-previews` skips). Both previews also run standalone.
