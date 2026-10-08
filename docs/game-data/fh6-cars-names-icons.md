# FH6 cars, names, regions and map icons

Part of the [FH6 game-file notes](fh6-game-files.md) (read its *Locating the install* and *Coordinate system* sections first). Everything here
is plaintext in the install and read-only; reference decoders: [`extract_cars.py`](../../tools/fh6-extract/extract_cars.py),
[`extract_names.py`](../../tools/fh6-extract/extract_names.py), [`extract_icons.py`](../../tools/fh6-extract/extract_icons.py) +
[`build_icon_mapping.py`](../../tools/fh6-extract/build_icon_mapping.py) (**the scripts are the ground truth**; if this doc and a script
disagree, trust the script and fix the doc). Written against game build 440853 (Sept 2026). Output is Playground Games data — never commit it,
never ship it (plan decision D11: names must be derived from the user's install at runtime, no lookup table, no key).

## 1. Car names — `CarOrdinal` → make/model

**Yes, from the install alone** — no gamedb (encrypted), no key. Two plaintext hops:

| Hop | Source | Detail |
|---|---|---|
| ordinal → `MediaName` | `media/Cars/<MediaName>.zip` (671 plain-deflate zips, e.g. `MAZ_RX7_92.zip`) | each contains `Scene/animations/Mojo/clip/carclips_<ordinal>.clipd`; **read only the zip central directory** (all 671 in ~0.4 s) |
| ordinal → name | `Stripped/StringTables/<LANG>.zip` → `Data_Car.str` (24 languages, DE works) | key = `strhash("IDS_ModelShort_<ordinal>")` → **full name incl. make** (`Mazda RX-7 '92`); key = `strhash("IDS_DisplayName_<ordinal>")` → model only (`RX-7 Type R`) |

(`strhash` / `.str` layout: [`fh6str.py`](../../tools/fh6-extract/fh6str.py), main doc *Race names*.) Example: ordinal **4144** → `MAZ_RX7_92` →
`Mazda RX-7 '92`.

- *Why the keys look swapped:* the gamedb columns are named `ModelShort` / `DisplayName` but hold the long / short text respectively. Use the content, not the column names.
- **Validation:** 638 / 638 ordinals matched the decrypted July-build gamedb's `Data_Car.Id`↔`MediaName` (creator dump, research only); 33 newer cars (not in that dump) resolve too. All 671 zips resolve to a full name and a model name.
- **Packet `CarOrdinal` == this id: VERIFIED live** — a running game reported 4277, which resolves to `HON_21_CivicWTA_92` ("#21 Civic WTAC"), the car being driven.
- **ModelShort does NOT always contain the make** (4277 → `#21 Civic WTAC`; 2574 → `M12S Warthog CST`; also short forms like `Lambo Countach`, `VW Golf R '10`, `AM DB5`). The year appears in only ~238 / 638 names, and the `MediaName` year suffix is unreliable (558 / 638), so do not parse it.
- **Make = heuristic, there is no make id in readable files** (`MakeID` is only in the encrypted gamedb). Make names are plaintext in `List_CarMake.str` (key `IDS_DisplayName_<MakeID>`, 92 makes; the table also holds placeholder strings like "Acura profile goes here...", so ids are recovered by hashing `IDS_DisplayName_0..1999`). Steps: **(a)** a make name that is a whole-word prefix (case-insensitive, after an optional `#nn ` tag, longest wins) of ModelShort or DisplayName; **(b)** else the majority make of the MediaName's prefix up to `_` (`HON` → Honda) over the (a) cars; **(c)** else the make that contains the prefix letters as a subsequence (first letters equal; most letters on word starts, then fewest skipped letters; a tie → none). On the Sept-2026 install: (a) 413, (b) 189, (c) 67, none 2 (`NULL CAR`, `M12S Warthog CST`) of 671. Earlier research measured ~591 / 638 exact (~97 % excluding policy cases such as Mercedes-Benz vs -AMG and Formula Drift) against the gamedb; that gamedb is no longer available, so the current accuracy is *not* re-measured. Known wrong guesses from step (c): `AC_*` → Acura (really Alumicraft), `PG_*` (Playground traffic) → Pagani. *Why a heuristic:* the licensing rule (D11) forbids a shipped table and the key, and the readable files carry no make id.
- **Display string** ("Make + model", `compose_display`): no make → the game's text alone (ModelShort, else DisplayName); text already starts with the make → as is (`Mazda RX-7 '92`); `#nn ` tag → `#nn Make <rest>` with the rest taken from **DisplayName** when it carries the same tag (it has the team/livery part: ModelShort `#21 Civic WTAC` + DisplayName `#21 Hardrace/JDMYard Civic WTAC` → `#21 Honda Hardrace/JDMYard Civic WTAC`, the in-game name); otherwise `Make ModelShort`. Known cosmetic flaw: abbreviations are not expanded (`Lamborghini Lambo Countach`, `Volkswagen VW Corrado`) — the in-game short text is kept verbatim.
- **Implementations** (must stay identical; compared on all 671 cars, 0 differences): Python [`extract_cars.py`](../../tools/fh6-extract/extract_cars.py) (`derive_makes`, `compose_display`) and Rust `src/gamedata/` (`cars.rs`: `CarDb::load(lang)` / `lookup(ordinal)`, JSON cache `app_data_dir()/car_names_<LANG>.json` keyed by media path + fingerprint of the car-zip count/sizes/newest mtime + string-zip size/mtime; `install.rs`: install detection (below); `process.rs` / `winsys.rs`: running-game detection; `strtable.rs`: `strhash` / `.str` parser).
- **Locating the install** (`install::find_media`), first hit wins: **(1)** `AppConfig::fh6_install_dir` (Setup → Game Install; game folder or its `media`; per-machine, excluded from profile export) → **(2)** env `FH6_INSTALL_DIR` (dev override) → **(3)** Steam: roots `~/.local/share/Steam`, `~/.steam/steam`, Flatpak `~/.var/app/com.valvesoftware.Steam/.local/share/Steam`, Snap `~/snap/steam/common/.local/share/Steam`, `C:\Program Files (x86)\Steam`, `C:\Program Files\Steam`, and on Windows the registry (`HKCU\Software\Valve\Steam` `SteamPath`, `HKLM\SOFTWARE\WOW6432Node\Valve\Steam` `InstallPath`); every `"path"` in each root's `steamapps/libraryfolders.vdf` is a library; per library the folder is `appmanifest_2483190.acf` `installdir` (fallback `ForzaHorizon6`), matched case-insensitively. A configured folder that is not valid falls through to (2)/(3) in the car DB; Setup shows the status of the typed value itself (`install::check`: found / not readable / media not found).
- **Detect from the running game** (Setup button; `process::detect_running`): a process matches when its `comm` or its argv[0] base name starts with `forzahorizon6` (case-insensitive; Linux `comm` is cut to `forzahorizon6.e`). Linux (Proton): scan `/proc/<pid>`, install folder candidates in order `environ` `STEAM_COMPAT_INSTALL_PATH`, the `cwd` link, the argv[0] folder (`Z:\x` mapped to `/x`; other Wine drives such as `S:\` can't be mapped, which is why cwd/environ come first); the first candidate that holds a `media` folder wins, else the first candidate (so Setup can say what is wrong). Unreadable `/proc` entries are skipped. Windows: ToolHelp32 process snapshot, `QueryFullProcessImageNameW`, exe folder. Microsoft Store / Game Pass installs (protected `XboxGames`) are out of scope: a folder that can't be listed shows *Found but not readable*. *Why:* a one-click setup that works on any machine (custom Steam libraries, Proton prefixes), with a typed path as the fallback; nothing machine-specific is hard-coded and the tests use synthetic fixtures.
- Runtime cost: cold scan ≈ 0.4–1 s (671 central-directory reads + `Data_Car.str` + `List_CarMake.str`; ~1.0 s in a debug build), warm (JSON cache) ≈ 4 ms. Call it from a background thread. If the language zip is missing it falls back to EN.

## 2. Area, landmark and region names (24 languages)

Languages in `Stripped/StringTables/`: BR, CHS, CHT, CZ, DE, DK, EL, EN, ES, FI, FR, GB, HU, IT, JP, KO, MX, NL, NO, PL, PT, RU, SV, TR
(`extract_names.py` writes `names.json`: all 24 per record; `regions.json`: outlines).

### Landmarks — 75 / 75 named

Positions: `Tracks/Brio/triggerzones/tz_world_constraints/landmark_triggers.tz` (75 sphere zones, slug + centre + radius). Names: `Landmarks.str`
key `strhash("IDS_Area_Discovered_<id>")`:

| How the name is found | Count |
|---|---|
| `<id>` = the tz slug (`IDS_Area_Discovered_shibuya_crossing`) | 54 |
| `<id>` differs from the slug — table `ALT_ID` in `extract_names.py` (`cedar_lane` → `cedar_avenue`, `golf_course` → `naruo_golf_course`, `narai_juku_hot_springs` → `hot_springs`, …) | 17 |
| id unknown; the string exists and is matched by text/position: `tokyo_railway_station` (slug == English text), `mountain_circuit` = **Soni Circuit** (tz at (2746, 4938) ≈ Soni race start (2788, 4991)) | 2 |
| sub-zone with no string of its own, labelled with the parent's name and flagged `name_is_parent`: `bandai_azuma_snow_corridor` → Bandai Azuma Skyline, `festival_site_parking_lot` → Horizon Festival Site | 2 |

*Why the ALT ids:* the strings exist, the slug just differs; each alternate id was found by hashing candidate words (map-profile landmark type names)
and **verified by an exact 32-bit hash match**, so they are not guesses.

- **Correction of earlier notes:** coverage is **75 / 75**, not 54 / 75, and the `seaside_circuit` / `seaside_offroad_circuit` strings are **not swapped**:
  `seaside_circuit` (2606, 2805) = Hokubu Circuit (race start 2827, 2696); `seaside_offroad_circuit` (2676, −5095) = Sekibe Scramble (2496, −5065).
- 8 `Landmarks.str` strings belong to no tz slug (`names.json` → `landmarks_unplaced`): *Seaweed Farm*, *Tateyama Kurobe Alpine Route* (landmarks with no trigger position) and 6 UI strings (`VIEW`, `{0}/{1}`, `Area Discovered`, `Beauty Spot Discovered`, …).
- The `PointsOfInterest` string table is **stale FH5 text** — do not use it.

### Map regions — 10 / 10, with outlines

`Tracks/Brio/trackroutes/map_region_<slug>.nt` = `Arena_NNN` locators: the region polygon, **in ascending `NNN` order** (`north_plains` repeats its first point
as the last — drop the duplicate). 0 self-intersections; the ten polygons tile the island (sum 188.1 km²). Telemetry-space x/z.

| slug | Name (EN) | DE | points | km² |
|---|---|---|---|---|
| `canyon` | Shimanoyama | Shimanoyama | 20 | 28.2 |
| `city` | Tokyo City | Tokio | 13 | 12.4 |
| `east_coast` | Ito | Ito | 27 | 30.2 |
| `festival` | Ohtani | Ohtani | 16 | 15.0 |
| `highlands` | Takashiro | Takashiro | 19 | 19.2 |
| `legend_island` | Legend Island | Legend Island | 9 | 7.6 |
| `north_plains` | Hokubu | Hokubu | 27 | 22.5 |
| `snowy_mountains` | Sotoyama | Sotoyama | 13 | 18.3 |
| `south_coast` | Nangan | Nangan | 12 | 17.6 |
| `south_plains` | Minamino | Minamino | 26 | 17.4 |

`MapRegion.str` holds each name twice: a long form (`Ito Region` / `Region Ito` by language; `Tokyo City` and `Legend Island` have no suffix) in the first ~10 entries,
and the short form after that — `regions.json` keeps both (`names`, `names_long_form`).

*Why the slug→name link is inferred, and why it is solid:* no file says which string belongs to which `map_region_*.nt`. Link used: the 200 mascots in
`Ribbon_00/GameObjs.xml` are `MASCOTS_REGION_<n>_*` (n = 1..9); each group lies **100 % inside** exactly one outline, and `ChallengeData` strings name the food
mascot per region ("Smash a Ramen Mascot in the Ohtani Region"): 1 ramen = Ohtani = `festival`, 2 = `city`, 3 omurice = Sotoyama = `snowy_mountains`,
4 curry rice = Shimanoyama = `canyon`, 5 matcha = Takashiro = `highlands`, 6 kakigori = Hokubu = `north_plains`, 7 edamame = Minamino = `south_plains`,
8 = `east_coast` (Ito; Ito Airfield lies in it), 9 tempura = Nangan = `south_coast`. `Legend Island` has no mascots (name is unambiguous). `extract_names.py`
re-runs the membership check every time and prints `consistent` / `MISMATCH`.

### Stunt name tables (names only)

`SpeedZone` 30, `SpeedTrap` 30, `DriftZone` 20, `DangerSign` 20, `Trailblazer` 11, `TimeAttack` 10, `DriftAttack` 2 string tables exist (in `names.json` →
`stunt_name_tables`, all languages) but **nothing links a name to a position** (`GameObjs.xml` ids are `SPEEDCAMERA_07_LEFT`, no name key). Usable as a pool of
names, not as labels on specific objects.

## 3. Map icons — the game's own symbols

Source: `media/UI/Textures/HiRes/Data_Bound/Horizon_Map.zip` — **1014 `.swatchbin`** files (the non-`HiRes` folder holds half-size copies), plus 6 icons the XML
references from `Eliminator.zip`, `Estate.zip`, `Rare_Car_Dealership.zip` (same folder). `extract_icons.py` → 1020 PNGs + **54 atlas crops = 1074 icons**
(`icons.json`), and `xml_symbols.json` = {map-element type tag → the icons the game draws for it}.

| Folder in the zip | Count | What |
|---|---|---|
| `icons/MapIcons/` | 472 | the map symbols (+ `_Gold`, `_Seasonal`, `_Locked`, `_EvolvingWorld` variants) and the atlas sheets |
| `pins/…` (Event_Icons, PRStunts_Icons, TopPictures, Seasonal, Stories, Finale, Showcase, …) | 342 | pause-menu / pin art |
| `filters/default`, `…/deuteranopia`, `…/protanopia`, `…/tritanopia` | 39 + 3 × 40 | map filter icons; **colour-blind variants exist** for filters and the atlas sheets |
| `regions/Mascots`, `TypeIcons`, `TopHeaderImages` | 41 | region/mascot art |

The `.swatchbin` container and its **pixel-format field** are in [Map imagery](fh6-game-files.md#2-map-imagery--uitexturesdata_boundmap_brio_seasonzip).
Pillow decodes BC7 directly (`Image.frombytes('RGBA', (w, h), data, 'bcn', 7)`; verified **pixel-identical to `texture2ddecoder` on all 1074 icons**) — `extract_icons.py` uses it and only falls back
to the optional `texture2ddecoder` package. Sizes need not be multiples of 4 (e.g. 121², 141²): pass the full padded block data.

### Atlas sheets

`ForteMapIconSheet` is **2048×1024** (also `HideSeekMapIconSheet`, `EliminatorMapIconSheet`, `MiniMapVehicleSheet`; each of the first three has `_Deuteranopia/_Protanopia/_Tritanopia` copies).
Slot grid: `UI.zip` → `MapProfiles/MapIncludes/MapIncludeSharedResources.xml` defines `atlas_1x1` = 64 px, `atlas_2x2` = 128 px, `atlas_4x4` = 256 px cells
(`x_slots`, `y_slots`), and each symbol names an `AtlasPos` (`x`, `y` in cell units). 4 referenced cells are blank in `ForteMapIconSheet` (`pos_fasttravelboard_found`, `pos_fasttravelboard_notfound`, `pos_beautyspot_notfound`,
`pos_tanky_collectible` — the symbol is drawn from elsewhere or unused); the 54 non-blank cells the XML references are cropped to `atlas/<Sheet>/<pos key>.png`.

### POI category → icon (`mapping.json`, built by `build_icon_mapping.py`)

53 POI categories of our extract scripts (`type` values of `pois.json` / `geochunk_pois.json` / `races.json`): **45 have an icon** — 38 `derived` (the game's own XML
draws that icon for the map-element type tag whose name matches the category), 7 `guessed`, 8 `none` (geometry-only: map_region, creature_zone, drift_zone_post, …).

- *Guessed* (need a human look): `fast_travel` (XML type points at atlas cells that are blank — the `pins/discount_board_fasttravel` art is the pause-menu version), `treasure_chest` + `treasure_chest_board` (Treasure Hunt chest by name only), `hide_seek_arena`, `special_event` (catch-all), `ie_route`, `drag_meet_finish`.
- **Race pins** are not one icon: the map picks by event class via `ResourceFilter` (not a `type` tag) → **10 surface/circuit variants** (`asphalt|crosscountry|mixedsurface` × `p2p|circuit`, `dragracing`, `streetracing`, `touge`, `midnight_battle`), plus `<class>_finish` badges.
- **Landmark icons are name badges with the English name baked into the pixels** — fine for English, wrong for German; draw our own text from [`names.json`](#2-area-landmark-and-region-names-24-languages) and use the badge only as a marker if at all.
- Variants per category (`seasonal`, `live` = Forzathon, `evolving_world`, `gold`, `collected`, mascots per region 1..9, …) are listed under `variants` in `mapping.json`.
- Licensing: the icons are Playground Games' art — extract at runtime from the install, never bundle.

### Rust icon reader (`src/gamedata/icons.rs`, `src/gamedata/bc7.rs`, I28b)

Consumed by the map renderer since I29b: the `map-layers` loader thread reads it once (`maprender::data::GameData::load`) and each egui context (Dashboard, HUD overlay) uploads the atlas itself (`maprender::icontex`); race pins use the route's class from the user's hand marks, mascots their region. Egui-free, std + the `zip` crate already in use, **no new crate, nothing bundled** (the art is Playground Games'; it is
read from the user's install at runtime).

```rust
PoiIcons::load(media: &Path) -> Result<PoiIcons, String>      // = load_sized(media, 64)
pub struct PoiIcons {
    pub size: u32,                          // atlas cell edge, 64
    pub atlas_rgba: Vec<u8>, pub atlas_w: u32, pub atlas_h: u32,   // RGBA8 straight alpha; 512 x 384 today
    pub uv: HashMap<PoiKind, [f32; 4]>,     // [u0, v0, u1, v1] of the whole (square) cell
    pub race: HashMap<RaceClass, [f32; 4]>, // 10 race-pin variants (a RacePin is drawn by its route's class)
    pub mascot: HashMap<u32, [f32; 4]>,     // region 1..=9 (`Poi::n` of a Mascot)
    pub skipped: Vec<String>,               // icons that could not be read (renderer falls back to shapes)
}
```

- **What it does:** reads `Horizon_Map.zip`, decodes the 43 distinct sources of `ICON_TABLE` (27 swatchbin files + 16 cells cut out of the 2048×1024 `ForteMapIconSheet`; the sheet
  is BC7 and only the needed block-aligned cells are decoded), scales each to fit a 64 px cell **keeping its aspect ratio** (area-average box filter in premultiplied alpha,
  1 px transparent margin against bilinear bleed, centred in the cell — so every UV rect is square and the quad is drawn centred on the POI) and packs them 8 per row.
  Rows with the same source share a cell. A missing / undecodable icon goes to `skipped`; `Err` only if the zip or nothing decodes.
- **Cost:** ~22 ms in a release build on 6 cores (~105 ms on one thread; the decode + scale step runs on up to 8 scoped threads), so **no disk cache** — a cache would cost more
  in invalidation than it saves. Still call it off the UI thread.
- **Atlas cells are baked into the table, not parsed from `UI.zip`.** The XML routes each symbol through templates, colour-blind variants and `atlas_NxM` slot grids; a runtime
  parser would be ~150 lines for 16 cells, and a game update that moves a cell only changes the picture. Grids: `atlas_4x4` = 8×4 slots of 256 px, `atlas_2x2` = 16×8 slots of 128 px.
  Re-derive with `extract_icons.py` (`icons.json` lists `cell` + `slots` for every crop). The loader applies the same cell geometry as the script (`width / slots`).
- **Not in the table (no UV → the renderer keeps a shape or draws nothing):** Landmark (name badges with the English name baked in — wrong for German), CreatureZone, Parking,
  FlagRushFlag (geometry only), Pinata (1536 of them), Eliminator (has an icon in `Eliminator.zip`, not read).

PoiKind → icon (`derived` = the game's own `MapProfiles` XML draws this icon for the map-element type tag of that name; `guessed` = chosen by file name, wants a human look):

| `PoiKind` | Icon (inside `Horizon_Map.zip`, no `.swatchbin`) | Basis |
|---|---|---|
| SpeedTrap / DangerSign / DriftZone / SpeedZone | atlas 8×4 cells (0,0) / (1,0) / (2,0) / (3,0) = `pos_prstunt_speedcamera` / `_dangersign` / `_driftzone` / `_speedzone` | derived |
| Trailblazer | atlas 8×4 (0,1) = `pos_prstunt_trailblazer_start` (the finish gate has `_end` at (1,1), unused: which gate is the start is unverified) | derived |
| XpBoard | atlas 16×8 (8,3) = `pos_influenceboard_notfound` (the game calls them influence boards) | derived |
| TreasureChest, TreasureChestBoard | `icons/MapIcons/TreasureHunt/Icon_Treasure_Discovered` (the *Treasure Hunt* chest; the repo's chest objects are linked by name only) | **guessed** |
| BarnFind / BarnFindHint | `icons/MapIcons/BarnFind` / `Barnfind_Radius_Icon` (search-radius badge) | derived |
| TreasureCar | `icons/MapIcons/SeeItDriveIt/TreasureCar` | derived |
| House | `icons/MapIcons/PlayerHouse/Icon_PlayerHouse_Default` | derived |
| Estate, EstateEntrance | `icons/MapIcons/PlayerHouse/Icon_Estate_Default` (the entrance shares it by name) | derived |
| FestivalSite | `icons/MapIcons/FestivalSites/Icon_HorizonFestival_Main` | derived |
| FastTravel | `pins/discount_board_fasttravel` (302×176, pause-menu art: the XML type `travel_board` points at atlas cells that are **blank**) | **guessed — the weakest row** |
| CarMeet | `icons/MapIcons/Icon_Car_Meet` | derived |
| Showcase | `icons/MapIcons/Showcases/Icon_Showcase_Mech` (planes / mech differ per showcase, not told apart) | derived |
| AftermarketSpot, AftermarketBoard | `icons/MapIcons/SeeItDriveIt/AftermarketCar` | derived |
| Upsell | `icons/MapIcons/SeeItDriveIt/PlaylistCar` | derived |
| DragMeet | `icons/MapIcons/HorizonLife/Icon_DragEvent` | derived |
| DragMeetFinish | `icons/MapIcons/CampaignObjective/drag_finish` (the XML has no finish-line symbol of its own) | **guessed** |
| RushEvent | `icons/MapIcons/Rush/Icon_Rush_Docks` (Docks / Ski / Rocket exist; Docks is the XML default) | derived |
| SpecialEvent | `icons/MapIcons/Icon_HallOfFame` (catch-all for invitational / legend events) | **guessed** |
| HorizonStory, StoryActivation | atlas 8×4 (1,2) = `pos_story_background` (the yellow arch; the per-story glyph is drawn on top in the game) | derived |
| HorizonJob, JobActivation | `icons/MapIcons/HorizonStories/Jobs_Background` (blue arch) | derived |
| RacePin | atlas 8×4 (3,3) = asphalt circuit (the XML's fallback for a pin without a class) | derived |
| TougeEvent | atlas 8×4 (4,2) = `pos_raceevent_touge_default` | derived |
| `RaceClass` ×10 | asphalt p2p (2,3), asphalt circuit (3,3), cross-country p2p (0,3) / circuit (1,3), mixed-surface p2p (4,3) / circuit (5,3), drag (6,3), street (7,3), touge (4,2) — all 8×4 — and `icons/MapIcons/Icon_Midnight_Battle` | derived |
| Mascot region 1..9 | `regions/Mascots/` Ramen, Dango, Omurice, Curry, Matcha, Kakigori, Edamame, Onigiri, Tempura | derived |

5 of the 50 table rows are guesses (TreasureChest, TreasureChestBoard, FastTravel, DragMeetFinish, SpecialEvent); the other kinds `mapping.json` guesses (`hide_seek_arena`, `ie_route`)
have no `PoiKind`. The `_Gold` / `_Seasonal` / `_Live` / `_EvolvingWorld` / `_Collected` variants of the families are not used (the files have no per-object state).

**BC7 decoder (`bc7.rs`, ~250 lines, hand-written).** All eight modes: subset count (1-3), partition bits, rotation (modes 4, 5), index selection (mode 4), endpoint precisions,
p-bit styles (unique, shared per subset, none), anchor indices with one bit less, interpolation weights 0/21/43/64 (2-bit), 3-bit and 4-bit tables, p-bit-aware endpoint expansion; a reserved
mode (first byte 0) decodes to transparent black. Texel layout is row-major per block; sizes need not be a multiple of 4 (`ceil(w/4)·ceil(h/4)` blocks, edge texels dropped — real icons are
257², 322×389, 380×340…).
- **Partition tables.** The 64 two-subset and 64 three-subset partitions and the anchor (fix-up) texels are format constants that cannot be derived. They were **read out of a reference
  decoder** (Pillow's, fed probe blocks whose subsets have distinct colours, and all-ones index bits to show which texel has the short index) and the two-subset anchors cross-checked with
  the published table. Why not type them from memory: one wrong entry corrupts only the blocks that use that partition — see the test below, which would catch it.
- **Validation.** (1) Synthetic blocks, one per feature: mode 6 (endpoints + 4-bit weights), mode 3 (2 subsets, partition 0, anchor 15), mode 2 (3 subsets, anchors 0/3/15), mode 5 with
  rotation 3, mode 4 with both index-selection values, p-bit expansion of modes 0/1/7, reserved mode, region and clipped decodes. (2) Real install: every distinct icon source the app
  uses is fingerprinted (FNV-1a over size + RGBA) against **Pillow's decode of the same swatchbin** and must match exactly (`real_install_icons_match_the_pillow_reference`, runs by
  default). (3) `every_icon_matches_the_python_pngs` (ignored; `FH6_ICON_PNG_DIR=<extract_icons.py --out dir> cargo test --release -- --ignored`) compares **all 1014** `Horizon_Map.zip`
  swatchbins with the Python PNGs pixel for pixel: **0 differing channel values** over 6 061 350 BC7 blocks, in which all eight modes occur
  (blocks per mode 0..7: 319 739 / 301 330 / 26 417 / 675 061 / 46 589 / 439 577 / 652 950 / 3 599 687; no reserved block).
- The BC1 map-tile path is untouched (`parse_swatchbin` is still BC1-only; `tiles.rs` has a generic `parse_swatch` + `Swatch::to_rgba` for the icons). `0x0d` RGBA8 is a copy;
  `0x07` (probably BC6H, HDR images) is rejected.
- A game update that redraws an icon fails the fingerprint test on purpose: re-run `extract_icons.py`, look at the new art, regenerate the table (`FH6_PRINT_ICON_HASHES=1 cargo test
  real_install_icons_match -- --nocapture` prints the Rust side's hashes in the table order; the reference values come from the Pillow PNGs). `FH6_ICON_ATLAS_OUT=<png> cargo test
  --release -- --ignored dump_atlas_png` writes the atlas as a contact sheet.
