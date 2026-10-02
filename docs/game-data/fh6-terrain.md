# FH6 terrain — elevation and surfaces

Part of the [FH6 game-file notes](fh6-game-files.md) (read its *Locating the install*, *Coordinate system* and *GeoChunk / PGZP*
sections first — everything here is read out of `Tracks/Brio/GeoChunk0.minizip`). Reference decoder:
[`tools/fh6-extract/extract_terrain.py`](../../tools/fh6-extract/extract_terrain.py) (**the script is the ground truth**; if this doc and
the script disagree, trust the script and fix the doc).

Status: research only — nothing in `src/` reads game files yet. The user checked both renders (elevation, surfaces) against the
in-game world and they look right: **the ids and heights are trusted; only the surface *names* are missing** (see
[Surface names](#surface-names--status)).

## What is where

All paths are in-game paths inside GeoChunk0 (lower-case, backslashes; see `ChunkContentsMiniZip0.txt`).

| Entries | Count | Holds | Used |
|---|---|---|---|
| `scene\tbheightfield\autoterrain_x{X}_z{Z}_cb_cluster{N}.i.modelbin` | 3153 | terrain render mesh, one 512 m cell per file | **elevation** |
| `…_ul_cluster{N}.i.modelbin` | 2278 | second set of the same; fills holes left by `cb` (what the difference means: unknown) | **elevation** |
| `…\autoterrain_x{X}_z{Z}_cluster{N}.i.modelbin` (no `cb`/`ul`) | 56 864 | more cluster models of the same cells | *not examined* |
| `…\autoterrain_x{X}_z{Z}_square{0..35}.phys` | 28 259 (676 MB) | terrain **collision** mesh, 6×6 squares of ~85 m per cell, 34.7 M triangles, per-triangle material id | **surfaces** |
| `scene\uberheightfield\autouberlod_x{X}_z{Z}_cluster{N}.i.modelbin` | 486 (26 MB) | coarse whole-island terrain LOD (2048 m cells) | optional coarse elevation (`--coarse`) |
| `scene\intheightfield\autoterrain_x{X}_z{Z}_summer.mtxmoddxt` (+`_spring` …) | per cell | per-cell megatexture material palette (strings such as `road_cst_asp_smooth_g`, `cst_sand_flat_g`, `alp_snow_lumpy_b`) | looked at for surface names, see below |
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

So a triangle's surface id is `M[materialSlot][1]`. Ids are **global** (~350 values in the whole game, **54–56 occur on the terrain**). Sample
numbers from a full run: 28 259 files, 34 716 033 triangles, 54 distinct ids by centroid.

`extract_terrain.py` writes `surfaces.npy` (uint16 dominant-by-area id per pixel; default 8 m) plus a colour render with the class
grouping below.

### Inferred classes (OUR labels, not the game's)

Derived from where each id occurs (next to roads, in water, in snow, under forest, palette co-occurrence). The ids are exact; **the
class names are guesses** — never present them as game names. (`NatalSurfaceTypes.xml` next to the encrypted `surfaceTypes.xml` is
plaintext but is the old Forza Motorsport/Natal table and unrelated.)

| Class (ours) | Material ids |
|---|---|
| Asphalt road | 9, 286 (286 ≈ `road_gen_asp_darkrural_g`) |
| Second road class (urban/dust?) | 8, 10 |
| Road shoulder / verge (assumed) | 280, 281, 242, 26 |
| Concrete / pavement | 27 |
| Snow road | 40 |
| Water / paddy / flat lake+sea floor | 207 |
| Sand / seabed / shore | 36, 39, 208, 60, 331 |
| Snow / alpine rock | 345, 211, 41, 328, 183 |
| Riverbank stones | 230 |
| Dirt / farmland / gravel | 7, 56, 19, 279, 43 |
| Forest floor / vegetation | 31, 20, 340, 336, 339, 23, 46, 53, 342 (342 bamboo), 17, 239 |
| Grass / lawn | 22, 346 (346 lawn / golf) |

Road class from the nav graph (4/5/6/8, see [Roads](fh6-game-files.md#1-roads--openworldbriofreeroambrio_00nav)) does **not** tell the surface.
Top 10 ids cover 84 % of terrain area, top 15 cover 92 %.

### Surface names — status

Plan decision **D7** (see `.claude/teamlead/plan/game-file-parsing.md`): try to read real names from files; otherwise name them by an in-game
survey with the user.

- **File decode attempt: no id → name mapping in any readable file.** The only table that has it, `Physics/surfaceTypes.xml`, is
  encrypted. The per-cell megatexture palettes (`.mtxmoddxt`) list visual material layers + splat maps and carry **no link to the physics ids**.
- Palette *co-occurrence* supports a few guesses (40 snow road, 7 rice field, 56 ploughed field, 342 bamboo/hillside, 230 riverbank, 22 grass,
  27 concrete, 346 golf course) — still guesses.
- `Audio/AudioSurfacesInfo.xml` (plaintext) lists ~43 *game* surface names (`Asphalt_Smooth`, `Dirt_Gravel`, `Snow_Compact_Road`, …) — usable
  as naming **vocabulary** for the survey, not as an id mapping.
- **Next step (open): in-game survey** — drive to one spot per id (38 ids cover the terrain; list + map were prepared in the session
  scratchpad, regenerate with `extract_terrain.py`) and have the user confirm what it is. Record the confirmed names here with a
  "user-confirmed" tag.

## Rust implementation notes

- Needs: PGZP reader (u32/u64 tables + three codecs), a raw **LZ4 block** decoder (~25 lines, see `lz4b.py`) and `flate2` raw deflate (already
  transitive). No zip crate needed for GeoChunk.
- Elevation ≈ 1–2 days: parse `burG` Mesh/IndB/VerB, rasterise, **bake once to a cache file** (don't redo it every start; it is minutes of work).
- Surfaces: moderate–high — needs the `.phys` decode and a decision on names (above).
- Cache location: the app data dir, never the repo (the data is Playground Games IP; see the licensing section of the main doc).
