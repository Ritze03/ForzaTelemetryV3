# FH6 terrain — elevation and surfaces

Part of the [FH6 game-file notes](fh6-game-files.md) (read its *Locating the install*, *Coordinate system* and *GeoChunk / PGZP*
sections first — everything here is read out of `Tracks/Brio/GeoChunk0.minizip`). Reference decoder:
[`tools/fh6-extract/extract_terrain.py`](../../tools/fh6-extract/extract_terrain.py) (**the script is the ground truth**; if this doc and
the script disagree, trust the script and fix the doc).

Status: research only — nothing in `src/` reads game files yet. The user checked both renders (elevation, surfaces) against the
in-game world and they look right: **the ids and heights are trusted; the surface *names* are ours** — 18 ids confirmed in-game, 4 seen, 32 reasoned
(see [Surface names](#surface-names-id-table)).

## What is where

All paths are in-game paths inside GeoChunk0 (lower-case, backslashes; see `ChunkContentsMiniZip0.txt`).

| Entries | Count | Holds | Used |
|---|---|---|---|
| `scene\tbheightfield\autoterrain_x{X}_z{Z}_cb_cluster{N}.i.modelbin` | 3153 | terrain render mesh, one 512 m cell per file | **elevation** |
| `…_ul_cluster{N}.i.modelbin` | 2278 | second set of the same; fills holes left by `cb` (what the difference means: unknown) | **elevation** |
| `…\autoterrain_x{X}_z{Z}_cluster{N}.i.modelbin` (no `cb`/`ul`) | 56 864 | more cluster models of the same cells | *not examined* |
| `…\autoterrain_x{X}_z{Z}_square{0..35}.phys` | 28 259 (676 MB) | terrain **collision** mesh, 6×6 squares of ~85 m per cell, 34.7 M triangles, per-triangle material id | **surfaces** |
| `scene\uberheightfield\autouberlod_x{X}_z{Z}_cluster{N}.i.modelbin` | 486 (26 MB) | coarse whole-island terrain LOD (2048 m cells) | optional coarse elevation (`--coarse`) |
| `scene\intheightfield\autoterrain_x{X}_z{Z}_summer.mtxmoddxt` (+`_spring` …) | per cell | per-cell megatexture material palette (strings such as `road_cst_asp_smooth_g`, `cst_sand_flat_g`, `alp_snow_lumpy_b`) | looked at for surface names — no link to the physics ids, see [Surface names](#surface-names-id-table) |
| `scene\…\mainmap_/submap_/autoglossf*_x{X}_z{Z}_x<season>x_….pb`, `…\wdepth\wdepth_*_season_N.dds`, `acoustics\*.ace`, `procphys\*.procphys` | — | terrain textures, water depth(?), audio zones, procedural physics | *not examined* |

**Cell addressing.** File names carry `X = round(world_x / 1023)·1023`-style numbers, so the world cell of a `tbheightfield` file is
`(round(X/1023)·512, round(Z/1023)·512)` — that point is the **minimum corner** of the 512 m cell (verified against decoded vertex
bboxes: e.g. `x-8184_z13299` → cell (−4096, 6656), vertices x −4096…−3584, z 6656…7168). `uberheightfield` files use the same rule but
are 2048 m cells. City core and the arena areas have no terrain `.phys`; one 512 m cell near (3830, −4870) has no terrain mesh at all.
`y` is telemetry height, sea level ≈ **y 100**.

*Why GeoChunk0 and not a smaller file:* GeoChunk0 is the only chunk with models/phys (GeoChunk1/2 = `.pb` only, GeoChunk3 = textures
only), so the 40 GB file is unavoidable — but only the needed entries are seek-read (676 MB of `.phys` plus the model files).

## Container: `burG` (`.modelbin`, `.swatchbin`, …)

Little endian. Same family as the map tiles ([Map imagery](fh6-game-files.md#2-map-imagery--uitexturesdata_boundmap_brio_seasonzip)).

| Offset | Meaning |
|---|---|
| 0 | `burG` |
| 4 | u32 version `0x101`, u32 header size, u32 total size, u32 section count |
| 20 + 24·k | section directory: 4-char tag **byte-reversed** (`Skel`, `MatI`, `Mesh`, `IndB`, `VLay`, `VerB`, `Modl`), u32 ver, u32 id, u32 offset, u32 size, u32 size |
| after directory | name chunks tagged `emaN` (reversed `Name`): +16 → NUL-terminated string; material names and mesh names |

Model data:

| Piece | Layout |
|---|---|
| `IndB` | 16 B header `{u32 count, u32 bytes, u16 stride, u16 ?, u32 layout id}` then indices (`u16` when stride 2) — indices are **absolute** into the vertex buffer |
| `VerB` (first one) | same 16 B header; stride **8**: 3 × snorm16 position (+2 unused bytes). `world = snorm / 32767 · scale + centre` |
| scale / centre | the last 8 f32 of the first `Mesh` section: `(sx, sy, sz, 0, cx, cy, cz, 0)` |
| `Mesh` record | u16 material index @ +2; u32 first index @ +34; i32 base vertex @ +38 (**ignore**); u32 index count @ +42 |

The terrain surface is the mesh whose **material name starts `GenMaterial_UberMap`**; other meshes in a cluster file are not terrain.

## Elevation

`extract_terrain.py` rasterises the UberMap triangles of all `cb` files, then `ul` into the `cb` holes, taking the **max y** per pixel
(so bridges and overpasses win over the ground below them). ~5.8 M triangles in total for the full island.

Validation (road-node heights from `Brio_00.nav` vs the raster):

| Raster | Coverage of the 38 473 road nodes | median \|Δy\| | p90 \|Δy\| |
|---|---|---|---|
| 4 m, full detail (`tbheightfield`) | 98 % | **0.17 m** | 10.9 m (bridges / tunnels — the terrain is under the road) |
| coarse `uberheightfield` | 97 % | 1.1 m | 11.5 m |
| small test region (−87,1460 … 1413,2960) | 98 % | 0.17 m | 0.5 m |

Full run: roughly 5–10 min and a few GB RAM for both rasters at 4 m (pure-Python LZ4 + per-triangle numpy rasteriser; a Rust port would be far
faster). A 22 010 m extent at 4 m is 5503² float32 = **121 MB**, so an app would **bake a coarser/cropped grid into a cache** rather
than keep it raw. Pixels with no terrain triangle (open sea beyond the island, the one missing cell) stay `NaN`.

*Why we trust it:* 0.17 m median error against an independent data set (the AI road graph) and the user's visual check of the hillshade.

## Surfaces (`.phys`)

`*_square{0..35}.phys` — one file per ~85 m square of a 512 m cell. Little endian, `o` = position after the name:

| Offset | Type | Meaning |
|---|---|---|
| 0 | u32 + bytes | name length + name |
| o + 0 | f32×3 (+4 pad) | bbox min |
| o + 16 | f32×3 (+4 pad) | bbox max |
| o + 32 … 63 | 32 B | undecoded |
| o + 64 | s32 | **−vertexCount** |
| o + 68 | vertexCount × 10 B | `3 × u16` x,y,z quantised over the bbox (`lo + u/65535·(hi−lo)`, ≈ 1 mm) + `2 × u16` unknown |
| then | u32 | triCount |
| then | triCount × 8 B | `u8 flags, u8 materialSlot, u16 i0, i1, i2` |
| then | u32 n2 + n2 × 32 B | undecoded |
| then | u32, u32 nMat | (first undecoded) |
| then | nMat × 4 × u16 | material table. Row = per-slot `{corner0 id, corner1 id, corner2 id, ?}`; **column 1 is used as the triangle's dominant id** |
| then | 8 B | tail |

So a triangle's surface id is `M[materialSlot][1]`. Ids are **global** (~350 values in the whole game, **54 occur on the terrain**). Sample
numbers from a full run: 28 259 files, 34 716 033 triangles, 54 distinct ids by centroid.

`extract_terrain.py` writes `surfaces.npy` (uint16 dominant-by-area id per pixel; default 8 m), `surfaces.json` (extent, ids present with names/status) and a colour render (its
class grouping is for the preview only; names: the [id table](#surface-names-id-table)).

## Surface kind (paved / off-road) and the surface under the roads

**Kind** = what a *road* over an id counts as. It is the third field of `fh6surfaces.SURFACES` and has the **same confidence as the entry's status** (a reasoned id has a reasoned kind; the entry comment after `| KIND` says why and flags weak ones).

| kind | ids |
|---|---|
| **paved** | 9 asphalt (confirmed), 8 asphalt deck (confirmed), 27 concrete (seen), 286 dark rural asphalt (reasoned), 10 urban asphalt variant (reasoned) |
| **offroad** | 31 22 20 345 340 230 336 211 342 7 56 41 339 19 23 (confirmed / seen), 281 packed dirt track (confirmed), and reasoned: 208 60 331 (sand), 280 (weak), 279, 46, 53, **40 (snow road)**, 43, 328, 183, 239, 17 (weak), 346, 242 (weak) |
| **water** (says nothing about a road → unknown) | 207 puddle, 36 seabed, 39 seabed/shore |
| **other** (unknown id → unknown) | 26 199 236 18 107 178 282 29 283 28 32 229 171 293 238 |

Judgement calls (all reasoned): **40** "snow road" is ambiguous (snow-covered asphalt?) and counted off-road, following the user's "snow track" wording — it only occurs on the alpine roads around Sotoyama / Snow Monster Forest (18 km). **280** (81 km of nav roads!) is counted off-road: on roads its neighbours are the vegetation/dirt ids 17, 239, 279, 282 and rarely 9, and its siblings 279-283 form a dirt-track id family around the confirmed 281; if the user finds 280 on a paved road in the viewer, flip it in `fh6surfaces.py` — the build re-runs in seconds.
**8** is not only the elevated expressway: 73 km of nav roads carry it (Tokyo streets, ramps) — 61 % of those road samples have a second terrain layer below, so road decks (bridges/ramps/overpasses) **do carry their own collision in the terrain `.phys` files**, mostly as id 8.

### Sampling the roads (`classify_roads.py`)

`classify_roads.py` densifies every nav polyline to 4 m (node heights interpolated), and for each sample finds the `.phys` triangle(s) containing it in x/z (barycentric test, 741 cells / 25 472 files, ~16 s with 8 processes); of several candidates the one whose height is **closest to the road node's height** wins. Result: `roadsurf.npz` (id, height difference, candidate count per sample). `build_viewer.py` then turns it into kinds:

- sample = **unknown** if no triangle (`no collision mesh`), if the best triangle is more than **3 m** from the road height (`elevated / tunnel`: bridge without deck collision, tunnel), or if the id's kind is water/other;
- runs shorter than **40 m** are merged into the longer neighbour (flicker, seams between collision squares: 15 transitions/km raw, mostly "no triangle ↔ asphalt");
- per polyline: % paved / off-road / unknown (by length) + dominant ids; per run: kind, length, top ids.

*Why exact triangles and not the 8 m raster:* the 8 m dominant-by-area raster (`surfaces.npy`) picks the verge next to a narrow road: on the same 170 k samples it gives a different id 11 % of the time and the wrong **kind** (paved ↔ off-road) for 32 of 672 km (4.7 %), nearly all "paved road labelled off-road because grass has more area in that pixel". The raster also cannot tell a bridge from the ground below it. The exact lookup is cheap (seconds), so the raster is only used for the hover/overlay.
*Why the height rule:* a plain top-most/lowest-triangle rule would pick the deck on every bridge or the ground under it; "closest to the nav node height" selects the deck when it has collision and flags the road as elevated when it does not.

Numbers (full island, 770 km of nav roads, after smoothing): **506 km paved (66 %), 202 km off-road (26 %), 62 km unknown (8 %)**; unknown = 35 km no collision mesh (city core / arenas / the 512 m cell near (3830, -4870)), 22 km elevated/tunnel (Rainbow Bridge etc., ~73 % of samples there are > 3 m from the terrain), 5 km water / unidentified id. Per nav class (km paved / off-road / unknown): 4: 35 / 19 / 4, 5: 396 / 172 / 37, 6: 74 / 10 / 21, 8: 1 / 0 / 0.2 — every class mixes surfaces.
Limits: centre line only (a road whose centre is paved but shoulder is dirt is "paved"); id kinds are only as good as the names (see judgement calls above); the nav graph and the collision mesh can be a few metres apart on very narrow roads; per-road % is per *polyline* (a road split at a >60 m node gap counts as two).

## Surface names (id table)

**The game's own id → name table is encrypted** (`Physics/surfaceTypes.xml`), so every name below is **ours**. Plan decisions D7/D9: the ids are trusted (the user checked the renders),
the names were found by an in-game survey — the user drove to one flat spot per id (ordered by terrain area; list + map generated from the `extract_terrain.py` output) and said what it is; the survey was
closed after three batches and every remaining id got a best guess, **marked as reasoned** ("use your best guess, but mark that they are reasoned"). The same table lives in code as
[`fh6surfaces.py`](../../tools/fh6-extract/fh6surfaces.py) (`SURFACES = {id: (name, status, kind)}`, one comment per entry); `extract_terrain.py` writes it into `surfaces.json`.

| status | meaning |
|---|---|
| **confirmed in-game** (18) | the user stood on this id (✓ or a screenshot) and named it |
| seen (visual only) (4) | the user saw the spot but it is outside the playable map or not reachable (ids 211, 41, 23, 27) |
| reasoned (32) | our guess from location (next to roads / coast / snow line / forest), megatexture-palette co-occurrence and neighbouring confirmed ids — **not checked in-game; never show as the game's name** |

54 ids occur on the terrain (230.4 km² of triangles in total; the game has ~350 global ids). The top 10 ids cover 84 % of the area, the top 15 cover 92 %; all of the top 10 are confirmed.

| id | Name (ours) | Status | km² | Evidence |
|---|---|---|---|---|
| 31 | Forest floor | **confirmed in-game** | 61.82 | forest floor (palette: forest litter / needles) |
| 22 | Grass | **confirmed in-game** | 37.92 | grass |
| 207 | Shallow water / puddle | **confirmed in-game** | 36.58 | shallow water / puddle |
| 20 | Forest floor (brownish) | **confirmed in-game** | 13.73 | forest floor, brownish variant |
| 345 | Snowy forest floor | **confirmed in-game** | 8.04 | snowy forest floor between trees (NOT alpine rock) |
| 340 | Dead grass / dry brown ground | **confirmed in-game** | 7.97 | dead grass / dry brown ground (screenshot) |
| 36 | Seabed (under water) | **confirmed in-game** | 7.53 | water, seabed under the water |
| 9 | Asphalt | **confirmed in-game** | 7.20 | asphalt, parking area |
| 230 | Packed gravel / dirt | **confirmed in-game** | 6.45 | packed gravel/dirt beside a puddle, docks/industrial (NOT riverbank stones) |
| 39 | Seabed / shore | **confirmed in-game** | 6.13 | seabed / shore |
| 336 | Forest floor | **confirmed in-game** | 4.71 | forest floor |
| 211 | Snow | seen (visual only) | 3.64 | snow, outside the playable map, visual only |
| 342 | Bamboo forest floor | **confirmed in-game** | 3.56 | bamboo forest floor |
| 7 | Gravel / stones | **confirmed in-game** | 3.23 | gravel/stones on an alpine lake shore (NOT a rice field, despite the palette) |
| 56 | Ploughed field | **confirmed in-game** | 3.11 | ploughed field |
| 41 | Snow / alpine | seen (visual only) | 3.01 | snow/alpine, outside the playable map, looks right |
| 339 | Forest floor | **confirmed in-game** | 1.85 | forest floor |
| 19 | Forest clearing (felled trees) | **confirmed in-game** | 1.57 | forest clearing with felled trees |
| 8 | Asphalt (elevated deck) | **confirmed in-game** | 1.49 | asphalt on the elevated Tokyo expressway interchange (bridge deck) |
| 23 | Grass (festival site) | seen (visual only) | 1.26 | festival-site grass, seen but not reachable |
| 27 | Concrete | seen (visual only) | 1.21 | concrete, industrial area, seen but not reachable |
| 208 | Sand / shore | reasoned | 0.92 | spot at y 96 m (sea level is ~100); sibling of the confirmed seabed/shore ids 36, 39 |
| 280 | Dirt track or road verge (weak) | reasoned | 0.89 | lies on roads (0 m from a road); 281 turned out to be a dirt track, so "verge" is doubtful |
| 279 | Dirt / farm track | reasoned | 0.81 | earlier class guess "dirt / farmland"; no other hint |
| 46 | Forest floor (steep) | reasoned | 0.80 | earlier class guess "forest"; only on steep ground (no flat patch to visit) |
| 53 | Forest floor | reasoned | 0.71 | earlier class guess "forest"; small patches |
| 40 | Snow road | reasoned | 0.56 | on alpine roads; megatexture palette road_alp_snw_flat |
| 43 | Dirt / gravel | reasoned | 0.49 | earlier class guess "dirt / farmland / gravel"; no other hint |
| 328 | Snow / alpine rock (weak) | reasoned | 0.49 | earlier class guess "snow", but the spot is at y 121 m (near sea level) - doubtful |
| 281 | Packed dirt track | **confirmed in-game** | 0.47 | packed dirt track at the junction of a rallycross-style dirt circuit (NOT road verge) |
| 60 | Sand / shore | reasoned | 0.44 | spot at y 101 m (sea level); sibling of 36, 39, 208 |
| 183 | Snow / alpine rock | reasoned | 0.40 | earlier class guess "snow / alpine rock"; spot at y 524 m next to a parking area |
| 331 | Sand / shore | reasoned | 0.27 | spot at y 102 m (sea level); sibling of 36, 39, 208 |
| 239 | Vegetation / undergrowth (weak) | reasoned | 0.26 | earlier class guess "forest / vegetation"; nothing more specific |
| 286 | Asphalt (dark rural) | reasoned | 0.19 | on rural roads; megatexture palette road_gen_asp_darkrural_g |
| 17 | Unknown (vegetation?) | reasoned | 0.17 | next to a bridge landmark; earlier class guess "forest"; no real evidence |
| 346 | Lawn / golf course grass | reasoned | 0.16 | earlier class guess "lawn / golf"; spot beside a car-parking area |
| 10 | Asphalt (urban variant) | reasoned | 0.11 | on a job-route road (10 m from it); sibling of confirmed asphalt 8 - assumed asphalt variant |
| 242 | Road verge / dirt (weak) | reasoned | 0.05 | beside roads; see 280 |
| 26 | Unknown (road shoulder?) | reasoned | 0.05 | beside a parking area; no real evidence |
| 199 | Unknown | reasoned | 0.03 | too small to survey, no evidence |
| 236 | Unknown | reasoned | 0.02 | — |
| 18 | Unknown | reasoned | 0.02 | — |
| 107 | Unknown | reasoned | 0.02 | — |
| 178 | Unknown | reasoned | 0.02 | — |
| 282 | Unknown | reasoned | 0.01 | — |
| 29 | Unknown | reasoned | 0.01 | — |
| 283 | Unknown | reasoned | 0.01 | — |
| 28 | Unknown | reasoned | 0.01 | — |
| 32 | Unknown | reasoned | 0.00 | — |
| 229 | Unknown | reasoned | 0.00 | — |
| 171 | Unknown | reasoned | 0.00 | — |
| 293 | Unknown | reasoned | 0.00 | — |
| 238 | Unknown | reasoned | 0.00 | — |

Notes on the surprises (several pre-survey guesses were wrong):
- **230** is packed gravel/dirt in the docks/industrial area, not "riverbank stones"; **7** is gravel/stones on an alpine lake shore, not a rice field (the palette co-occurrence misled); **345** is snowy forest floor between the trees, not alpine rock.
- **281** turned out to be a packed dirt track (junction of a rallycross-style circuit), not "road verge" — so the sibling "verge" guesses **280** and **242** are weak and may be dirt-track surfaces too.
- Nav-graph road class (4/5/6/8, see [Roads](fh6-game-files.md#1-roads--openworldbriofreeroambrio_00nav)) does **not** tell the surface: every class mixes paved and off-road (user-checked in the viewer; numbers above). The surface kind of a road comes from the terrain ids under it.
- `Audio/AudioSurfacesInfo.xml` (plaintext) lists ~43 *game* surface names (`Asphalt_Smooth`, `Dirt_Gravel`, `Dirt_Forest`, `Grass_General`, `Snow_Compact_Road`, `Scree`, `Slate`, `Concrete`, …) — **vocabulary only, not id-numbered**: a good target set if the app ever maps ids to the game's terms.
  `Physics/NatalSurfaceTypes.xml` next to the encrypted file is the old Forza Motorsport/Natal table and unrelated.
- The megatexture palettes (`.mtxmoddxt`) list visual material layers + splat maps and carry **no link to the physics ids** (D7 file-decode attempt: no id → name mapping in any readable file).
- `extract_terrain.CLASSES` (12 colour groups) only drives the preview PNG; use `fh6surfaces.py` for names.

## Rust implementation notes

- Needs: PGZP reader (u32/u64 tables + three codecs), a raw **LZ4 block** decoder (~25 lines, see `lz4b.py`) and `flate2` raw deflate (already
  transitive). No zip crate needed for GeoChunk.
- Elevation ≈ 1–2 days: parse `burG` Mesh/IndB/VerB, rasterise, **bake once to a cache file** (don't redo it every start; it is minutes of work).
- Surfaces: moderate–high — needs the `.phys` decode; names: a `confirmed`/`seen`/`reasoned` flag per id lets the UI hedge (show reasoned names with a "?" or not at all).
- Cache location: the app data dir, never the repo (the data is Playground Games IP; see the licensing section of the main doc).
