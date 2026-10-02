//! `CarOrdinal` -> car make + model, read from the user's own FH6 install at runtime.
//!
//! No shipped table, no key: see `docs/game-data/fh6-cars-names-icons.md` section 1.
//! 1. ordinal -> MediaName: every `media/Cars/<MediaName>.zip` holds
//!    `Scene/animations/Mojo/clip/carclips_<ordinal>.clipd` (zip central directory only).
//! 2. ordinal -> names: `media/Stripped/StringTables/<LANG>.zip` -> `Data_Car.str`
//!    (`IDS_ModelShort_<n>` = long name, `IDS_DisplayName_<n>` = model only).
//! 3. make: `List_CarMake.str` names + a heuristic ([`derive_makes`]); no make id exists in plaintext.
//! 4. [`compose_display`] builds the "Make + model" string.
//!
//! The scan is blocking (~0.4 s cold) and the result is cached as JSON in `app_data_dir()`,
//! so call [`CarDb::load`] from a background thread.

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::install::{ci, find_media};
use super::strtable::{parse_str, strhash};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarName {
    pub ordinal: u32,
    /// `media/Cars/<media_name>.zip`, e.g. `HON_21_CivicWTA_92`.
    pub media_name: String,
    /// The game's `IDS_ModelShort_<n>` text (the *long* one, usually with make; may lack it).
    pub model_short: Option<String>,
    /// The game's `IDS_DisplayName_<n>` text (model only).
    pub display_name: Option<String>,
    /// Heuristic make from `List_CarMake.str`; `None` when nothing plausible matched.
    pub make: Option<String>,
    /// "Make + model" for the UI.
    pub display: String,
}

pub struct CarDb {
    cars: HashMap<u32, CarName>,
    /// The `media` dir the data came from.
    pub media: PathBuf,
    pub lang: String,
    /// True when served from the JSON cache instead of scanning the install.
    pub from_cache: bool,
}

impl CarDb {
    /// Load for `lang` (`"EN"`, `"DE"`, ... a `Stripped/StringTables/<LANG>.zip` name), auto-detecting
    /// the install. Falls back to EN when the language zip is missing. Blocking; use a thread.
    pub fn load(lang: &str) -> Result<CarDb, String> {
        Self::load_from(None, lang)
    }

    /// As [`CarDb::load`] with an explicit install path (game folder or its `media` folder).
    pub fn load_from(install: Option<&Path>, lang: &str) -> Result<CarDb, String> {
        let media = find_media(install)
            .ok_or_else(|| "Forza Horizon 6 install not found (set FH6_INSTALL_DIR)".to_string())?;
        let mut lang = lang.to_ascii_uppercase();
        if string_zip(&media, &lang).is_none() {
            lang = "EN".into();
        }
        let zip = string_zip(&media, &lang).ok_or("StringTables zip not found")?;
        let stamp = fingerprint(&media, &zip);
        let cache = cache_path(&lang);
        if let Some(cars) = read_cache(&cache, &media, &stamp) {
            return Ok(CarDb { cars, media, lang, from_cache: true });
        }
        let cars = scan(&media, &zip)?;
        write_cache(&cache, &media, &stamp, &cars);
        Ok(CarDb { cars, media, lang, from_cache: false })
    }

    pub fn lookup(&self, ordinal: u32) -> Option<&CarName> {
        self.cars.get(&ordinal)
    }

    pub fn len(&self) -> usize {
        self.cars.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &CarName> {
        self.cars.values()
    }
}

// ── scanning ─────────────────────────────────────────────────────────────────────────────────

fn string_zip(media: &Path, lang: &str) -> Option<PathBuf> {
    ci(media, &format!("Stripped/StringTables/{lang}.zip")).filter(|p| p.is_file())
}

/// `carclips_<digits>.clipd` -> ordinal.
fn ordinal_of_entry(name: &str) -> Option<u32> {
    let l = name.to_ascii_lowercase();
    let rest = l.rsplit('/').next()?.strip_prefix("carclips_")?.strip_suffix(".clipd")?;
    rest.parse().ok()
}

/// `{ordinal: MediaName}` from the central directories of `media/Cars/*.zip`.
pub fn ordinal_map(media: &Path) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let Some(dir) = ci(media, "Cars") else { return out };
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref() != Some("zip") {
            continue;
        }
        let Ok(f) = std::fs::File::open(&p) else { continue };
        let Ok(z) = zip::ZipArchive::new(BufReader::new(f)) else { continue };
        let media_name = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        for n in z.file_names() {
            if let Some(o) = ordinal_of_entry(n) {
                out.insert(o, media_name.clone());
            }
        }
    }
    out
}

fn read_table(z: &mut zip::ZipArchive<BufReader<std::fs::File>>, table: &str) -> Result<HashMap<u32, String>, String> {
    use std::io::Read;
    let want = format!("{table}.str").to_ascii_lowercase();
    let name = z
        .file_names()
        .find(|n| n.to_ascii_lowercase() == want)
        .map(str::to_owned)
        .ok_or_else(|| format!("{table}.str missing in string table zip"))?;
    let mut buf = Vec::new();
    z.by_name(&name).map_err(|e| e.to_string())?.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(parse_str(&buf))
}

fn scan(media: &Path, zip_path: &Path) -> Result<HashMap<u32, CarName>, String> {
    let om = ordinal_map(media);
    if om.is_empty() {
        return Err("no car zips found under media/Cars".into());
    }
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(BufReader::new(f)).map_err(|e| e.to_string())?;
    let data_car = read_table(&mut z, "Data_Car")?;
    let make_tab = read_table(&mut z, "List_CarMake").unwrap_or_default();
    // Make ids are unknown (encrypted gamedb); recover the ids by hashing `IDS_DisplayName_<id>`.
    let mut makes: Vec<String> = (0..2000u32)
        .filter_map(|n| make_tab.get(&strhash(&format!("IDS_DisplayName_{n}"))))
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .collect();
    makes.sort();
    makes.dedup();

    let mut inputs: Vec<CarInput> = om
        .into_iter()
        .map(|(o, media_name)| CarInput {
            ordinal: o,
            media_name,
            model_short: data_car.get(&strhash(&format!("IDS_ModelShort_{o}"))).cloned(),
            display_name: data_car.get(&strhash(&format!("IDS_DisplayName_{o}"))).cloned(),
        })
        .collect();
    inputs.sort_by_key(|c| c.ordinal);
    let makes_by_car = derive_makes(&inputs, &makes);
    Ok(inputs
        .into_iter()
        .map(|c| {
            let make = makes_by_car.get(&c.ordinal).cloned();
            let display = compose_display(
                make.as_deref(),
                c.model_short.as_deref(),
                c.display_name.as_deref(),
                &c.media_name,
            );
            (
                c.ordinal,
                CarName {
                    ordinal: c.ordinal,
                    media_name: c.media_name,
                    model_short: c.model_short,
                    display_name: c.display_name,
                    make,
                    display,
                },
            )
        })
        .collect())
}

// ── make heuristic ───────────────────────────────────────────────────────────────────────────

pub struct CarInput {
    pub ordinal: u32,
    pub media_name: String,
    pub model_short: Option<String>,
    pub display_name: Option<String>,
}

/// Split a leading race-number tag: `"#21 Civic"` -> `(Some("#21"), "Civic")`.
pub fn split_tag(s: &str) -> (Option<&str>, &str) {
    if let Some(r) = s.strip_prefix('#') {
        let digits = r.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && r[digits..].starts_with(' ') {
            return (Some(&s[..digits + 1]), r[digits..].trim_start());
        }
    }
    (None, s)
}

/// Does `s` (after an optional `#nn ` tag) start with `make` (case-insensitive, whole word)?
pub fn starts_with_make(s: &str, make: &str) -> bool {
    let (_, s) = split_tag(s);
    let (sl, ml) = (s.to_lowercase(), make.to_lowercase());
    sl.strip_prefix(&ml).is_some_and(|rest| !rest.chars().next().is_some_and(char::is_alphanumeric))
}

/// Longest make name that `s` starts with (so `Mercedes-AMG` beats `Mercedes`).
fn prefix_make<'a>(s: &str, makes: &'a [String]) -> Option<&'a String> {
    makes.iter().filter(|m| starts_with_make(s, m)).max_by_key(|m| m.len())
}

/// MediaName prefix up to the first `_`, upper-case (`HON_21_..` -> `HON`).
fn media_prefix(media_name: &str) -> String {
    media_name.split('_').next().unwrap_or("").to_ascii_uppercase()
}

/// Letter-subsequence match of a media prefix against make names (first letters equal; fewest
/// skipped letters wins; a tie between different makes -> `None`).
fn subsequence_make<'a>(prefix: &str, makes: &'a [String]) -> Option<&'a String> {
    let letters: Vec<char> = prefix.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_lowercase()).collect();
    if letters.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, &String)> = None;
    let mut tie = false;
    for m in makes {
        let mc: Vec<char> = m.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
        if mc.first() != letters.first() {
            continue;
        }
        let (mut li, mut last) = (0, 0);
        for (i, c) in mc.iter().enumerate() {
            if li < letters.len() && *c == letters[li] {
                li += 1;
                last = i;
            }
        }
        if li < letters.len() {
            continue;
        }
        let score = last + 1 - letters.len();
        match best {
            Some((s, _)) if s < score => {}
            Some((s, _)) if s == score => tie = true,
            _ => {
                best = Some((score, m));
                tie = false;
            }
        }
    }
    if tie { None } else { best.map(|(_, m)| m) }
}

/// Make per ordinal: (a) a make name that prefixes ModelShort/DisplayName; else (b) the majority
/// make of the MediaName prefix over all (a) cars; else (c) a letter-subsequence match of the prefix.
pub fn derive_makes(cars: &[CarInput], makes: &[String]) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let mut votes: HashMap<String, HashMap<&str, u32>> = HashMap::new();
    for c in cars {
        let hit = [&c.model_short, &c.display_name]
            .into_iter()
            .flatten()
            .find_map(|s| prefix_make(s, makes));
        if let Some(m) = hit {
            out.insert(c.ordinal, m.clone());
            *votes.entry(media_prefix(&c.media_name)).or_default().entry(m).or_default() += 1;
        }
    }
    let majority = |p: &str| -> Option<String> {
        votes
            .get(p)?
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) // most votes, then alphabetical
            .map(|(m, _)| m.to_string())
    };
    let mut by_prefix: HashMap<String, Option<String>> = HashMap::new();
    for c in cars {
        if out.contains_key(&c.ordinal) {
            continue;
        }
        let p = media_prefix(&c.media_name);
        let m = by_prefix
            .entry(p.clone())
            .or_insert_with(|| majority(&p).or_else(|| subsequence_make(&p, makes).cloned()))
            .clone();
        if let Some(m) = m {
            out.insert(c.ordinal, m);
        }
    }
    out
}

/// "Make + model". See `docs/game-data/fh6-cars-names-icons.md` for why.
/// - no make: the game's own text alone (ModelShort, else DisplayName, else `fallback`);
/// - text already starts with the make: as is;
/// - `#nn ` tag: tag + make + rest, taking the rest from DisplayName when it carries the same tag
///   (it holds the livery/team part: "#21 Honda Hardrace/JDMYard Civic WTAC");
/// - otherwise "Make ModelShort".
pub fn compose_display(make: Option<&str>, model_short: Option<&str>, display_name: Option<&str>, fallback: &str) -> String {
    let base = model_short.or(display_name).unwrap_or(fallback);
    let Some(make) = make else { return base.to_string() };
    let (tag, rest) = split_tag(base);
    if let Some(tag) = tag {
        let rest = match display_name.map(split_tag) {
            Some((Some(t), r)) if t == tag => r,
            _ => rest,
        };
        return if starts_with_make(rest, make) { format!("{tag} {rest}") } else { format!("{tag} {make} {rest}") };
    }
    if starts_with_make(base, make) { base.to_string() } else { format!("{make} {base}") }
}

// ── cache ────────────────────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct CacheFile {
    media: String,
    stamp: String,
    cars: Vec<CarName>,
}

fn cache_path(lang: &str) -> PathBuf {
    crate::config::app_data_dir().join(format!("car_names_{lang}.json"))
}

fn mtime_secs(p: &Path) -> u64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Changes when a car zip is added/removed/replaced or the string table zip is patched.
fn fingerprint(media: &Path, strings: &Path) -> String {
    let (mut n, mut size, mut newest) = (0u64, 0u64, 0u64);
    if let Some(dir) = ci(media, "Cars") {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if let Ok(m) = e.metadata() {
                n += 1;
                size += m.len();
                newest = newest.max(mtime_secs(&e.path()));
            }
        }
    }
    let sz = std::fs::metadata(strings).map_or(0, |m| m.len());
    format!("v1;{n};{size};{newest};{sz};{}", mtime_secs(strings))
}

fn read_cache(path: &Path, media: &Path, stamp: &str) -> Option<HashMap<u32, CarName>> {
    let c: CacheFile = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (c.media == media.to_string_lossy() && c.stamp == stamp).then(|| c.cars.into_iter().map(|c| (c.ordinal, c)).collect())
}

fn write_cache(path: &Path, media: &Path, stamp: &str, cars: &HashMap<u32, CarName>) {
    let mut v: Vec<CarName> = cars.values().cloned().collect();
    v.sort_by_key(|c| c.ordinal);
    let file = CacheFile { media: media.to_string_lossy().into_owned(), stamp: stamp.to_owned(), cars: v };
    let Ok(json) = serde_json::to_vec(&file) else { return };
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    fn car(o: u32, media: &str, ms: Option<&str>, dn: Option<&str>) -> CarInput {
        CarInput { ordinal: o, media_name: media.into(), model_short: ms.map(Into::into), display_name: dn.map(Into::into) }
    }

    #[test]
    fn tag_split() {
        assert_eq!(split_tag("#21 Civic"), (Some("#21"), "Civic"));
        assert_eq!(split_tag("#Civic"), (None, "#Civic"));
        assert_eq!(split_tag("Civic #2"), (None, "Civic #2"));
    }

    #[test]
    fn make_prefix_is_whole_word_and_longest() {
        let makes = s(&["Mercedes-Benz", "Mercedes-AMG", "GR", "Ram", "MINI"]);
        assert_eq!(prefix_make("Mercedes-AMG GT", &makes).unwrap(), "Mercedes-AMG");
        assert!(prefix_make("Grand Cherokee", &makes).is_none());
        assert!(prefix_make("Ramair", &makes).is_none());
        assert_eq!(prefix_make("Mini Cooper S", &makes).unwrap(), "MINI");
        assert_eq!(prefix_make("#7 GR Yaris", &makes).unwrap(), "GR");
    }

    #[test]
    fn heuristic_steps() {
        let makes = s(&["Honda", "Lamborghini", "Volkswagen", "Volvo", "Aston Martin", "Austin-Healey"]);
        let cars = vec![
            car(1, "HON_Civic_92", Some("Honda Civic '92"), Some("Civic")), // (a)
            car(2, "HON_21_CivicWTA_92", Some("#21 Civic WTAC"), None),     // (b) HON -> Honda
            car(3, "LAM_Countach_88", Some("Lambo Countach"), None),        // (c) LAM -> Lamborghini
            car(4, "VW_Golf_90", Some("VW Golf"), None),                    // (c) VW -> Volkswagen
            car(5, "AST_DB5_64", Some("AM DB5"), None),                     // (c) closest subsequence
            car(6, "343_Warthog_15", Some("M12S Warthog CST"), None),       // none
        ];
        let m = derive_makes(&cars, &makes);
        assert_eq!(m[&1], "Honda");
        assert_eq!(m[&2], "Honda");
        assert_eq!(m[&3], "Lamborghini");
        assert_eq!(m[&4], "Volkswagen");
        assert_eq!(m[&5], "Aston Martin");
        assert!(!m.contains_key(&6));
    }

    #[test]
    fn display_composition() {
        // already starts with the make
        assert_eq!(compose_display(Some("Mazda"), Some("Mazda RX-7 '92"), Some("RX-7 Type R"), "x"), "Mazda RX-7 '92");
        // make missing from ModelShort
        assert_eq!(compose_display(Some("Lamborghini"), Some("Countach '88"), None, "x"), "Lamborghini Countach '88");
        // tag: make inserted after it, rest from DisplayName
        assert_eq!(
            compose_display(Some("Honda"), Some("#21 Civic WTAC"), Some("#21 Hardrace/JDMYard Civic WTAC"), "x"),
            "#21 Honda Hardrace/JDMYard Civic WTAC"
        );
        // tag, no usable DisplayName
        assert_eq!(compose_display(Some("Honda"), Some("#21 Civic WTAC"), None, "x"), "#21 Honda Civic WTAC");
        // tag, make already there
        assert_eq!(compose_display(Some("Honda"), Some("#21 Honda Civic"), None, "x"), "#21 Honda Civic");
        // no make: the game's name alone
        assert_eq!(compose_display(None, Some("M12S Warthog CST"), Some("M12S Warthog CST"), "x"), "M12S Warthog CST");
        assert_eq!(compose_display(None, None, None, "NUL_CAR_00"), "NUL_CAR_00");
    }

    #[test]
    fn entry_names() {
        assert_eq!(ordinal_of_entry("Scene/animations/Mojo/clip/carclips_4277.clipd"), Some(4277));
        assert_eq!(ordinal_of_entry("Scene/animations/Mojo/clip/other.clipd"), None);
    }

    /// Against the real install; skipped (with a note) when FH6 isn't found.
    #[test]
    fn real_install_names() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_names: FH6 install not found");
            return;
        };
        let t0 = std::time::Instant::now();
        let zip = string_zip(&media, "EN").expect("EN.zip");
        let cars = scan(&media, &zip).expect("scan");
        eprintln!("scan: {} cars in {:?}", cars.len(), t0.elapsed());
        let honda = &cars[&4277];
        eprintln!("4277 -> {honda:?}");
        assert!(honda.display.contains("Honda") && honda.display.contains("Civic WTAC"), "{honda:?}");
        let wart = &cars[&2574];
        eprintln!("2574 -> {wart:?}");
        assert_eq!(wart.display, "M12S Warthog CST");
        let (with, total) = (cars.values().filter(|c| c.make.is_some()).count(), cars.len());
        eprintln!("make found for {with}/{total}");
        assert!(with * 100 / total >= 90);
    }

    /// `CarDb::load` cold then warm (writes the real cache file under app_data_dir()).
    #[test]
    fn real_install_cache_roundtrip() {
        if find_media(None).is_none() {
            eprintln!("SKIP real_install_cache_roundtrip: FH6 install not found");
            return;
        }
        let _ = std::fs::remove_file(cache_path("EN"));
        let t = std::time::Instant::now();
        let a = CarDb::load("EN").expect("cold");
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let b = CarDb::load("EN").expect("warm");
        eprintln!("load cold {cold:?} (cache={}), warm {:?} (cache={})", a.from_cache, t.elapsed(), b.from_cache);
        assert!(!a.from_cache && b.from_cache);
        assert_eq!(a.lookup(4277), b.lookup(4277));
        let de = CarDb::load("DE").expect("DE");
        eprintln!("DE 4277 -> {:?}", de.lookup(4277).map(|c| &c.display));
    }
}
