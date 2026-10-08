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
//! speed-limit signs); those are not read here: `GameObjs.xml` supersedes the guesses, and the
//! GeoChunk reads need seconds of seek-reads in a 40 GB file.
//!
//! **No XML crate.** The files are machine-written with a fixed attribute order, so a forward
//! `str::find` scan is enough (all sources together ≈ 18 ms in release) and adds no dependency.
//! The scan therefore assumes that order; a differently ordered file yields fewer items (never a
//! panic or wrong data), and the real-install test below fails loudly on a count change.

use std::collections::BTreeMap;
use std::path::Path;

use super::install::ci;

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
                items.push(Poi { kind: PoiKind::RacePin, x: z.x, z: z.z, y: z.y, name: z.name, n });
            }
        }

        // --- route0.nt: the big named-locator list
        for l in read_in(&tb, "trackroutes/route0.nt", &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
            if let Some((kind, name, n)) = classify_route0(&l.name) {
                items.push(Poi { kind, x: l.x, z: l.z, y: l.y, name, n });
            }
        }

        // --- the dense one-kind locator files
        for (rel, kind) in [
            ("trackroutes/pinata_locators.nt", PoiKind::Pinata),
            ("trackroutes/eliminator_locators.nt", PoiKind::Eliminator),
            ("trackroutes/parkingareas.nt", PoiKind::Parking),
        ] {
            for l in read_in(&tb, rel, &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
                items.push(Poi { kind, x: l.x, z: l.z, y: l.y, name: l.name, n: 0 });
            }
        }

        // --- trigger zones
        for (rel, kind) in [
            ("triggerzones/tz_world_constraints/landmark_triggers.tz", PoiKind::Landmark),
            ("triggerzones/tz_creatures/creatures_all.tz", PoiKind::CreatureZone),
        ] {
            for z in read_in(&tb, rel, &mut out.skipped).map(|t| tzones(&t)).unwrap_or_default() {
                items.push(Poi { kind, x: z.x, z: z.z, y: z.y, name: z.name, n: 0 });
            }
        }
        for z in read_in(&tb, "triggerzones/tz_bucket_challenges/tz_horizonstories.tz", &mut out.skipped).map(|t| tzones(&t)).unwrap_or_default() {
            if let Some(name) = z.name.strip_suffix("_activation_zone") {
                let kind = if z.name.starts_with("HS_") { PoiKind::StoryActivation } else { PoiKind::JobActivation };
                items.push(Poi { kind, x: z.x, z: z.z, y: z.y, name: name.to_string(), n: 0 });
            }
        }

        // --- car meets / upsell pins hidden in the small route files
        for rel in ["trackroutes/route40001.nt", "trackroutes/route40900.nt"] {
            for l in read_in(&tb, rel, &mut out.skipped).map(|t| locators(&t)).unwrap_or_default() {
                if let Some((PoiKind::CarMeet, name, n)) = classify_route0(&l.name) {
                    items.push(Poi { kind: PoiKind::CarMeet, x: l.x, z: l.z, y: l.y, name, n });
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
                    ups.push((key, Poi { kind: PoiKind::Upsell, x: l.x, z: l.z, y: l.y, name: l.name[5..].to_string(), n: 0 }));
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
                        items.push(Poi { kind: PoiKind::TreasureChestBoard, x, z, y, name: o.id, n: 0 });
                    }
                } else if o.id.contains("_FR_FLAG_") {
                    items.push(Poi { kind: PoiKind::FlagRushFlag, x, z, y, name: o.id, n: 0 });
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
    let mut push = |kind, name: String, p: [f32; 3], n: u32| items.push(Poi { kind, x: p[0], z: p[2], y: p[1], name, n });
    let mid = |a: [f32; 3], b: [f32; 3]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0];
    for (&id, &p) in &map {
        if let Some((v, num)) = id.strip_prefix("DISCOUNT_BOARD_XP_").and_then(|r| r.split_once('_')) {
            if matches!(v, "A" | "B" | "C") {
                if let Some(n) = all_digits(num) {
                    push(PoiKind::XpBoard, id.to_string(), p, n);
                }
            }
        } else if let Some(n) = id.strip_prefix("DISCOUNT_BOARD_TREASURE_CHEST_").and_then(all_digits) {
            push(PoiKind::TreasureChest, id.to_string(), p, n);
        } else if let Some((region, num)) = id.strip_prefix("MASCOTS_REGION_").and_then(|r| r.split_once('_')) {
            if let (Some(r), Some(_)) = (all_digits(region), all_digits(num)) {
                push(PoiKind::Mascot, id.to_string(), p, r);
            }
        } else if let Some(n) = id.strip_prefix("ESTATE_ENTRANCE_").and_then(all_digits) {
            push(PoiKind::EstateEntrance, id.to_string(), p, n);
        }
    }
    // speed traps: LEFT/RIGHT camera poles either side of the road; the position is the midpoint
    for (&id, &l) in &map {
        let Some(num) = id.strip_prefix("SPEEDCAMERA_").and_then(|r| r.strip_suffix("_LEFT")) else { continue };
        let (Some(n), Some(&r)) = (all_digits(num), map.get(format!("SPEEDCAMERA_{num}_RIGHT").as_str())) else { continue };
        push(PoiKind::SpeedTrap, format!("SPEEDCAMERA_{num}"), mid(l, r), n);
    }
    // gate pairs: speed zones, trailblazers, drift zones. Gate 1 vs 2 (start / end) is unverified: emit both.
    for (prefix, kind) in [("SPEEDCAMERAZONE", PoiKind::SpeedZone), ("TRAILBLAZER", PoiKind::Trailblazer), ("DRIFTZONEMARKER", PoiKind::DriftZone)] {
        for &id in map.keys() {
            let Some(num) = id.strip_prefix(prefix).and_then(|r| r.strip_prefix('_')).and_then(|r| r.strip_suffix("_LEFT_1")) else { continue };
            let Some(n) = all_digits(num) else { continue };
            for gate in [1, 2] {
                if let (Some(&l), Some(&r)) = (map.get(format!("{prefix}_{num}_LEFT_{gate}").as_str()), map.get(format!("{prefix}_{num}_RIGHT_{gate}").as_str())) {
                    push(kind, format!("{prefix}_{num}_gate{gate}"), mid(l, r), n);
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
        let d = of(PoiKind::DriftZone);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].name.as_str(), d[0].x, d[0].z, d[0].n), ("DRIFTZONEMARKER_03_gate1", 5.0, 100.0, 3));
        assert_eq!(d[1].name, "DRIFTZONEMARKER_03_gate2");
        assert_eq!(of(PoiKind::XpBoard).len(), 1);
        assert_eq!(of(PoiKind::Mascot)[0].n, 4);
        assert_eq!(of(PoiKind::EstateEntrance)[0].n, 9);
        assert_eq!(of(PoiKind::TreasureChest)[0].n, 2);
        assert_eq!(items.len(), 1 + 2 + 1 + 1 + 1 + 1, "nothing else may be picked up (ANIM_* at 0,0,0)");
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

        // finite and not at the origin (the 27 (0,0,0) GameObjs are not picked up by any category)
        for i in &p.items {
            assert!(i.x.is_finite() && i.z.is_finite() && i.y.is_finite(), "{i:?}");
            assert!(!(i.x == 0.0 && i.z == 0.0), "{i:?}");
        }
        let far: Vec<_> = p.items.iter().filter(|i| i.x.abs() > 12000.0 || i.z.abs() > 12000.0).collect();
        // the only off-map items: 11 parking areas at z ~ 18.2-18.4 km (the same off-map band as race routes 102 / 103)
        assert!(far.len() == 11 && far.iter().all(|i| i.kind == Parking && i.z > 18000.0), "{far:?}");
    }
}
