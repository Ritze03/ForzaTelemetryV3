# Forza Horizon 6 game files — reverse-engineering notes

What we know about reading **map imagery, roads, points of interest (POIs), race starts, terrain
elevation and surfaces** straight from the user's own FH6 install, so a feature like "load map /
roads / POIs / race lines from the game" can be built without redoing the research. Everything
here was derived from the PC (Steam) build and verified with the scripts in
[`tools/fh6-extract/`](../../tools/fh6-extract/README.md) — **the scripts are the ground truth**; if
this doc and a script disagree, trust the script and fix the doc. Terrain has its own page:
[FH6 terrain](fh6-terrain.md).

Status: research only. Nothing in `src/` reads game files yet. Written against the Steam install
at game build 440853 (from `PreCrashReport.xml`), Sept 2026; a game update can change file contents
and which files are encrypted — re-run the scripts and compare counts before trusting a number here.

## Why this exists, and the licensing decision

The map, road graph and POI positions are **Playground Games' IP**. Decision: the app must
**read them from the user's own install at runtime** (auto-detect Steam through
`libraryfolders.vdf`, folder-picker fallback) instead of bundling extracted data.

- *Why:* we cannot redistribute the data; reading the user's copy sidesteps that entirely.
- *Bonus:* once the map comes from the install, the ~110 MB of bundled `assets/maps/*.jpg` can be
  dropped (they are re-encodes of the same imagery — see [Map imagery](#2-map-imagery)).
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
| Terrain surface ids | GeoChunk0 `tbheightfield\*_square*.phys` | 34.7 M tris, 54–56 ids | ids exact; class names **inferred (ours)** | **no** (table encrypted) | **yes** — [terrain](fh6-terrain.md) |
| Race start line + 12-slot grid + heading + finish | `OpenWorld/Brio/AITracks/Route<N>.nav` (`RVAN` block) | 169 routes (+ route 99 test) | exact (on racing line < 0.1 m) | partial, see next rows | **yes** |
| Race racing line | `AITracks/Route<N>.owt` | 170 | exact | n/a | **yes** |
| Race map pin (activation sphere) | `Tracks/Brio/triggerzones/tz_race_activations/race_triggers.tz` | 36 | exact pin, **not** the start (median 185 m off) | slug `rt<N>` only | **yes** |
| Race display names | `Stripped/StringTables/EN.zip` `CareerRaceCollection.str` | 110 unique | text exact | yes, but **route ↔ name link is not in the files** | strings **yes**; link: 15 routes (3 rush via locators, 5 IE + 7 chase by id convention) without the dump, +10 exact +44 heuristic +10 guess with it |
| Race types (sprint, circuit, scramble, …) | `Entities/Brio/campaign_slots.xml` | 88 routes | exact | yes | **creator dump only** |
| Landmarks | `Tracks/Brio/triggerzones/tz_world_constraints/landmark_triggers.tz` | 75 | exact (sphere centre) | slug (`shibuya_crossing`); English name for 54/75 via `Landmarks.IDS_Area_Discovered_<slug>`, all via `Entities/…/landmark_areas.xml` | slugs **yes**; full names partly (dump for the rest) |
| Named locators (houses, fast travel, festival, barn finds + hint areas, car/drag meets, touge, showcases, treasure cars, aftermarket spots/boards, Horizon Jobs/Stories, rush, invitational/legend, upsell) | `Tracks/Brio/trackroutes/route0.nt` (+ `route40001/40900/4004x/4005x.nt`) | ~370 + extras | exact | internal slugs | **yes** |
| Pinatas / eliminator spawns / parking areas | `trackroutes/{pinata_locators,eliminator_locators,parkingareas}.nt` | 1536 / 373 / 2664 | exact | no | **yes** |
| Horizon Story/Job activation zones, creature zones, map regions | `triggerzones/tz_bucket_challenges`, `tz_creatures`, `trackroutes/map_region_*.nt` | 11+6 / 47 / 10 | exact (regions: centroid) | slugs | **yes** |
| XP boards (A/B/C = 100/75/25), speed traps, speed zones, trailblazers, drift zones, mascots, estate entrances, treasure-chest boards | `Tracks/Brio/Ribbon_00/GameObjs.xml` | 200 / 30 / 30 (×2 gates) / 12 (×2) / 20 (×2) / 200 / 37 / 3 | **exact** + orientation | ids (`SPEEDCAMERA_07_LEFT`) | **yes** (plain XML!) |
| Danger signs, drift-zone marker posts, drift-circuit props | GeoChunk0 `.pgeo` | 15 / 1027 / 13 | exact (16.16 fixed) | ids | **yes** (needs PGZP) |
| Speed-limit signs, rush ramps (as instances) | GeoChunk0 `.pgeo` | ~1659 / ~1700 | exact | — | yes, **not extracted** by our scripts |
| Playground arenas (3), Hide & Seek arenas (6), flag-rush flags (6), rural train line (505 pts) | `trackroutes/route30xx/8100-8105.nt`, `Stripped/gs/brio/gameobjs.xml`, `OpenWorld/Brio/Freeroam/Ambient_RuralTrain_RuralLine.owcp` | 3 / 6 / 6 / 1 | exact (arenas: outline centroid) | arena names partly inferred | **yes** |
| Photo spots | `Entities/Brio/entities_photo_challenge_landmarks.xml` | 37 | exact | slugs | **creator dump only** (not in any pgeo) |
| Time-attack boards (4), Horizon Chase starts (7), backstage passes, labyrinth entrance, IE activator, community gift shop, Legend Island gates (2) | `Entities/Brio/*.xml`, `Templates/*.xml` | — | exact | partly | **creator dump only** |
| Map element / filter types (the game's own ~230) | `UI.zip` → `MapProfiles/MapIncludes/*.xml` | ~230 | n/a | yes (plaintext XML) | **yes** |
| String tables (all UI text) | `Stripped/StringTables/<LANG>.zip` | 291 tables (EN) | n/a | yes | **yes** |
| Which speed traps / drift zones are *this week's* Festival Playlist | server data | — | — | — | **no, unobtainable** — [verdict](#6-seasonal--weekly-festival-playlist-verdict) |
| Car database, rules, tunables, physics surface names | `gamedbRC.slt`, `Rules.zip`, `GameTunableSettings.zip`, `Physics/surfaceTypes.xml` | — | — | — | **no, encrypted** |

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
| `Stripped/StringTables/<LANG>.zip` (`*.str`) | plaintext | all UI/event/race text |
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

Caveats: **node 0 is not the race start** (0–1200 m off — it is just lead-in); some routes have a NaN
node 0; circuits have first node == last; files whose u32 @0x20 is 4 carry 112 extra header bytes and 2 extra
nodes (`extract_races.py:owt_line`). The .owt gives the route's driven racing line (the script records
`route_node0` + end + circuit flag). Start positions: the `RVAN` block below, **not** `race_triggers.tz`.


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
| `GeoChunk1`, `GeoChunk2` | only `.pb` entries (not pursued; row order may be permuted vs the name list) |
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
| 32 + 4M | u32 N, u64 | N again, then the absolute offset of the first segment |
| per segment | 512 × `{u32 off, u32 usize, u32 flags}` + u64 | one row per entry, then the absolute offset of the **next** segment |

- Entry data starts at `segment_start + off`. **Compressed size** = (next row's `off`) − `off` inside a segment, or (next segment start) − this start for the last row.
- `flags & 0xff` selects the codec: **`0x1f` = raw LZ4 block** (no frame header; decoded size = `usize`), **`0x08` = raw deflate** (zlib `wbits = −15`), **`0x00` = stored** (terrain `.phys` entries often are). The upper 24 bits are unknown (look like a running counter / hint).
- *Why a hand decoder:* no Python/Rust zip crate reads this; the LZ4 block format is ~25 lines (`lz4b.py`), deflate is `flate2`.

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

Also present as instances but **not extracted**: 1659 speed-limit signs and ~1700 rush ramps. Photo spots are not in any pgeo.

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

**Race names** are the *second half* of `CareerRaceCollection.str` (entries ≥ 154 of 323 — the lower entries are descriptions; the upper block mixes ~110 race names with playlist, Playground-arena and drag-strip names).
**There is no direct route-id → name link in the readable files** (the `CareerRace` key scheme is uncracked), so `extract_races.py` assigns names by increasing
confidence (recorded in `extra.name_confidence` / `name_evidence`; only `exact`, `locator` and `landmark<=400m` are written to `race_name`,
the rest to `race_name_guess`):

| Method | Routes | Needs |
|---|---|---|
| IE routes by id convention (`3333–3336` = Drive Section 1–4, `3337` = City Tour); Horizon Chase `30100 + n − 1` | 5 + 7 | nothing |
| Horizon Rush: nearest `sidi_rush_{docks,ski,spaceport}` locator in `route0.nt` | 3 | install |
| Finales via `campaign_slots.xml` `ContextId` (`The<Name>RaceEventActivation`): Titan (route 1023), Goliath (5555), Gauntlet (2052), Colossus (132) | 4 | **creator dump** |
| Exit locator in `post_race_locators.xml` (`race_<collectionId>` → `<name>_custom_exit_locator`) | 6 | **creator dump** |
| Heuristic: per event family, assign that family's names to its routes by **Hungarian matching** on the distance landmark keyword → racing line (keyword table `K` in the script; 5/5 manual checks matched) | 44 (≤ 400 m) + 9 (≤ 1000 m) + 1 (elimination) | **creator dump** (for the family) + scipy |

So **without the creator dump only 15 of 169 routes get a name**; the app can still show circuit / point-to-point (finish ≠ start), length and
`Route <N>`. Race *types* for 88 routes (`brio_{circuit,sprint,scramble,trail,street,touge,finale,horizon_rush}_<N>` in `campaign_slots.xml`, with
`CareerRaceCollectionId` 80–87 = cross-country circuit, 88–97 = cross-country sprint) are also creator-dump only.

Traps: the landmark strings for `seaside_circuit` / `seaside_offroad_circuit` look **swapped** (race evidence: Hokubu Circuit is near (2830, 2700), Sekibe
near (2500, −5000)); `Landmarks.IDS_Area_Discovered_<slug>` resolves for 54 of 75 landmark slugs, the rest need `landmark_areas.xml`.

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
- Surfaces → 34.7 M triangles, 54–56 global material ids on the terrain. The **names are not in any readable file** (`Physics/surfaceTypes.xml` is encrypted), so any
  class name (asphalt, water, sand, snow, forest, grass…) is **ours**. Status: *surface names — in progress (plan D7)*: file decode attempt failed, in-game survey with the user is the next step.
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
| `.owt` node 0 as race start | Wrong — lead-in node, 0–1200 m off. |
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
extract_terrain.py --out DIR [--region X0,Z0,X1,Z1] [--coarse]# elevation.npy/.png, surfaces.npy/.png
roaddist.py / plot_pois.py                                    # validation helpers
```

Library modules: `fh6common.py` (install detection, case-insensitive paths, `.nt`/`.tz` readers), `pgzp.py` + `lz4b.py` (PGZP reader),
`fh6str.py` (string tables), `fh6bxml.py` (BXML, creator dump only).

Verified against the install (Sept 2026 build): `extract_geochunk.py` → 793 GameObjs objects, 1649 records, pgeo error 0.000 m, identical to the original research output;
`extract_races.py` → 169 race starts, identical to the original research output (names: 22 exact incl. IE/chase, 3 locator, 44 landmark ≤ 400 m, 10 weaker);
`extract_terrain.py` on a 1.5 km region → median |Δy| 0.17 m, full-island surfaces → 34 716 033 triangles; `extract_poi.py` → 5578 records.

## 9. What a runtime implementation needs

1. Locate `media/` (Steam vdf → folder-picker fallback; remember the choice in config).
2. Map: open `Map_Brio_<Season>.zip`, decode the needed pyramid level's BC1 tiles into a texture
   (prefer L2 for RAM); calibration is the existing one.
3. Roads: parse `Brio_00.nav` per [Roads](#1-roads--openworldbriofreeroambrio_00nav) (it is a flat
   binary — only needs positions, road table and list A).
4. POIs: parse the small XML/text files (regex is enough, see `extract_poi.py`) **and `Ribbon_00/GameObjs.xml`** (speed traps, zones, drift zones, XP boards… — no binary needed).
5. Race starts/grids: the `RVAN` block of `AITracks/Route<N>.nav` (flat binary; no PGZP needed). Names only for ~15 routes without the (encrypted) entity data.
6. Anything from GeoChunk (danger signs, drift posts, terrain) additionally needs the PGZP reader (u32/u64 tables, LZ4 block, deflate). Do it lazily and cache a *derived, compact* grid in the app data dir.
7. Do it lazily on a background thread and cache nothing derived in the repo.
8. Never rely on the *creator-dump only* categories, and never present inferred names (terrain classes, arena names, name guesses) as the game's own.
