# Forza Horizon 6 game files — reverse-engineering notes

What we know about reading **map imagery, roads, points of interest (POIs), race starts and race
lines, speed-limit signs, terrain elevation and surfaces, car names, area/region names and the
game's map icons** straight from the user's own FH6 install, so a feature like "load map /
roads / POIs / race lines from the game" can be built without redoing the research. Everything
here was derived from the PC (Steam) build and verified with the scripts in
[`tools/fh6-extract/`](../../tools/fh6-extract/README.md) — **the scripts are the ground truth**; if
this doc and a script disagree, trust the script and fix the doc. Two sub-pages:
[FH6 terrain](fh6-terrain.md) (elevation + the surface-id table) and
[FH6 cars, names, regions and icons](fh6-cars-names-icons.md) (`CarOrdinal` → name, 24-language area names, region outlines, map icons).
Open items (things nobody has verified yet) are collected in [Open items](#10-open-items).

Status: research only. Nothing in `src/` reads game files yet. Written against the Steam install
at game build 440853 (from `PreCrashReport.xml`), Sept 2026; a game update can change file contents
and which files are encrypted — re-run the scripts and compare counts before trusting a number here.

## Why this exists, and the licensing decision

The map, road graph and POI positions are **Playground Games' IP**. Decision: the app must
**read them from the user's own install at runtime** (auto-detect Steam through
`libraryfolders.vdf`, folder-picker fallback) instead of bundling extracted data.

- *Why:* we cannot redistribute the data; reading the user's copy sidesteps that entirely.
- *Done for the map:* the app reads the season map tiles from the install (`src/gamedata/tiles.rs`) and no
  longer bundles the ~110 MB of `assets/maps/*.jpg` (re-encodes of the same imagery — see
  [Map imagery](#2-map-imagery); `docs/features/minimap.md`).
- **Never commit extracted output** (json/png/npy) to the repo. The scripts default to `./fh6-out`;
  point `--out` outside the repo.
- Everything is read-only on the install. Never write into the game directory.
- **Names we invent are labelled as ours.** Where the game's own name is not readable (terrain
  surface classes, "Hide & Seek arena: city_east", race-name guesses) the data records carry a
  confidence / `*_basis` field and the UI must not present them as game names. *Why:* a wrong
  "official" label is worse than none, and the user can only verify them by playing.
- **No game-memory reading, ever.** Live state (current season/playlist, active events) would be
  easiest to get from the running game's memory, but the game is online and has anti-cheat; a
  memory read risks a ban for no essential gain (the data we need is on disk or derivable). A
  passive live probe (open files, logs, sockets — no process memory) was done instead, see
  [Live-game probe](#7-live-game-probe).

## Data catalogue — what the install gives us

"**Install**" = loadable from a normal user install (plaintext). "**Creator dump only**" = we only
saw it in a decrypted July-build `media.zip` the Horizon Nav creator sent; the same file is
**encrypted** in the current install, so an app cannot load it ([Encryption status](#encryption-status)).
Precision: *exact* = world coordinates straight from the file; *approx* = reconstructed.

| Category | Source (under `media/`) | Count | Precision | Names readable? | Loadable from user install? |
|---|---|---|---|---|---|
| Map imagery, 4 seasons | `UI/Textures/Data_Bound/Map_Brio_<Season>.zip` | 4 × 85 BC1 tiles (8192²) | exact | n/a | **yes** |
| Road graph | `OpenWorld/Brio/Freeroam/Brio_00.nav` | 38 473 nodes / 1 532 roads | exact positions; class partly guessed, one-way/tunnel flags undecoded | no street names | **yes** |
| Terrain elevation | GeoChunk0 `tbheightfield\*_cb_/_ul_cluster*.modelbin` | ~5.8 M tris | median 0.17 m vs roads (4 m raster) | n/a | **yes** (needs PGZP) — [terrain](fh6-terrain.md) |
| Terrain surface ids | GeoChunk0 `tbheightfield\*_square*.phys` | 34.7 M tris, 54 ids | ids exact; **names ours**: 18 confirmed in-game, 4 seen, 32 reasoned | **no** (table encrypted) | **yes** — [terrain](fh6-terrain.md#surface-names-id-table) |
| Race start line + 12-slot grid + heading + finish | `OpenWorld/Brio/AITracks/Route<N>.nav` (`RVAN` block) | 169 routes (+ route 99 test) | exact (on racing line < 0.1 m) | partial, see next rows | **yes** |
| Race racing line + track width | `AITracks/Route<N>.owt` | 170 (127 point-to-point + 43 circuits; 2 off-map, 1 test) | exact; edges = pos ± half-width vector | n/a | **yes** — [Race lines](#race-lines-the-owt-racing-line-files) |
| Race map pin (activation sphere) | `Tracks/Brio/triggerzones/tz_race_activations/race_triggers.tz` (+ `route0.nt` `sidi_touge_event_5411`) | 36 (+1) | exact pin, **not** the start (median 185 m off) | slug `rt<N>` = route id | **yes** — the **only** exact map-pin source ([search results](#race-map-pins-which-routes-have-one)) |
| Race display names, **exact** (route → name) | `ObjectModelGame.zip` `TrackInfoDataSet` (`InfoByRouteId` → `DisplayName` string id) + `Stripped/StringTables/<LANG>.zip` `CareerTrackInfo.str` | 111 routes, EN + DE (and the other 22 languages) | exact | yes | **yes** — [exact race names and types](#exact-race-names-and-types-objectmodelgamezip) |
| Race types, **exact** (road / street / rally / cross country / touge / drag) | `ObjectModelGame.zip` `CareerRaceDataSet.UITheme`, `TrackInfoDataSet.UseCrossCountryAI`, `RaceCollectionUIOverridesMap` StreetRace flyer | 67 of the 170 routes | exact, **only what the files state** | n/a | **yes** (same section); the other 103 routes have no stated type |
| Race display names (older fallback) | `Stripped/StringTables/EN.zip` `CareerRaceCollection.str` | 110 unique | text exact | link route ↔ name only for the ~60 routes without a TrackInfo entry, via the older methods in [Race names](#race-names) | superseded for the 111 TrackInfo routes |
| Race event families (sprint, circuit, scramble, …) | `Entities/Brio/campaign_slots.xml` | 88 routes | exact | yes | **creator dump only** (only needed now for the heuristic fallback names) |
| Landmarks | `Tracks/Brio/triggerzones/tz_world_constraints/landmark_triggers.tz` | 75 | exact (sphere centre) | slug (`shibuya_crossing`); **names 75/75 in 24 languages** from `Landmarks.str` (54 direct, 17 alternate ids, 2 by name/position, 2 borrow the parent name) | **yes** — [names](fh6-cars-names-icons.md#2-area-landmark-and-region-names-24-languages) |
| Named locators (houses, fast travel, festival, barn finds + hint areas, car/drag meets, touge, showcases, treasure cars, aftermarket spots/boards, Horizon Jobs/Stories, rush, invitational/legend, upsell) | `Tracks/Brio/trackroutes/route0.nt` (+ `route40001/40900/4004x/4005x.nt`) | ~370 + extras | exact | internal slugs | **yes** |
| Pinatas / eliminator spawns / parking areas | `trackroutes/{pinata_locators,eliminator_locators,parkingareas}.nt` | 1536 / 373 / 2664 | exact | no | **yes** |
| Horizon Story/Job activation zones, creature zones | `triggerzones/tz_bucket_challenges`, `tz_creatures` | 11+6 / 47 | exact | slugs | **yes** |
| XP boards (A/B/C = 100/75/25), speed traps, speed zones, trailblazers, drift zones, mascots, estate entrances, treasure-chest boards | `Tracks/Brio/Ribbon_00/GameObjs.xml` | 200 / 30 / 30 (×2 gates) / 12 (×2) / 20 (×2) / 200 / 37 / 3 | **exact** + orientation | ids (`SPEEDCAMERA_07_LEFT`) | **yes** (plain XML!) |
| Danger signs, drift-zone marker posts, drift-circuit props | GeoChunk0 `.pgeo` | 15 / 1027 / 13 | exact (16.16 fixed) | ids | **yes** (needs PGZP) |
| Speed-limit signs (limit / high-speed / end-of-limit) | GeoChunk0 `c300_signs_do*.pgeo` | 1659 / 217 / 265 | exact position + facing; road snap median 7 m | limit is a **variant 0–5**; **km/h not in the files** | positions **yes** (needs PGZP) — [Speed signs](#speed-limit-signs) |
| Rush ramps (as instances) | GeoChunk0 `.pgeo` | ~1700 | exact | — | yes, **not extracted** by our scripts |
| Playground arenas (3), Hide & Seek arenas (6), flag-rush flags (6), rural train line (505 pts) | `trackroutes/route30xx/8100-8105.nt`, `Stripped/gs/brio/gameobjs.xml`, `OpenWorld/Brio/Freeroam/Ambient_RuralTrain_RuralLine.owcp` | 3 / 6 / 6 / 1 | exact (arenas: outline centroid) | arena names partly inferred | **yes** |
| Photo spots | `Entities/Brio/entities_photo_challenge_landmarks.xml` | 37 | exact | slugs | **creator dump only** (not in any pgeo) |
| Time-attack boards (4), Horizon Chase starts (7), backstage passes, labyrinth entrance, IE activator, community gift shop, Legend Island gates (2) | `Entities/Brio/*.xml`, `Templates/*.xml` | — | exact | partly | **creator dump only** |
| Map element / filter types (the game's own ~230) | `UI.zip` → `MapProfiles/MapIncludes/*.xml` | ~230 | n/a | yes (plaintext XML) | **yes** |
| Map regions (10) with outlines | `trackroutes/map_region_<slug>.nt` (`Arena_NNN` polygon) | 10 | exact outline (0 self-intersections, tile the island, 188 km²) | **10/10 in 24 languages** (slug → name inferred from mascots) | **yes** — [regions](fh6-cars-names-icons.md#map-regions--10--10-with-outlines) |
| Map icons (the game's own symbols) | `UI/Textures/HiRes/Data_Bound/Horizon_Map.zip` (+ atlas `ForteMapIconSheet`, slot grid in `UI.zip`) | 1014 swatchbins + 6 ext + 54 atlas crops = 1074 PNGs; 45 / 53 POI categories mapped | n/a | type tag → icon from the game's XML | **yes** (BC7) — [icons](fh6-cars-names-icons.md#3-map-icons--the-games-own-symbols) |
| Car names (`CarOrdinal` → make + model) | `Cars/<MediaName>.zip` (ordinal) + `StringTables/<LANG>.zip` `Data_Car.str` (name) | 671 cars, 24 languages | exact; packet `CarOrdinal` == this id **verified live** (4277) | long name (make usually, not always), model-only; make is a heuristic | **yes** — [cars](fh6-cars-names-icons.md#1-car-names--carordinal--makemodel) |
| Stunt name tables (speed zone / trap, drift zone, danger sign, trailblazer, time/drift attack) | `StringTables/<LANG>.zip` | 30 / 30 / 20 / 20 / 11 / 10 / 2 | names only — **no link to a position** | yes | strings **yes** |
| String tables (all UI text) | `Stripped/StringTables/<LANG>.zip` | 291 tables (EN), 24 languages | n/a | yes | **yes** |
| Which speed traps / drift zones are *this week's* Festival Playlist | server data | — | — | — | **no, unobtainable** — [verdict](#6-seasonal--weekly-festival-playlist-verdict) |
| Car database (make field, performance, prices), rules, tunables, physics surface names | `gamedbRC.slt`, `Rules.zip`, `GameTunableSettings.zip`, `Physics/surfaceTypes.xml` | — | — | — | **no, encrypted** |

## Locating the install

| Item | Value |
|---|---|
| Steam app id | `2483190` |
| Folder | `steamapps/common/ForzaHorizon6/` ; game data root is its **`media/`** subfolder |
| Library list | `~/.local/share/Steam/steamapps/libraryfolders.vdf` (Linux; also `~/.steam/steam/...`, Flatpak `~/.var/app/com.valvesoftware.Steam/.local/share/Steam/...`; Windows `C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf`). Each library is a `"path"  "<dir>"` entry; the game may live in any of them (on the dev machine it is in a secondary library, so iterate all `path` entries). |
| Path case | Mixed on disk (`Tracks/Brio/trackroutes`, `UI/Textures/Data_Bound`, `Stripped/gs/brio`). Resolve **case-insensitively** — `fh6common.ci()` does this. |

Reference implementation of detection: `tools/fh6-extract/fh6common.py` (`autodetect_media`).

## Encryption status

Some files in the **current** install are encrypted; the older July-build `media.zip` the Horizon Nav creator
shared was decrypted. Signature of the encrypted ones: zip method **22** (or a raw blob), entropy ≈ 8, size
`16·n + 4` bytes → block cipher (AES-like) plus a 4-byte trailer; the key presumably lives in the game exe.
**We did not attempt to break it** (and an app must not ship a key anyway).

| File (under `media/` unless noted) | State in the current install | What it holds |
|---|---|---|
| `Stripped/gamedbRC.slt` | **encrypted** (the creator's July copy is a SQLite DB — that is where the "SQLite gamedb" in earlier notes came from) | car/track database; no world coordinates either way |
| `Stripped/EntityModel.zip` | **encrypted** (July copy: plain deflate zip of `BXML` files) | event slots, race types, photo-challenge spots, time attacks, chase events, landmark areas |
| `Stripped/stateflow.zip`, `Rules.zip`, `GameTunableSettings.zip`, `Camera.zip`, `zipmanifest.xml` | **encrypted** | game rules / tunables / camera |
| `Physics/surfaceTypes.xml` | **encrypted** | the terrain-surface id → name table |
| profile backups; `…/compatdata/2483190/pfx/…/AppData/Local/ForzaHorizon6/CmsCache/*` | **encrypted** | save data; server-delivered content (CMS) cache |
| `UI.zip` (incl. `MapProfiles/MapIncludes/*.xml`) | plaintext | UI definitions, the game's own map filter/icon types |
| `ObjectModelGame.zip` (7121 × `source/ScribbleData/<id>.om.xml`, `BXML`) | **plaintext** (found 2026-10-03) | game data sets: `TrackInfoDataSet`, `CareerRaceDataSet`, `RaceCollectionUIOverridesMap` → exact race names and types; the other ~7100 files are unexplored |
| `Stripped/StringTables/<LANG>.zip` (`*.str`, 24 languages) | plaintext | all UI/event/race text, car names (`Data_Car.str`), landmark / region names |
| `Cars/<MediaName>.zip` (671), `UI/Textures/HiRes/Data_Bound/Horizon_Map.zip` | plaintext (deflate) | car clip ids (= `CarOrdinal`), map icons |
| `UI/Textures/Data_Bound/Map_Brio_<Season>.zip` | plaintext | map tiles |
| everything under `Tracks/` and `OpenWorld/` used so far (`.nt`, `.tz`, `GameObjs.xml`, `Route<N>.nav/.owt`, `Brio_00.nav`, `ChunkContentsMiniZip*.txt`) and `GeoChunk*.minizip` | plaintext | world data |
| `Physics/NatalSurfaceTypes.xml`, `Audio/AudioSurfacesInfo.xml` | plaintext | old unrelated surface table / ~43 game surface names for audio |
| `PreCrashReport.xml`, `UserConfigSelections`, PlayFab `localstate.json` (in the Wine prefix) | plaintext | crash-report startup snapshot (build 440853, session id), graphics settings, livery list |

Consequence: anything marked *creator dump only* in the catalogue cannot be a runtime feature unless
we get the data some other way (embed a derived list — which the licensing decision forbids — or
ask the user for a file). Treat those as "nice to know".

## Coordinate system and map calibration

All world data below (nav nodes, trigger zones, locators, AI-track nodes) is already in
**telemetry space**: the same `x`/`z` (metres) as the packet's `PositionX`/`PositionZ`; `y` is
height. No transform is needed to compare with live telemetry.

Map pixel mapping (identical to `MapCalibration::DEFAULT` in `src/minimap.rs`, see
[Minimap](../features/minimap.md)), for the full 8192×8192 map, north up (+z up):

```
px = (x - (-12540)) * 0.3722
py = (10738 - z)    * 0.3722
```

This applies **unchanged** to the images extracted from the game (validated: roads drawn with this
mapping sit on the roads in the imagery; race-start triggers land a median 2.8 m from a decoded road).
For lower pyramid levels scale by `size/8192`.

World bounds of the road nodes: x −8052..6717, z −9492..9274, y 93..1150.

---

## 1. Roads — `OpenWorld/Brio/Freeroam/Brio_00.nav`

Little-endian binary, magic `WVAN` ("nav" graph). The same format is used by the small route graphs
`OpenWorld/Brio/AITracks/Route<N>.nav`.

### Header

| Offset | Type | Meaning |
|---|---|---|
| 0x00 | `char[4]` | `WVAN` |
| 0x04 | u32 | version = 2 |
| 0x08 | 8 B | id (`701fc4e9…`, shared with `Brio_00.owbs`) |
| 0x10..0x58 | | zeros |
| 0x58 | u32[14] | `hdr[]`, below |

`hdr[]`:

| idx | Value (Brio_00) | Meaning |
|---|---|---|
| 0 | 38473 | node count |
| 1 | 1532 | road count |
| 2, 3 | 40966 | length of the road→node index list (list **A**) |
| 4 | 31139 | length of a second list (**B**) — *undecoded* |
| 5 | 23 | number of property names |
| 6 | 2193 | number of strings |
| 7..9 | ? | unknown |

### Nodes — at 0x90, `hdr[0]` × 48 bytes

| Off | Type | Field |
|---|---|---|
| 0 | f32×3 | position `x, y(height), z` |
| 12 | f32×3 | up vector |
| 24 | u16 | `a` — **stable node id** (= the id in `media/LegacyNavNodeIndexToIdMappings.xml`) |
| 26 | u16 | `b` — unknown |
| 28 | u16 | `c` — unknown |
| 30 | u16 | 0 |
| 32 | u32 | `i` — unknown (the script calls it `id`) |
| 36 | u32 | 0 |
| 40 | u32 | `l0` — unknown; `0xffffffff` on plain nodes |
| 44 | u32 | `l1` — unknown; `0xffffffff` on plain nodes |

Nodes are ~20 m apart along a road.

### Road table — at `0x90 + 48·N` (= 0x1c2e40)

1531 full rows × 24 B, then a short 8-byte row for the 1532nd road:

| Off | Type | Field |
|---|---|---|
| 0 | u32 | `count` — number of nodes in this road |
| 4 | u32 | `flags` — `flags & 0xffff` = road class; `flags >> 16` unknown id |
| 8 | u64 | cumulative end index into list A |
| 16 | u64 | cumulative end index into list B |

The 1532nd row is only `{u32 count = 4, u32 flags}` (its two u64 are absent).

### List A — directly after the road table

`u64[hdr[2]]` of **0-based node indices** into the node array. Road *r* =
`A[start_r : start_r + count_r]` with a running `start`. `sum(count) == hdr[2]` (asserted).
**Junction nodes appear in several roads** (shared), which is how connectivity is encoded.

### Road class (`flags & 0xffff`) — NOT a surface type

Values {4, 5, 6, 8}, counts 191 / 1084 / 255 / 2. The earlier guess "4 = highway, 5 = normal road, 6 = dirt/track" is **wrong**: the user checked it in the map viewer and every class mixes
paved and unpaved roads (measured from the terrain surface under the roads, km paved / off-road / unknown: class 4 = 35 / 19 / 4, 5 = 396 / 172 / 37, 6 = 74 / 10 / 21, 8 = 1 / 0 / 0.2).
Class 8 is two roads, 97 % on id 10 (asphalt variant; the airfield strip / an oddity). What the class means is unknown (not verified against the property strings); `flags >> 16` unknown.
*Why it matters:* to know whether a road is paved or off-road, sample the terrain surface id under it — `classify_roads.py`, see [fh6-terrain.md](fh6-terrain.md#surface-kind-paved--off-road-and-the-surface-under-the-roads).
(`decode_nav.py` also exports the node heights `y` per polyline vertex as `roads.json` `heights`; the road-surface sampler needs them.)

**Node ids for external keys.** `decode_nav.py` also exports `ids` (the stable node id `a` per polyline vertex), `nav` ({file, sha1, nodes}) and `orphans` (nodes in no polyline; 0 today) in
`roads.json`. **`a` is unique across all 38 473 nodes** (range 1..52504; the u32 `i` at offset 32 is unique too), and the 1544 polylines contain 39 383 distinct node-to-node edges (no
duplicates), so an edge can be keyed `"<idA>-<idB>"` (idA < idB). The viewer's road editor exports its hand-painted road types that way (see `tools/fh6-extract/README.md`, "Road editor").
*Why:* ids instead of coordinates keeps game data out of the export (licensing rule), and the ids stay valid across re-reads of the same nav file.

### Splitting into polylines

A few consecutive entries are jump links (>60 m apart). `decode_nav.py` **splits a road wherever
two consecutive nodes are >60 m apart** (≈12 gaps), giving 1544 polylines from 1532 roads.

### Unknowns

- ~1.17 MB of further tables after list A (list B etc.) are undecoded.
- The **string pool** at the end holds property names — `ai_disabled`, `deadend`,
  `oneway_forward`, `road_type`, `road_level`, `is_tunnel`, `is_layered`, `give_way`, `satnav`,
  … (23 names) — but the **per-road values are not decoded**. This is where one-way / tunnel /
  bridge flags and a trustworthy road class would come from.
- Node fields `b`, `c`, `i`, `l0`, `l1`; header words 7..9.

---

## 2. Map imagery — `UI/Textures/Data_Bound/Map_Brio_<Season>.zip`

`Map_Brio_{Spring,Summer,Autumn,Winter}.zip`, ~36 MB each, plain Deflate zip (any zip library).
Each holds **85 entries** `<level>-<row>-<col>.swatchbin` (row first), a tile pyramid:

| Level | Tiles | Full size |
|---|---|---|
| L0 | 1 | 1024² |
| L1 | 2×2 | 2048² |
| L2 | 4×4 | 4096² |
| L3 | 8×8 | **8192²** (the full map; = the size of the former bundled jpgs) |

Every tile is 1024×1024. Lower levels are cheaper in RAM (L2 4096 px, L1 2048 px).

### `.swatchbin` tile (all little-endian)

| Offset | Meaning |
|---|---|
| 0x00 | `burG` magic |
| 0x04 | u32 version `0x0101` |
| 0x08 | u32 header size = 140 (0x8c) |
| 0x0c | u32 total file size (= entry length; 524428 = 140 + 524288) |
| 0x14 | section tag `BCXT` |
| 0x2c | section tag `HCXT` |
| 0x4c | u32 width (1024) |
| 0x50 | u32 height (1024) |
| 0x54 | u32 mip count (= 1) |
| **0x74** | **u32 pixel format** (table below) — all other header words are identical across formats |
| 0x80 | u32 top-mip data size (524288 = 1024²/2 for these tiles) |
| `hdr_size` | pixel data |

Pixel data = `bytes[0x8c : 0x8c + size@0x80]`, row-major 4×4 blocks, no gamma conversion needed. **Pixel format = u32 LE at 0x74**
(earlier notes said this field was unknown and guessed BC1 from the size; the icons exposed it):

| 0x74 | Format | Block | Seen in |
|---|---|---|---|
| `0x00` | **BC1 / DXT1** (RGB565 endpoints, 4-colour mode when `c0 > c1`, else 3-colour + transparent; 2-bit indices LSB first) | 8 B | the map tiles (`Map_Brio_*.zip`) |
| `0x09` | **BC7** | 16 B | all 1020 map icons (`Horizon_Map.zip` + 6 ext) |
| `0x0d` | RGBA8, byte order R, G, B, A | 4 B / px | other zips |
| `0x07` | 16 B / block, **probably BC6H** (HDR) — *unverified, not decoded* | 16 B | `HDRImages.zip` only |

Width @0x4c, height @0x50 need not be multiples of 4 (icons are e.g. 121², 141²): the data is `ceil(w/4)·ceil(h/4)` blocks. Decoders:
`extract_map.py:decode_bc1` (BC1, numpy), `extract_icons.py:decode` (BC1/RGBA8 + BC7 through Pillow's `bcn` decoder — `Image.frombytes('RGBA', (w, h), data, 'bcn', 7)` —
with an optional `texture2ddecoder` fallback). Rust: no BCn in the `image` crate — BC1 by hand, BC7 via a crate (`texture2ddecoder` exists).

### Validation

Decoded L3 is pixel-identical (JPEG-noise level) to the former bundled `assets/maps/*.jpg` (since
removed) — so those jpgs were re-encodes of this data and `MapCalibration::DEFAULT` applies unchanged.

### Rust implementation notes

- Implemented in `src/gamedata/tiles.rs`: zip reading with the **`zip` crate (deflate)**, already in
  `Cargo.toml`; one thread per tile row, each with its own `ZipArchive` (L3 ~60 ms release).
- BC1 is decoded by hand (~40 lines, no crate); the `image` 0.25 crate cannot decode BC1.
  `parse_swatchbin` rejects a wrong magic, a total-size mismatch and any pixel format other than 0 (BC1).
- `decode_bc1()` in `extract_map.py` is a compact reference (RGB565 endpoints, 4-colour mode when
  `c0 > c1`, else 3-colour; 2-bit indices LSB first).

### Not the map

`HiRes/Data_Bound` has no map variant. `Horizon_Map.zip` holds 1014 small icon/filter swatchbins — **the game's own map icons**, decoded and mapped to our POI
categories: [FH6 cars, names, regions and icons → Map icons](fh6-cars-names-icons.md#3-map-icons--the-games-own-symbols).

---

## 3. POIs — plain-text files under `Tracks/Brio/`

All positions are in telemetry space (see above). `extract_poi.py` merges everything into one
`pois.json` (`{type, name, x, z, y?, source, precision: "exact"|"cell", extra?}`; 5578 records, 5632
with `--entity-model`). `roaddist.py` checks each type's distance to the decoded roads — exact types
sit within a few metres, cell-based ones do not.

The catalogue near the top lists every category; the subsections below give the format and the
traps for each source. Order of preference: **exact plain-text sources first** (below), then
[GameObjs.xml](#gameobjsxml--exact-gameplay-props), then [GeoChunk/PGZP](#geochunk--pgzp--exact-positions-of-the-above)
for what only the binary has. The "approximate cell centre" records are superseded by those.

### Exact sources

| File (under `media/`) | Format | Gives |
|---|---|---|
| `Tracks/Brio/triggerzones/tz_race_activations/race_triggers.tz` | XML `<triggerzone type name><position x y z/><size…>` | `race_trigger_zone_rt<N>` spheres, radius 100, N = route id → **36 race map pins** (`race_pin` in `pois.json`; median 2.8 m from a road). **They are the pin, not the start line** — 0–780 m (median 185 m) from the real start, circuits anywhere on the loop. Real starts: [Race starts](#race-starts--the-rvan-block-of-the-route-files). |
| `…/tz_world_constraints/landmark_triggers.tz` | same | 75 landmarks with slugs (`shibuya_crossing`, `tokyo_tower`…) |
| `…/tz_bucket_challenges/tz_horizonstories.tz` | same | 11 Horizon Story + 6 Horizon Job activation zones (`*activation_zone`) |
| `…/tz_creatures/creatures_all.tz` | same | 47 creature zones |
| `Tracks/Brio/trackroutes/route0.nt` | XML `<Locator><Name value=…/><SceneTransform value._41 value._42 value._43/>` (`_41`=x, `_42`=height, `_43`=z) | 371 named locators: houses (`player_house_*_root_locator`), fast travel, festival sites, barn finds (`barn_finds_cinematic_<CAR>`) + hint areas (`barn_finds_anna_hint_*`), car/drag meets, touge, showcases, rush, aftermarket spots/boards, treasure cars, `sidi_hj_*` / `sidi_hs_*`, invitational/legend, upsell |
| `trackroutes/pinata_locators.nt` | same | 1536 pinatas |
| `trackroutes/eliminator_locators.nt` | same | 373 eliminator spawns |
| `trackroutes/parkingareas.nt` | same | 2664 parking areas |
| `trackroutes/map_region_<region>.nt` | same | `Arena_NNN` locators = outline points of a map region; `extract_poi.py` reports the **centroid** (10 regions), `extract_names.py` the full outline + names ([regions](fh6-cars-names-icons.md#map-regions--10--10-with-outlines)) |
| `trackroutes/{job,bucket}_challenges_startend_locations.nt` | same | `VOL_HJ_*` / `VOL_HS_*` volumes → mean of their `_start<N>` locators |
| `Stripped/gs/brio/gameobjs.xml` | XML `<Obj GameplayID><Pos value="x,y,z"/>` | 12 treasure-chest discount boards, 14 `BARN_FIND_INTERIOR_*` (interior coords) |
| `OpenWorld/Brio/AITracks/Route<N>.owt` | binary, magic `FTWO` | route node paths, below |

`.nt`/`.tz`/`gameobjs.xml` may start with a UTF-8 BOM (read as `utf-8-sig`).

### Race lines (the `.owt` racing-line files)

`OpenWorld/Brio/AITracks/Route<N>.owt` (magic `FTWO`, little endian) is the route's driven **racing line with the track width**: 170 files, one per route.
Decoder: [`fh6owt.py`](../../tools/fh6-extract/fh6owt.py) (layout) + [`extract_racelines.py`](../../tools/fh6-extract/extract_racelines.py) (trim + width → `racelines.json`).

**Header** — `u32[24]` at offset 0:

| idx | Meaning |
|---|---|
| 8 | **section count**: 1 normally; **4–5 in routes 132, 281, 351, 1181, 1281, 8008** (extra header block, see node offset) |
| 9 | **node count** |
| 11 | low u16 = **start node index** — the node the `RVAN` `start_line` sits on (matches 170 / 170) |
| 20 | = node count again, except in the 6 multi-section files (there it is the loop part) |
| 21 | **256** point-to-point · **257** circuit · **258** circuit with a lead-out tail |

**Nodes** start at `0x60 + (0 if h[8] == 1 else 16 + 48·(h[8] − 2))`, `h[9]` × **56 B**, followed by a 16 or 24 B tail (`FTWO` footer + hash).
Verified for all 170 files: `offset + count·56 + tail == file length` (`read_owt` asserts it).

| Node offset | Type | Meaning |
|---|---|---|
| +0 | f32[3] | position `x, y(height), z` (telemetry space) |
| +12 | f32[3] | **left half-width vector** `A`: horizontal, ⟂ to the path, length **2.5–12.5 m** (usually 5–6). Left edge = `p + A`, right edge = `p − A`; `A = −cross(up, tangent)` |
| +24 | f32[3] | unit up vector |
| +36 | i16, i16 (+40 copy of the first) | smooth per-node signed values — **undecoded** (correlation −0.4 with signed curvature) |
| +44 | u16[4] | run-constant tag (`1,1,1,1`, `19,19,19,187`, …) — **undecoded** (looks like a section / surface id) |
| +52 | u32 | `16` on every 5th–10th node, else 0 — **undecoded** |

Node 0 is **not** the race start (0–1200 m off — lead-in), and some files have NaN nodes (filter with `isfinite`); use header word 11.

**Trimming to one drive** (what `extract_racelines.py` does; the raw file also holds lead-in / lead-out):
- *point-to-point (127)*: `nodes[start .. finish]`, finish = the node nearest the `RVAN` `finish_line` at or after the start. All 127 end within 3 m (max 2.1 m).
- *circuit (43)*: one lap rotated to begin at the start node (`nodes[start..j] + nodes[0..start−1]`); `j` = first local-minimum node after the start within 2.6 m of node 0 (regular circuits: the last node). All 43 close within 3 m. 6 files with a lead-out tail (258) are cut at that node.
- Routes **102 and 103 lie off the playable map**; route **99** is a test route (RVAN start at 0,0). Totals from the script: 170 routes, 171 564 points at 5 m decimation.

*Why the old reader was wrong:* the first version used `u32 @0x20 == 4` → "112 extra header bytes and 2 extra nodes". The real rule is the section count in `h[8]` (4 or 5) and the node count `h[9]`; the old
code dropped nodes on the multi-section files. `extract_races.py` / `extract_poi.py` now use `fh6owt.py` (start line within 0.1 m of its own line: 165 / 169, was 164 / 169).

Start positions come from the `RVAN` block below, **not** from `race_triggers.tz`.

### Approximate only (cell centre, ±100 m or worse) — superseded

> **Superseded** by [GameObjs.xml](#gameobjsxml--exact-gameplay-props) and
> [GeoChunk / PGZP](#geochunk--pgzp--exact-positions-of-the-above), which give exact positions for
> everything here except the barn buildings. Kept because `extract_poi.py` still emits them
> (`precision: "cell"`) as a cross-check.

Drift zones, danger signs, XP boards, drift-circuit props and barn buildings can be found as
**file names** in `Tracks/Brio/ChunkContentsMiniZip0-3.txt` (lists of geometry streams). Proc-cell
`.pgeo` paths look like `scene\proc\cellsize\<S>\<i>_<j>\….pgeo`; the script takes the cell centre
`((i+.5)·S, (j+.5)·S)` and marks it `precision: "cell"`. Matched name patterns:
`driftzonemarker_<NN>_(left|right)_<n>` (only `left` kept — two gate markers per zone),
`tag_dangersign_bm_<n>`, `discount_board_xp_a_<n>`, `tag_time_attack_drift_circuit_*`,
`barn_find_*`. `roaddist.py` shows these types are far from roads (cell-centre error), as expected.
`extract_geochunk.py` validates them (if `pois.json` is in `--out`): 172 of 174 old cell-centre records have an exact point inside their cell (worst 111 m).

### GameObjs.xml — exact gameplay props

**`Tracks/Brio/Ribbon_00/GameObjs.xml`** — a 327 KB *plain XML* file with **exact** position and orientation
for 793 gameplay objects. (An earlier version of these notes called it a dead end — "all 0,0,0". That
was wrong: only 27 `ANIM_*` / `STADIUM_FLOOR` objects sit at the origin.)

```xml
<Obj GameplayID="SPEEDCAMERA_07_LEFT"><Pos value="x,y,z"/><Orientation><ZAxis value="…"/>…</Orientation></Obj>
```

| `GameplayID` pattern | Count | Meaning |
|---|---|---|
| `DISCOUNT_BOARD_XP_{A,B,C}_NNN` | 100 + 75 + 25 | XP boards (variant letter kept in `extra.variant`) |
| `SPEEDCAMERA_NN_{LEFT,RIGHT}` | 30 | **speed traps** — the two camera poles either side of the road; the script reports the midpoint + pole distance (`width`) |
| `SPEEDCAMERAZONE_NN_{LEFT,RIGHT}_{1,2}` | 30 zones × 2 gates | **speed zones** (average-speed sections) |
| `TRAILBLAZER_NN_{LEFT,RIGHT}_{1,2}` | 12 zones × 2 gates | trailblazers |
| `DRIFTZONEMARKER_NN_{LEFT,RIGHT}_{1,2}` | 20 zones × 2 gates | **drift zones** |
| `MASCOTS_REGION_R_NNN` | 200 | mascots (region number kept) |
| `ESTATE_ENTRANCE_NN` | 37 | estate entrances |
| `DISCOUNT_BOARD_TREASURE_CHEST_N` | 3 | treasure-chest boards |

The highest-numbered `DISCOUNT_BOARD_TREASURE_CHEST_N` (over `gameobjs.xml` and the GeoChunk; 015 today) is the **current season's treasure chest** (user observation, 2026-10-03). The map viewer picks it at build time and shows it as its own highlighted layer (`treasure_chest_current`, on by default).
| `BARN_FIND_*` | some | barn-find objects (not extracted by the script) |
| `ANIM_*`, `STADIUM_FLOOR…` | 27 | at 0,0,0 — ignore |

Notes:
- Speed traps, speed zones, trailblazers and photo spots exist **only** here (and photo spots not even here) — they are in no `.pgeo`.
- Gate `_1` / `_2` = start / end of the zone is **not verified** (the script emits both gates and `length_to_gate1`).
- There is **no per-object season / playlist field** — see the [playlist verdict](#6-seasonal--weekly-festival-playlist-verdict).
- *Why this matters:* the app needs **no binary GeoChunk parsing** for these categories — a regex over this XML is enough.

`extract_geochunk.py` reads this file first (works instantly with `--skip-pgeo`) and writes `geochunk_pois.json`.

Another plaintext file: `Stripped/gs/brio/gameobjs.xml` (separate, smaller: treasure-chest boards, `BARN_FIND_INTERIOR_*` with interior
coordinates, `*_FR_FLAG_*` flag-rush flags).

### GeoChunk / PGZP — exact positions of the above

`Tracks/Brio/GeoChunk{0,1,2,3}.minizip` (39.7 / 3.6 / 49.4 / 4.8 GB) are `PGZP` containers: the game's
streamed geometry packages. **Never read them whole** — the script seek-reads only the entries it needs
(`pgzp.py`; the whole prop extraction takes about a second). The entry *names* are in the plaintext
`Tracks/Brio/ChunkContentsMiniZip<k>.txt` (line *i* = entry *i*).

| Chunk | Holds |
|---|---|
| `GeoChunk0` | everything useful: 411 013 entries — `.pgeo` (96 234 prop groups), `.phys` (collision), `.modelbin` (render models), `.mtxmoddxt` palettes, some `.pb`… |
| `GeoChunk1`, `GeoChunk2` | only `.pb` entries = `burG` texture swatch mips named `…_quality<n>.pb` (chunk 2: 78 727 `.pb` + 1113 `.dxt`; higher-quality mips than chunk 0 — q3 = 512 px, q4 = 1024 px). Entry count == name-list length for both, so names line up with rows once the u64-N table is read (below). Not pursued further |
| `GeoChunk3` | textures only (`hqdxt` / `hqbc3`) |

**Name list.** Each line looks like `<PREZIPPED>d:\scratch\p4\forte_main\zipcache\pc\tracks\brio\scene\proc\cellsize\200\3_4\x.pgeo|<n>` —
strip the build-machine prefix and the `|n` suffix (`n` is *not* the entry index; its meaning is unknown).

**Container layout** (little endian):

| Offset | Type | Meaning |
|---|---|---|
| 0 | u32 | `PGZP` (0x505a4750) |
| 4 | u32 | version 101 |
| 8 | u32 | header size 32 |
| 12 | u32 | N = number of entries (= line count of `ChunkContentsMiniZip<k>.txt`) |
| 16 | u32 | M = length of the id list |
| 20 | u32 | 512 = entries per segment |
| 24 | u32 | S = segment count = ceil(N/512) (803 for GeoChunk0) |
| 28 | u32 | 0 |
| 32 | u32[M] | ascending id list (subset of entry ids; unused) |
| 32 + 4M | N, u64 | N again, then the absolute offset of the first segment. **N is a u32 in GeoChunk0/1/3 but a u64 in GeoChunk2** (high word 0), which shifts everything after it by 4 bytes — `pgzp.py` detects it (`S[1] == 0` ⇒ wide), the old reader asserted on GeoChunk2 |
| per segment | 512 × `{u32 off, u32 usize, u32 flags}` + u64 | one row per entry, then the absolute offset of the **next** segment (for the last segment this word is not an offset: the last entry's compressed size is then taken to end-of-file — `pgzp.py`) |

- Entry data starts at `segment_start + off`. **Compressed size** = (next row's `off`) − `off` inside a segment, or (next segment start) − this start for the last row.
- `flags & 0xff` selects the codec: **`0x1f` = raw LZ4 block** (no frame header; decoded size = `usize`), **`0x08` = raw deflate** (zlib `wbits = −15`), **`0x00` = stored** (terrain `.phys` entries often are). The upper 24 bits are unknown (look like a running counter / hint).
- *Why a hand decoder:* no Python/Rust zip crate reads this; the LZ4 block format is ~25 lines (`lz4b.py`), deflate is `flate2`.
- *Rust (I25):* `src/gamedata/{lz4,pgzp,burg}.rs` port `lz4b.py` / `pgzp.py` / the `burG` helpers of `extract_terrain.py`. Raw deflate (`flags & 0xff == 0x08`) is **not** implemented (no entry the app needs uses it; adding it would mean `flate2`). The road graph is read by `src/gamedata/nav.rs` (port of `decode_nav.py`: polylines split at > 60 m, stable ids, SHA-1 of the whole file, reference 38 473 nodes / 1 544 polylines / 39 383 edges). See [terrain](fh6-terrain.md#rust-implementation-notes) and [map tooling](fh6-map-tooling.md#rust-reader-and-the-current-road-types-srcgamedataroadtypesrs-i25).

**`.pgeo` layout** (decoded entry; one "section" per file; `extract_geochunk.py:parse_pgeo`):

| Piece | Layout |
|---|---|
| section name | u32 len + name, e.g. `c200_props_do_rt_0_discount_board_xp_a_077_cellx2z45_section0` |
| header | u32 0, u32 13, u32 15, f32 bbox_min[3], u32 0, f32 bbox_max[3], f32 1.0, then a `PROPS`/`SIGNS` block + tag string |
| per model | u32 len + name (`gpy_gbl_bonusboard_01_a_3D`), u32 count, then `count` × **80-byte instances** |
| instance | **position** = 3 × u32 in **sign-magnitude 16.16 fixed point** (bit 31 = sign, value = `(u & 0x7fffffff) / 65536`; x, y, z); 6 × f32 (right + up unit vectors); 3 × f32 scale (1,1,1); 32 B undecoded |
| after | a LOD block with the same name minus `_3D` (not needed) |

*Why the odd position encoding matters:* misreading it as f32 gives garbage; validated by comparing 180 pgeo instances with the matching
`GameObjs.xml` objects → **0.000 m** error (max and median).

What the `rt`/`tag` pgeo files give (one file per tag per cell, so a file *is* a gameplay prop group):

| Tag | Result (`extract_geochunk.py`) |
|---|---|
| `tag_dangersign_bm_NN` | **15** danger signs (rush-ramp + construction board + cones; centre = mean of the boards) |
| `tag_drift_zone_NN` | **1027** drift-zone marker posts lining the zones |
| `driftzonemarker_*`, `discount_board_*` | duplicates of `GameObjs.xml` positions (used for the 0.000 m cross-check) |
| `tag_time_attack_drift_circuit_*` | 13 drift-circuit prop clusters |

Also present as instances: ~1700 rush ramps (**not extracted**) and the speed-limit signs below. Photo spots are not in any pgeo.

#### Speed-limit signs

`extract_speedsigns.py` seek-reads only the **1182 `c300_signs_do*.pgeo`** entries of GeoChunk0 (~5 MB, a few seconds). Same 80-byte instance as above; models:

| Model | Count | `kind` |
|---|---|---|
| `sgn_gbl_info_speed_01_a` | 1659 | `limit` — speed-limit sign (red ring) |
| `sgn_gbl_info_high_speed_01_a` | 217 | `limit_high` — same family on a pole / expressway |
| `sgn_gbl_info_pfb_speedend_01_a` | 265 | `limit_end` — "end of speed limit" (white disc, slash) |

The 32 undecoded instance bytes (`+48..+79`): `+48 0x80800000`, `+52 0xffffffff`, `+56 0` (constants); **`+60` u32 VARIANT 0..5**; `+64 0`; `+68` per-instance hash; `+72 0`; `+76` 256 or 512.
Variant counts for `limit`: v0 661, v1 718, v2 129, v3 17, v4 45, v5 89 (`limit_high`: 79 / 70 / 42 / 2 / 18 / 6; `limit_end` is always 0).

- **The variant is the printed number.** The model has materials `SGN_GBL_INFO_SPEED_01_A_Alpha_Retro` (= variant 0) and `_VARIANT_1.._5`; their `MatI` blocks differ only by two float
  parameters, a UV offset `(0,0) (.25,0) (.5,0) (.75,0) (0,.25) (.25,.25)` — a cell of a **4-column number atlas**. That atlas texture is **not** in the model's swatches (they hold the blank red-ring face) and was not found elsewhere,
  so **the km/h value of each variant is not readable**. `speedsigns.json` has `limit_kmh: null` unless you pass `--variant-map "0=50,1=60,…"`.
  *Guess from where each variant stands (spatial inference), lowest → highest limit: 5 / 3 < 0 < 1 < 2 < 4* — **a guess, needs an in-game check.** One sign per variant to read the number off in-game (x, z): v0 (3097, −2360), v1 (1290, 1788), v2 (−1142, 62), v3 (−2073, 1451), v4 (1962, −2538), v5 (−2117, −4773).
- **Facing:** `heading = atan2(n.x, n.z)` with face normal `n = −(right × up)` (the face quad sits at local z = −0.05, the pole at z > 0); 0 = +z (north). Signs face **oncoming traffic**; 97.4 % are consistent with left-hand traffic.
- **Road snap:** median 7.1–7.2 m to the nearest `Brio_00.nav` polyline (signs stand beside the road, not on it); `extract_speedsigns.py` adds `road_dist_m` / `road_class` when `roads.json` (from `decode_nav.py`) is available.

### Race starts — the `RVAN` block of the route files

`OpenWorld/Brio/AITracks/Route<N>.nav` is **not just a road graph**: after the `WVAN` graph there is a second block, magic
**`RVAN`**, that holds the race locators of that route. This gives the **exact start line, finish line and 12-slot starting grid with
headings for 169 of 170 routes** (route 99 is a test route at 0,0). `extract_races.py` decodes it.

| Offset in block | Type | Meaning |
|---|---|---|
| (file) | `RVAN`, u32 ver = 2, u32 id, u32 size | 16 B header; block = the next `size` bytes |
| +0 | f32[4] | `start_line` (x, y, z, 0) |
| +16 | f32[4] | `finish_line` (== `start_line` for circuits) |
| +56 | u32 | `nrect`: checkpoint-gate records of 76 B (centre + 4 corners + id), starting at +80 |
| +60 | u32 | `npts` = 14 |
| +64 | u32 | 251 (magic) |
| … | 14 × 48 B | `{f32 pos[3], 0, f32 dir[3], 0, u32 idx, 0, 0, 0}` for `start_line`, `finish_line` and the 12 grid slots `start_location_000..011` |
| end | NUL-terminated names | the name pool; **hash-sorted**, and the 14 point records are in the *same* order — so read the names from the pool, don't assume an order |

Validation:
- The start line lies on the route's own `.owt` racing line (< 0.1 m for 168 of 170 in the original check, 164 of 169 with `extract_races.py`'s nearest-node metric; median 0.00 m).
- It is a median **3.1 m** from the nearest `Brio_00.nav` road.
- The `race_trigger_zone_rt<N>` spheres are **map pins, not starts**: 0–780 m away (median 185 m); for circuits the pin sits anywhere on the loop.
  *Why this matters:* the first version of these notes used the pins as "race starts" — drawing a start line or grid there is wrong.
- Heading: `atan2(dir.x, dir.z)` in degrees (0 = +z = north, 90 = +x).

### Race map pins: which routes have one

The editor draws each race at the in-game **map pin** (user, 2026-10-03: the pin circles match the in-game map exactly, the RVAN start lines are offset from them). Exact route -> pin links that exist in the install:

| Source | Routes | Link |
|---|---|---|
| `race_triggers.tz` `race_trigger_zone_rt<N>` | 36: 71, 101, 121, 131, 141, 161, 201, 281, 301, 311, 351, 352, 1021, 1023, 1171, 1211, 1281, 1421, 2091, 2311, 4351, 4501-4503, 5031, 5041, 5191, 5201, 5555, 6001, 8001-8006 | `<N>` in the name = route id |
| `route0.nt` `sidi_touge_event_<N>` | 5031, 5041, 5191, 5201, 5411 (4 of them also have a sphere, 7-20 m away; the sphere wins, 5411 uses this locator) | `<N>` in the name = route id |

That is **37 of the 170 routes**. The user's three examples are all in the first table, route ids 1281 Edogawa Cross Country Circuit (start -2126.0, -4603.2 -> pin -2155.4, -4744.1), 1421 Nangan (-1213.4, -8928.8 -> -1263.7, -8894.4) and 4502 Irokawa Space Center Drag Strip (-1052.7, -8731.9 -> -988.1, -8652.9); the viewer now draws them at the pin.

**No other pin source exists in a user install** (searched 2026-10-03, all negative):
- other `.tz` files: `tz_races/triggers_route_{3333-3336,8001-8005}.tz` are IE route meshes and `bullet_time_trigger` boxes (no pins), the rest are world constraints, creatures, landmarks, Horizon Jobs/Stories, particles, sky;
- `.nt` locators: `route0.nt` has only the 5 touge events and the 3 `drag_meet_NN_activation` locators (meet `01..03`, no route id; routes 4501-4503 already have spheres) as race-related names; the other `.nt` files are jobs/stories, pinatas, parking, eliminator, arenas, regions;
- `Ribbon_00/GameObjs.xml` (793 objects) and `Stripped/gs/brio/gameobjs.xml`: no race/route object; `Locators.xml` / `TriggerVolumes.xml` are empty; `Route<N>.nav` strings only contain `start_line`, `finish_line`, `start_location_000..011`;
- `ObjectModelGame.zip` (all 7121 data sets decoded): the only coordinates are Horizon Story/Job destinations, Stunt Party centres, Hide & Seek spawns, drift waypoints and `FallbackWorldXYZ` (all `0,0,0`); the per-race data sets (`TrackInfoDataSet`, `CareerRaceDataSet`, `EntityIdToChallengeData`, `SlotDataMap`) hold no position, only guid references (`Domain=zip`) into the **encrypted `Stripped/EntityModel.zip`** (585 entries, zip method 22);
- `ChunkContentsMiniZip*.txt`: asset lists only.

So the remaining 133 routes' pin positions are presumably entity placements in the encrypted `EntityModel.zip` (the creator dump's `campaign_slots.xml` family) - not loadable. The editor keeps those at the RVAN start line and marks them as such. Do **not** fill them by nearest-neighbour matching: it was ruled out by the user, and with the sphere distances above (0-780 m from the start) it would be a guess.
Update: for the routes without a sphere the start line is a *validated prediction* (their pin sits there, within 41 m), see [Predicting race type and map-pin position](#predicting-race-type-and-map-pin-position-validated-against-the-users-marks); the viewer labels it "predicted: at start line (+-N m)".

### Race names

**String tables.** `Stripped/StringTables/<LANG>.zip` (plaintext) holds one `<Table>.str` per UI area (291 tables in `EN.zip`;
`fh6str.py`):

| Offset | Meaning |
|---|---|
| 0x00–0x93 | header |
| 0x94 | u32 count |
| 0x98 | count × `{u32 key, u32 offset}` |
| after | NUL-terminated UTF-8 strings (offset relative to the end of the key table) |

`key` = hash of the identifier: `h = 0xFFFFFFFF; for each byte c: h = rotl32(h ^ c, 7)`. Example:
`Landmarks.IDS_Area_Discovered_ine` → table `Landmarks`, key `hash("IDS_Area_Discovered_ine")` → `Ine`. Database strings `_&<u64>` use the
**low 32 bits** as the key (search all tables).

**The exact route → name link exists** (found 2026-10-03; an earlier version of this page said it did not): `ObjectModelGame.zip` → see
[Exact race names and types](#exact-race-names-and-types-objectmodelgamezip) below. It covers 111 of the 170 routes and `extract_races.py` lets it override everything else.
The rest of this section is the **older fallback** for the ~60 routes without a TrackInfo entry (route files 11000–11045, 20000–20005, 30000–30006, 30100–30106, 99, …); 
`CareerRaceCollection.str` holds race names in its *second half* (entries ≥ 154 of 323 — the lower entries are descriptions; the upper block mixes ~110 race names with playlist, Playground-arena and drag-strip names).
The fallback methods, by increasing confidence (recorded in `extra.name_confidence` / `name_evidence`; only `exact`, `locator` and `landmark<=400m` are written to `race_name`,
the rest to `race_name_guess`):

| Method | Routes | Needs |
|---|---|---|
| IE routes by id convention (`3333–3336` = Drive Section 1–4, `3337` = City Tour); Horizon Chase `30100 + n − 1` | 5 + 7 | nothing |
| Horizon Rush: nearest `sidi_rush_{docks,ski,spaceport}` locator in `route0.nt` | 3 | install |
| Finales via `campaign_slots.xml` `ContextId` (`The<Name>RaceEventActivation`): Titan (route 1023), Goliath (5555), Gauntlet (2052), Colossus (132) | 4 | **creator dump** |
| Exit locator in `post_race_locators.xml` (`race_<collectionId>` → `<name>_custom_exit_locator`) | 6 | **creator dump** |
| Heuristic: per event family, assign that family's names to its routes by **Hungarian matching** on the distance landmark keyword → racing line (keyword table `K` in the script; 5/5 manual checks matched) | 44 (≤ 400 m) + 9 (≤ 1000 m) + 1 (elimination) | **creator dump** (for the family) + scipy |

These methods (before the exact link was found) gave only 15 of 169 routes a name without the creator dump; they are now just the fallback behind the exact names.
Event families for 88 routes (`brio_{circuit,sprint,scramble,trail,street,touge,finale,horizon_rush}_<N>` in `campaign_slots.xml`, with
`CareerRaceCollectionId` 80–87 = cross-country circuit, 88–97 = cross-country sprint) remain creator-dump only — and were the **ground truth** the exact types below were validated against.

### Exact race names and types (`ObjectModelGame.zip`)

`<media>/ObjectModelGame.zip` is a **plaintext** zip of 7121 `source/ScribbleData/<id>.om.xml` files, each a `BXML` binary XML (decoder `fh6bxml.py`, `bxml_decode`;
the same format as the creator's `EntityModel.zip`, but this zip is *not* encrypted in the user install). Code: `tools/fh6-extract/fh6careers.py` (`race_types`), called by `extract_races.py`.
Three files matter (ids stable between the Sept and Oct 2026 builds we looked at):

| Data set | File id | Content |
|---|---|---|
| `TrackInfoDataSet` | `13260499414882115191` | two maps. `Data` (112 entries; key = *CareerRace key*): `RouteId`, `RibbonConfig` (`P2P` / `Circuit` / `Playground`), `UseCrossCountryAI`, `DisplayName` = `CareerTrackInfo.IDS_DisplayName_<guid>` (resolve with `fh6str.StringTables(media, lang).ids(...)`, any of the 24 languages), `CustomRouteId`, …. `InfoByRouteId` (111 entries): **route id → key into `Data`** — this is the link the old notes said did not exist. Route 2091 also appears in `Data[4048]`; `InfoByRouteId` says key 39 (use that one). One `Data` entry has no `RouteId`. |
| `CareerRaceDataSet` | `16066973273702787567` | only 35 keys (5–11, 20–43, 60–63): `RaceMode` (Touge / LapsRace / P2P / Scramble / Drag / Showcase), `UITheme` (`asphalt_series`, `touge_series`, `drag_racing`, `mixed_surface_series`, `showcase_*`), `CustomEntityName` (`touge_event_5411`, …) |
| `RaceCollectionUIOverridesMap` | `6322925578581225131` | 31 keys → flyer texture. Keys 102–116 → `Backgrounds\Custom\StreetRace.png` (the 15 street races); also `Touge_Racing_*` for keys 1, 6–9 and finale flyers |

**Pitfall:** the `TrackInfoDataSet` object holds *two* maps; a naive "all `map_element`s under the object" walk mixes the `InfoByRouteId` entries (route-id keys) into `Data` and corrupts it. Select the map by its `property id`.

**Type mapping (exact fields only):**

| Editor type | Field |
|---|---|
| `road` | `CareerRaceDataSet[key].UITheme = asphalt_series` |
| `touge` | `UITheme = touge_series` |
| `drag` | `UITheme = drag_racing` |
| `rally` | `UITheme = mixed_surface_series` |
| `street` | key has the `StreetRace.png` flyer in `RaceCollectionUIOverridesMap` |
| `cross_country` | `TrackInfoDataSet[key].UseCrossCountryAI = True` (keys 80–97 and 200 = The Titan), **except the Initial-Experience routes 3333–3337** |

Result on the Oct-2026 build: **67 of 170 routes typed** — road 21, cross country 19 (18 + The Titan), street 15, touge 5, rally 4 (keys 60–63), drag 3 (routes 4501–4503) — and 103 untyped.
Validation: against the creator dump's `campaign_slots.xml` families (circuit/sprint/street/touge/…) there were **0 contradictions** for the 67 (road = circuit + sprint, street incl. key 109 = route 4341 "Rainbow Bridge Descent").
Names: **111/111 EN and 111/111 DE** (e.g. 281 "Highway Circuit" / "Highway-Rundkurs", 5411 "Hakone Nanamagari", 1023 "The Titan" / "Der Titan").

**Routes left without a type, on purpose:**
- **Initial-Experience routes 3333–3337** carry `UseCrossCountryAI=True` (key 1257, 1258, 1308–1310) but are tutorial drives, not cross-country races, so the extractor skips the flag for them (an explicit rule in `fh6careers.INITIAL_EXPERIENCE`; they have no other type field, so they stay unmarked).
- **The 16 scramble / trail routes** (keys 64–79: scramble routes 121, 162, 181, 271, 301, 341; trail routes 2051, 2061, 2071, 2101, 2121, 2211, 2271, 2281, 2301, 2311): no field states a type for them (`CareerRaceDataSet` has no entry above key 63). Their *names* contain "Scramble"/"Trail", but that is a name, not a type field — see the Why below. The user marked all of them `rally`, and the AI-driver-family field agrees: all **17** (these 16 plus the finale 2052 Gauntlet) use the `Dirt_*` family, see [Predicting race type](#predicting-race-type-and-map-pin-position-validated-against-the-users-marks).
- Finales (132 Colossus, 2052 Gauntlet, 5555 Goliath), Horizon Rush/showcase/invitational routes 8001–8008, Playground arenas 3001–3023, 4251, route 0, and the ~59 routes with no TrackInfo entry (11000–11045, 20000–20005, 30000–30006, 30100–30106, 99): no editor-type field, or no entry at all.

*Why: exact fields only, no inference.* The user's call (2026-10-03), after seeing types guessed from route geometry, surface and name words: "That's a bad way of doing it". A type the files do not state is left unset (the map viewer shows a grey "?") for the user to mark by hand, so every pre-marked type can be traced to a named game field (`extra.type_source`). Do not add heuristics here without the user asking.

Consumers: `extract_races.py` writes `names{lang}`, `type_exact` (absent when none), `type_source`, `career_key`, `ribbon` into the `extra` of each route's `race_start` / `ie_route` / `horizon_chase` record, and the EN name becomes `race_name` (confidence `exact`); the viewer's **Prefill race types (exact)** button uses `type_exact`.

Corrected trap (earlier notes were wrong twice): the landmark strings for `seaside_circuit` / `seaside_offroad_circuit` are **not swapped** — `seaside_circuit` (2606, 2805)
is the Hokubu Circuit (race start 2827, 2696) and `seaside_offroad_circuit` (2676, −5095) is Sekibe Scramble (2496, −5065); and landmark names resolve for **75 of 75** slugs from the
install (not 54 of 75 — 17 slugs just use a different string id, see [names](fh6-cars-names-icons.md#landmarks--75--75-named)), so `landmark_areas.xml` (creator dump) is not needed.

### Hand-classified road and race types

The user marked the whole road network and the race types **by hand** in the map viewer's editor on 2026-10-03, against the in-game map. The result is committed as
`assets/map/fh6-road-types.json` (moved there from `tools/fh6-extract/data/` in I25; originally ids and types only, **no coordinates**, so no game data; format `fh6-road-types` v1 when first committed, v2 now with the user's own points, see [fh6-map-tooling.md](fh6-map-tooling.md); read in Rust by `src/gamedata/roadtypes.rs`). The viewer
starts from it and has a "Reset to project data" button. Contents (nav sha1 `a88c69f4…`, 38 473 nodes): 39 360 of 39 383 edges painted + 43 added links (road 553.9 km, offroad 207.5,
other 6.7, not set 0.4) and **93 of 170 races typed**: rally 21, road 21, cross country 19, street 17, story 6, touge 5, drag 3, wristband 1.

- **Race types** are the six the game states (road / street / rally / cross country / touge / drag) plus **story** (a Horizon Story pin: Horizon Invitational 8006,
  story routes 11017, 11044, 30002–30004) and **wristband** (a Wristband event; DE "Armband-Event", the game's own term; so far only 8004 "Mech My Day", a showcase like "Launch Control").
  The first editor versions had a single `midnight` ("Midnight Battle") type; the user's marked "Midnight Battle" pins are Stories, except 8004. Importing a legacy file maps
  `midnight` to `story` (8004 to `wristband`). The Colossus 132 was marked story first; the user re-marked it **street** on 2026-10-04 ("Its a Street Race.").
- **The user's marks agree 100 % with every exact type** (`type_exact`, see above): 0 conflicts over the 61 races both cover. The canonical file additionally contains the 6 exact-typed
  routes the user did not mark (1211 cross country; 2031, 2201, 2241, 2261, 2361 road), filled from `type_exact`; everything else in it is the user's own marking.
- **Story / wristband in the game files** (only what is exact): route 8004 has `CareerRaceDataSet[10]` `EventType=Showcase`, `UITheme=showcase_mech` (8005 key 11 is `showcase_planes`);
  `Showcase` is not literally "wristband", so it is not used to set the type. Routes 11001–11045 and 20000–20005 are `HorizonStoryChallengeData.RouteId`s (the story chapters' destinations;
  11017 = `VOL_HS_CanyonDaytrip_chapter_02`, 11044 = `VOL_HS_TanakasAuto_chapter_06`), so those two are exactly stories. For 8006 and 30002–30004 no field was found
  (no TrackInfo/CareerRace entry, not a story challenge route id), so those marks are the user's call alone.
- **Why:** the game files do not state the road surface class (see [fh6-terrain.md](fh6-terrain.md)) nor, for most races, a pin or a type; the user chose to mark them by hand
  against the in-game map rather than have them inferred from geometry or names. Nothing in this file is inferred.

### Predicting race type and map-pin position (validated against the user's marks)

Everything above is **exact** (a named game field) or the **user's own mark**. For the other routes the build adds *predictions*: `tools/fh6-extract/racetype_method.py` (type), `pin_method.py` (pin),
driven by `extract_predictions.py` -> `predictions.json` (per route: `type_predicted`, `type_kind`, `type_confidence`, `type_source`, `type_why`, `type_alternatives`, `pin_predicted {x, z, method, expected_err_m, note}`).
`build_viewer.py` merges them into the `extra` of each route's `race_start` / `ie_route` / `horizon_chase` record. The viewer shows them as three separate rows (Game files / Your mark / Predicted) and queues the
doubtful ones in the editor's **Review queue** (Accept = an ordinary undoable mark; predictions never write into `fh6-road-types.json` or the user's marks by themselves).
The user's hand marks (93 routes) are the ground truth the method was built and checked against (user, 2026-10-03: "use the actual data that I've provided so far to try and harden the method").

**Race type: ordered sources, the first that answers wins**

| # | `type_source` | Answers when | Confidence |
|---|---|---|---|
| 1 | `exact` | `fh6careers.race_types` states a type (the exact fields above) | 1.0 |
| 1b | `finale_icon` | an explicit per-route override (`FINALE_ICON` in `racetype_method.py`; 5555 The Goliath and 132 The Colossus -> `street`) | 1.0 |
| 2 | `event` | `HorizonStoryChallengeData.RouteId` -> `story`; `CareerRaceDataSet.EventType = Showcase` -> `wristband` (analogy to the user-marked 8004) | 0.90 / 0.70 |
| 3 | `ai_family` | the route has `Route<N>_<level>` DifficultyLevels (86 routes) -> its AI-driver family | 0.80-0.96 per family (precision on the marks), 0.5 on a Street/Road tie |
| 4 | `not_a_race` | Initial-Experience drives, Horizon Chase, routes with no collision under the line and all AI tags 0 (99, 102, 103) | 0.8-0.95 |
| 5 | `copy` | id >= 30000 and >= 95 % of its racing line lies within 3 m of another route's line -> `story` (event copy; 30002-30004 are marked story) | 0.80 |
| 6 | `line` / `id_convention` | fitted rules on the racing line (below) | leaf probability |

**The AI-family field (source 3).** `ObjectModelGame.zip` holds 613 `DifficultyLevel` objects (root `type="DifficultyLevel"`, one `.om.xml` each; the file ids differ per object, so find them by the type attribute, not by id.
The type name is in the BXML string table, so a byte pre-filter is enough, 0.2 s). Each has a `DifficultyName` and object-guid slots (`RaceTrack`, `RaceStart`, ...). The game's *generic* levels are named `<Family>_<level>`
(`Dirt_`, `Cross_`, `Street_`, `Road_`, `Drag_`, `Touge_` x `AboveAverage ... Unbeatable`); a route has its own `Route<N>_<level>` set (86 routes in the Oct-2026 build, 9 levels each). **Fingerprint = the
(`RaceTrack`, `RaceStart`) guid pair per level**: a family scores 1 per level whose pair equals the generic one, the best family wins (margin = best - second). Mapping: `Dirt` -> rally, `Cross` -> cross_country,
`Street` -> street, `Road` -> road, `Drag` -> drag, `Touge` -> touge. Margins in this build: Dirt 5, Cross 6, Drag 6, Touge 5; **Road vs Street is the only close pair (margin 1: they differ in one level, `Unbeatable`)**.
*Why this source:* it is a **game field** (which AI drivers the game itself runs on the route), not a guess from geometry or names, and nothing in it is fitted. So it outranks the line rules, and unlike them it needs no calibration.
It also settles the 17 scramble / trail routes: all use the `Dirt_*` family, consistent with the user's `rally` marks.

**Line rules (source 6; only for what 1-5 leave open: 9 routes in this build).** Features are computed on the trimmed racing line (`racelines.json` points):
`s_off` = share of line points whose exact terrain triangle (height-matched within 15 m, same sampler as `classify_roads.py`, kind table `fh6surfaces.SURFACES`) is off-road; `t_dirt` = share (by length) of `.owt`
node `tag[0]` in 272-274. Ordered rules (thresholds fitted by an exhaustive 1-D search on the 86 race-typed marks): `s_off >= 0.124` -> off-road, within that `t_dirt >= 0.148` -> `rally` else `cross_country`;
point-to-point and length <= 1844 m -> `drag`; circuit -> `road`; paved point-to-point -> road / street / touge are not separable from the line, so the **route-id thousands digit** is used (2xxx road, 4xxx street, 5xxx touge)
and the source is labelled `id_convention`. *Why labelled:* the digit is a **naming convention** in the route ids that happens to hold on the 85 marks, **not a game field**; a future build could break it, so the viewer
says so on every prediction that rests on it (currently only 4301). `extract_predictions.py --refit` re-fits the thresholds after a game update (prints leave-one-out accuracy; writes nothing).

**Accuracy against the user's 93 marks** (`extract_predictions.py` prints it on every run; the build log repeats it):

| Test | Result |
|---|---|
| Full pipeline, all 93 marks | **92 / 93** (98.9 %); the one miss is listed below |
| Hand-only marks (26 without an exact type) | **25 / 26** |
| Race-typed marks (86) | 86 / 86 |
| AI family alone, no fitted parameter (85 marks with a family) | **83 / 85** (97.6 %); on the 66 exact-typed ones 66 / 66 |
| Line rules alone, leave-one-out, without the id hint | 67 / 85 = **78.8 %** (road+street merged 91.8 %) |
| Line rules alone, leave-one-out, with the id hint | 81 / 85 = **95.3 %** |
| Line-rule confidence calibration (with id hint) | 0.6-0.9 bucket: 78 / 81 held-out correct; < 0.6: 3 / 4 |

Miss of the full pipeline - **flagged for review**, none is forced:
- ~~5555 The Goliath~~ and ~~132 The Colossus~~ - fixed by the `finale_icon` override (next paragraph).
- **8006 Horizon Invitational**, marked story, predicted road at 0.5 from the line: unverifiable (no family, no story-challenge entry).

**Series finales carry the street icon (`finale_icon`, source 1b).** Each race series has one finale. The Goliath (5555) has the `Road` AI family (6/6; `Street` 5/6, it lacks the `Street_Unbeatable` match),
but in-game it shows the street (purple) icon and players treat it as the street finale, so `racetype_method.FINALE_ICON = {5555: 'street'}` predicts `street` (confidence 1.0); 132 The Colossus (AI family `Road` 6/6, a 37.7 km paved circuit) is listed too. User rule, 2026-10-03: "Goliath is technically a
road race, but it has the purple icon, so everyone considers it a street race, especially since for every race type there's one final race, so it should be purple." For 132 (2026-10-04): "Its a Street Race."
*Why:* the in-game icon is what players see; the AI family is an implementation detail. Only 5555 and 132 are listed: the other finales (2052 The Gauntlet, 1023 The Titan) are not decided.

Classes the method **cannot separate**: road vs street on a route without an AI family (the line is the same kind of paved road; only the id digit hints), story / wristband / invitational pins vs the race they
sit on (events are only recognised by `HorizonStoryChallengeData` / `Showcase` / a copied line), and touge vs drag / street without the family (touge: 4 of 5 held out). 11000 and 11002 sit next to story-challenge ids
without an entry of their own (the viewer's queue says so).

**Map pin (`pin_method.py`).** Rules in order: **exact** (a `race_trigger_zone_rt<N>` sphere or the touge locator, 37 routes, error 0) -> **event POI** (route 8001-8099 without a pin: the nearest *unclaimed* special-event POI
(`rush_event` / `showcase` / `special_event`) within 600 m of the start; in this build only 8007 -> `legendevent`, 23 m from its start line) -> **start line** (the RVAN start).
*Why the start line:* the game only needs an extra trigger sphere where the pin is **not** at the start, so a route without a sphere has its pin at the start. Validated against an independent in-game-captured pin set:
**55 of 55** checked routes without a sphere have their pin at the start line, within **<= 41 m (median 5 m, p90 18 m)** along the road. Expected errors reported with each prediction: 18 m races, 20 m event rule, 25 m
unverified (128, 4301, 8008), 30 m story / job routes (11000-30100; the start sits on the story activation); no pin expected for test / tutorial / chase routes. **No geometric method predicts a *displaced* pin**
(every hypothesis tested was ~150-500 m off, leave-one-out ~150 m), so a displaced pin is only knowable from the exact sources. The viewer wording is "predicted: at start line (+-N m)".

**Caveats.** The guids and tags were checked on the Oct-2026 build only (re-run after a game update; `--refit` for the line thresholds). The drag threshold (1844 m) rests on 3 samples and is fragile. `.owt` tag 272-274 is an
**undecoded** field (60 % of the rally lines, 0-3 % of road / street / touge), so its use is empirical. Confidences are Laplace-smoothed precisions on the user's marks, not probabilities of truth, and the marks themselves are
human: a mismatch is as likely a slip in the mark as in the prediction, which is why the review queue exists. The `why` strings are English only.

*Why predictions are kept apart:* the user's earlier call (exact fields only, no inference) still holds for the *exact* rows. Predictions are a separate, labelled layer, so a guess can never be mistaken for a game field or
for the user's own mark, and they only become a mark when the user clicks Accept.

### More categories (route files, arenas, train, creator dump)

From the install (all added to `pois.json` by `extract_poi.py`):

| Category | Source | Notes |
|---|---|---|
| Car meets (extra) | `route40001.nt` (Evolving World), `route40900.nt` (Hokubu): `carmeet_<name>_locator` | **were never read by the first version**; `…_characterlocator_NNN` / `…_parkinglocator_NNN` are the character/parking slots |
| Upsell pins | `route40900/4004x/4005x.nt` `sidi_upsell_*` | the series-4/5 pins share one spot → merged |
| Playground Games arenas (3) | `route3001-3003` (docks), `3011-3013` (spaceport), `3021-3023` (ski resort) `.nt`: `Arena_NNN` outline points | each area has 3 route files (team King / Survival / Flag Rush; names in `CareerRace.str`) with 12 `start_location_NN` + `finish_line_NN`; position = outline centroid |
| Hide & Seek arenas (6) | `route8100-8105.nt` | area names from `Stripped/gs/tracks/brio/scene/gameplay_locations_boundries/hideseek/gpl_hideseek_<name>.i.zip` (`arena_01, city_east, city_west, east_coast, spaceport, west_coast`); the **name ↔ route-file assignment is inferred from geography** |
| Flag-rush flags (6) | `Stripped/gs/brio/gameobjs.xml` `*_FR_FLAG_*` | |
| Rural train line | `OpenWorld/Brio/Freeroam/Ambient_RuralTrain_RuralLine.owcp`: magic `PCWO`, 0x20 B header, then 16 B `{x,y,z,0}` points (505) | polyline of the ambient train |
| Map element / filter types | `UI.zip` → `MapProfiles/MapIncludes/*.xml` | the game's own ~230 map element/filter types — a ready vocabulary for a POI legend |

**Creator dump only** (`--entity-model PATH` on `extract_poi.py` / `extract_races.py`; `fh6bxml.py` decodes the `BXML` binary XML): photo spots (37,
`entities_photo_challenge_landmarks.xml`, `TriggerZonePosition`), time-attack leaderboard boards (4; names from `TimeAttack.str` matched to the nearest landmark circuit —
inferred), Horizon Chase starts (7, `RouteStartTeleportPoint`), Backstage Passes, Labyrinth matchmaking entrance, IE activator, community-gift shop, Legend Island gates (2),
and the race types/names above. **Not loadable from a user install** because `Stripped/EntityModel.zip` is encrypted.

`BXML` layout: `BXML`, u8 version, u32 string count, u32 size, then `count` × (u16 len + UTF-8), one filler byte, then the node tree: u8 `op` (bit 1 = has
attributes, bit 2 = has children), string index (u8 if < 256 strings else u16), `[u8 nattr, nattr × (name idx, value idx)]`, `[u8 nchildren, u8 0, children…]`.

**Bugs fixed in `extract_poi.py`** (so old outputs are wrong): the `sidi_` name slices were off — names came out as `et_001`, `easurecar_001`, `howcase_mech` (now
`aftermarket_001`, `treasurecar_001`, `showcase_mech`); `carmeet_*` locators in `route40001/40900` were never read; and `race_start` (the activation sphere) is now `race_pin`.

---

## 4. Terrain: elevation and surfaces

The whole island's terrain **height** (render mesh) and a per-triangle **surface id** (collision mesh) are readable from GeoChunk0. Details, layouts,
validation and the open surface-naming question: **[FH6 terrain](fh6-terrain.md)**. Headlines:

- Elevation → 4 m raster; median |Δy| **0.17 m** against the 38 473 road nodes (98 % coverage; p90 10.9 m = bridges/tunnels). Coarse whole-island LOD (26 MB): 1.1 m.
- Surfaces → 34.7 M triangles, **54** global material ids on the terrain. The game's **name table is not readable** (`Physics/surfaceTypes.xml` is encrypted), so every name is **ours**;
  after the in-game survey (plan D7/D9) the [id table](fh6-terrain.md#surface-names-id-table) marks each id **confirmed in-game (18)**, **seen (4)** or **reasoned (32)**, and `fh6surfaces.py` carries the same split in code.
- The user visually verified both renders.

---

## 5. Dead ends (don't retry)

| Source | Result |
|---|---|
| `OpenWorld/Brio/Brio_00.owbs` (magic `FBWO`/`LBWO`) | Road-segment / tile quads; no usable extra data found. |
| `OpenWorld/Brio/Brio_00.oww` (magic `FWWO`) | Coarse cell grid only. |
| `Stripped/gamedbRC.slt` | **Encrypted in the current install** (the SQLite DB we once opened was in the creator's decrypted July-build dump). Even that copy had no coordinates: `NewProfile_*` tables empty, `Tracks` table without positions. Strings are hashed as `_&<u64>`; the readable text is in `Stripped/StringTables/<lang>.zip`. |
| `routedefinitions.zip` `.rtd` | Asset index lists only. |
| `routeassettransforms.txt` | 1.19 M `x,y,z:routeid` prop transforms — cones/barriers along race routes; too noisy to be useful. |
| ~~`Ribbon_00/GameObjs.xml`~~ | **Not a dead end** — it was wrongly recorded as "all 0,0,0". Exact positions for 793 objects, see [GameObjs.xml](#gameobjsxml--exact-gameplay-props). |
| Race-trigger spheres as race starts | Wrong — they are map pins; use the `RVAN` block. |
| `.owt` node 0 as race start | Wrong — lead-in node, 0–1200 m off; the start node is `header[11] & 0xffff`. |
| `.owt` node count at `0x24` with nodes at `0x60` | Wrong for the 6 multi-section files (132, 281, 351, 1181, 1281, 8008) — node offset depends on the section count, see [Race lines](#race-lines-the-owt-racing-line-files). |
| `PointsOfInterest.str` | **Stale FH5 text** — not FH6 names. |
| Speed-limit number atlas | The texture with the printed km/h numbers was not found in any readable file (sign swatches hold the blank face) — values need an in-game check. |
| A `media.zip` sent by the Horizon Nav creator | July-build dump, **decrypted**: it has the SQLite gamedb, physics, audio **and `Stripped/EntityModel.zip`** (race types, photo spots, time attacks…). Useful for research only — the same files are encrypted in a user install. |
| Breaking the encryption | Not attempted (key presumably in the exe); not something an app can ship. |
| Reading the running game's memory | Deliberately not done — anti-cheat risk, see the licensing/safety section. |
| Nav road class 4/5/6/8 as a surface type | Does not correlate with the terrain material ids. |

Seen but **not examined**: `Tracks/Brio/wdepth/wdepth_*_season_N.dds` (water depth?), `acoustics/*.ace`, `procphys/*.procphys`, `Physics/GroundCoverSurfaceMap.xml`,
the 56 864 plain `autoterrain_*_cluster*.i.modelbin`, the `mainmap/submap/autoglossf*.pb` terrain textures, `Audio/*` (surface/biome audio tables).

---

## 6. Seasonal / weekly Festival Playlist verdict

**Not obtainable from the files or the telemetry packet.** What is and isn't knowable:

- The game's map uses a **runtime `season_state` flag set from server data** (`UI.zip` filter `filter_pr_stunt_seasonal` = `season_state=active…`).
  There is **no per-object season field** in `GameObjs.xml` or the pgeo files, so "this week's speed traps / drift zones / trailblazers" cannot be derived from positions.
- The server-delivered content cache `…/AppData/Local/ForzaHorizon6/CmsCache/*` is **encrypted** and did not change while a live session ran.
- `EN.zip` `FestivalPassSeriesData.str` has series launch dates, but only as marketing text (Thursdays, 28-day cadence) — not machine data.
- The telemetry packet has no season / event field.
- *Consequence:* all positions are known; which are active is not. A feature can show **all** speed traps / zones / drift zones and must not claim "active this week".

**Map season (the `Spring → Summer → Autumn → Winter` imagery) is different and already solved by wall-clock:** `src/minimap.rs::current_season()` rotates
weekly from the epoch `1_749_738_600` = **2025-06-12 14:30 UTC**; the user confirms it has always matched the in-game season. Only the *comment* above it
(`// Spring started 2026-06-12 …`) has the wrong year (the value is right) — left untouched because this task did not touch `src/`.

---

## 7. Live-game probe

A passive probe while the user was in the game (open file handles, logs, sockets — **no process memory**):

- **No seasonal / route files are held open** — the game streams them on demand.
- The only plaintext written: `PreCrashReport.xml` (build **440853**, session id, startup snapshot), `UserConfigSelections` (graphics settings), the PlayFab
  `cloudsync` `localstate.json` (livery list). Logs are startup snapshots only.
- `CmsCache` got no new content; the network is TLS only.
- **Memory reading was deliberately not done.** *Why:* the game is online with anti-cheat; reading process memory risks a ban, and everything the app needs is either
  on disk, derivable, or genuinely server-side (and a memory read would not make server-side state legitimate to ship anyway).

---

## 8. Using the scripts

Details and flags: [`tools/fh6-extract/README.md`](../../tools/fh6-extract/README.md). Python 3 + numpy + Pillow (scipy only for the name heuristic);
all read-only on the install; every script takes `--media` (default: auto-detect Steam) and `--out`.

```
extract_map.py --out DIR [--level 3] [--seasons Summer,...]   # map_<season>_L<level>.png (+ _preview)
decode_nav.py  --out DIR [--map map_summer_L3.png]            # roads.json, roads.png, roads_on_map.png
extract_poi.py --out DIR [--entity-model OLD_EntityModel.zip] # pois.json (5578 records; 5632 with the creator dump)
extract_geochunk.py --out DIR [--skip-pgeo]                   # geochunk_pois.json (1649 exact prop records)
extract_races.py --out DIR [--entity-model ...]               # races.json (169 race starts + grids + finishes, names)
extract_racelines.py --out DIR [--step 5]                     # racelines.json (170 trimmed racing lines + track edges)
extract_predictions.py --out DIR [--refit]                    # predictions.json (needs races/pois/racelines.json in DIR): predicted race type + map pin per route, self-check vs the marks
extract_speedsigns.py --out DIR [--roads roads.json] [--variant-map "0=50,1=60,..."]   # speedsigns.json (2141 signs)
extract_terrain.py --out DIR [--region X0,Z0,X1,Z1] [--coarse]# elevation.npy/.png, surfaces.npy/.png (+ names in surfaces.json)
extract_cars.py --out DIR [--lang EN,DE|all] [--ordinal N ..] # cars.json (671 ordinals -> media name + full/model name + make + display)
extract_names.py --out DIR                                    # names.json (75 landmarks, 24 languages, stunt name tables), regions.json (10 outlines)
extract_icons.py --out DIR; build_icon_mapping.py DIR         # png/ (1074 icons), icons.json, xml_symbols.json; mapping.json (POI category -> icon)
roaddist.py / plot_pois.py                                    # validation helpers
build_viewer.py --out DIR_OUTSIDE_REPO                         # local Leaflet map viewer of all of the above (runs the extractors itself; opens from file://)
```

Library modules: `fh6common.py` (install detection, case-insensitive paths, `.nt`/`.tz` readers), `pgzp.py` + `lz4b.py` (PGZP reader, u32 **and** u64 table),
`fh6str.py` (string tables), `fh6owt.py` (`.owt` racing line + `RVAN` block), `fh6surfaces.py` (terrain surface-id names with their confirmed/reasoned status), `fh6bxml.py` (BXML; also decodes the plaintext `ObjectModelGame.zip`), `fh6careers.py` (exact race names + types from it). `racetype_method.py` / `pin_method.py` (predicted race type / map pin, see "Predicting race type and map-pin position").

Verified against the install (Sept 2026 build): `extract_geochunk.py` → 793 GameObjs objects, 1649 records, pgeo error 0.000 m, identical to the original research output;
`extract_races.py` → 169 race starts, start line within 0.1 m of its own racing line for 165 (was 164 before the `.owt` fix) (names: 22 exact incl. IE/chase, 3 locator, 44 landmark ≤ 400 m, 10 weaker with the creator dump);
`extract_terrain.py` on a 1.5 km region → median |Δy| 0.17 m, full-island surfaces → 34 716 033 triangles; `extract_poi.py` → 5578 records;
`extract_racelines.py` → 170 routes (127 p2p + 43 circuits, all closed/ending within 3 m); `extract_speedsigns.py` → 1659 / 217 / 265; `extract_cars.py` → 671 ordinals (4144 → `Mazda RX-7 '92`);
`extract_names.py` → 75 / 75 landmarks, 10 regions, 24 languages; `extract_icons.py` → 1074 icons (pixel-identical with the `texture2ddecoder` decode); `pgzp.py` opens GeoChunk0–3.

## 9. What a runtime implementation needs

1. Locate `media/` (Steam vdf → folder-picker fallback; remember the choice in config).
2. Map: open `Map_Brio_<Season>.zip`, decode the needed pyramid level's BC1 tiles into a texture
   (prefer L2 for RAM); calibration is the existing one.
3. Roads: parse `Brio_00.nav` per [Roads](#1-roads--openworldbriofreeroambrio_00nav) (it is a flat
   binary — only needs positions, road table and list A).
4. POIs: parse the small XML/text files (regex is enough, see `extract_poi.py`) **and `Ribbon_00/GameObjs.xml`** (speed traps, zones, drift zones, XP boards… — no binary needed).
5. Race starts/grids: the `RVAN` block of `AITracks/Route<N>.nav` (flat binary; no PGZP needed). Exact names (and types) for 111 routes from the plaintext `ObjectModelGame.zip` ([how](#exact-race-names-and-types-objectmodelgamezip)). Racing line / track edges: `Route<N>.owt` ([layout](#race-lines-the-owt-racing-line-files); flat binary, mind the section count).
6. Anything from GeoChunk (danger signs, drift posts, speed-limit signs, terrain) additionally needs the PGZP reader (u32/u64 tables, LZ4 block, deflate). Do it lazily and cache a *derived, compact* grid in the app data dir.
7. Do it lazily on a background thread and cache nothing derived in the repo.
8. Names: car names (`Cars/*.zip` central directories + `Data_Car.str`), landmark / region names (`Landmarks.str`, `MapRegion.str`, any of 24 languages) and the game's map icons all come from plaintext
   install files — see [cars, names, regions and icons](fh6-cars-names-icons.md). Pick the language from the app's own i18n setting (EN/DE) with an English fallback.
9. Never rely on the *creator-dump only* categories, and never present inferred names (terrain surface names that are not `confirmed`, arena names, race-name guesses) as the game's own.

## 10. Open items

Things the research could not settle — each needs a human, a live game, or a decision:

| Item | State | How to close it |
|---|---|---|
| ~~Packet `CarOrdinal` == `carclips_<ordinal>` id~~ | **closed — verified live** (4277 → `HON_21_CivicWTA_92`) | — |
| Car **make** | heuristic only (`extract_cars.py` / `src/gamedata/cars.rs`; known misses `AC_*`, `PG_*`); ModelShort does not always contain it (4277, 2574) | the exact `MakeID` is only in the encrypted gamedb — not an option (D11) |
| Speed-limit **variant → km/h** | **unknown** (number atlas not in any readable file); guessed order 5/3 < 0 < 1 < 2 < 4 | read the number on one sign per variant in-game ([coordinates](#speed-limit-signs)), then feed `--variant-map` |
| Speed zone / trailblazer / drift zone **gate `_1` vs `_2`** = start vs end | **not verified** (both gates are emitted) | drive one zone and see which gate starts the timer |
| `.owt` undecoded node fields (+36 i16 pair, +44 u16[4] tag, +52 flag) | **undecoded** — guesses: curvature-like value, section/surface id | correlate with the race's surface type / corners if a feature needs them |
| Terrain surface names | 18 confirmed, 4 seen, **32 reasoned** — see the [id table](fh6-terrain.md#surface-names-id-table); 280 / 242 may be dirt track rather than verge | more in-game spot checks, or the encrypted `surfaceTypes.xml` (not attempted) |
| BC6H swatchbins (`0x07`) | probable format, not decoded | only if HDR images are ever needed |
| `Brio_00.nav` per-road values (one-way, tunnel, road type), list B | undecoded | see [Roads → Unknowns](#unknowns) |
| Which stunts are this week's Festival Playlist | **unobtainable** | [verdict](#6-seasonal--weekly-festival-playlist-verdict) |
