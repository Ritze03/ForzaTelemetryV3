//! The `fh6-road-types` file: which type each edge of the game's road graph ([`super::nav`])
//! has, plus the user-placed points / links of the road-type project. Format versions 1 and 2;
//! written by the map editor's Export and by `tools/fh6-extract/fix_highways.py`. See
//! `docs/game-data/fh6-map-tooling.md` for the format and the editor.
//!
//! ```text
//! {"format":"fh6-road-types","version":2,
//! "nav":{"file":"Brio_00.nav","sha1":"…","nodes":38473},
//! "types":{            one "<a>-<b>":"<type>" per line, game edges only, a < b; unset edges absent
//! "added":[            one {"a":…,"b":…,"type":"…"|null} per line: links between any two points
//! "points":{           "<id>":[x,z,y], user points, ids >= 1 000 000
//! "moved":{            "<id>":[x,z,y], position overrides of game nodes
//! "removed":["a-b",…], one line: game edges deleted
//! "jump_from":{        "a-b": take-off id (jump edges have a direction)
//! "races":{            "<route id>":"<kind>"  -- opaque here, written back untouched
//! "counts":{…}}        informational
//! ```
//! v1 has only `types` (road / offroad / other). Parsing is lenient like the editor's
//! `importObj`: unknown type names, bad keys, out-of-range ids are skipped and counted in
//! [`RoadTypes::warnings`]; only a wrong `format` / `version` or malformed JSON is an error.
//!
//! **Where the data comes from.** The *project* file (`assets/map/fh6-road-types.json`, ids and
//! user-placed points only, no game data) is embedded ([`RoadTypes::project`]). A user may have
//! saved their own version, the *override* ([`override_path`]). **The override replaces the
//! project file wholesale** ([`RoadTypes::current`]); there is no per-entry merge. *Why:* the
//! editor always saves the complete state; "this edge is now unset" and "this added link was
//! deleted" are expressed by *absence*, which an overlay cannot represent without tombstones
//! (a new format surface for little gain). The cost: users with a saved override do not get
//! later project improvements automatically. Mitigation: Save stamps `"based_on"` = SHA-1 of
//! the project file it started from, and [`Current::project_updated_since_save`] says when the
//! embedded one has moved on (Setup then offers "Reset to project").
//!
//! [`RoadTypes::raw`] is the bare nav graph: every edge unset, no user points or links.
//!
//! The writer ([`RoadTypes::to_json_string`]) reproduces `fix_highways.py write_v2` / the
//! editor's `exportText` line for line (one entry per line, so a pull-request diff shows only
//! the changed entries); writing the embedded project file back is byte-identical (tested).
//! Entry order is kept, so maps are `Vec`s / order lists, not plain hash maps.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;
use sha1::{Digest, Sha1};

use super::nav::Nav;

/// The project road-type file (also the source of truth for contributions via pull request).
const PROJECT_JSON: &str = include_str!("../../assets/map/fh6-road-types.json");
/// User point ids start here; game nav ids stay below.
pub const USER_ID0: u32 = 1_000_000;

/// Edge / link type. Index order = the editor's brush keys 1..9 (0 = not set).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoadType {
    Road,
    Offroad,
    Other,
    Trail,
    Crosscountry,
    Tunnel,
    Jump,
    Highway,
    Turnaround,
}

impl RoadType {
    pub const ALL: [RoadType; 9] = [
        RoadType::Road,
        RoadType::Offroad,
        RoadType::Other,
        RoadType::Trail,
        RoadType::Crosscountry,
        RoadType::Tunnel,
        RoadType::Jump,
        RoadType::Highway,
        RoadType::Turnaround,
    ];

    /// 1..=9 (the editor's / 3D page's type index; 0 is "not set").
    pub fn index(self) -> u8 {
        Self::ALL.iter().position(|&t| t == self).unwrap() as u8 + 1
    }

    /// The name used in the files.
    pub fn name(self) -> &'static str {
        match self {
            RoadType::Road => "road",
            RoadType::Offroad => "offroad",
            RoadType::Other => "other",
            RoadType::Trail => "trail",
            RoadType::Crosscountry => "crosscountry",
            RoadType::Tunnel => "tunnel",
            RoadType::Jump => "jump",
            RoadType::Highway => "highway",
            RoadType::Turnaround => "turnaround",
        }
    }

    pub fn from_name(s: &str) -> Option<RoadType> {
        Self::ALL.into_iter().find(|t| t.name() == s)
    }

    /// Inverse of [`RoadType::index`] (`0` and out-of-range → `None`).
    pub fn from_index(i: u8) -> Option<RoadType> {
        (i as usize).checked_sub(1).and_then(|k| Self::ALL.get(k).copied())
    }
}

/// `nav` block: which nav file the ids belong to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavRef {
    pub file: String,
    pub sha1: String,
    pub nodes: u32,
}

/// An undirected edge between two node ids, `a < b`; text form `"a-b"`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct EdgeKey {
    pub a: u32,
    pub b: u32,
}

impl EdgeKey {
    /// Normalised (smaller id first).
    pub fn new(a: u32, b: u32) -> EdgeKey {
        EdgeKey { a: a.min(b), b: a.max(b) }
    }
}

impl std::fmt::Display for EdgeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.a, self.b)
    }
}

impl FromStr for EdgeKey {
    type Err = ();
    fn from_str(s: &str) -> Result<EdgeKey, ()> {
        let (a, b) = s.split_once('-').ok_or(())?;
        Ok(EdgeKey::new(a.parse().map_err(|_| ())?, b.parse().map_err(|_| ())?))
    }
}

/// A user link between any two points (`ty` = `None`: not set yet).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddedLink {
    pub a: u32,
    pub b: u32,
    pub ty: Option<RoadType>,
}

/// A parsed road-type file. Entry order is the file's (see the module docs).
#[derive(Clone, Debug)]
pub struct RoadTypes {
    pub version: u32,
    pub nav: Option<NavRef>,
    /// `"based_on"`: SHA-1 of the project file this (override) file was saved from.
    pub based_on: Option<String>,
    /// Typed game edges (unset edges are absent).
    pub types: HashMap<EdgeKey, RoadType>,
    /// File order of `types` (the writer's order; editor edge order = nav polyline order).
    pub types_order: Vec<EdgeKey>,
    pub added: Vec<AddedLink>,
    /// User points: id (>= [`USER_ID0`]) → `[x, z, height]`.
    pub points: BTreeMap<u32, [f64; 3]>,
    /// Position overrides of game nodes: id → `[x, z, height]`.
    pub moved: BTreeMap<u32, [f64; 3]>,
    /// Game edges that were deleted.
    pub removed: Vec<EdgeKey>,
    /// Jump edge → take-off node id, in file order.
    pub jump_from: Vec<(EdgeKey, u32)>,
    /// Race marks: **opaque passthrough**, never interpreted here (the file has ~94 and they
    /// must survive every load → save). Route id → kind, in file order.
    pub races: Vec<(String, Value)>,
    /// The informational `counts` block, verbatim.
    pub counts: Option<Box<RawValue>>,
    /// First few things skipped while parsing (unknown type name, bad key, id out of range…).
    pub warnings: Vec<String>,
    /// How many entries were skipped in total (`warnings` is capped).
    pub skipped: usize,
}

/// Where [`Current::types`] came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Project,
    Override,
}

/// The road types the app should use now ("current"): see [`RoadTypes::current`].
#[derive(Clone, Debug)]
pub struct Current {
    pub types: RoadTypes,
    pub source: Source,
    /// Human-readable explanation when something was ignored or off (shown in Setup).
    pub note: Option<String>,
    /// The override was saved from an older project file than the one embedded now.
    pub project_updated_since_save: bool,
}

// ------------------------------------------------------------------------------------------------ parsing

/// A JSON object read as an ordered list (the editor's key order matters for the writer).
#[derive(Default)]
struct Ordered<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Ordered<T>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Ordered<T>, A::Error> {
                let mut v = Vec::with_capacity(m.size_hint().unwrap_or(0).min(1 << 20));
                while let Some(kv) = m.next_entry::<String, T>()? {
                    v.push(kv);
                }
                Ok(Ordered(v))
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

#[derive(Deserialize)]
struct Raw {
    format: Option<String>,
    version: Option<u32>,
    #[serde(default)]
    nav: Option<Value>,
    #[serde(default)]
    based_on: Option<String>,
    #[serde(default)]
    types: Ordered<Value>,
    #[serde(default)]
    added: Vec<Value>,
    #[serde(default)]
    points: Ordered<Value>,
    #[serde(default)]
    moved: Ordered<Value>,
    #[serde(default)]
    removed: Vec<Value>,
    #[serde(default)]
    jump_from: Ordered<Value>,
    #[serde(default)]
    races: Ordered<Value>,
    #[serde(default)]
    counts: Option<Box<RawValue>>,
}

const MAX_WARNINGS: usize = 50;

impl RoadTypes {
    fn empty(version: u32) -> RoadTypes {
        RoadTypes {
            version,
            nav: None,
            based_on: None,
            types: HashMap::new(),
            types_order: Vec::new(),
            added: Vec::new(),
            points: BTreeMap::new(),
            moved: BTreeMap::new(),
            removed: Vec::new(),
            jump_from: Vec::new(),
            races: Vec::new(),
            counts: None,
            warnings: Vec::new(),
            skipped: 0,
        }
    }

    fn skip(&mut self, why: String) {
        self.skipped += 1;
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(why);
        }
    }

    /// Parse a road-type file (v1 or v2). Errors only on malformed JSON or a wrong
    /// `format` / `version`; bad entries are skipped and reported in `warnings`.
    pub fn parse(json: &str) -> Result<RoadTypes, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| format!("not a road-type file: {e}"))?;
        if raw.format.as_deref() != Some("fh6-road-types") {
            return Err("not a road-type file (format is not \"fh6-road-types\")".into());
        }
        let version = match raw.version {
            Some(v @ (1 | 2)) => v,
            other => return Err(format!("unsupported road-type file version {other:?}")),
        };
        let mut rt = RoadTypes::empty(version);
        rt.based_on = raw.based_on;
        rt.counts = raw.counts;
        if let Some(n) = raw.nav.filter(|n| !n.is_null()) {
            match serde_json::from_value::<NavRef>(n) {
                Ok(n) => rt.nav = Some(n),
                Err(e) => rt.skip(format!("nav block unreadable: {e}")),
            }
        }
        for (k, v) in raw.types.0 {
            let ty = v.as_str().and_then(RoadType::from_name);
            match (k.parse::<EdgeKey>(), ty) {
                (Ok(key), Some(ty)) => {
                    if rt.types.insert(key, ty).is_none() {
                        rt.types_order.push(key);
                    }
                }
                (Err(()), _) => rt.skip(format!("types: bad edge key {k:?}")),
                (_, None) => rt.skip(format!("types: unknown type {v} for {k}")),
            }
        }
        for q in raw.added {
            let id = |f: &str| q.get(f).and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok());
            let ty = match q.get("type") {
                None | Some(Value::Null) => Some(None),
                Some(Value::String(s)) if s == "unset" => Some(None),
                Some(Value::String(s)) => RoadType::from_name(s).map(Some),
                _ => None,
            };
            match (id("a"), id("b"), ty) {
                (Some(a), Some(b), Some(ty)) if a != b => {
                    let k = EdgeKey::new(a, b);
                    rt.added.push(AddedLink { a: k.a, b: k.b, ty });
                }
                _ => rt.skip(format!("added: bad link {q}")),
            }
        }
        let xyz = |v: &Value| -> Option<[f64; 3]> {
            let a = v.as_array()?;
            let n: Vec<f64> = a.iter().take(3).filter_map(Value::as_f64).filter(|x| x.is_finite()).collect();
            (a.len() >= 3 && n.len() == 3).then(|| [n[0], n[1], n[2]])
        };
        for (k, v) in raw.points.0 {
            match (k.parse::<u32>(), xyz(&v)) {
                (Ok(id), Some(p)) if id >= USER_ID0 => {
                    rt.points.insert(id, p);
                }
                _ => rt.skip(format!("points: bad entry {k:?}")),
            }
        }
        for (k, v) in raw.moved.0 {
            match (k.parse::<u32>(), xyz(&v)) {
                (Ok(id), Some(p)) if id < USER_ID0 => {
                    rt.moved.insert(id, p);
                }
                _ => rt.skip(format!("moved: bad entry {k:?}")),
            }
        }
        for v in raw.removed {
            match v.as_str().and_then(|s| s.parse::<EdgeKey>().ok()) {
                Some(k) => rt.removed.push(k),
                None => rt.skip(format!("removed: bad edge {v}")),
            }
        }
        for (k, v) in raw.jump_from.0 {
            match (k.parse::<EdgeKey>(), v.as_u64().and_then(|v| u32::try_from(v).ok())) {
                (Ok(key), Some(from)) => rt.jump_from.push((key, from)),
                _ => rt.skip(format!("jump_from: bad entry {k:?}")),
            }
        }
        rt.races = raw.races.0;
        Ok(rt)
    }

    /// The embedded project file (`assets/map/fh6-road-types.json`).
    pub fn project() -> RoadTypes {
        Self::parse(PROJECT_JSON).expect("embedded project road-type file is valid (tested)")
    }

    /// The embedded project file's text (served by the map editor as `project.json`).
    pub fn project_json() -> &'static str {
        PROJECT_JSON
    }

    /// SHA-1 (hex) of the embedded project file: what Save stamps into `based_on`.
    pub fn project_sha1() -> String {
        hex(&Sha1::digest(PROJECT_JSON.as_bytes()))
    }

    /// The bare nav graph: every edge unset, no points / links / moves / removals, no race
    /// marks, no nav reference.
    pub fn raw() -> RoadTypes {
        Self::empty(2)
    }

    /// Does this file belong to `nav` (SHA-1 and node count — the editor's `navOk`)? A file
    /// without a `nav` block counts as matching, like the editor does.
    pub fn nav_matches(&self, nav: &Nav) -> bool {
        self.nav.as_ref().is_none_or(|n| n.sha1 == nav.sha1 && n.nodes as usize == nav.nodes)
    }

    /// The text for the Save endpoint: parse, then require a `nav` block that matches `nav`
    /// and no skipped entries. `Err` is a message for the editor's status line.
    pub fn validate_for_save(text: &str, nav: &Nav) -> Result<RoadTypes, String> {
        let rt = Self::parse(text)?;
        match &rt.nav {
            None => return Err("the file has no nav block".into()),
            Some(n) if n.sha1 != nav.sha1 || n.nodes as usize != nav.nodes => {
                return Err("the file belongs to a different game road network (nav mismatch)".into())
            }
            _ => {}
        }
        if rt.skipped > 0 {
            return Err(format!("{} invalid entries, e.g. {}", rt.skipped, rt.warnings.first().map_or("?", |s| s)));
        }
        Ok(rt)
    }

    /// D57 "current": the embedded project file, replaced wholesale by the user's override when
    /// that is valid for `nav`. Never deletes or modifies the override file; anything wrong
    /// with it ends up in [`Current::note`] and the project data is used instead.
    pub fn current(user_file: &Path, nav: &Nav) -> Current {
        Self::current_with(Self::project(), &Self::project_sha1(), user_file, nav)
    }

    /// [`RoadTypes::current`] with the project data given (tests use their own).
    pub fn current_with(project: RoadTypes, project_sha1: &str, user_file: &Path, nav: &Nav) -> Current {
        let mut notes: Vec<String> = Vec::new();
        let mut over: Option<RoadTypes> = None;
        match std::fs::read_to_string(user_file) {
            Ok(text) => match Self::parse(&text) {
                Ok(rt) if rt.nav_matches(nav) => over = Some(rt),
                Ok(_) => notes.push(
                    "Your saved road types were ignored: they were made for a different version of the game's road network. The project data is used instead.".into(),
                ),
                Err(e) => notes.push(format!("Your saved road types were ignored ({e}). The project data is used instead.")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => notes.push(format!("Your saved road types could not be read ({e}). The project data is used instead.")),
        }
        let (types, source) = match over {
            Some(o) => (o, Source::Override),
            None => (project, Source::Project),
        };
        if source == Source::Project && !types.nav_matches(nav) {
            notes.push("The project road-type data is for a different game version than your install; it is still used, so some edges may be missing or wrong.".into());
        }
        let project_updated_since_save = source == Source::Override && types.based_on.as_deref().is_some_and(|b| b != project_sha1);
        Current { types, source, note: (!notes.is_empty()).then(|| notes.join(" ")), project_updated_since_save }
    }
}

/// Where the user's saved road types live: `<app_data_dir>/map_editor/fh6-road-types.user.json`.
pub fn override_path() -> PathBuf {
    crate::config::app_data_dir().join("map_editor").join("fh6-road-types.user.json")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

// ------------------------------------------------------------------------------------------------ writing

/// A number the way JS `JSON.stringify` prints it: integral values without `.0`.
fn num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        serde_json::to_string(&v).unwrap_or_else(|_| "0".into())
    }
}

fn xyz_json(p: &[f64; 3]) -> String {
    format!("[{},{},{}]", num(p[0]), num(p[1]), num(p[2]))
}

fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

impl RoadTypes {
    /// The version-2 file text, line for line what `fix_highways.py write_v2` and the editor's
    /// `exportText` produce (one entry per line, trailing newline). `counts` is written as
    /// parsed (informational; it is not recomputed). `based_on`, when set, is written on its own
    /// line after `nav`.
    pub fn to_json_string(&self) -> String {
        let mut l: Vec<String> = vec!["{\"format\":\"fh6-road-types\",\"version\":2,".into()];
        l.push(format!("\"nav\":{},", self.nav.as_ref().map_or("null".into(), |n| serde_json::to_string(n).unwrap_or_else(|_| "null".into()))));
        if let Some(b) = &self.based_on {
            l.push(format!("\"based_on\":{},", js(b)));
        }
        fn block(l: &mut Vec<String>, name: &str, items: Vec<(String, String)>) {
            l.push(format!("\"{name}\":{{"));
            let n = items.len();
            for (i, (k, v)) in items.into_iter().enumerate() {
                l.push(format!("{}:{}{}", js(&k), v, if i + 1 < n { "," } else { "" }));
            }
            l.push("},".into());
        }
        // types: file order first, then anything added since (sorted)
        let mut seen = HashSet::new();
        let mut keys: Vec<EdgeKey> = self.types_order.iter().copied().filter(|k| self.types.contains_key(k) && seen.insert(*k)).collect();
        let mut extra: Vec<EdgeKey> = self.types.keys().copied().filter(|k| !seen.contains(k)).collect();
        extra.sort();
        keys.extend(extra);
        block(&mut l, "types", keys.iter().map(|k| (k.to_string(), js(self.types[k].name()))).collect());
        l.push("\"added\":[".into());
        let n = self.added.len();
        for (i, q) in self.added.iter().enumerate() {
            let ty = q.ty.map_or("null".to_owned(), |t| js(t.name()));
            l.push(format!("{{\"a\":{},\"b\":{},\"type\":{}}}{}", q.a, q.b, ty, if i + 1 < n { "," } else { "" }));
        }
        l.push("],".into());
        block(&mut l, "points", self.points.iter().map(|(k, p)| (k.to_string(), xyz_json(p))).collect());
        block(&mut l, "moved", self.moved.iter().map(|(k, p)| (k.to_string(), xyz_json(p))).collect());
        let removed: Vec<String> = self.removed.iter().map(|k| js(&k.to_string())).collect();
        l.push(format!("\"removed\":[{}],", removed.join(",")));
        block(&mut l, "jump_from", self.jump_from.iter().map(|(k, f)| (k.to_string(), f.to_string())).collect());
        block(&mut l, "races", self.races.iter().map(|(k, v)| (k.clone(), serde_json::to_string(v).unwrap_or_else(|_| "null".into()))).collect());
        l.push(format!("\"counts\":{}}}", self.counts.as_ref().map_or("{}", |c| c.get())));
        l.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT_SHA1_NAV: &str = "a88c69f49c16e8aa86b883cd978328d737144202";

    fn fake_nav(sha1: &str, nodes: usize) -> Nav {
        Nav { sha1: sha1.into(), nodes, polys: vec![], cls: vec![], hi: vec![], orphans: vec![] }
    }

    fn project_nav() -> Nav {
        fake_nav(PROJECT_SHA1_NAV, 38473)
    }

    #[test]
    fn project_file_numbers() {
        let p = RoadTypes::project();
        assert_eq!(p.version, 2);
        let nav = p.nav.as_ref().expect("nav");
        assert_eq!((nav.file.as_str(), nav.sha1.as_str(), nav.nodes), ("Brio_00.nav", PROJECT_SHA1_NAV, 38473));
        assert_eq!(p.types.len(), 39382);
        assert_eq!(p.types_order.len(), 39382);
        assert_eq!(p.added.len(), 218);
        assert_eq!(p.points.len(), 124);
        assert!(p.points.keys().all(|&id| id >= USER_ID0));
        assert_eq!(p.moved.len(), 1);
        assert_eq!(p.removed.len(), 1);
        assert_eq!(p.jump_from.len(), 18);
        assert_eq!(p.races.len(), 94, "race marks are an opaque passthrough but must all be kept");
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.skipped, 0);
        assert!(p.based_on.is_none());
        assert!(p.nav_matches(&project_nav()));
        assert!(!p.nav_matches(&fake_nav("deadbeef", 38473)));
        assert!(!p.nav_matches(&fake_nav(PROJECT_SHA1_NAV, 5)));
    }

    #[test]
    fn project_file_uses_all_nine_types() {
        let p = RoadTypes::project();
        let mut seen: HashSet<RoadType> = p.types.values().copied().collect();
        seen.extend(p.added.iter().filter_map(|q| q.ty));
        for t in RoadType::ALL {
            assert!(seen.contains(&t), "{t:?} is not used by the project file");
        }
    }

    #[test]
    fn type_names_and_indices() {
        let names: Vec<&str> = RoadType::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["road", "offroad", "other", "trail", "crosscountry", "tunnel", "jump", "highway", "turnaround"]);
        for (i, t) in RoadType::ALL.into_iter().enumerate() {
            assert_eq!(t.index() as usize, i + 1);
            assert_eq!(RoadType::from_name(t.name()), Some(t));
            assert_eq!(RoadType::from_index(t.index()), Some(t));
        }
        assert_eq!(RoadType::from_index(0), None);
        assert_eq!(RoadType::from_index(10), None);
        assert_eq!(RoadType::from_name("unset"), None);
    }

    /// parse -> write of the embedded file is byte-identical: proves the one-entry-per-line
    /// writer matches `fix_highways.py write_v2` / the editor's Export, entry order included.
    #[test]
    fn roundtrip_is_byte_identical() {
        let p = RoadTypes::project();
        let out = p.to_json_string();
        assert_eq!(out.len(), PROJECT_JSON.len());
        if out != PROJECT_JSON {
            let at = out.bytes().zip(PROJECT_JSON.bytes()).position(|(a, b)| a != b).unwrap();
            panic!("first difference at byte {at}: {:?} vs {:?}", &out[at.saturating_sub(40)..(at + 40).min(out.len())], &PROJECT_JSON[at.saturating_sub(40)..(at + 40).min(PROJECT_JSON.len())]);
        }
        // and the output parses back to the same thing
        let q = RoadTypes::parse(&out).unwrap();
        assert_eq!((q.types.len(), q.added.len(), q.points.len(), q.races.len()), (39382, 218, 124, 94));
    }

    #[test]
    fn v1_sample_parses() {
        let t = r#"{"format":"fh6-road-types","version":1,"nav":{"file":"Brio_00.nav","sha1":"abc","nodes":3},"types":{"1-2":"road","3-2":"offroad","4-5":"other"}}"#;
        let r = RoadTypes::parse(t).unwrap();
        assert_eq!(r.version, 1);
        assert_eq!(r.types.len(), 3);
        assert_eq!(r.types[&EdgeKey::new(2, 3)], RoadType::Offroad);
        assert!(r.added.is_empty() && r.points.is_empty() && r.moved.is_empty() && r.removed.is_empty() && r.jump_from.is_empty() && r.races.is_empty());
        assert!(r.warnings.is_empty());
        // writing a v1 file upgrades it to a v2 layout
        assert!(r.to_json_string().starts_with("{\"format\":\"fh6-road-types\",\"version\":2,\n\"nav\":"));
    }

    #[test]
    fn v2_sample_parses_all_blocks() {
        let t = r#"{"format":"fh6-road-types","version":2,"nav":null,
"types":{"1-2":"highway","2-3":"turnaround","3-4":"tunnel","5-4":"trail"},
"added":[{"a":9,"b":1000000,"type":"crosscountry"},{"a":2,"b":1,"type":null},{"a":3,"b":4,"type":"jump"}],
"points":{"1000000":[10,20.5,30],"1000001":[-1.25,2,3.5]},
"moved":{"7":[1,2,3]},
"removed":["8-9","2-1"],
"jump_from":{"3-4":4},
"races":{"41":"rally","2":"story"},
"counts":{"z":1,"a":2}}"#;
        let r = RoadTypes::parse(t).unwrap();
        assert_eq!(r.version, 2);
        assert!(r.nav.is_none());
        assert_eq!(r.types[&EdgeKey::new(4, 5)], RoadType::Trail);
        assert_eq!(r.types_order, vec![EdgeKey::new(1, 2), EdgeKey::new(2, 3), EdgeKey::new(3, 4), EdgeKey::new(4, 5)]);
        assert_eq!(r.added.len(), 3);
        assert_eq!(r.added[0], AddedLink { a: 9, b: 1_000_000, ty: Some(RoadType::Crosscountry) });
        assert_eq!(r.added[1], AddedLink { a: 1, b: 2, ty: None }, "a < b normalised, null = unset");
        assert_eq!(r.points[&1_000_001], [-1.25, 2.0, 3.5]);
        assert_eq!(r.moved[&7], [1.0, 2.0, 3.0]);
        assert_eq!(r.removed, vec![EdgeKey::new(8, 9), EdgeKey::new(1, 2)]);
        assert_eq!(r.jump_from, vec![(EdgeKey::new(3, 4), 4)]);
        assert_eq!(r.races.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), vec!["41", "2"], "race order kept");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        // writer: integral coordinates print without ".0", counts verbatim (key order kept), races untouched
        let out = r.to_json_string();
        assert!(out.contains("\"1000000\":[10,20.5,30],\n"), "{out}");
        assert!(out.contains("\"races\":{\n\"41\":\"rally\",\n\"2\":\"story\"\n},\n\"counts\":{\"z\":1,\"a\":2}}\n"), "{out}");
        assert!(out.contains("\"removed\":[\"8-9\",\"1-2\"],\n"));
        assert!(out.contains("{\"a\":1,\"b\":2,\"type\":null},\n"));
        // the written text parses back to the same data
        let r2 = RoadTypes::parse(&out).unwrap();
        assert_eq!((r2.types.len(), r2.added.len(), r2.points.len(), r2.races.len()), (4, 3, 2, 2));
    }

    #[test]
    fn lenient_parse_skips_and_warns() {
        let t = r#"{"format":"fh6-road-types","version":2,"types":{"1-2":"warp","x-y":"road","3-4":"road","5-6":7},
"added":[{"a":1,"b":1,"type":"road"},{"a":1,"b":2,"type":"nope"},{"b":2},"junk"],
"points":{"5":[1,2,3],"1000000":[1,2],"1000001":[1,2,3]},"moved":{"1000005":[1,2,3],"6":[1,2,"x"]},
"removed":["nope",3],"jump_from":{"1-2":"x"},"races":{}}"#;
        let r = RoadTypes::parse(t).unwrap();
        assert_eq!(r.types.len(), 1);
        assert!(r.added.is_empty());
        assert_eq!(r.points.len(), 1);
        assert!(r.moved.is_empty() && r.removed.is_empty() && r.jump_from.is_empty());
        assert_eq!(r.skipped, 3 + 4 + 2 + 2 + 2 + 1);
        assert!(r.warnings.iter().any(|w| w.contains("warp")));
    }

    #[test]
    fn rejects_wrong_format_and_version() {
        assert!(RoadTypes::parse("not json").is_err());
        assert!(RoadTypes::parse(r#"{"format":"other","version":2}"#).is_err());
        assert!(RoadTypes::parse(r#"{"format":"fh6-road-types","version":3}"#).is_err());
        assert!(RoadTypes::parse(r#"{"format":"fh6-road-types"}"#).is_err());
        assert!(RoadTypes::parse(r#"{"format":"fh6-road-types","version":2,"types":[]}"#).is_err());
    }

    #[test]
    fn raw_has_no_types() {
        let r = RoadTypes::raw();
        assert!(r.types.is_empty() && r.added.is_empty() && r.points.is_empty() && r.moved.is_empty());
        assert!(r.removed.is_empty() && r.jump_from.is_empty() && r.races.is_empty() && r.nav.is_none());
        assert!(r.nav_matches(&project_nav()));
    }

    fn user_file(dir: &Path, text: &str) -> PathBuf {
        let f = dir.join("user.json");
        std::fs::write(&f, text).unwrap();
        f
    }

    fn small_project() -> RoadTypes {
        RoadTypes::parse(r#"{"format":"fh6-road-types","version":2,"nav":{"file":"Brio_00.nav","sha1":"AAA","nodes":10},"types":{"1-2":"road"},"races":{"1":"rally"}}"#).unwrap()
    }

    fn override_text(sha: &str, extra: &str) -> String {
        format!(r#"{{"format":"fh6-road-types","version":2,"nav":{{"file":"Brio_00.nav","sha1":"{sha}","nodes":10}},{extra}"types":{{"3-4":"highway"}},"races":{{"9":"drag"}}}}"#)
    }

    /// The override replaces the project file wholesale (no per-entry merge); anything wrong
    /// with it is ignored with a note and never deleted.
    #[test]
    fn current_precedence() {
        let dir = crate::gamedata::tempdir("rtcurrent");
        let nav = fake_nav("AAA", 10);
        let (proj, psha) = (small_project(), "proj-sha");
        // 1. no override file -> project, no note
        let c = RoadTypes::current_with(proj.clone(), psha, &dir.join("missing.json"), &nav);
        assert_eq!(c.source, Source::Project);
        assert!(c.note.is_none() && !c.project_updated_since_save);
        assert_eq!(c.types.types.len(), 1);
        // 2. valid override -> replaces: the project's 1-2 edge and race mark are GONE
        let f = user_file(&dir, &override_text("AAA", ""));
        let c = RoadTypes::current_with(proj.clone(), psha, &f, &nav);
        assert_eq!(c.source, Source::Override);
        assert!(c.note.is_none());
        assert_eq!(c.types.types.len(), 1);
        assert_eq!(c.types.types[&EdgeKey::new(3, 4)], RoadType::Highway);
        assert!(!c.types.types.contains_key(&EdgeKey::new(1, 2)));
        assert_eq!(c.types.races.len(), 1);
        assert_eq!(c.types.races[0].0, "9");
        // 3. wrong nav -> ignored with a note, file kept, project used
        let f = user_file(&dir, &override_text("BBB", ""));
        let c = RoadTypes::current_with(proj.clone(), psha, &f, &nav);
        assert_eq!(c.source, Source::Project);
        assert!(c.note.as_deref().unwrap().contains("different version"), "{:?}", c.note);
        assert!(f.exists());
        // 4. garbage -> ignored with a note, file kept
        let f = user_file(&dir, "this is not json");
        let c = RoadTypes::current_with(proj.clone(), psha, &f, &nav);
        assert_eq!(c.source, Source::Project);
        assert!(c.note.as_deref().unwrap().contains("ignored"));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "this is not json");
        // 5. wrong format / version -> ignored
        let f = user_file(&dir, r#"{"format":"fh6-road-types","version":9}"#);
        assert_eq!(RoadTypes::current_with(proj.clone(), psha, &f, &nav).source, Source::Project);
        // 6. project for another nav version -> still used, with a note
        let c = RoadTypes::current_with(proj.clone(), psha, &dir.join("missing.json"), &fake_nav("CCC", 10));
        assert_eq!(c.source, Source::Project);
        assert!(c.note.as_deref().unwrap().contains("different game version"));
        // 7. both problems -> both notes
        let f = user_file(&dir, "{");
        let c = RoadTypes::current_with(proj, psha, &f, &fake_nav("CCC", 10));
        let note = c.note.unwrap();
        assert!(note.contains("ignored") && note.contains("different game version"), "{note}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn based_on_marks_updated_project() {
        let dir = crate::gamedata::tempdir("rtbased");
        let nav = fake_nav("AAA", 10);
        let flag = |extra: &str, psha: &str| {
            let f = user_file(&dir, &override_text("AAA", extra));
            RoadTypes::current_with(small_project(), psha, &f, &nav).project_updated_since_save
        };
        assert!(!flag("", "p2"), "missing based_on -> false");
        assert!(!flag(r#""based_on":"p2","#, "p2"), "same project -> false");
        assert!(flag(r#""based_on":"p1","#, "p2"), "older project -> true");
        // writer keeps it
        let t = RoadTypes::parse(&override_text("AAA", r#""based_on":"p1","#)).unwrap();
        assert_eq!(t.based_on.as_deref(), Some("p1"));
        assert!(t.to_json_string().contains("\n\"based_on\":\"p1\",\n\"types\":{"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn project_sha_is_stable_hex() {
        let s = RoadTypes::project_sha1();
        assert_eq!(s.len(), 40);
        assert!(s.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(s, RoadTypes::project_sha1());
    }

    #[test]
    fn validate_for_save_is_strict() {
        let nav = fake_nav("AAA", 10);
        assert!(RoadTypes::validate_for_save(&override_text("AAA", ""), &nav).is_ok());
        assert!(RoadTypes::validate_for_save(&override_text("BBB", ""), &nav).unwrap_err().contains("nav"));
        assert!(RoadTypes::validate_for_save(r#"{"format":"fh6-road-types","version":2,"types":{}}"#, &nav).is_err(), "no nav block");
        let bad = r#"{"format":"fh6-road-types","version":2,"nav":{"file":"Brio_00.nav","sha1":"AAA","nodes":10},"types":{"1-2":"warp"}}"#;
        assert!(RoadTypes::validate_for_save(bad, &nav).unwrap_err().contains("invalid"));
        assert!(RoadTypes::validate_for_save("nope", &nav).is_err());
    }

    #[test]
    fn override_path_is_under_app_data() {
        let p = override_path();
        assert!(p.ends_with("map_editor/fh6-road-types.user.json") || p.ends_with("map_editor\\fh6-road-types.user.json"));
    }

    /// With the real nav of the install: the project file's nav block matches it, so `current`
    /// with no override is the project, without a note.
    #[test]
    fn real_install_project_matches_nav() {
        let Some(media) = crate::gamedata::install::find_media(None) else {
            eprintln!("SKIP real_install_project_matches_nav: FH6 install not found");
            return;
        };
        let nav = Nav::load(&media).expect("nav");
        let dir = crate::gamedata::tempdir("rtreal");
        let c = RoadTypes::current(&dir.join("none.json"), &nav);
        assert_eq!(c.source, Source::Project);
        assert!(c.note.is_none(), "{:?}", c.note);
        // every typed game edge of the project file is an edge of the real nav graph
        let edges: HashSet<(u32, u32)> = nav.edges().into_iter().collect();
        let missing = c.types.types.keys().filter(|k| !edges.contains(&(k.a, k.b))).count();
        assert_eq!(missing, 0, "typed edges that are not in the nav graph");
        // the file's edge order = the nav's first-seen edge order (except for edges removed / re-typed)
        let nav_edges = nav.edges();
        let order: Vec<(u32, u32)> = c.types.types_order.iter().map(|k| (k.a, k.b)).collect();
        let mut it = nav_edges.iter();
        assert!(order.iter().all(|e| it.any(|n| n == e)), "types order does not follow the nav edge order");
        std::fs::remove_dir_all(dir).ok();
    }
}
