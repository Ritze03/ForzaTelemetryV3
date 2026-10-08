//! Points of interest read from the user's FH6 install: map pins, locations, events, collectibles,
//! stunt gates. A port of the *exact* parts of `tools/fh6-extract/extract_poi.py` and
//! `extract_geochunk.py` (see `docs/game-data/fh6-game-files.md`, "Rust readers").
//!
//! Sources (all under `<media>/`, XML, optionally with a UTF-8 BOM):
//! * `Tracks/Brio/trackroutes/*.nt` — `<Locator><Name value=".."/> .. <SceneTransform value._41=x
//!   value._42=height value._43=z>` ([`locators`]);
//! * `Tracks/Brio/triggerzones/*/*.tz` — `<triggerzone type name><position x y z/><size x y z/>`
//!   ([`tzones`]);
//! * `Tracks/Brio/Ribbon_00/GameObjs.xml` and `Stripped/gs/brio/gameobjs.xml` —
//!   `<Obj GameplayID=".."><Pos value="x,y,z"/>..` ([`gameobjs`]).
//!
//! All coordinates are telemetry space already (x, z metres on the ground, y = height).
//!
//! **Exact sources only.** The Python also derives cell-centre guesses (±100 m) from the
//! `ChunkContentsMiniZip*.txt` file lists and PGZP/GeoChunk props (danger signs, drift posts,
//! speed-limit signs); those are not read by [`Pois::load`]: `GameObjs.xml` supersedes the
//! guesses, and the GeoChunk reads need the PGZP index of a 40 GB file. The one GeoChunk-only
//! category the map shows, the 15 danger signs, has its own lazy loader
//! ([`Pois::load_danger_signs`], ~0.13 s warm).
//!
//! **No XML crate.** The files are machine-written with a fixed attribute order, so a forward
//! `str::find` scan is enough (all sources together ≈ 18 ms in release) and adds no dependency.
//! The scan therefore assumes that order; a differently ordered file yields fewer items (never a
//! panic or wrong data), and the real-install test below fails loudly on a count change.

use std::collections::BTreeMap;
use std::path::Path;

use super::install::ci;
use super::pgzp::Pgzp;

/// What a [`Poi`] is. Grouping for the UI is a presentation concern (the map renderer's job).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PoiKind {
    /// Race map pin (`race_trigger_zone_rt<N>` sphere); `n` = route id.
    RacePin,
    /// Touge event (`sidi_touge_event_<N>` locator); `n` = route id.
    TougeEvent,
    Landmark,
    /// Horizon Story activation zone (`HS_…_activation_zone`).
    StoryActivation,
    /// Other (job) activation zone of `tz_horizonstories.tz`.
    JobActivation,
    CreatureZone,
    House,
    FastTravel,
    FestivalSite,
    Estate,
    CarMeet,
    DragMeet,
    DragMeetFinish,
    AftermarketSpot,
    AftermarketBoard,
    TreasureCar,
    Showcase,
    RushEvent,
    SpecialEvent,
    HorizonJob,
    HorizonStory,
    Upsell,
    BarnFind,
    BarnFindHint,
    Pinata,
    Eliminator,
    Parking,
    /// XP board; `n` = its number, the variant letter A/B/C is in the name.
    XpBoard,
    /// Mascot; `n` = region.
    Mascot,
    EstateEntrance,
    TreasureChest,
    /// Speed trap: midpoint of the two camera poles; `n` = trap number.
    SpeedTrap,
    /// One gate (midpoint of its LEFT/RIGHT markers) of a speed zone; `n` = zone number, the
    /// name ends `_gate1` / `_gate2` (which one is the start is unverified).
    SpeedZone,
    /// Like [`PoiKind::SpeedZone`], for trailblazers.
    Trailblazer,
    /// Like [`PoiKind::SpeedZone`], for drift zones.
    DriftZone,
    /// Treasure-chest board of the stripped `gameobjs.xml` (deduplicated by id and rounded x/z).
    TreasureChestBoard,
    FlagRushFlag,
    /// Danger sign (PR stunt): the centre of its construction boards. **Not** part of
    /// [`Pois::load`]: it needs the GeoChunk0 archive, see [`Pois::load_danger_signs`]. `n` = the
    /// sign's number (`tag_dangersign_bm_<n>`).
    DangerSign,
}

/// One point of interest.
#[derive(Clone, Debug)]
pub struct Poi {
    pub kind: PoiKind,
    pub x: f32,
    pub z: f32,
    /// Height.
    pub y: f32,
    /// Slug as in the file (`carmeet_daikoku`, `SPEEDCAMERA_07`, `treasurecar_001`; `sidi_`
    /// prefix and fixed suffixes stripped like the Python does). Kept for later labelling.
    pub name: String,
    /// Route id (`RacePin`, `TougeEvent`), zone / trap / board number (gate kinds, `SpeedTrap`,
    /// `XpBoard`, `EstateEntrance`, `TreasureChest`), region (`Mascot`); 0 otherwise.
    pub n: u32,
    /// For [`PoiKind::SpeedTrap`] and the gate kinds ([`PoiKind::SpeedZone`], [`PoiKind::Trailblazer`],
    /// [`PoiKind::DriftZone`]): x/z of the `LEFT` and `RIGHT` markers whose midpoint is
    /// `(x, z)`, i.e. the line across the road. `None` for every other kind (a trap or gate
    /// whose partner marker is missing is not emitted at all).
    pub gate: Option<[[f32; 2]; 2]>,
}

/// A map region's outline (`map_region_<slug>.nt`, the `Arena_NNN` locators in ascending order).
#[derive(Clone, Debug)]
pub struct Region {
    pub slug: String,
    pub outline: Vec<[f32; 2]>,
}

/// Everything [`Pois::load`] found.
#[derive(Clone, Debug, Default)]
pub struct Pois {
    pub items: Vec<Poi>,
    pub regions: Vec<Region>,
    /// Sources that were missing or unreadable (relative path + reason). They are skipped, so a
    /// game update that moves a file costs one category, not the whole layer.
    pub skipped: Vec<String>,
}

impl Pois {
    /// Read every source under `media` (the install's `media` folder). `Err` only when not a single
    /// item could be read; a missing or odd file skips that source and adds a line to `skipped`.
    pub fn load(media: &Path) -> Result<Pois, String> {
        let tb = ci(media, "Tracks/Brio")
            .filter(|p| p.is_dir())
            .ok_or_else(|| "no Tracks/Brio folder in the install".to_string())?;
        let mut out = Pois::default();
        let items = &mut out.items;

        // --- race map pins (sphere trigger zones)
        for z in read_in(&tb, "triggerzones/tz_race_activations/race_triggers.tz", &mut out.skipped).map(|t| tzones(&t)).unwrap_or_default() {
            if let Some(n) = z.name.rsplit_once("rt").and_then(|(_, d)| all_digits(d)) {
                items.push(Poi { kind: PoiKind::RacePin, x: z.x, z: z.z, y: z.y, name: z.name, n, gate: None });
            }
        }

        // --- route0.nt: the big named-locator list
        for l in read_in(&tb, "trackroutes/route0.nt", &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
            if let Some((kind, name, n)) = classify_route0(&l.name) {
                items.push(Poi { kind, x: l.x, z: l.z, y: l.y, name, n, gate: None });
            }
        }

        // --- the dense one-kind locator files
        for (rel, kind) in [
            ("trackroutes/pinata_locators.nt", PoiKind::Pinata),
            ("trackroutes/eliminator_locators.nt", PoiKind::Eliminator),
            ("trackroutes/parkingareas.nt", PoiKind::Parking),
        ] {
            for l in read_in(&tb, rel, &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
                items.push(Poi { kind, x: l.x, z: l.z, y: l.y, name: l.name, n: 0, gate: None });
            }
        }

        // --- trigger zones
        for (rel, kind) in [
            ("triggerzones/tz_world_constraints/landmark_triggers.tz", PoiKind::Landmark),
            ("triggerzones/tz_creatures/creatures_all.tz", PoiKind::CreatureZone),
        ] {
            for z in read_in(&tb, rel, &mut out.skipped).map(|t| tzones(&t)).unwrap_or_default() {
                items.push(Poi { kind, x: z.x, z: z.z, y: z.y, name: z.name, n: 0, gate: None });
            }
        }
        for z in read_in(&tb, "triggerzones/tz_bucket_challenges/tz_horizonstories.tz", &mut out.skipped).map(|t| tzones(&t)).unwrap_or_default() {
            if let Some(name) = z.name.strip_suffix("_activation_zone") {
                let kind = if z.name.starts_with("HS_") { PoiKind::StoryActivation } else { PoiKind::JobActivation };
                items.push(Poi { kind, x: z.x, z: z.z, y: z.y, name: name.to_string(), n: 0, gate: None });
            }
        }

        // --- car meets / upsell pins hidden in the small route files
        for rel in ["trackroutes/route40001.nt", "trackroutes/route40900.nt"] {
            for l in read_in(&tb, rel, &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
                if let Some((PoiKind::CarMeet, name, n)) = classify_route0(&l.name) {
                    items.push(Poi { kind: PoiKind::CarMeet, x: l.x, z: l.z, y: l.y, name, n, gate: None });
                }
            }
        }
        // The series-4/5 upsell pins share one spot per series: merged by rounded x/z (first name wins).
        let mut ups: Vec<((i32, i32), Poi)> = Vec::new();
        for rel in [
            "trackroutes/route40900.nt",
            "trackroutes/route40041.nt",
            "trackroutes/route40042.nt",
            "trackroutes/route40043.nt",
            "trackroutes/route40044.nt",
            "trackroutes/route40051.nt",
            "trackroutes/route40052.nt",
            "trackroutes/route40053.nt",
            "trackroutes/route40054.nt",
        ] {
            for l in read_in(&tb, rel, &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
                let lower = l.name.to_ascii_lowercase();
                let wanted = lower.strip_prefix("sidi_upsell_").is_some_and(|r| !r.is_empty() && r.bytes().all(is_word)) && !lower.ends_with("_exit");
                let key = (l.x.round() as i32, l.z.round() as i32);
                if wanted && !ups.iter().any(|(k, _)| *k == key) {
                    ups.push((key, Poi { kind: PoiKind::Upsell, x: l.x, z: l.z, y: l.y, name: l.name[5..].to_string(), n: 0, gate: None }));
                }
            }
        }
        items.extend(ups.into_iter().map(|(_, p)| p));

        // --- GameObjs.xml (exact)
        if let Some(t) = read_in(&tb, "Ribbon_00/GameObjs.xml", &mut out.skipped) {
            ribbon_items(&gameobjs(&t), items);
        }
        if let Some(t) = read_in(media, "Stripped/gs/brio/gameobjs.xml", &mut out.skipped) {
            let mut seen = std::collections::HashSet::new();
            for o in gameobjs(&t) {
                let [x, y, z] = o.pos;
                if o.id.starts_with("DISCOUNT_BOARD_TREASURE_CHEST") {
                    if seen.insert((o.id.clone(), x.round() as i32, z.round() as i32)) {
                        items.push(Poi { kind: PoiKind::TreasureChestBoard, x, z, y, name: o.id, n: 0, gate: None });
                    }
                } else if o.id.contains("_FR_FLAG_") {
                    items.push(Poi { kind: PoiKind::FlagRushFlag, x, z, y, name: o.id, n: 0, gate: None });
                }
            }
        }

        // --- map region outlines
        if let Some(dir) = ci(&tb, "trackroutes").filter(|p| p.is_dir()) {
            let mut slugs: Vec<String> = std::fs::read_dir(&dir)
                .map(|rd| rd.flatten().filter_map(|e| region_slug(&e.file_name().to_string_lossy())).collect())
                .unwrap_or_default();
            slugs.sort();
            for slug in slugs {
                let Some(text) = read_in(&dir, &format!("map_region_{slug}.nt"), &mut out.skipped) else { continue };
                if let Some(outline) = region_outline(&text) {
                    out.regions.push(Region { slug, outline });
                }
            }
        }

        if out.items.is_empty() && out.regions.is_empty() {
            return Err(format!("no point of interest could be read ({} source(s) skipped)", out.skipped.len()));
        }
        Ok(out)
    }

    /// Items of one kind.
    pub fn of(&self, kind: PoiKind) -> impl Iterator<Item = &Poi> {
        self.items.iter().filter(move |p| p.kind == kind)
    }

    /// The treasure chest that is current in game week `week` ([`week_index_at`]): the chest
    /// numbered [`treasure_chest_number`]`(week)` over the Ribbon chests ([`PoiKind::TreasureChest`])
    /// and the stripped-gameobjs boards ([`PoiKind::TreasureChestBoard`]); if the install has no
    /// chest with exactly that number (it is older than the week, or the number is outside the
    /// file) the highest-numbered chest *below* it, else `None`. On a tie the board wins.
    ///
    /// **The weekly mapping is inferred, not read from a file.** The install has no date or
    /// season field for the chests (see "Seasonal / weekly Festival Playlist verdict" in
    /// `fh6-game-files.md`). The evidence: the user saw chest 015 current on 2026-10-03; the
    /// Festival Playlist series are 28 days (`FestivalPassSeriesData.str`: 21 May, 18 Jun, 16 Jul,
    /// 13 Aug, 10 Sep, 8 Oct 2026) and the chests come four per series (004-007 from 16 Jul, ...,
    /// 012-015 from 10 Sep, 016-019 added by the 6 Oct update for the 8 Oct series), so one chest
    /// per week: 015 = the week of Thursday 1 Oct 2026 14:30 UTC, which is exactly week 68 of the
    /// weekly epoch `minimap::current_season` uses. *Why not "the highest number" like the map
    /// viewer:* since the 6 Oct update the file holds 016-019 ahead of time, so the highest is
    /// three weeks too new. To re-check: 016 should go live at 2026-10-08 14:30 UTC.
    pub fn current_treasure_chest(&self, week: i64) -> Option<&Poi> {
        let wanted = treasure_chest_number(week);
        self.items
            .iter()
            .filter(|p| matches!(p.kind, PoiKind::TreasureChest | PoiKind::TreasureChestBoard))
            .filter_map(|p| Some((i64::from(trailing_number(&p.name)?), p.kind == PoiKind::TreasureChestBoard, p)))
            .filter(|&(n, _, _)| n <= wanted)
            .max_by_key(|&(n, is_board, _)| (n, is_board))
            .map(|(_, _, p)| p)
    }

    /// The danger signs (PR stunt, [`PoiKind::DangerSign`]): 15 on this install. They exist only
    /// as props inside the 40 GB `GeoChunk0.minizip` (`.pgeo` entries `…tag_dangersign_bm_<NN>…`),
    /// so they are **not** part of [`Pois::load`]: reading them costs the PGZP index (~10 MB) and
    /// the 411 013-line name list, ~0.2 s, plus a few seek-reads of ~40 KB. Call this lazily (or
    /// on a worker thread) and `items.extend(..)` the result.
    ///
    /// A port of `extract_geochunk.py:geochunk` (danger-sign part): each sign is one prop group
    /// of a rush ramp, construction boards and cones; the position is the mean of the
    /// construction boards (else cones, else the ramp). `Poi::name` = `bm_<NN>`, `n` = NN.
    pub fn load_danger_signs(media: &Path) -> Result<Vec<Poi>, String> {
        let tb = ci(media, "Tracks/Brio").filter(|p| p.is_dir()).ok_or("no Tracks/Brio folder in the install")?;
        let chunk = ci(&tb, "GeoChunk0.minizip").filter(|p| p.is_file()).ok_or("no GeoChunk0.minizip in the install")?;
        let names = ci(&tb, "ChunkContentsMiniZip0.txt").filter(|p| p.is_file()).ok_or("no ChunkContentsMiniZip0.txt in the install")?;
        let pg = Pgzp::open(&chunk, &names, &|n| n.ends_with(".pgeo") && n.contains("tag_dangersign_bm_"))?;
        let mut file = pg.open_file()?;
        // sign number (as written in the name, e.g. "01") -> its model groups in first-seen order
        let mut signs: BTreeMap<String, Vec<(String, Vec<[f64; 3]>)>> = BTreeMap::new();
        let order = pg.sorted(pg.names().iter().map(|(i, _)| *i).collect());
        let name_of: std::collections::HashMap<usize, &str> = pg.names().iter().map(|(i, n)| (*i, n.as_str())).collect();
        for r in order {
            let leaf = name_of[&r].rsplit("cellsize\\").next().unwrap_or("");
            let Some(nn) = leaf.split_once("tag_dangersign_bm_").map(|(_, t)| t.bytes().take_while(u8::is_ascii_digit).map(char::from).collect::<String>()).filter(|s| !s.is_empty())
            else {
                continue;
            };
            let groups = signs.entry(nn).or_default();
            for (model, pts) in pgeo_models(&pg.entry(&mut file, r)?) {
                let key = model.replace("_3D", "");
                match groups.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, v)) => v.extend(pts),
                    None => groups.push((key, pts)),
                }
            }
        }
        let mut out = Vec::new();
        for (nn, groups) in signs {
            let pick = |needle: &str| groups.iter().find(|(k, _)| k.contains(needle)).map(|(_, v)| v).filter(|v| !v.is_empty());
            let Some(main) = pick("constructionbrd").or_else(|| pick("cone")).or_else(|| pick("rush_ramp")) else { continue };
            let c = (0..3).map(|k| (main.iter().map(|p| p[k]).sum::<f64>() / main.len() as f64) as f32).collect::<Vec<_>>();
            out.push(Poi { kind: PoiKind::DangerSign, x: c[0], z: c[2], y: c[1], name: format!("bm_{nn}"), n: nn.parse().unwrap_or(0), gate: None });
        }
        if out.is_empty() {
            return Err("no danger sign found in GeoChunk0".to_string());
        }
        Ok(out)
    }
}

/// The game's weekly rotation epoch, Unix seconds: Thursday 2025-06-12 14:30 UTC (the same
/// value `minimap::current_season` rotates the map skin by).
pub const WEEK_EPOCH: i64 = 1_749_738_600;
const WEEK_SECS: i64 = 604_800;
/// Chest number minus week index: chest 015 is week 68 (the week from Thursday 2026-10-01 14:30 UTC).
const TREASURE_CHEST_WEEK_OFFSET: i64 = 53;

/// Weekly rotation index at Unix time `unix` (0 = the week from [`WEEK_EPOCH`]; negative before it).
pub fn week_index_at(unix: i64) -> i64 {
    (unix - WEEK_EPOCH).div_euclid(WEEK_SECS)
}

/// [`week_index_at`] for now (wall clock).
pub fn week_index_now() -> i64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    week_index_at(now)
}

/// Number of the treasure chest that is current in game week `week` (inferred, see
/// [`Pois::current_treasure_chest`]).
pub fn treasure_chest_number(week: i64) -> i64 {
    week - TREASURE_CHEST_WEEK_OFFSET
}

/// The digits at the end of `name` (`DISCOUNT_BOARD_TREASURE_CHEST_015` → 15).
fn trailing_number(name: &str) -> Option<u32> {
    let start = name.bytes().rposition(|b| !b.is_ascii_digit()).map_or(0, |i| i + 1);
    all_digits(&name[start..])
}

/// Props of a decoded `.pgeo` entry: `(model name, instance positions)` in file order. A port of
/// `extract_geochunk.py:parse_pgeo`: a section name, a header with the bounding box, then models
/// found by scanning for `u32 len, name, u32 count, count × 80-byte instances` whose first
/// position lies inside the box (±3 m); anything else is skipped byte by byte. Positions are
/// 3 × u32 in sign-magnitude 16.16 fixed point at the start of each 80-byte instance.
fn pgeo_models(d: &[u8]) -> Vec<(String, Vec<[f64; 3]>)> {
    const STRIDE: usize = 80;
    let n = d.len();
    let u32at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let f32at = |o: usize| f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as f64;
    let sm = |u: u32| {
        let v = (u & 0x7fff_ffff) as f64 / 65536.0;
        if u & 0x8000_0000 != 0 {
            -v
        } else {
            v
        }
    };
    let pos = |o: usize| [sm(u32at(o)), sm(u32at(o + 4)), sm(u32at(o + 8))];
    let mut models = Vec::new();
    if n < 4 {
        return models;
    }
    let p = 4 + u32at(0) as usize + 12;
    if p.checked_add(32).is_none_or(|e| e > n) {
        return models;
    }
    let (lo, hi) = ([f32at(p), f32at(p + 4), f32at(p + 8)], [f32at(p + 16), f32at(p + 20), f32at(p + 24)]);
    let mut o = p + 32;
    while o + 8 < n {
        let len = u32at(o) as usize;
        if (4..=120).contains(&len) && o + 8 + len <= n {
            let s = &d[o + 4..o + 4 + len];
            if s.iter().all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'[' | b']')) {
                let cnt = u32at(o + 4 + len) as usize;
                let q = o + 8 + len;
                if (1..=100_000).contains(&cnt) && q + 12 <= n {
                    let [x, y, z] = pos(q);
                    if (0..3).all(|k| [x, y, z][k] >= lo[k] - 3.0 && [x, y, z][k] <= hi[k] + 3.0) {
                        let pts = (0..cnt).take_while(|k| q + k * STRIDE + 12 <= n).map(|k| pos(q + k * STRIDE)).collect();
                        models.push((String::from_utf8_lossy(s).into_owned(), pts));
                        o = q + cnt * STRIDE;
                        continue;
                    }
                }
            }
        }
        o += 1;
    }
    models
}

// ---------------------------------------------------------------------------------------------
// classification

/// `[A-Za-z0-9_]`, the regex `\w` for the ASCII names these files use.
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `s` as a number if it is non-empty and only digits.
fn all_digits(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `\d+` at the start of `s` → (value, rest).
fn leading_digits(s: &str) -> Option<(u32, &str)> {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    Some((all_digits(&s[..end])?, &s[end..]))
}

/// `name[skip .. len - cut]`, or the whole name if that is out of range (callers have already
/// matched an ASCII prefix/suffix of at least that length).
fn cut(name: &str, skip: usize, cut: usize) -> String {
    name.get(skip..name.len().saturating_sub(cut)).unwrap_or(name).to_string()
}

/// The if/elif chain of `extract_poi.py` for a `route0.nt` locator name → (kind, slug, n).
/// The order matters (e.g. `estate_fast_travel_locator` is a fast-travel point, not an estate).
/// Every `sidi_` prefix is 5 characters; slice with that, not with per-kind magic numbers.
fn classify_route0(n: &str) -> Option<(PoiKind, String, u32)> {
    let l = n.to_ascii_lowercase();
    let l = l.as_str();
    let k = |kind, name: String| Some((kind, name, 0));
    if l.starts_with("barn_finds_cinematic_") {
        k(PoiKind::BarnFind, cut(n, 21, 0))
    } else if l.starts_with("barn_finds_anna_hint_") {
        k(PoiKind::BarnFindHint, cut(n, 21, 0))
    } else if l.starts_with("player_house_") && l.ends_with("_root_locator") {
        k(PoiKind::House, cut(n, 13, 13))
    } else if l.ends_with("fast_travel_locator") {
        k(PoiKind::FastTravel, cut(n, 0, 20))
    } else if l == "festival_root_locator" || l == "legend_island_outpost_root_locator" {
        k(PoiKind::FestivalSite, cut(n, 0, 13))
    } else if l.starts_with("estate_") && l != "estate_fast_travel_locator" {
        k(PoiKind::Estate, n.to_string())
    } else if l.strip_prefix("carmeet_").and_then(|r| r.strip_suffix("_locator")).is_some_and(|m| !m.is_empty() && m.bytes().all(is_word))
        && !l.contains("character")
        && !l.contains("parking")
    {
        k(PoiKind::CarMeet, cut(n, 0, 8))
    } else if l.strip_prefix("drag_meet_").and_then(leading_digits).is_some_and(|(_, r)| r.starts_with("_activation")) {
        k(PoiKind::DragMeet, cut(n, 0, 11))
    } else if l.strip_prefix("drag_meet_").and_then(leading_digits).is_some_and(|(_, r)| r.starts_with("_finish_line")) {
        k(PoiKind::DragMeetFinish, cut(n, 0, 12))
    } else if l.strip_prefix("sidi_aftermarket_board_").and_then(all_digits).is_some() {
        k(PoiKind::AftermarketBoard, cut(n, 5, 0))
    } else if l.strip_prefix("sidi_aftermarket_").and_then(all_digits).is_some() {
        k(PoiKind::AftermarketSpot, cut(n, 5, 0))
    } else if l.strip_prefix("sidi_treasurecar_").and_then(leading_digits).is_some_and(|(_, r)| r == "_spawn") {
        k(PoiKind::TreasureCar, cut(n, 5, 6))
    } else if l.starts_with("sidi_touge_event") {
        // `sidi_touge_event_<route id>`
        let id = l.strip_prefix("sidi_touge_event_").and_then(all_digits).unwrap_or(0);
        Some((PoiKind::TougeEvent, cut(n, 5, 0), id))
    } else if l.starts_with("sidi_showcase") {
        k(PoiKind::Showcase, cut(n, 5, 0))
    } else if l.starts_with("sidi_hj_") {
        k(PoiKind::HorizonJob, cut(n, 8, 0))
    } else if l.starts_with("sidi_hs_") {
        k(PoiKind::HorizonStory, cut(n, 8, 0))
    } else if l.starts_with("sidi_rush_") {
        k(PoiKind::RushEvent, cut(n, 5, 0))
    } else if l.starts_with("sidi_invitational") || l.starts_with("sidi_legendevent") {
        k(PoiKind::SpecialEvent, cut(n, 5, 0))
    } else if l.starts_with("sidi_upsell") && !l.ends_with("_exit") {
        k(PoiKind::Upsell, cut(n, 5, 0))
    } else {
        None
    }
}

/// `map_region_<slug>.nt` → slug (case-insensitive file name).
fn region_slug(file: &str) -> Option<String> {
    let l = file.to_ascii_lowercase();
    let s = l.strip_prefix("map_region_")?.strip_suffix(".nt")?;
    (!s.is_empty()).then(|| s.to_string())
}

/// The `Arena_NNN` locators in ascending NNN as an x/z outline; a last point that repeats the first
/// (`north_plains`) is dropped. `None` for fewer than 3 points.
fn region_outline(text: &str) -> Option<Vec<[f32; 2]>> {
    let mut pts: Vec<(u32, [f32; 2])> = locators(text)
        .into_iter()
        .filter_map(|l| Some((all_digits(l.name.strip_prefix("Arena_")?)?, [l.x, l.z])))
        .collect();
    pts.sort_by_key(|p| p.0);
    let mut out: Vec<[f32; 2]> = pts.into_iter().map(|p| p.1).collect();
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    (out.len() >= 3).then_some(out)
}

/// The `Ribbon_00/GameObjs.xml` categories (a port of `extract_geochunk.py:gameobjs`). Ids are
/// unique keys like in the Python (a repeated id keeps the last one); iteration is sorted by id.
fn ribbon_items(objs: &[GameObj], items: &mut Vec<Poi>) {
    let map: BTreeMap<&str, [f32; 3]> = objs.iter().map(|o| (o.id.as_str(), o.pos)).collect();
    let mut push = |kind, name: String, p: [f32; 3], n: u32, gate: Option<[[f32; 2]; 2]>| items.push(Poi { kind, x: p[0], z: p[2], y: p[1], name, n, gate });
    let mid = |a: [f32; 3], b: [f32; 3]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
    for (&id, &p) in &map {
        if let Some((v, num)) = id.strip_prefix("DISCOUNT_BOARD_XP_").and_then(|r| r.split_once('_')) {
            if matches!(v, "A" | "B" | "C") {
                if let Some(n) = all_digits(num) {
                    push(PoiKind::XpBoard, id.to_string(), p, n, None);
                }
            }
        } else if let Some(n) = id.strip_prefix("DISCOUNT_BOARD_TREASURE_CHEST_").and_then(all_digits) {
            push(PoiKind::TreasureChest, id.to_string(), p, n, None);
        } else if let Some((region, num)) = id.strip_prefix("MASCOTS_REGION_").and_then(|r| r.split_once('_')) {
            if let (Some(r), Some(_)) = (all_digits(region), all_digits(num)) {
                push(PoiKind::Mascot, id.to_string(), p, r, None);
            }
        } else if let Some(n) = id.strip_prefix("ESTATE_ENTRANCE_").and_then(all_digits) {
            push(PoiKind::EstateEntrance, id.to_string(), p, n, None);
        }
    }
    // speed traps: LEFT/RIGHT camera poles either side of the road; the position is the midpoint
    for (&id, &l) in &map {
        let Some(num) = id.strip_prefix("SPEEDCAMERA_").and_then(|r| r.strip_suffix("_LEFT")) else { continue };
        let (Some(n), Some(&r)) = (all_digits(num), map.get(format!("SPEEDCAMERA_{num}_RIGHT").as_str())) else { continue };
        push(PoiKind::SpeedTrap, format!("SPEEDCAMERA_{num}"), mid(l, r), n, Some([[l[0], l[2]], [r[0], r[2]]]));
    }
    // gate pairs: speed zones, trailblazers, drift zones. Gate 1 vs 2 (start / end) is unverified: emit both.
    for (prefix, kind) in [("SPEEDCAMERAZONE", PoiKind::SpeedZone), ("TRAILBLAZER", PoiKind::Trailblazer), ("DRIFTZONEMARKER", PoiKind::DriftZone)] {
        for &id in map.keys() {
            let Some(num) = id.strip_prefix(prefix).and_then(|r| r.strip_prefix('_')).and_then(|r| r.strip_suffix("_LEFT_1")) else { continue };
            let Some(n) = all_digits(num) else { continue };
            for gate in [1, 2] {
                if let (Some(&l), Some(&r)) = (map.get(format!("{prefix}_{num}_LEFT_{gate}").as_str()), map.get(format!("{prefix}_{num}_RIGHT_{gate}").as_str())) {
                    push(kind, format!("{prefix}_{num}_gate{gate}"), mid(l, r), n, Some([[l[0], l[2]], [r[0], r[2]]]));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// file readers (pure, unit-tested on synthetic XML)

/// A `.nt` locator.
#[derive(Clone, Debug, PartialEq)]
pub struct Locator {
    pub name: String,
    pub x: f32,
    /// Height (`value._42`).
    pub y: f32,
    pub z: f32,
}

/// A `.tz` trigger zone.
#[derive(Clone, Debug, PartialEq)]
pub struct Tzone {
    /// `sphere` / `box` / `mesh`.
    pub kind: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// `<size x>` (the radius of a sphere), 0 when absent.
    pub size_x: f32,
}

/// A `GameObjs.xml` object.
#[derive(Clone, Debug, PartialEq)]
pub struct GameObj {
    pub id: String,
    pub pos: [f32; 3],
    /// `<Orientation><ZAxis>` when present.
    pub zaxis: Option<[f32; 3]>,
}

/// The text between `key` (e.g. `value="`) and the next `"`, searching `s` forward.
fn attr<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let i = s.find(key)? + key.len();
    let j = s[i..].find('"')?;
    Some(&s[i..i + j])
}

fn attr_f32(s: &str, key: &str) -> Option<f32> {
    attr(s, key)?.parse().ok()
}

/// `"x,y,z"` → three floats.
fn vec3(s: &str) -> Option<[f32; 3]> {
    let mut it = s.split(',').map(|v| v.trim().parse::<f32>());
    Some([it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?])
}

/// `.nt` locators. A locator without a name or one of the three coordinates is skipped.
pub fn locators(text: &str) -> Vec<Locator> {
    let mut out = Vec::new();
    for chunk in text.split("<Locator").skip(1) {
        let chunk = chunk.split("</Locator>").next().unwrap_or(chunk);
        let (Some(name), Some(x), Some(y), Some(z)) = (
            attr(chunk, "<Name value=\""),
            attr_f32(chunk, " value._41=\""),
            attr_f32(chunk, " value._42=\""),
            attr_f32(chunk, " value._43=\""),
        ) else {
            continue;
        };
        out.push(Locator { name: name.to_string(), x, y, z });
    }
    out
}

/// `.tz` trigger zones. A zone without type, name or a complete `<position>` is skipped.
pub fn tzones(text: &str) -> Vec<Tzone> {
    let mut out = Vec::new();
    for chunk in text.split("<triggerzone ").skip(1) {
        let chunk = chunk.split("</triggerzone>").next().unwrap_or(chunk);
        let (Some(kind), Some(name)) = (attr(chunk, "type=\""), attr(chunk, "name=\"")) else { continue };
        let Some(p) = chunk.find("<position ") else { continue };
        let pos = &chunk[p..];
        let (Some(x), Some(y), Some(z)) = (attr_f32(pos, " x=\""), attr_f32(pos, " y=\""), attr_f32(pos, " z=\"")) else { continue };
        let size_x = chunk.find("<size ").and_then(|i| attr_f32(&chunk[i..], " x=\"")).unwrap_or(0.0);
        out.push(Tzone { kind: kind.to_string(), name: name.to_string(), x, y, z, size_x });
    }
    out
}

/// `GameObjs.xml` objects. An object without a parsable `<Pos value="x,y,z">` is skipped.
pub fn gameobjs(text: &str) -> Vec<GameObj> {
    let mut out = Vec::new();
    for chunk in text.split("<Obj GameplayID=\"").skip(1) {
        let Some(j) = chunk.find('"') else { continue };
        let rest = &chunk[j..];
        let rest = rest.split("</Obj>").next().unwrap_or(rest);
        let Some(pos) = attr(rest, "<Pos value=\"").and_then(vec3) else { continue };
        let zaxis = rest.find("<ZAxis ").and_then(|i| attr(&rest[i..], "value=\"")).and_then(vec3);
        out.push(GameObj { id: chunk[..j].to_string(), pos, zaxis });
    }
    out
}

/// Read a game text file: UTF-8 (lossy), BOM stripped.
fn read_text(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.trim_start_matches('\u{feff}').to_string())
}

/// Read `rel` (case-insensitive) under `root`; on failure note it in `skipped` and return `None`.
fn read_in(root: &Path, rel: &str, skipped: &mut Vec<String>) -> Option<String> {
    let path = match ci(root, rel).filter(|p| p.is_file()) {
        Some(p) => p,
        None => {
            skipped.push(format!("{rel}: not found"));
            return None;
        }
    };
    match read_text(&path) {
        Ok(t) => Some(t),
        Err(e) => {
            skipped.push(format!("{rel}: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;

    const NT: &str = "\u{feff}<TrackLocators>\r\n  <Locator Version=\"2\">\r\n    <Name value=\"carmeet_daikoku_locator\"/>\r\n    <GUID value=\"0\"/>\r\n    <SceneTransform value._11=\"1\" value._41=\"-409.1\" value._42=\"102.0\" value._43=\"-6541.2\" value._44=\"1\"/>\r\n  </Locator>\r\n  <Locator Version=\"2\">\r\n    <Name value=\"no_coords\"/>\r\n    <SceneTransform value._41=\"1\"/>\r\n  </Locator>\r\n  <Locator Version=\"2\">\r\n    <Name value=\"sidi_treasurecar_001_spawn\"/>\r\n    <SceneTransform value._41=\"1176.9\" value._42=\"828\" value._43=\"7605.4\"/>\r\n  </Locator>\r\n</TrackLocators>\r\n";

    const TZ: &str = "<triggerzones>\r\n  <triggerzone type=\"sphere\" name=\"race_trigger_zone_rt101\" mobile=\"false\">\r\n    <position x=\"3070.1\" y=\"116.4\" z=\"2574.5\" />\r\n    <size x=\"100.0\" y=\"100.0\" z=\"100.0\" />\r\n  </triggerzone>\r\n  <triggerzone type=\"box\" name=\"no_position\">\r\n    <size x=\"1\" y=\"1\" z=\"1\" />\r\n  </triggerzone>\r\n  <triggerzone type=\"box\" name=\"HS_one_activation_zone\">\r\n    <position x=\"-5\" y=\"6\" z=\"7\" />\r\n  </triggerzone>\r\n</triggerzones>\r\n";

    fn obj(id: &str, pos: &str, zaxis: Option<&str>) -> String {
        let z = zaxis.map(|z| format!("<ZAxis value=\"{z}\" />")).unwrap_or_default();
        format!("<Obj GameplayID=\"{id}\">\r\n<Pos value=\"{pos}\" />\r\n<Orientation>{z}</Orientation>\r\n</Obj>\r\n")
    }

    #[test]
    fn locators_crlf_and_missing_attributes() {
        let l = locators(NT.trim_start_matches('\u{feff}'));
        assert_eq!(l.len(), 2, "the locator without y/z is skipped: {l:?}");
        assert_eq!(l[0], Locator { name: "carmeet_daikoku_locator".into(), x: -409.1, y: 102.0, z: -6541.2 });
        assert_eq!(l[1].name, "sidi_treasurecar_001_spawn");
    }

    #[test]
    fn tzones_crlf_missing_position_and_size() {
        let z = tzones(TZ);
        assert_eq!(z.len(), 2, "the zone without <position> is skipped: {z:?}");
        assert_eq!(z[0], Tzone { kind: "sphere".into(), name: "race_trigger_zone_rt101".into(), x: 3070.1, y: 116.4, z: 2574.5, size_x: 100.0 });
        assert_eq!((z[1].name.as_str(), z[1].size_x), ("HS_one_activation_zone", 0.0));
    }

    #[test]
    fn gameobjs_zaxis_optional_and_bad_pos_skipped() {
        let text = format!(
            "<Objs>{}{}{}{}</Objs>",
            obj("A", "1,2,3", Some("0,0,1")),
            obj("B", "4.5,5,-6", None),
            obj("C", "1,2", None),
            obj("D", "x,y,z", None)
        );
        let o = gameobjs(&text);
        assert_eq!(o.len(), 2, "{o:?}");
        assert_eq!(o[0], GameObj { id: "A".into(), pos: [1.0, 2.0, 3.0], zaxis: Some([0.0, 0.0, 1.0]) });
        assert_eq!(o[1].pos, [4.5, 5.0, -6.0]);
        assert_eq!(o[1].zaxis, None);
        // a ZAxis of the *next* object must not leak into an object that has none
        let text = format!("{}{}", obj("P", "0,0,0", None), obj("Q", "1,1,1", Some("1,0,0")));
        let o = gameobjs(&text);
        assert_eq!((o[0].zaxis, o[1].zaxis), (None, Some([1.0, 0.0, 0.0])));
    }

    #[test]
    fn classification_chain() {
        let c = |n: &str| classify_route0(n).map(|(k, name, id)| (k, name, id));
        assert_eq!(c("carmeet_daikoku_locator"), Some((PoiKind::CarMeet, "carmeet_daikoku".into(), 0)));
        assert_eq!(c("carmeet_x_parking_locator"), None);
        assert_eq!(c("carmeet_character_locator"), None);
        assert_eq!(c("carmeet__locator"), None, "\\w+ needs at least one character");
        assert_eq!(c("estate_fast_travel_locator").map(|t| t.0), Some(PoiKind::FastTravel));
        assert_eq!(c("estate_one").map(|t| t.0), Some(PoiKind::Estate));
        assert_eq!(c("sidi_treasurecar_001_spawn"), Some((PoiKind::TreasureCar, "treasurecar_001".into(), 0)));
        assert_eq!(c("sidi_treasurecar_001"), None);
        assert_eq!(c("sidi_aftermarket_12"), Some((PoiKind::AftermarketSpot, "aftermarket_12".into(), 0)));
        assert_eq!(c("sidi_aftermarket_board_12").map(|t| t.0), Some(PoiKind::AftermarketBoard));
        assert_eq!(c("sidi_aftermarket_x"), None);
        assert_eq!(c("sidi_touge_event_5031"), Some((PoiKind::TougeEvent, "touge_event_5031".into(), 5031)));
        assert_eq!(c("drag_meet_01_activation_x").map(|t| t.0), Some(PoiKind::DragMeet));
        assert_eq!(c("drag_meet_01_finish_line").map(|t| t.0), Some(PoiKind::DragMeetFinish));
        assert_eq!(c("player_house_a_root_locator"), Some((PoiKind::House, "a".into(), 0)));
        assert_eq!(c("sidi_upsell_001").map(|t| t.0), Some(PoiKind::Upsell));
        assert_eq!(c("sidi_upsell_001_exit"), None);
        assert_eq!(c("Barn_Finds_Cinematic_HON_NSX_05"), Some((PoiKind::BarnFind, "HON_NSX_05".into(), 0)));
        assert_eq!(c("something_else"), None);
    }

    #[test]
    fn ribbon_gates_traps_and_dedup() {
        let mut t = String::new();
        t += &obj("SPEEDCAMERA_07_LEFT", "0,10,0", None);
        t += &obj("SPEEDCAMERA_07_RIGHT", "10,20,4", None);
        t += &obj("SPEEDCAMERA_08_LEFT", "0,0,0", None); // no RIGHT: skipped
        for (side, x) in [("LEFT", 0), ("RIGHT", 10)] {
            for g in [1, 2] {
                t += &obj(&format!("DRIFTZONEMARKER_03_{side}_{g}"), &format!("{},{},{}", x, g, g * 100), None);
            }
        }
        t += &obj("DRIFTZONEMARKER_04_LEFT_1", "0,0,0", None); // incomplete zone: no gates
        t += &obj("DISCOUNT_BOARD_XP_B_012", "1,2,3", None);
        t += &obj("DISCOUNT_BOARD_XP_Z_012", "1,2,3", None);
        t += &obj("MASCOTS_REGION_4_017", "5,6,7", None);
        t += &obj("ESTATE_ENTRANCE_09", "5,6,7", None);
        t += &obj("DISCOUNT_BOARD_TREASURE_CHEST_2", "5,6,7", None);
        t += &obj("ANIM_Train", "0,0,0", None);
        let mut items = Vec::new();
        ribbon_items(&gameobjs(&t), &mut items);
        let of = |k| items.iter().filter(|p| p.kind == k).collect::<Vec<_>>();
        let trap = of(PoiKind::SpeedTrap);
        assert_eq!(trap.len(), 1);
        assert_eq!((trap[0].x, trap[0].y, trap[0].z, trap[0].n), (5.0, 15.0, 2.0, 7));
        assert_eq!(trap[0].name, "SPEEDCAMERA_07");
        assert_eq!(trap[0].gate, Some([[0.0, 0.0], [10.0, 4.0]]), "x/z of the LEFT and RIGHT pole");
        let d = of(PoiKind::DriftZone);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].x, d[0].z, d[0].n), ("DRIFTZONEMARKER_03_gate1", 5.0, 100.0, 3));
        assert_eq!(d[1].name, "DRIFTZONEMARKER_03_gate2");
        assert_eq!(d[0].gate, Some([[0.0, 100.0], [10.0, 100.0]]));
        assert_eq!(d[1].gate, Some([[0.0, 200.0], [10.0, 200.0]]));
        assert!(items.iter().filter(|p| !matches!(p.kind, PoiKind::SpeedTrap | PoiKind::DriftZone)).all(|p| p.gate.is_none()), "only traps and gates carry a gate line");
        assert_eq!(of(PoiKind::XpBoard).len(), 1);
        assert_eq!(of(PoiKind::Mascot)[0].n, 4);
        assert_eq!(of(PoiKind::EstateEntrance)[0].n, 9);
        assert_eq!(of(PoiKind::TreasureChest)[0].n, 2);
        assert_eq!(items.len(), 1 + 2 + 1 + 1 + 1 + 1, "nothing else may be picked up (ANIM_* at 0,0,0)");
    }

    /// A hand-built `.pgeo`: junk bytes between models are skipped, positions are sign-magnitude
    /// 16.16, instances are 80 bytes, a model whose first position is outside the box is not one.
    #[test]
    fn pgeo_models_scan() {
        fn fx(v: f64) -> u32 {
            let m = (v.abs() * 65536.0).round() as u32;
            if v < 0.0 {
                m | 0x8000_0000
            } else {
                m
            }
        }
        let mut d: Vec<u8> = Vec::new();
        let name = b"c200_props_x_section0";
        d.extend((name.len() as u32).to_le_bytes());
        d.extend(name);
        for v in [0u32, 13, 15] {
            d.extend(v.to_le_bytes()); // u32 0, 13, 15
        }
        // bbox: lo (-10, 0, -10), 4 filler bytes, hi (10, 100, 10)
        for v in [-10f32, 0.0, -10.0] {
            d.extend(v.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
        for v in [10f32, 100.0, 10.0] {
            d.extend(v.to_le_bytes());
        }
        d.extend(1.0f32.to_le_bytes());
        let model = |d: &mut Vec<u8>, name: &str, pts: &[[f64; 3]]| {
            d.extend((name.len() as u32).to_le_bytes());
            d.extend(name.as_bytes());
            d.extend((pts.len() as u32).to_le_bytes());
            for p in pts {
                let start = d.len();
                for c in p {
                    d.extend(fx(*c).to_le_bytes());
                }
                d.resize(start + 80, 0);
            }
        };
        d.extend([0xff, 0xff, 0xff]); // junk before the first model
        model(&mut d, "sgn_gbl_constructionbrd_02_a_3D", &[[1.5, 20.25, -3.0], [-2.0, 21.0, 4.0]]);
        model(&mut d, "outside_the_box", &[[500.0, 0.0, 0.0]]); // first position far outside the bbox: not a model
        model(&mut d, "prp_gbl_traffic_cone_01_a_3D", &[[0.5, 19.0, 0.25]]);
        let m = pgeo_models(&d);
        let names: Vec<_> = m.iter().map(|(n, p)| (n.as_str(), p.len())).collect();
        assert_eq!(names, [("sgn_gbl_constructionbrd_02_a_3D", 2), ("prp_gbl_traffic_cone_01_a_3D", 1)]);
        assert_eq!(m[0].1, vec![[1.5, 20.25, -3.0], [-2.0, 21.0, 4.0]]);
        assert_eq!(m[1].1, vec![[0.5, 19.0, 0.25]]);
        // truncated / tiny inputs give nothing, never a panic
        assert!(pgeo_models(&d[..10]).is_empty());
        assert!(pgeo_models(&[]).is_empty());
        assert!(pgeo_models(&[0xff; 64]).is_empty());
    }

    #[test]
    fn weeks_and_chest_numbers() {
        // the weekly boundary: Thursday 14:30 UTC (1790865000 = 2026-10-01 14:30 UTC)
        assert_eq!(week_index_at(WEEK_EPOCH), 0);
        assert_eq!(week_index_at(WEEK_EPOCH - 1), -1);
        assert_eq!(week_index_at(1_790_864_999), 67);
        assert_eq!(week_index_at(1_790_865_000), 68);
        assert_eq!(week_index_at(1_791_469_799), 68, "2026-10-08 14:29:59 UTC is still chest 015's week");
        assert_eq!(week_index_at(1_791_469_800), 69);
        // the user's observation: chest 015 was current on 2026-10-03 (week 68); 016 goes live a week later
        assert_eq!(treasure_chest_number(68), 15);
        assert_eq!(treasure_chest_number(69), 16);
    }

    #[test]
    fn current_treasure_chest_follows_the_week() {
        let chest = |kind, name: &str, x: f32| Poi { kind, x, z: 0.0, y: 0.0, name: name.to_string(), n: 0, gate: None };
        let mut p = Pois::default();
        assert!(p.current_treasure_chest(68).is_none());
        p.items.push(chest(PoiKind::TreasureChest, "DISCOUNT_BOARD_TREASURE_CHEST_3", 1.0));
        for (n, x) in [("004", 2.0), ("015", 3.0), ("006", 4.0), ("016", 5.0), ("019", 6.0)] {
            p.items.push(chest(PoiKind::TreasureChestBoard, &format!("DISCOUNT_BOARD_TREASURE_CHEST_{n}"), x));
        }
        p.items.push(chest(PoiKind::CarMeet, "DISCOUNT_BOARD_TREASURE_CHEST_099", 9.0)); // other kinds are ignored
        let at = |week| p.current_treasure_chest(week).map(|c| c.x);
        assert_eq!(at(68), Some(3.0), "week 68 = chest 015");
        assert_eq!(at(69), Some(5.0), "the next week: 016");
        assert_eq!(at(72), Some(6.0), "019");
        assert_eq!(at(100), Some(6.0), "past the file: the newest it has");
        assert_eq!(at(57), Some(2.0), "57 - 53 = 4: board 004");
        assert_eq!(at(56), Some(1.0), "chest 3 only exists as a Ribbon chest");
        assert_eq!(at(55), None, "nothing at or below 2");
        // a tie goes to the board (the viewer lists boards first)
        p.items.push(chest(PoiKind::TreasureChest, "DISCOUNT_BOARD_TREASURE_CHEST_15", 7.0));
        assert_eq!(p.current_treasure_chest(68).map(|c| c.x), Some(3.0));
        assert_eq!(trailing_number("DISCOUNT_BOARD_TREASURE_CHEST_015"), Some(15));
        assert_eq!(trailing_number("no_number_"), None);
        assert_eq!(trailing_number("007"), Some(7));
    }

    #[test]
    fn region_outline_sorted_and_closed_point_dropped() {
        let loc = |n: &str, x: i32, z: i32| format!("<Locator><Name value=\"{n}\"/><SceneTransform value._41=\"{x}\" value._42=\"0\" value._43=\"{z}\"/></Locator>");
        let t = format!("{}{}{}{}{}{}", loc("Arena_002", 10, 10), loc("Arena_000", 0, 0), loc("Arena_001", 10, 0), loc("Arena_003", 0, 0), loc("Other", 99, 99), "");
        // sorted: (0,0) (10,0) (10,10) (0,0) -> the repeated first point goes
        assert_eq!(region_outline(&t), Some(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]));
        assert_eq!(region_outline(&loc("Arena_000", 0, 0)), None);
        assert_eq!(region_slug("MAP_REGION_North_Plains.NT"), Some("north_plains".into()));
        assert_eq!(region_slug("route0.nt"), None);
    }

    /// A fake install on disk: mixed-case folders, a BOM, CRLF, missing sources.
    #[test]
    fn load_synthetic_install() {
        let root = crate::gamedata::tempdir("poi_load");
        let tb = root.join("tracks/BRIO");
        std::fs::create_dir_all(tb.join("TrackRoutes")).unwrap();
        std::fs::create_dir_all(tb.join("triggerzones/tz_race_activations")).unwrap();
        std::fs::write(tb.join("TrackRoutes/Route0.nt"), NT).unwrap();
        std::fs::write(tb.join("triggerzones/tz_race_activations/race_triggers.tz"), TZ).unwrap();
        let r = Pois::load(&root).unwrap();
        let kinds: Vec<_> = r.items.iter().map(|p| (p.kind, p.name.as_str(), p.n)).collect();
        assert_eq!(
            kinds,
            vec![(PoiKind::RacePin, "race_trigger_zone_rt101", 101), (PoiKind::CarMeet, "carmeet_daikoku", 0), (PoiKind::TreasureCar, "treasurecar_001", 0)]
        );
        assert!(r.skipped.iter().any(|s| s.starts_with("trackroutes/pinata_locators.nt")), "{:?}", r.skipped);
        assert!(r.skipped.len() >= 8, "every missing source is noted: {:?}", r.skipped);
        // nothing readable at all -> Err
        let empty = crate::gamedata::tempdir("poi_empty");
        std::fs::create_dir_all(empty.join("Tracks/Brio")).unwrap();
        assert!(Pois::load(&empty).is_err());
        assert!(Pois::load(&empty.join("nope")).is_err());
    }

    /// Counts of the Python reference (`extract_poi.py` / `extract_geochunk.py --skip-pgeo`) on the
    /// install of 2026-10 (route0.nt of 6 Oct). A game update changes these files, so these are
    /// *today's* numbers, not constants of the game: on a mismatch re-run the Python extractors,
    /// check the diff makes sense and update both here and in `docs/game-data/fh6-game-files.md`.
    /// (The docs' older "5578 records / 371 locators" were the same data before an update.)
    #[test]
    fn real_install_counts_and_anchors() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_counts_and_anchors: FH6 install not found");
            return;
        };
        let t0 = std::time::Instant::now();
        let p = Pois::load(&media).expect("pois");
        eprintln!("Pois::load: {:?}, {} items, skipped {:?}", t0.elapsed(), p.items.len(), p.skipped);
        assert!(p.skipped.is_empty(), "{:?}", p.skipped);
        let count = |k| p.of(k).count();
        use PoiKind::*;
        let mut total = 0;
        for (kind, want) in [
            (RacePin, 36),
            (TougeEvent, 5),
            (Landmark, 75),
            (StoryActivation, 11),
            (JobActivation, 6),
            (CreatureZone, 47),
            (House, 8),
            (FastTravel, 11),
            (FestivalSite, 2),
            (Estate, 4),
            (CarMeet, 6),
            (DragMeet, 3),
            (DragMeetFinish, 3),
            (AftermarketSpot, 44),
            (AftermarketBoard, 44),
            (TreasureCar, 10),
            (Showcase, 2),
            (RushEvent, 3),
            (SpecialEvent, 3),
            (HorizonJob, 5),
            (HorizonStory, 9),
            (Upsell, 5),
            (BarnFind, 15),
            (BarnFindHint, 15),
            (Pinata, 1536),
            (Eliminator, 373),
            (Parking, 2664),
            (XpBoard, 200),
            (Mascot, 200),
            (EstateEntrance, 37),
            (TreasureChest, 3),
            (SpeedTrap, 30),
            (SpeedZone, 60),
            (Trailblazer, 24),
            (DriftZone, 40),
            (TreasureChestBoard, 16),
            (FlagRushFlag, 6),
        ] {
            assert_eq!(count(kind), want, "{kind:?}");
            total += want;
        }
        assert_eq!(p.items.len(), total, "no item of an unlisted kind");
        assert_eq!(p.regions.len(), 10);
        let slugs: Vec<_> = p.regions.iter().map(|r| r.slug.as_str()).collect();
        assert_eq!(slugs, ["canyon", "city", "east_coast", "festival", "highlands", "legend_island", "north_plains", "snowy_mountains", "south_coast", "south_plains"]);
        assert!(p.regions.iter().all(|r| r.outline.len() >= 3 && r.outline.first() != r.outline.last()));

        let at = |name: &str| p.items.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("no POI {name}"));
        let near = |poi: &Poi, x: f32, z: f32| (poi.x - x).abs() < 0.06 && (poi.z - z).abs() < 0.06;
        let c = at("carmeet_daikoku");
        assert!(near(c, -409.1, -6541.2), "{c:?}");
        assert!(p.of(DragMeet).any(|i| i.name == "drag_meet_01" && near(i, -1888.7, -181.1)));
        assert!(p.of(DragMeetFinish).any(|i| i.name == "drag_meet_01" && near(i, -1888.7, -1190.2)));
        let t = at("treasurecar_001");
        assert!(near(t, 1176.9, 7605.4), "{t:?}");

        // gate lines: present exactly for traps and gate kinds, and (x, z) is their midpoint
        for i in &p.items {
            let has = matches!(i.kind, SpeedTrap | SpeedZone | Trailblazer | DriftZone);
            assert_eq!(i.gate.is_some(), has, "{i:?}");
            if let Some([l, r]) = i.gate {
                assert!(((l[0] + r[0]) / 2.0 - i.x).abs() < 0.01 && ((l[1] + r[1]) / 2.0 - i.z).abs() < 0.01, "{i:?}");
                let w = (l[0] - r[0]).hypot(l[1] - r[1]);
                assert!(w > 1.0 && w < 60.0, "implausible gate width {w} m: {i:?}");
            }
        }

        // finite and not at the origin (the 27 (0,0,0) GameObjs are not picked up by any category)
        for i in &p.items {
            assert!(i.x.is_finite() && i.z.is_finite() && i.y.is_finite(), "{i:?}");
            assert!(!(i.x == 0.0 && i.z == 0.0), "{i:?}");
        }
        let far: Vec<_> = p.items.iter().filter(|i| i.x.abs() > 12000.0 || i.z.abs() > 12000.0).collect();
        // the only off-map items: 11 parking areas at z ~ 18.2-18.4 km (the same off-map band as race routes 102 / 103)
        assert!(far.len() == 11 && far.iter().all(|i| i.kind == Parking && i.z > 18000.0), "{far:?}");

        // the install holds chests 001-019 (the 6 Oct update added 016-019); week 68 (from 2026-10-01) = 015, the next week 016
        let c = p.current_treasure_chest(68).expect("a chest");
        assert_eq!((c.kind, c.name.as_str()), (TreasureChestBoard, "DISCOUNT_BOARD_TREASURE_CHEST_015"));
        assert!(near(c, -1179.3, -8341.3), "{c:?}");
        assert_eq!(p.current_treasure_chest(69).map(|c| c.name.as_str()), Some("DISCOUNT_BOARD_TREASURE_CHEST_016"));
        let nums: Vec<u32> = p.items.iter().filter(|i| matches!(i.kind, TreasureChest | TreasureChestBoard)).filter_map(|i| trailing_number(&i.name)).collect();
        assert_eq!((nums.iter().min(), nums.iter().max(), nums.len()), (Some(&1), Some(&19), 19));
    }

    /// The 15 danger signs of `extract_geochunk.py` (`geochunk_pois.json`, positions rounded to
    /// 0.01 m): the Rust PGZP / `.pgeo` port must land on the same centres. Timing is printed:
    /// release ~0.2 s, which is why they are not part of [`Pois::load`].
    #[test]
    fn real_install_danger_signs_match_python() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_danger_signs_match_python: FH6 install not found");
            return;
        };
        let t0 = std::time::Instant::now();
        let signs = Pois::load_danger_signs(&media).expect("danger signs");
        eprintln!("Pois::load_danger_signs: {:?}, {} signs", t0.elapsed(), signs.len());
        // (name, x, y, z) from the Python reference; each sign's centre is the mean of its construction boards, else cones, else the ramp
        let want: [(&str, f32, f32, f32); 15] = [
            ("bm_02", 379.73, 510.96, 6376.02),
            ("bm_03", 1577.13, 377.65, 4461.47),
            ("bm_04", 3142.29, 411.06, 4447.69),
            ("bm_05", 1977.79, 233.88, 3123.21),
            ("bm_06", -248.89, 160.73, 2358.66),
            ("bm_08", -3077.84, 278.88, 562.82),
            ("bm_09", 2621.49, 224.29, -5420.31),
            ("bm_11", 3049.47, 188.9, -382.65),
            ("bm_13", -6198.24, 230.53, -1942.35),
            ("bm_14", -4332.45, 416.94, -2558.01),
            ("bm_15", -3772.54, 364.42, -3793.64),
            ("bm_16", 1183.35, 158.36, -2473.31),
            ("bm_17", 1677.79, 132.79, -4186.32),
            ("bm_18", -3552.52, 245.02, -5880.04),
            ("bm_20", -1608.64, 241.08, -8929.3),
        ];
        assert_eq!(signs.len(), 15, "{signs:?}");
        assert!(signs.iter().all(|s| s.kind == PoiKind::DangerSign));
        for (name, x, y, z) in want {
            let s = signs.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no {name}: {signs:?}"));
            assert!((s.x - x).abs() <= 0.011 && (s.y - y).abs() <= 0.011 && (s.z - z).abs() <= 0.011, "{name}: {s:?} vs ({x}, {y}, {z})");
            assert_eq!(s.n, name[3..].parse::<u32>().unwrap());
        }
    }
}
