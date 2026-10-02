# Forza Horizon 6 game files — reverse-engineering notes

What we know about reading **map imagery, roads and points of interest (POIs)** straight from the
user's own FH6 install, so a feature like "load map / roads / POIs from the game" can be built
without redoing the research. Everything here was derived from the PC (Steam) build and verified
with the scripts in [`tools/fh6-extract/`](../../tools/fh6-extract/README.md) — **the scripts are the
ground truth**; if this doc and a script disagree, trust the script and fix the doc.

Status: research only. Nothing in `src/` reads game files yet.

## Why this exists, and the licensing decision

The map, road graph and POI positions are **Playground Games' IP**. Decision: the app must
**read them from the user's own install at runtime** (auto-detect Steam through
`libraryfolders.vdf`, folder-picker fallback) instead of bundling extracted data.

- *Why:* we cannot redistribute the data; reading the user's copy sidesteps that entirely.
- *Bonus:* once the map comes from the install, the ~110 MB of bundled `assets/maps/*.jpg` can be
  dropped (they are re-encodes of the same imagery — see [Map imagery](#2-map-imagery)).
- **Never commit extracted output** (json/png) to the repo. The scripts default to `./fh6-out`;
  point `--out` outside the repo.
- Everything is read-only on the install. Never write into the game directory.

## Locating the install

| Item | Value |
|---|---|
| Steam app id | `2483190` |
| Folder | `steamapps/common/ForzaHorizon6/` ; game data root is its **`media/`** subfolder |
| Library list | `~/.local/share/Steam/steamapps/libraryfolders.vdf` (Linux; also `~/.steam/steam/...`, Flatpak `~/.var/app/com.valvesoftware.Steam/.local/share/Steam/...`; Windows `C:\Program Files (x86)\Steam\steamapps\libraryfolders.vdf`). Each library is a `"path"  "<dir>"` entry; the game may live in any of them (on the dev machine it is in a secondary library, so iterate all `path` entries). |
| Path case | Mixed on disk (`Tracks/Brio/trackroutes`, `UI/Textures/Data_Bound`, `Stripped/gs/brio`). Resolve **case-insensitively** — `fh6common.ci()` does this. |

Reference implementation of detection: `tools/fh6-extract/fh6common.py` (`autodetect_media`).

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

### Road class (`flags & 0xffff`) — partly guessed

Values {4, 5, 6, 8}, counts 191 / 1084 / 255 / 2. Guess: **4 = highway, 5 = normal road,
6 = dirt/track (also the airfield strip), 8 = one oddity.** Not verified against the property
strings. `flags >> 16` unknown.

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
| L3 | 8×8 | **8192²** (the full map; = the bundled jpg size) |

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
| 0x80 | u32 top-mip data size (524288 = 1024²/2) |
| `hdr_size` | pixel data |

Pixel data = `bytes[0x8c : 0x8c + 524288]`, **BC1 / DXT1**, row-major 4×4 blocks (8 B each), no gamma
conversion needed. Which header word holds the DXGI format is **unknown** (candidates 0x48 / 0x58;
observed values there: `0x01000000` and `0x06010001`) — BC1 was determined from the size
(0.5 B/px) and by the decoded result matching the bundled jpgs.

### Validation

Decoded L3 is pixel-identical (JPEG-noise level) to the bundled `assets/maps/*.jpg` — so those
jpgs are re-encodes of this data and `MapCalibration::DEFAULT` applies unchanged.

### Rust implementation notes

- Need a zip reader: the **`zip` crate (deflate)** is not in `Cargo.toml` (`flate2`/`miniz_oxide` are
  only transitive).
- Need a BC1 decoder: ~40 lines by hand, or `texpresso` / `bcdec_rs`. The `image` 0.25 crate
  cannot decode BC1.
- `decode_bc1()` in `extract_map.py` is a compact reference (RGB565 endpoints, 4-colour mode when
  `c0 > c1`, else 3-colour; 2-bit indices LSB first).

### Not the map

`HiRes/Data_Bound` has no map variant. `Horizon_Map.zip` holds ~1000 small icon/filter
swatchbins — not imagery, possibly the map icons (**unchecked**).

---

## 3. POIs — plain-text files under `Tracks/Brio/`

All positions are in telemetry space (see above). `extract_poi.py` merges everything into one
`pois.json` (`{type, name, x, z, y?, source, precision: "exact"|"cell", extra?}`; 5557 records).
`roaddist.py` checks each type's distance to the decoded roads — exact types sit within a few
metres, cell-based ones do not.

### Exact sources

| File (under `media/`) | Format | Gives |
|---|---|---|
| `Tracks/Brio/triggerzones/tz_race_activations/race_triggers.tz` | XML `<triggerzone type name><position x y z/><size…>` | `race_trigger_zone_rt<N>` spheres, radius 100, N = route id → **36 exact race activations** (median 2.8 m from a road) |
| `…/tz_world_constraints/landmark_triggers.tz` | same | 75 landmarks with slugs (`shibuya_crossing`, `tokyo_tower`…) |
| `…/tz_bucket_challenges/tz_horizonstories.tz` | same | 11 Horizon Story + 6 Horizon Job activation zones (`*activation_zone`) |
| `…/tz_creatures/creatures_all.tz` | same | 47 creature zones |
| `Tracks/Brio/trackroutes/route0.nt` | XML `<Locator><Name value=…/><SceneTransform value._41 value._42 value._43/>` (`_41`=x, `_42`=height, `_43`=z) | 371 named locators: houses (`player_house_*_root_locator`), fast travel, festival sites, barn finds (`barn_finds_cinematic_<CAR>`) + hint areas (`barn_finds_anna_hint_*`), car/drag meets, touge, showcases, rush, aftermarket spots/boards, treasure cars, `sidi_hj_*` / `sidi_hs_*`, invitational/legend, upsell |
| `trackroutes/pinata_locators.nt` | same | 1536 pinatas |
| `trackroutes/eliminator_locators.nt` | same | 373 eliminator spawns |
| `trackroutes/parkingareas.nt` | same | 2664 parking areas |
| `trackroutes/map_region_<region>.nt` | same | `Arena_NNN` locators = outline points of a map region; script reports the **centroid** (10 regions) |
| `trackroutes/{job,bucket}_challenges_startend_locations.nt` | same | `VOL_HJ_*` / `VOL_HS_*` volumes → mean of their `_start<N>` locators |
| `Stripped/gs/brio/gameobjs.xml` | XML `<Obj GameplayID><Pos value="x,y,z"/>` | 12 treasure-chest discount boards, 14 `BARN_FIND_INTERIOR_*` (interior coords) |
| `OpenWorld/Brio/AITracks/Route<N>.owt` | binary, magic `FTWO` | route node paths, below |

`.nt`/`.tz`/`gameobjs.xml` may start with a UTF-8 BOM (read as `utf-8-sig`).

### `AITracks/Route<N>.owt` (magic `FTWO`)

| Offset | Meaning |
|---|---|
| 0x24 | u32 node count |
| 0x60 | nodes, stride **56 B**, first 12 B = f32 `x, y, z` |

Caveats: **node 0 is not the race start** (0–1200 m off); some routes have a NaN node 0;
circuits have first node == last. Use `race_triggers.tz` for start positions; the .owt gives the
route's driven path (the script records `route_node0` + end + circuit flag).

### Approximate only (cell centre, ±100 m or worse)

Drift zones, danger signs, XP boards, drift-circuit props and barn buildings are only found as
**file names** in `Tracks/Brio/ChunkContentsMiniZip0-3.txt` (lists of geometry streams). Proc-cell
`.pgeo` paths look like `scene\proc\cellsize\<S>\<i>_<j>\….pgeo`; the script takes the cell centre
`((i+.5)·S, (j+.5)·S)` and marks it `precision: "cell"`. Matched name patterns:
`driftzonemarker_<NN>_(left|right)_<n>` (only `left` kept — two gate markers per zone),
`tag_dangersign_bm_<n>`, `discount_board_xp_a_<n>`, `tag_time_attack_drift_circuit_*`,
`barn_find_*`. `roaddist.py` shows these types are far from roads (cell-centre error), as expected.

### GeoChunk / PGZP — exact positions of the above

> **In progress — see board task #53.** Exact positions of drift zones / danger signs / XP boards
> live in `Tracks/Brio/GeoChunk*.minizip` (magic `PGZP`, 3.5–49 GB). Another worker is decoding
> them; this section will be filled in by the lead.

### Race names / more race starts

> **TODO — board task #54.** (Route id → display name; race starts beyond the 36 exact
> activations.) To be filled in by the lead.

---

## 4. Dead ends (don't retry)

| Source | Result |
|---|---|
| `OpenWorld/Brio/Brio_00.owbs` (magic `FBWO`/`LBWO`) | Road-segment / tile quads; no usable extra data found. |
| `OpenWorld/Brio/Brio_00.oww` (magic `FWWO`) | Coarse cell grid only. |
| `Stripped/gamedbRC.slt` (SQLite) | No coordinates. `NewProfile_*` tables empty, `Tracks` table has no coordinates. Strings are hashed as `_&<u64>`; string tables are in `Stripped/StringTables/<lang>.zip` (`.str`) — useful for names (task #54), not for positions. |
| `routedefinitions.zip` `.rtd` | Asset index lists only. |
| `routeassettransforms.txt` | 1.19 M `x,y,z:routeid` prop transforms — cones/barriers along race routes; too noisy to be useful. |
| `Ribbon_00/GameObjs.xml` | All positions 0,0,0. |
| A `media.zip` sent by the Horizon Nav creator | Contained none of this (only gamedb, physics, audio). The data comes from the user's own install. |

---

## 5. Using the scripts

Details and flags: [`tools/fh6-extract/README.md`](../../tools/fh6-extract/README.md). Python 3 + numpy +
Pillow; all read-only on the install.

```
extract_map.py --out DIR [--level 3] [--seasons Summer,...]   # map_<season>_L<level>.png (+ _preview)
decode_nav.py  --out DIR [--map map_summer_L3.png]            # roads.json, roads.png, roads_on_map.png
extract_poi.py --out DIR                                       # pois.json
roaddist.py / plot_pois.py                                     # validation helpers
```

Verified against the install after cleanup (`--level 2 --seasons Summer`): `roads.json` and
`pois.json` are byte-identical to the original research output; roads drawn on the L2 map follow
the terrain.

## 6. What a runtime implementation needs

1. Locate `media/` (Steam vdf → folder-picker fallback; remember the choice in config).
2. Map: open `Map_Brio_<Season>.zip`, decode the needed pyramid level's BC1 tiles into a texture
   (prefer L2 for RAM); calibration is the existing one.
3. Roads: parse `Brio_00.nav` per [Roads](#1-roads--openworldbriofreeroambrio_00nav) (it is a flat
   binary — only needs positions, road table and list A).
4. POIs: parse the small XML/text files (regex is enough, see `extract_poi.py`).
5. Do it lazily on a background thread and cache nothing derived in the repo.
