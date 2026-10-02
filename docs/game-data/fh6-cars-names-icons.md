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
- **Implementations** (must stay identical; compared on all 671 cars, 0 differences): Python [`extract_cars.py`](../../tools/fh6-extract/extract_cars.py) (`derive_makes`, `compose_display`) and Rust `src/gamedata/` (`cars.rs`: `CarDb::load(lang)` / `lookup(ordinal)`, JSON cache `app_data_dir()/car_names_<LANG>.json` keyed by media path + fingerprint of the car-zip count/sizes/newest mtime + string-zip size/mtime; `install.rs`: Steam detection + `FH6_INSTALL_DIR` override; `strtable.rs`: `strhash` / `.str` parser).
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

### Rust notes

- BC7 needs a decoder: the `texture2ddecoder` Rust crate exists (`bcdec_rs` is a smaller alternative); the `image` crate has no BCn. BC1 (map tiles) is ~40 lines by hand.
- Atlas crops are plain integer-cell rectangles; load each sheet once and slice with the `AtlasPos`/slot values from `MapIncludeSharedResources.xml` (parse the XML at runtime — it is plaintext and ships in `UI.zip`).
- `0x0d` = RGBA8 is trivial; `0x07` (probably BC6H, HDR images) is unverified and not needed for map icons.
