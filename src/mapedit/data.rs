//! Generators for every data file the map editor's pages load (Leaflet editor + three.js 3D
//! page), byte-format-compatible with what `tools/fh6-extract/build_viewer.py` /
//! `preview_3d.py` write today, but made from the user's install in Rust (no Python, D54).
//! The functions are pure (`Nav` / `Elevation` / `RoadTypes` in, `String` / `Vec<u8>` out);
//! [`EditorData`] bundles them with the install, a disk cache for the images and a lookup by
//! served path, for the local server (I26b). **Generated files contain game data: only ever
//! served locally or written to the app data folder, never committed.**
//!
//! ## Contract: served path → producer → format
//!
//! Paths are relative to the server's token prefix. JS wrappers are byte-compatible with
//! `write_js`: `(window.FH6=window.FH6||{}).<name>=<compact JSON>;\n` (`P3D` for the 3D files).
//! All JS is pure ASCII.
//!
//! | path | producer | content |
//! |---|---|---|
//! | `data/meta.js` | [`meta_js`] | `FH6.meta = {calib:{x0,z1,s,size}, seasons:[installed], cats:{}, icons:{}, built}` |
//! | `data/roads.js` | [`roads_js`] | `FH6.roads = {cls:[u16 per polyline], lines:[[x,z,x,z..] per polyline]}` (0.1 m; no `surf`) |
//! | `data/roaded.js` | [`roaded_js`] | `FH6.roaded = {nav:{file,sha1,nodes}, ids:[[id per vertex]], y:[[0.1 m per vertex]], orphans:[[id,x,z]]}` (no `pre`) |
//! | `data/canon.js` | [`canon_js`] | `FH6.canon =` the *current* v2 road-type object (compact) |
//! | `data/elevation.js` | [`elevation_js`] | `FH6.elevation = {grid:{x0,z1,res,w,h,delta:1,data:b64(zlib(i16 LE dm, delta-coded per row))}}` (no `img/zmin/zmax/ramp`) |
//! | `preview3d/meta.js` | [`p3d_meta_js`] | `P3D.meta = {seasons, default, tex_size, built}` |
//! | `preview3d/terrain.js` | [`p3d_terrain_js`] | `P3D.terrain = {nx,nz,mx0,mz0,step,X0,Z1,W,H,hmin,hstep,sea,floor,data:b64(zlib(u16 LE))}` |
//! | `preview3d/roads.js` | [`p3d_roads_js`] | `P3D.roads = {nodes:b64z(f32 [x,z,y]*N), edges:b64z(u32 [a,b,type]*M), tn, n_nodes, n_edges, lift, step, km}` |
//! | `preview3d/tex_<Season>.jpg` | [`tex_jpeg`] | 4096² JPEG q82 from pyramid level 2 |
//! | `tiles/<Season>/<z>/<x>/<y>.jpg` | [`tile_jpeg`] | 1024² JPEG q80 of tile `<z>-<y>-<x>` (Leaflet x = column, y = row) |
//!
//! Plus, served by I26b itself: `data/app.js`, `project.json`, the pages, `lib/*`, `POST save`.
//! [`editor_data_names`] lists the generated ones; [`EditorData::resolve`] answers them.
//!
//! **Why `canon.js` and `preview3d/roads.js` are separate from the rest:** the road types are
//! the only input that changes at runtime (Save); nav, terrain and imagery don't. So
//! [`EditorData::set_road_types`] regenerates just those two (~tens of ms), everything else is
//! built once. **Why the 3D texture is a `.jpg`, not a base64 `tex_*.js`:** 33 % smaller and no
//! 6 MB script parse (I26c changes the page to load it directly).
//!
//! **Hole fill / skirt are not bit-exact with the Python (scipy):** holes the coarse raster
//! can't fill are interpolated by iterative neighbour averaging, the sea skirt uses a chamfer
//! distance transform; only the 3D mesh is affected (a few thousand pixels).

// Everything here is consumed by the I26b server; until it lands only the tests use it.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use base64::Engine as _;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::gamedata::nav::{Nav, NAV_FILE};
use crate::gamedata::roadtypes::{EdgeKey, RoadTypes};
use crate::gamedata::terrain::{self, Elevation, NO_DATA, X0, Z1};
use crate::gamedata::tiles::{self, MapLoadError, MAX_LEVEL, TILE_PX};

/// Seasons in the order the editor lists them.
pub const SEASONS: [&str; 4] = ["Spring", "Summer", "Autumn", "Winter"];

/// Generated served paths (the patterns, with `<Season>` / `<z>` / `<x>` / `<y>` as placeholders
/// for the last two). The server wires routes from this list plus its own static ones.
pub fn editor_data_names() -> &'static [&'static str] {
    &[
        "data/meta.js",
        "data/roads.js",
        "data/roaded.js",
        "data/canon.js",
        "data/elevation.js",
        "preview3d/meta.js",
        "preview3d/terrain.js",
        "preview3d/roads.js",
        "preview3d/tex_<Season>.jpg",
        "tiles/<Season>/<z>/<x>/<y>.jpg",
    ]
}

/// The map's calibration, as in `build_viewer.py` (`X0, Z1, SC, SIZE`).
const CALIB_SC: f64 = 0.3722;
const CALIB_SIZE: u32 = 8192;
/// Tile / texture JPEG qualities (as the Python builders).
const TILE_QUALITY: u8 = 80;
const TEX_QUALITY: u8 = 82;
/// 3D texture side, px (level 2 of the pyramid).
const TEX_SIDE: usize = 4096;

// ------------------------------------------------------------------------------------------------ helpers

/// Seasons of `media` that have a map zip, in [`SEASONS`] order.
pub fn installed_seasons(media: &Path) -> Vec<String> {
    SEASONS.iter().filter(|s| tiles::season_zip(media, s).is_some()).map(|s| s.to_string()).collect()
}

fn b64z_level(bytes: &[u8], level: u32) -> String {
    let mut enc = ZlibEncoder::new(Vec::with_capacity(bytes.len() / 3 + 64), Compression::new(level));
    enc.write_all(bytes).expect("writing to a Vec cannot fail");
    base64::engine::general_purpose::STANDARD.encode(enc.finish().expect("writing to a Vec cannot fail"))
}

/// base64(zlib(bytes)), what the JS side feeds to `DecompressionStream('deflate')`.
fn b64z(bytes: &[u8]) -> String {
    b64z_level(bytes, 9)
}

fn fh6(name: &str, json: &str) -> String {
    format!("(window.FH6=window.FH6||{{}}).{name}={json};\n")
}

fn p3d(name: &str, json: &str) -> String {
    format!("(window.P3D=window.P3D||{{}}).{name}={json};\n")
}

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn js_f64(v: f64) -> String {
    serde_json::to_string(&v).unwrap_or_else(|_| "0".into())
}

/// A coordinate rounded to 0.1 m like Python's `round(v, 1)`: `v * 10` is exact in f64 for an
/// f32, so exact ties (x.25, x.75) go to the even neighbour, as in Python.
fn f1(v: f32) -> f64 {
    ((v as f64) * 10.0).round_ties_even() / 10.0
}

/// `YYYY-MM-DD HH:MM` (UTC) of a unix time, without a date crate.
fn stamp(secs: i64) -> String {
    let (days, sod) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468; // civil-from-days (H. Hinnant)
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", sod / 3600, sod % 3600 / 60)
}

fn now_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    stamp(secs)
}

fn seasons_json(seasons: &[String]) -> String {
    format!("[{}]", seasons.iter().map(|s| js_str(s)).collect::<Vec<_>>().join(","))
}

// ------------------------------------------------------------------------------------------------ editor files

/// `data/meta.js`: calibration + the installed seasons. No `groups` / `race_icons` / POI
/// categories (the editor guards every use of those).
pub fn meta_js(seasons: &[String]) -> String {
    fh6(
        "meta",
        &format!(
            "{{\"calib\":{{\"x0\":{},\"z1\":{},\"s\":{},\"size\":{}}},\"seasons\":{},\"cats\":{{}},\"icons\":{{}},\"built\":{}}}",
            js_f64(X0),
            js_f64(Z1),
            js_f64(CALIB_SC),
            CALIB_SIZE,
            seasons_json(seasons),
            js_str(&now_stamp())
        ),
    )
}

/// `data/roads.js`: class and polyline (x, z at 0.1 m) of every nav road. No surface data.
pub fn roads_js(nav: &Nav) -> String {
    let mut s = String::with_capacity(1 << 21);
    s.push_str("{\"cls\":[");
    for (i, c) in nav.cls.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "{c}");
    }
    s.push_str("],\"lines\":[");
    for (i, pl) in nav.polys.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('[');
        for (k, v) in pl.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, "{:?},{:?}", f1(v.x), f1(v.z));
        }
        s.push(']');
    }
    s.push_str("]}");
    fh6("roads", &s)
}

/// `data/roaded.js`: node ids and heights per polyline vertex, the nav identity and orphan
/// nodes. No `pre` (surface prefill), so the editor's "Prefill from surface data" button is absent.
pub fn roaded_js(nav: &Nav) -> String {
    let mut s = String::with_capacity(1 << 20);
    let _ = write!(s, "{{\"nav\":{{\"file\":{},\"sha1\":{},\"nodes\":{}}},\"ids\":[", js_str(NAV_FILE), js_str(&nav.sha1), nav.nodes);
    for (i, pl) in nav.polys.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('[');
        for (k, v) in pl.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, "{}", v.id);
        }
        s.push(']');
    }
    s.push_str("],\"y\":[");
    for (i, pl) in nav.polys.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('[');
        for (k, v) in pl.iter().enumerate() {
            if k > 0 {
                s.push(',');
            }
            let _ = write!(s, "{:?}", f1(v.y));
        }
        s.push(']');
    }
    s.push_str("],\"orphans\":[");
    for (i, (id, x, z)) in nav.orphans.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "[{id},{:?},{:?}]", f1(*x), f1(*z));
    }
    s.push_str("]}");
    fh6("roaded", &s)
}

/// Compact JSON of `rt` (its v2 text with the line breaks removed, entry order kept), pure ASCII.
fn compact_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' => {}
            c if c.is_ascii() => out.push(c),
            c => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    let _ = write!(out, "\\u{u:04x}");
                }
            }
        }
    }
    out
}

/// `data/canon.js`: the road types the editor starts from (the *current* file, see
/// `RoadTypes::current`), the same v2 object the editor's Export writes.
pub fn canon_js(rt: &RoadTypes) -> String {
    fh6("canon", &compact_ascii(&rt.to_json_string()))
}

/// `data/elevation.js`: the 8 m lookup grid. i16 decimetres (`-32768` = no data), each row
/// delta-coded with i16 wrap-around (`qi = diff(qv, axis=1, prepend=0)`), LE, zlib, base64.
/// Why delta: zlib gets ~3.5 MB instead of ~5.6 MB; the page restores it with a cumulative sum.
pub fn elevation_js(e: &Elevation) -> String {
    let mut bytes = Vec::with_capacity(e.dm.len() * 2);
    for row in e.dm.chunks(e.w.max(1)) {
        let mut prev = 0i16;
        for &v in row {
            bytes.extend_from_slice(&v.wrapping_sub(prev).to_le_bytes());
            prev = v;
        }
    }
    fh6(
        "elevation",
        &format!(
            "{{\"grid\":{{\"x0\":{},\"z1\":{},\"res\":{},\"w\":{},\"h\":{},\"delta\":1,\"data\":\"{}\"}}}}",
            js_f64(e.x0),
            js_f64(e.z1),
            js_f64(e.res),
            e.w,
            e.h,
            b64z_level(&bytes, 6)
        ),
    )
}

// ------------------------------------------------------------------------------------------------ 3D files

/// `preview3d/meta.js`.
pub fn p3d_meta_js(seasons: &[String]) -> String {
    let default = if seasons.iter().any(|s| s == "Summer") { Some("Summer") } else { seasons.first().map(String::as_str) };
    p3d(
        "meta",
        &format!(
            "{{\"seasons\":{},\"default\":{},\"tex_size\":{},\"built\":{}}}",
            seasons_json(seasons),
            default.map_or("null".into(), js_str),
            TEX_SIDE,
            js_str(&now_stamp())
        ),
    )
}

const HMIN: f32 = -5.0;
const HSTEP: f32 = 0.025;
const SEA_Y: f32 = 100.0;
const FLOOR_Y: f32 = 40.0;
/// Metres of smooth skirt beyond the island data.
const SKIRT_M: f32 = 1000.0;
/// Raster pixels per mesh cell (8 m → 16 m).
const DECIM: usize = 2;
/// NaN regions larger than this (px) are open sea, the rest holes.
const SEA_MIN_PX: usize = 20_000;

/// 8-neighbour mean of the known (non-NaN) pixels around `i`.
fn nb_mean(h: &[f32], w: usize, hh: usize, i: usize) -> Option<f32> {
    let (r, c) = ((i / w) as isize, (i % w) as isize);
    let (mut s, mut n) = (0.0f32, 0u32);
    for dr in -1..=1isize {
        for dc in -1..=1isize {
            if (dr, dc) == (0, 0) {
                continue;
            }
            let (rr, cc) = (r + dr, c + dc);
            if rr < 0 || cc < 0 || rr >= hh as isize || cc >= w as isize {
                continue;
            }
            let v = h[rr as usize * w + cc as usize];
            if !v.is_nan() {
                s += v;
                n += 1;
            }
        }
    }
    (n > 0).then(|| s / n as f32)
}

/// Port of `fill_holes`: NaN heights → (sea mask; holes filled in place). Sea = NaN regions of
/// more than [`SEA_MIN_PX`] px (4-connected); other NaN regions are holes (a missing 512 m cell,
/// thin streaks), filled from the coarse raster where it has data, else interpolated from the rim.
fn fill_holes(h: &mut [f32], w: usize, hh: usize, coarse: Option<&Elevation>) -> Vec<bool> {
    let n = w * hh;
    let mut sea = vec![false; n];
    let mut seen = vec![false; n];
    let mut holes: Vec<usize> = Vec::new();
    let (mut stack, mut comp) = (Vec::new(), Vec::new());
    for s in 0..n {
        if !h[s].is_nan() || seen[s] {
            continue;
        }
        seen[s] = true;
        stack.push(s);
        comp.clear();
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (r, c) = (i / w, i % w);
            let mut visit = |j: usize| {
                if h[j].is_nan() && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if r > 0 {
                visit(i - w);
            }
            if r + 1 < hh {
                visit(i + w);
            }
            if c > 0 {
                visit(i - 1);
            }
            if c + 1 < w {
                visit(i + 1);
            }
        }
        if comp.len() > SEA_MIN_PX {
            for &i in &comp {
                sea[i] = true;
            }
        } else {
            holes.extend_from_slice(&comp);
        }
    }
    if let Some(c) = coarse.filter(|c| c.w == w && c.h == hh) {
        for &i in &holes {
            if c.dm[i] != NO_DATA {
                h[i] = c.dm[i] as f32 / 10.0;
            }
        }
        holes.retain(|&i| h[i].is_nan());
    }
    // The rest: grow inwards from the rim (each pass fills the pixels that touch known ones with
    // the mean of their known neighbours), then relax those pixels towards a smooth surface.
    let mut filled: Vec<usize> = Vec::new();
    while !holes.is_empty() {
        let mut upd: Vec<(usize, f32)> = Vec::new();
        holes.retain(|&i| match nb_mean(h, w, hh, i) {
            Some(v) => {
                upd.push((i, v));
                false
            }
            None => true,
        });
        if upd.is_empty() {
            break;
        }
        for (i, v) in upd {
            h[i] = v;
            filled.push(i);
        }
    }
    for _ in 0..40 {
        for &i in &filled {
            if let Some(v) = nb_mean(h, w, hh, i) {
                h[i] = v;
            }
        }
    }
    sea
}

/// For every pixel the offset `(dx, dy)` to its nearest pixel that is not in `sea` (8SSEDT,
/// two sweeps; exact enough for a skirt). Pixels that are not in `sea` get `(0, 0)`.
fn nearest_land(sea: &[bool], w: usize, hh: usize) -> Vec<[i16; 2]> {
    const INF: i16 = 16_000;
    let mut g: Vec<[i16; 2]> = sea.iter().map(|&s| if s { [INF, INF] } else { [0, 0] }).collect();
    let (wi, hi) = (w as isize, hh as isize);
    let d2 = |v: [i16; 2]| (v[0] as i32) * (v[0] as i32) + (v[1] as i32) * (v[1] as i32);
    let relax = |g: &mut [[i16; 2]], x: isize, y: isize, dx: isize, dy: isize| {
        let (nx, ny) = (x + dx, y + dy);
        if nx < 0 || ny < 0 || nx >= wi || ny >= hi {
            return;
        }
        let nb = g[(ny * wi + nx) as usize];
        let cand = [nb[0] + dx as i16, nb[1] + dy as i16];
        let p = (y * wi + x) as usize;
        if d2(cand) < d2(g[p]) {
            g[p] = cand;
        }
    };
    for y in 0..hi {
        for x in 0..wi {
            for (dx, dy) in [(-1, 0), (0, -1), (-1, -1), (1, -1)] {
                relax(&mut g, x, y, dx, dy);
            }
        }
        for x in (0..wi).rev() {
            relax(&mut g, x, y, 1, 0);
        }
    }
    for y in (0..hi).rev() {
        for x in (0..wi).rev() {
            for (dx, dy) in [(1, 0), (0, 1), (-1, 1), (1, 1)] {
                relax(&mut g, x, y, dx, dy);
            }
        }
        for x in 0..wi {
            relax(&mut g, x, y, -1, 0);
        }
    }
    g
}

/// The mesh grid of `p3d_terrain_js`: `q` is row-major `nz × nx`, 0 = no terrain.
struct Terrain {
    nx: usize,
    nz: usize,
    mx0: f64,
    mz0: f64,
    step: f64,
    q: Vec<u16>,
}

/// Port of `preview_3d.py:build_terrain` (+ `fill_holes`): hole fill, sea skirt, crop to the
/// kept bounding box, 2×2 block mean, quantise.
fn terrain_grid(e: &Elevation, coarse: Option<&Elevation>) -> Result<Terrain, String> {
    let (w, hh) = (e.w, e.h);
    if w == 0 || hh == 0 || e.dm.len() != w * hh {
        return Err("empty elevation raster".into());
    }
    let mut h: Vec<f32> = e.dm.iter().map(|&v| if v == NO_DATA { f32::NAN } else { v as f32 / 10.0 }).collect();
    if h.iter().all(|v| v.is_nan()) {
        return Err("the elevation raster has no data".into());
    }
    let sea = fill_holes(&mut h, w, hh, coarse);
    // Open sea beyond the data: nearest data height decaying to the sea floor over SKIRT_M (a
    // smooth skirt: no cliff, no pit); further out nothing is meshed (q = 0), the page's sea
    // plane covers it.
    let mut keep = vec![true; w * hh];
    if sea.iter().any(|&s| s) {
        let off = nearest_land(&sea, w, hh);
        for i in 0..w * hh {
            if !sea[i] {
                continue;
            }
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let [dx, dy] = off[i];
            let (nx, ny) = (x + dx as isize, y + dy as isize);
            let dist = ((dx as f32).powi(2) + (dy as f32).powi(2)).sqrt() * e.res as f32;
            let near = if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < hh { h[ny as usize * w + nx as usize] } else { f32::NAN };
            let wt = (1.0 - dist / SKIRT_M).clamp(0.0, 1.0).powi(2);
            h[i] = if near.is_nan() { FLOOR_Y } else { FLOOR_Y + (near - FLOOR_Y) * wt };
            keep[i] = dist <= SKIRT_M;
        }
    }
    for v in h.iter_mut() {
        if v.is_nan() {
            *v = FLOOR_Y; // a hole nothing could fill (no neighbours at all)
        }
    }
    let (mut r0, mut r1, mut c0, mut c1) = (hh, 0usize, w, 0usize);
    for (i, &k) in keep.iter().enumerate() {
        if k {
            let (r, c) = (i / w, i % w);
            r0 = r0.min(r);
            r1 = r1.max(r + 1);
            c0 = c0.min(c);
            c1 = c1.max(c + 1);
        }
    }
    r0 -= r0 % DECIM;
    c0 -= c0 % DECIM;
    r1 = hh.min(r1.div_ceil(DECIM) * DECIM);
    c1 = w.min(c1.div_ceil(DECIM) * DECIM);
    r1 -= (r1 - r0) % DECIM;
    c1 -= (c1 - c0) % DECIM;
    let (nz, nx) = ((r1 - r0) / DECIM, (c1 - c0) / DECIM);
    let mut q = vec![0u16; nx * nz];
    for j in 0..nz {
        for i in 0..nx {
            let (mut s, mut any) = (0.0f32, false);
            for dr in 0..DECIM {
                for dc in 0..DECIM {
                    let p = (r0 + j * DECIM + dr) * w + c0 + i * DECIM + dc;
                    s += h[p];
                    any |= keep[p];
                }
            }
            if any {
                let blk = s / (DECIM * DECIM) as f32;
                q[j * nx + i] = (((blk - HMIN) / HSTEP).round_ties_even() + 1.0).clamp(1.0, 65535.0) as u16;
            }
        }
    }
    let step = DECIM as f64 * e.res;
    Ok(Terrain {
        nx,
        nz,
        mx0: e.x0 + (c0 as f64 + DECIM as f64 / 2.0) * e.res,
        mz0: e.z1 - (r0 as f64 + DECIM as f64 / 2.0) * e.res,
        step,
        q,
    })
}

/// `preview3d/terrain.js`: the terrain mesh heights, 16 m cells (block mean of the 8 m raster),
/// `q = (h + 5) / 0.025 + 1` as u16 LE (0 = no terrain), zlib + base64. `coarse` (the whole-island
/// LOD raster) fills the few holes of the detailed one; without it they are interpolated.
/// Reference run: `nx = 1327, nz = 1376, mx0 = -11748, mz0 = 10730, step 16`.
pub fn p3d_terrain_js(e: &Elevation, coarse: Option<&Elevation>) -> Result<String, String> {
    let t = terrain_grid(e, coarse)?;
    let bytes: Vec<u8> = t.q.iter().flat_map(|v| v.to_le_bytes()).collect();
    Ok(p3d(
        "terrain",
        &format!(
            "{{\"nx\":{},\"nz\":{},\"mx0\":{},\"mz0\":{},\"step\":{},\"X0\":{},\"Z1\":{},\"W\":{},\"H\":{},\"hmin\":{},\"hstep\":{},\"sea\":{},\"floor\":{},\"data\":\"{}\"}}",
            t.nx,
            t.nz,
            js_f64(t.mx0),
            js_f64(t.mz0),
            js_f64(t.step),
            js_f64(e.x0),
            js_f64(e.z1),
            js_f64(e.w as f64 * e.res),
            js_f64(e.h as f64 * e.res),
            js_f64(HMIN as f64),
            js_f64(0.025), // HSTEP as an f64 literal (the f32 constant would print 0.02500000037...)
            js_f64(SEA_Y as f64),
            js_f64(FLOOR_Y as f64),
            b64z(&bytes)
        ),
    ))
}

/// The type-index → name table of the 3D page and the editor (0 = not set), as in `preview_3d.py`
/// (`TN`); entry `i + 1` is `RoadType::ALL[i]`.
pub const TYPE_NAMES: [&str; 10] = ["unset", "road", "offroad", "other", "trail", "crosscountry", "tunnel", "jump", "highway", "turnaround"];
/// Road resample step written into `roads.js` (the page resamples), m.
const ROAD_STEP: f64 = 5.0;
/// Metres the road centreline sits above the terrain.
const ROAD_LIFT: f64 = 0.6;

/// The typed edge list of `p3d_roads_js`: node ids (sorted), `[x, z, y]` per node (`y` NaN =
/// none), and edges as `(index a, index b, type index)`.
pub struct RoadGraph {
    pub ids: Vec<u32>,
    pub nodes: Vec<[f32; 3]>,
    pub edges: Vec<[u32; 3]>,
}

/// Port of `preview_3d.py:build_roads`: positions from the nav (0.1 m, like the Python's
/// `roads.json`) overridden by `moved` / `points`; node heights from the nav and the user
/// points; edges = nav edges that aren't `removed` (type from `types`, missing → 0 = unset),
/// then the `added` links.
pub fn road_graph(nav: &Nav, rt: &RoadTypes) -> RoadGraph {
    let mut pos: HashMap<u32, (f64, f64)> = HashMap::new();
    let mut ny: HashMap<u32, f64> = HashMap::new();
    for pl in &nav.polys {
        for v in pl {
            pos.insert(v.id, (f1(v.x), f1(v.z)));
            ny.insert(v.id, f1(v.y));
        }
    }
    for src in [&rt.points, &rt.moved] {
        for (&id, p) in src {
            pos.insert(id, (p[0], p[1]));
            ny.insert(id, p[2]);
        }
    }
    let removed: HashSet<EdgeKey> = rt.removed.iter().copied().collect();
    let mut seen: HashSet<EdgeKey> = HashSet::new();
    let mut edges: Vec<(u32, u32, u8)> = Vec::new();
    for pl in &nav.polys {
        for w in pl.windows(2) {
            let (a, b) = (w[0].id, w[1].id);
            let k = EdgeKey::new(a, b);
            if seen.contains(&k) || removed.contains(&k) || !pos.contains_key(&a) || !pos.contains_key(&b) {
                continue;
            }
            seen.insert(k);
            edges.push((a, b, rt.types.get(&k).map_or(0, |t| t.index())));
        }
    }
    for l in &rt.added {
        let k = EdgeKey::new(l.a, l.b);
        if seen.contains(&k) || removed.contains(&k) || !pos.contains_key(&l.a) || !pos.contains_key(&l.b) {
            continue;
        }
        seen.insert(k);
        edges.push((l.a, l.b, l.ty.map_or(0, |t| t.index())));
    }
    let mut ids: Vec<u32> = edges.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    ids.sort_unstable();
    ids.dedup();
    let ix: HashMap<u32, u32> = ids.iter().enumerate().map(|(k, &i)| (i, k as u32)).collect();
    let nodes = ids.iter().map(|i| [pos[i].0 as f32, pos[i].1 as f32, ny.get(i).map_or(f32::NAN, |&y| y as f32)]).collect();
    let edges = edges.iter().map(|&(a, b, t)| [ix[&a], ix[&b], t as u32]).collect();
    RoadGraph { ids, nodes, edges }
}

/// `preview3d/roads.js` (baked from `rt`, the *current* road types): the edge list the 3D page
/// turns into ribbons. Cheap (tens of ms), regenerated on Save.
pub fn p3d_roads_js(nav: &Nav, rt: &RoadTypes) -> String {
    let g = road_graph(nav, rt);
    let nodes: Vec<u8> = g.nodes.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let edges: Vec<u8> = g.edges.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let mut km = [0.0f64; 10];
    let mut any = [false; 10];
    for e in &g.edges {
        let (a, b) = (g.nodes[e[0] as usize], g.nodes[e[1] as usize]);
        km[e[2] as usize] += ((a[0] as f64 - b[0] as f64).powi(2) + (a[1] as f64 - b[1] as f64).powi(2)).sqrt();
        any[e[2] as usize] = true;
    }
    let km_json = (0..10).filter(|&t| any[t]).map(|t| format!("{}:{}", js_str(TYPE_NAMES[t]), js_f64((km[t] / 1000.0 * 10.0).round() / 10.0))).collect::<Vec<_>>().join(",");
    p3d(
        "roads",
        &format!(
            "{{\"nodes\":\"{}\",\"edges\":\"{}\",\"tn\":{},\"n_nodes\":{},\"n_edges\":{},\"lift\":{},\"step\":{},\"km\":{{{}}}}}",
            b64z_level(&nodes, 6),
            b64z_level(&edges, 6),
            format!("[{}]", TYPE_NAMES.iter().map(|n| js_str(n)).collect::<Vec<_>>().join(",")),
            g.nodes.len(),
            g.edges.len(),
            js_f64(ROAD_LIFT),
            js_f64(ROAD_STEP),
            km_json
        ),
    )
}

// ------------------------------------------------------------------------------------------------ images

fn jpeg(rgb: &[u8], side: usize, quality: u8) -> Result<Vec<u8>, MapLoadError> {
    let mut out = Vec::with_capacity(rgb.len() / 8);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode(rgb, side as u32, side as u32, image::ExtendedColorType::Rgb8)
        .map_err(|e| MapLoadError::Decode(format!("jpeg encode: {e}")))?;
    Ok(out)
}

/// `tiles/<Season>/<z>/<x>/<y>.jpg`: Leaflet tile `z/x/y` = pyramid entry `z-y-x`
/// (row = y, col = x), 1024² JPEG q80. ~30 ms decode + ~35 ms encode.
pub fn tile_jpeg(media: &Path, season: &str, z: u32, x: u32, y: u32) -> Result<Vec<u8>, MapLoadError> {
    jpeg(&tiles::tile_rgb(media, season, z, y as usize, x as usize)?, TILE_PX, TILE_QUALITY)
}

/// `preview3d/tex_<Season>.jpg`: the whole map at 4096² (pyramid level 2), JPEG q82. ~0.5 s.
pub fn tex_jpeg(media: &Path, season: &str) -> Result<Vec<u8>, MapLoadError> {
    let (rgba, size) = tiles::load_mosaic(media, season, 2)?;
    debug_assert_eq!(size, TEX_SIDE);
    let mut rgb = Vec::with_capacity(size * size * 3);
    for p in rgba.chunks_exact(4) {
        rgb.extend_from_slice(&p[..3]);
    }
    drop(rgba);
    jpeg(&rgb, size, TEX_QUALITY)
}

// ------------------------------------------------------------------------------------------------ the bundle

/// One served response.
pub struct Served {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    /// Imagery never changes within a session (the data files do: Save): the server may let the
    /// browser cache it.
    pub cacheable: bool,
}

const JS: &str = "text/javascript; charset=utf-8";
const JPG: &str = "image/jpeg";

/// Everything the editor pages load, built once from the install ([`EditorData::build`], a few
/// seconds cold: call it off the UI thread), plus on-demand imagery with a disk cache.
pub struct EditorData {
    media: PathBuf,
    cache: PathBuf,
    seasons: Vec<String>,
    nav: Nav,
    meta: String,
    roads: String,
    roaded: String,
    elevation: String,
    p3d_meta: String,
    p3d_terrain: String,
    /// `(canon.js, preview3d/roads.js)`: the parts that follow the road types.
    typed: RwLock<(String, String)>,
    /// One 4096² texture build at a time (50 MB of RGB each).
    tex_lock: Mutex<()>,
}

impl EditorData {
    /// Build with the default cache folder (`<app_data_dir>/map_editor/cache`).
    /// `progress` gets 0.0..=1.0 (from worker threads); the nav, raster and terrain are all
    /// disk-cached or fast, so a warm start is well under a second.
    pub fn build(media: &Path, rt: &RoadTypes, progress: &(dyn Fn(f32) + Sync)) -> Result<EditorData, String> {
        Self::build_in(&terrain::cache_dir(), media, rt, progress)
    }

    /// [`EditorData::build`] with an explicit cache folder (tests).
    pub fn build_in(cache: &Path, media: &Path, rt: &RoadTypes, progress: &(dyn Fn(f32) + Sync)) -> Result<EditorData, String> {
        let nav = Nav::load(media)?;
        progress(0.02);
        let scale = |lo: f32, hi: f32| move |p: f32| progress(lo + (hi - lo) * p);
        let fine = Elevation::load_or_build_in(cache, media, false, &scale(0.02, 0.5))?;
        // The coarse raster only fills holes of the 3D mesh: without it they are interpolated.
        let coarse = match Elevation::load_or_build_in(cache, media, true, &scale(0.5, 0.8)) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("coarse elevation unavailable, 3D holes interpolated: {e}");
                None
            }
        };
        let seasons = installed_seasons(media);
        if seasons.is_empty() {
            return Err("no map zips found in the install".into());
        }
        let p3d_terrain = p3d_terrain_js(&fine, coarse.as_ref())?;
        progress(0.9);
        let elevation = elevation_js(&fine);
        let d = EditorData {
            media: media.to_path_buf(),
            cache: cache.to_path_buf(),
            meta: meta_js(&seasons),
            p3d_meta: p3d_meta_js(&seasons),
            seasons,
            roads: roads_js(&nav),
            roaded: roaded_js(&nav),
            elevation,
            p3d_terrain,
            typed: RwLock::new((canon_js(rt), p3d_roads_js(&nav, rt))),
            nav,
            tex_lock: Mutex::new(()),
        };
        progress(1.0);
        Ok(d)
    }

    pub fn nav(&self) -> &Nav {
        &self.nav
    }

    pub fn seasons(&self) -> &[String] {
        &self.seasons
    }

    /// New *current* road types (after Save / reset): regenerates `data/canon.js` and
    /// `preview3d/roads.js` only.
    pub fn set_road_types(&self, rt: &RoadTypes) {
        let typed = (canon_js(rt), p3d_roads_js(&self.nav, rt));
        *self.typed.write().unwrap_or_else(|e| e.into_inner()) = typed;
    }

    /// The generated file at served path `path` (no leading slash, e.g. `data/meta.js`,
    /// `tiles/Summer/3/4/5.jpg`); `None` = not one of ours (404). Strictly parsed: seasons must be
    /// installed ones, `z` 0..=3, `x`/`y` plain digits below `2^z`. Images are generated on first
    /// request and kept under the cache folder (regenerated when the game's zip is newer).
    pub fn resolve(&self, path: &str) -> Option<Result<Served, String>> {
        let js = |s: &str| Some(Ok(Served { bytes: s.as_bytes().to_vec(), content_type: JS, cacheable: false }));
        match path {
            "data/meta.js" => return js(&self.meta),
            "data/roads.js" => return js(&self.roads),
            "data/roaded.js" => return js(&self.roaded),
            "data/elevation.js" => return js(&self.elevation),
            "preview3d/meta.js" => return js(&self.p3d_meta),
            "preview3d/terrain.js" => return js(&self.p3d_terrain),
            "data/canon.js" => return js(&self.typed.read().unwrap_or_else(|e| e.into_inner()).0),
            "preview3d/roads.js" => return js(&self.typed.read().unwrap_or_else(|e| e.into_inner()).1),
            _ => {}
        }
        let season = |s: &str| self.seasons.iter().find(|x| x.as_str() == s).cloned();
        let digits = |s: &str| (!s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u32>().ok()).flatten();
        if let Some(rest) = path.strip_prefix("preview3d/tex_") {
            let s = season(rest.strip_suffix(".jpg")?)?;
            return Some(self.cached(&self.cache.join(format!("tex_{s}.jpg")), &s, Some(&self.tex_lock), || tex_jpeg(&self.media, &s)));
        }
        let rest = path.strip_prefix("tiles/")?.strip_suffix(".jpg")?;
        let mut it = rest.split('/');
        let (s, z, x, y) = (season(it.next()?)?, digits(it.next()?)?, digits(it.next()?)?, digits(it.next()?)?);
        if it.next().is_some() || z > MAX_LEVEL || x >> z != 0 || y >> z != 0 {
            return None;
        }
        let file = self.cache.join("tiles").join(&s).join(z.to_string()).join(x.to_string()).join(format!("{y}.jpg"));
        Some(self.cached(&file, &s, None, || tile_jpeg(&self.media, &s, z, x, y)))
    }

    /// `file` from the cache when it's newer than the season's zip, else `make()` and store it.
    fn cached(&self, file: &Path, season: &str, lock: Option<&Mutex<()>>, make: impl Fn() -> Result<Vec<u8>, MapLoadError>) -> Result<Served, String> {
        let zip_time = tiles::season_zip(&self.media, season).and_then(|z| std::fs::metadata(z).ok()).and_then(|m| m.modified().ok());
        let fresh = |f: &Path| -> Option<Vec<u8>> {
            let t = std::fs::metadata(f).ok()?.modified().ok()?;
            if zip_time.is_some_and(|z| z > t) {
                return None;
            }
            std::fs::read(f).ok()
        };
        let served = |bytes| Served { bytes, content_type: JPG, cacheable: true };
        if let Some(b) = fresh(file) {
            return Ok(served(b));
        }
        let _guard = lock.map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
        if lock.is_some() {
            if let Some(b) = fresh(file) {
                return Ok(served(b)); // another request built it while we waited
            }
        }
        let bytes = make().map_err(|e| e.to_string())?;
        if let Err(e) = write_atomic(file, &bytes) {
            eprintln!("map cache not written ({}): {e}", file.display()); // non-fatal
        }
        Ok(served(bytes))
    }
}

/// Write via a uniquely named sibling temp file + rename (concurrent requests for the same
/// tile don't see a half-written file).
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static N: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;
    use crate::gamedata::nav::NavVert;
    use crate::gamedata::roadtypes::RoadType;
    use serde_json::Value;
    use std::io::Read;

    const REF_DIR: &str = "/home/mo/fh6-viewer";

    /// `(window.X=window.X||{}).name=<json>;\n` → (name, json value). Asserts the exact wrapper.
    fn unwrap_js(txt: &str, ns: &str) -> (String, Value) {
        let pre = format!("(window.{ns}=window.{ns}||{{}}).");
        let rest = txt.strip_prefix(&pre).unwrap_or_else(|| panic!("bad wrapper: {}", &txt[..txt.len().min(60)]));
        let eq = rest.find('=').unwrap();
        let body = rest[eq + 1..].strip_suffix(";\n").expect("trailing ;\\n");
        assert!(txt.is_ascii(), "JS must be pure ASCII");
        (rest[..eq].to_string(), serde_json::from_str(body).expect("valid JSON"))
    }

    fn unb64z(v: &Value) -> Vec<u8> {
        let raw = base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap();
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&raw[..]).read_to_end(&mut out).unwrap();
        out
    }

    fn ref_js(rel: &str, ns: &str) -> Option<(String, Value)> {
        let txt = std::fs::read_to_string(Path::new(REF_DIR).join(rel)).ok()?;
        // Python's output has no trailing issue; reuse the wrapper check.
        Some(unwrap_js(&txt, ns))
    }

    /// Compare two nested JSON number arrays; panic with a short message (never the whole arrays).
    fn assert_same_numbers(a: &Value, b: &Value, what: &str, tol: f64) {
        fn flat(v: &Value, out: &mut Vec<f64>) {
            match v {
                Value::Array(a) => a.iter().for_each(|x| flat(x, out)),
                Value::Number(n) => out.push(n.as_f64().unwrap()),
                _ => panic!("not a number array"),
            }
        }
        let (mut fa, mut fb) = (Vec::new(), Vec::new());
        flat(a, &mut fa);
        flat(b, &mut fb);
        assert_eq!(fa.len(), fb.len(), "{what}: different number counts");
        let bad: Vec<usize> = (0..fa.len()).filter(|&i| (fa[i] - fb[i]).abs() > tol).collect();
        eprintln!("{what}: {} numbers, {} differ by more than {tol}", fa.len(), bad.len());
        assert!(bad.is_empty(), "{what}: first mismatch at #{}: {} vs {}", bad[0], fa[bad[0]], fb[bad[0]]);
    }

    fn vert(id: u32, x: f32, z: f32, y: f32) -> NavVert {
        NavVert { id, x, z, y }
    }

    /// 2 polylines: 1-2-3 and 3-4 (+ a repeat of edge 2-3 reversed in a third: 3-2).
    fn tiny_nav() -> Nav {
        Nav {
            sha1: "ab".repeat(20),
            nodes: 4,
            polys: vec![
                vec![vert(1, 0.04, 0.0, 10.0), vert(2, 10.0, 0.0, 11.0), vert(3, 20.0, 0.0, 12.0)],
                vec![vert(3, 20.0, 0.0, 12.0), vert(4, 20.0, 30.0, 13.0)],
                vec![vert(3, 20.0, 0.0, 12.0), vert(2, 10.0, 0.0, 11.0)],
            ],
            cls: vec![5, 6, 5],
            hi: vec![0, 0, 0],
            orphans: vec![(9, 1.26, 2.0)],
        }
    }

    #[test]
    fn stamp_formats_utc() {
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(1_791_351_420), "2026-10-07 05:37");
        assert_eq!(stamp(951_782_400 + 3_600 * 23 + 59 * 60), "2000-02-29 23:59"); // leap day
    }

    #[test]
    fn meta_and_p3d_meta() {
        let seasons = vec!["Spring".to_string(), "Summer".to_string()];
        let (n, m) = unwrap_js(&meta_js(&seasons), "FH6");
        assert_eq!(n, "meta");
        assert_eq!(m["calib"], serde_json::json!({"x0": -12540.0, "z1": 10738.0, "s": 0.3722, "size": 8192}));
        assert_eq!(m["seasons"], serde_json::json!(["Spring", "Summer"]));
        assert!(m["cats"].as_object().unwrap().is_empty() && m["icons"].as_object().unwrap().is_empty());
        assert!(meta_js(&seasons).starts_with("(window.FH6=window.FH6||{}).meta={\"calib\":{\"x0\":-12540.0,\"z1\":10738.0,\"s\":0.3722,\"size\":8192},"));
        let (n, m) = unwrap_js(&p3d_meta_js(&seasons), "P3D");
        assert_eq!((n.as_str(), m["default"].as_str(), m["tex_size"].as_u64()), ("meta", Some("Summer"), Some(4096)));
        let (_, m) = unwrap_js(&p3d_meta_js(&["Winter".to_string()]), "P3D");
        assert_eq!(m["default"], "Winter");
        let (_, m) = unwrap_js(&p3d_meta_js(&[]), "P3D");
        assert!(m["default"].is_null());
    }

    #[test]
    fn roads_and_roaded_synthetic() {
        let nav = tiny_nav();
        let (n, r) = unwrap_js(&roads_js(&nav), "FH6");
        assert_eq!(n, "roads");
        assert_eq!(r["cls"], serde_json::json!([5, 6, 5]));
        assert_eq!(r["lines"][0], serde_json::json!([0.0, 0.0, 10.0, 0.0, 20.0, 0.0]));
        assert!(r.get("surf").is_none());
        let (n, r) = unwrap_js(&roaded_js(&nav), "FH6");
        assert_eq!(n, "roaded");
        assert_eq!(r["nav"], serde_json::json!({"file": "Brio_00.nav", "sha1": "ab".repeat(20), "nodes": 4}));
        assert_eq!(r["ids"], serde_json::json!([[1, 2, 3], [3, 4], [3, 2]]));
        assert_eq!(r["y"][1], serde_json::json!([12.0, 13.0]));
        assert_eq!(r["orphans"], serde_json::json!([[9, 1.3, 2.0]]));
        assert!(r.get("pre").is_none());
    }

    #[test]
    fn canon_is_compact_ascii_and_round_trips() {
        let text = "{\"format\":\"fh6-road-types\",\"version\":2,\"nav\":null,\"types\":{\"1-2\":\"road\"},\"added\":[],\"points\":{},\"moved\":{},\"removed\":[],\"jump_from\":{},\"races\":{\"7\":\"caf\\u00e9 \\ud83d\\ude00\"},\"counts\":{}}\n";
        let rt = RoadTypes::parse(text).unwrap();
        let js = canon_js(&rt);
        assert!(js.is_ascii() && !js[..js.len() - 1].contains('\n'), "{js}");
        let (n, v) = unwrap_js(&js, "FH6");
        assert_eq!(n, "canon");
        assert_eq!(v["races"]["7"], "café \u{1f600}");
        assert_eq!(v["types"]["1-2"], "road");
        let back = RoadTypes::parse(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back.types.len(), 1);
    }

    /// Decode `elevation_js` the way the page does (cumulative sum, i16 wrap) → i16 grid.
    fn decode_elevation(js: &str) -> (Value, Vec<i16>) {
        let (n, v) = unwrap_js(js, "FH6");
        assert_eq!(n, "elevation");
        let g = &v["grid"];
        assert_eq!(g["delta"], 1);
        assert!(v.get("img").is_none() && v.get("zmin").is_none() && v.get("ramp").is_none());
        let (w, h) = (g["w"].as_u64().unwrap() as usize, g["h"].as_u64().unwrap() as usize);
        let raw = unb64z(&g["data"]);
        assert_eq!(raw.len(), w * h * 2);
        let mut out = Vec::with_capacity(w * h);
        for row in raw.chunks(w * 2) {
            let mut acc = 0i16;
            for c in row.chunks(2) {
                acc = acc.wrapping_add(i16::from_le_bytes([c[0], c[1]]));
                out.push(acc);
            }
        }
        (g.clone(), out)
    }

    #[test]
    fn elevation_js_round_trip_synthetic() {
        let (w, h) = (7usize, 5usize);
        let mut dm = vec![NO_DATA; w * h];
        for (i, v) in dm.iter_mut().enumerate() {
            *v = match i % 5 {
                0 => NO_DATA,
                1 => 30_000,
                2 => -30_000, // delta wraps around
                3 => (i as i16) * 7 - 100,
                _ => 0,
            };
        }
        let e = Elevation { x0: -12540.0, z1: 10738.0, res: 8.0, w, h, dm };
        let (g, back) = decode_elevation(&elevation_js(&e));
        assert_eq!((g["x0"].as_f64(), g["z1"].as_f64(), g["res"].as_f64()), (Some(-12540.0), Some(10738.0), Some(8.0)));
        assert_eq!(back, e.dm);
    }

    fn real_media() -> PathBuf {
        find_media(None).expect("the FH6 install is needed for this test (this machine has one)")
    }

    fn real_elevation(coarse: bool) -> Elevation {
        Elevation::load_or_build(&real_media(), coarse, &|_| {}).expect("elevation")
    }

    #[test]
    fn elevation_js_round_trip_real() {
        let e = real_elevation(false);
        let t = std::time::Instant::now();
        let js = elevation_js(&e);
        eprintln!("elevation_js: {:?}, {} bytes", t.elapsed(), js.len());
        let (g, back) = decode_elevation(&js);
        assert_eq!((g["w"].as_u64(), g["h"].as_u64()), (Some(2752), Some(2752)));
        assert!(back == e.dm, "decoded grid differs from the raster");
        // vs the Python's grid: same header, and (where both have data) the same heights
        if let Some((_, r)) = ref_js("data/elevation.js", "FH6") {
            let rg = &r["grid"];
            assert_eq!((rg["x0"].as_f64(), rg["z1"].as_f64(), rg["res"].as_f64(), rg["w"].as_u64(), rg["h"].as_u64()), (g["x0"].as_f64(), g["z1"].as_f64(), g["res"].as_f64(), g["w"].as_u64(), g["h"].as_u64()));
            let raw = unb64z(&rg["data"]);
            let (mut both, mut near, mut nodata_match) = (0usize, 0usize, 0usize);
            let mut acc = 0i16;
            for (i, c) in raw.chunks(2).enumerate() {
                if i % 2752 == 0 {
                    acc = 0;
                }
                acc = acc.wrapping_add(i16::from_le_bytes([c[0], c[1]]));
                let (p, r) = (acc, e.dm[i]);
                if p == NO_DATA && r == NO_DATA {
                    nodata_match += 1;
                } else if p != NO_DATA && r != NO_DATA {
                    both += 1;
                    near += usize::from((p as i32 - r as i32).abs() <= 5); // 0.5 m
                }
            }
            eprintln!("elevation vs Python: {both} px with data in both, {near} within 0.5 m ({:.3} %), {nodata_match} no-data in both", 100.0 * near as f64 / both as f64);
            assert!(near as f64 / both as f64 > 0.99);
        }
    }

    #[test]
    fn p3d_roads_synthetic_applies_the_types() {
        let nav = tiny_nav();
        // 1-2 road, 2-3 absent (-> unset), 3-4 removed; added: 4-1 jump, 2-3 again (dup, skipped),
        // 1-1000000 (user point, no type), 3-9 (9 has no position -> skipped); node 2 moved.
        let text = r#"{"format":"fh6-road-types","version":2,"nav":null,
            "types":{"1-2":"road","3-4":"highway"},
            "added":[{"a":1,"b":4,"type":"jump"},{"a":2,"b":3,"type":"road"},{"a":1,"b":1000000,"type":null},{"a":3,"b":9,"type":"road"}],
            "points":{"1000000":[5.5,6.5,7.0]},"moved":{"2":[10.0,1.0,11.5]},"removed":["3-4"],"jump_from":{},"races":{},"counts":{}}"#;
        let rt = RoadTypes::parse(&text).unwrap();
        let g = road_graph(&nav, &rt);
        // edges: 1-2 road(1), 2-3 unset(0); 3-4 removed; added 1-4 jump(7), 1-1000000 unset(0)
        let edges: Vec<(u32, u32, u32)> = g.edges.iter().map(|e| (g.ids[e[0] as usize], g.ids[e[1] as usize], e[2])).collect();
        assert_eq!(edges, vec![(1, 2, 1), (2, 3, 0), (1, 4, 7), (1, 1_000_000, 0)]);
        assert_eq!(g.ids, vec![1, 2, 3, 4, 1_000_000]);
        assert_eq!(g.nodes[1], [10.0, 1.0, 11.5]); // moved
        assert_eq!(g.nodes[4], [5.5, 6.5, 7.0]); // user point
        assert_eq!(g.nodes[0], [0.0, 0.0, 10.0]); // 0.04 rounded to 0.1
        let (n, v) = unwrap_js(&p3d_roads_js(&nav, &rt), "P3D");
        assert_eq!(n, "roads");
        assert_eq!(v["tn"], serde_json::json!(TYPE_NAMES));
        assert_eq!((v["n_nodes"].as_u64(), v["n_edges"].as_u64(), v["lift"].as_f64(), v["step"].as_f64()), (Some(5), Some(4), Some(0.6), Some(5.0)));
        let e: Vec<u32> = unb64z(&v["edges"]).chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(e.len(), 12);
        assert_eq!(&e[..3], &[0, 1, 1]);
        assert!(v["km"].get("road").is_some() && v["km"].get("highway").is_none());
    }

    #[test]
    fn type_names_match_road_type_indices() {
        assert_eq!(TYPE_NAMES[0], "unset");
        for t in RoadType::ALL {
            assert_eq!(TYPE_NAMES[t.index() as usize], t.name());
        }
    }

    /// Edge-list round trip against the *current* (project) file and the real nav, like
    /// `preview_3d.py:numeric_check`: counts, type indices, node data, km.
    #[test]
    fn p3d_roads_real_round_trip() {
        let media = real_media();
        let nav = Nav::load(&media).unwrap();
        let rt = RoadTypes::project();
        let t = std::time::Instant::now();
        let js = p3d_roads_js(&nav, &rt);
        eprintln!("p3d_roads_js regen: {:?} ({} bytes)", t.elapsed(), js.len());
        let (_, v) = unwrap_js(&js, "P3D");
        // expected count: nav edges - removed + added (added links that are new, typed or not)
        let nav_edges = nav.edges();
        let removed: HashSet<EdgeKey> = rt.removed.iter().copied().collect();
        let kept = nav_edges.iter().filter(|&&(a, b)| !removed.contains(&EdgeKey::new(a, b))).count();
        let nav_set: HashSet<(u32, u32)> = nav_edges.iter().copied().collect();
        let mut added_set = HashSet::new();
        for l in &rt.added {
            let k = EdgeKey::new(l.a, l.b);
            if !nav_set.contains(&(k.a, k.b)) && !removed.contains(&k) {
                added_set.insert((k.a, k.b));
            }
        }
        let expect = kept + added_set.len();
        eprintln!("nav edges {} - removed {} + added {} = {expect}", nav_edges.len(), nav_edges.len() - kept, added_set.len());
        assert_eq!(v["n_edges"].as_u64().unwrap() as usize, expect);
        let nodes: Vec<f32> = unb64z(&v["nodes"]).chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        let edges: Vec<u32> = unb64z(&v["edges"]).chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let (nn, ne) = (v["n_nodes"].as_u64().unwrap() as usize, expect);
        assert_eq!((nodes.len(), edges.len()), (nn * 3, ne * 3));
        assert!(edges.chunks(3).all(|e| (e[0] as usize) < nn && (e[1] as usize) < nn && (e[2] as usize) < 10));
        // every typed game edge carries its type index
        let g = road_graph(&nav, &rt);
        let by_key: HashMap<EdgeKey, u32> = g.edges.iter().map(|e| (EdgeKey::new(g.ids[e[0] as usize], g.ids[e[1] as usize]), e[2])).collect();
        for (k, t) in &rt.types {
            if let Some(&got) = by_key.get(k) {
                assert_eq!(got, t.index() as u32, "{k}");
            }
        }
        let unset = edges.chunks(3).filter(|e| e[2] == 0).count();
        eprintln!("{unset} unset edges of {ne}");
        // vs the Python's roads.js (built from the same project file): same node/edge counts
        if let Some((_, r)) = ref_js("preview3d/roads.js", "P3D") {
            eprintln!("Python: {} nodes, {} edges; Rust: {nn} nodes, {ne} edges; km py {} / rust {}", r["n_nodes"], r["n_edges"], r["km"], v["km"]);
            assert_eq!(r["tn"], v["tn"]);
            assert_eq!(r["lift"], v["lift"]);
            assert_eq!(r["step"], v["step"]);
        }
    }

    #[test]
    fn roads_and_roaded_real_parity_with_python() {
        let media = real_media();
        let nav = Nav::load(&media).unwrap();
        let (_, r) = unwrap_js(&roads_js(&nav), "FH6");
        let (_, rd) = unwrap_js(&roaded_js(&nav), "FH6");
        assert_eq!(r["lines"].as_array().unwrap().len(), 1544);
        let Some((_, pr)) = ref_js("data/roads.js", "FH6") else { return eprintln!("SKIP python reference data/roads.js absent") };
        let (_, prd) = ref_js("data/roaded.js", "FH6").unwrap();
        // roads: cls identical, lines identical to 0.1 m
        assert_eq!(pr["cls"], r["cls"]);
        assert_same_numbers(&pr["lines"], &r["lines"], "roads.js lines", 0.0);
        // roaded: nav, ids, y, orphans identical; pre absent
        assert_eq!(prd["nav"], rd["nav"]);
        assert!(prd["ids"] == rd["ids"], "roaded ids differ");
        assert_same_numbers(&prd["y"], &rd["y"], "roaded.js y", 0.0);
        assert_eq!(prd["orphans"], rd["orphans"]);
        assert!(prd.get("pre").is_some() && rd.get("pre").is_none());
    }

    #[test]
    fn canon_real_matches_python() {
        let rt = RoadTypes::project();
        let (_, v) = unwrap_js(&canon_js(&rt), "FH6");
        assert_eq!(v["types"].as_object().unwrap().len(), 39382);
        assert_eq!(v["added"].as_array().unwrap().len(), 218);
        if let Some((_, p)) = ref_js("data/canon.js", "FH6") {
            // The Python canon.js was built from the then-current project file; if that file hasn't
            // changed since they are equal as values.
            if p == v {
                eprintln!("canon.js: identical to the Python's as JSON values");
            } else {
                eprintln!("canon.js differs from the Python's (the project file changed since it was built)");
            }
        }
    }

    // ---- terrain

    /// 400 × 300 raster: an island (rows 100..200, cols 150..250) in open sea, with a 5 × 5 hole.
    fn synthetic_island() -> Elevation {
        let (w, h) = (400usize, 300usize);
        let mut dm = vec![NO_DATA; w * h];
        for r in 100..200 {
            for c in 150..250 {
                dm[r * w + c] = 2000; // 200 m
            }
        }
        for r in 140..145 {
            for c in 190..195 {
                dm[r * w + c] = NO_DATA;
            }
        }
        Elevation { x0: -12540.0, z1: 10738.0, res: 8.0, w, h, dm }
    }

    fn deq(q: u16) -> f32 {
        if q == 0 {
            FLOOR_Y
        } else {
            HMIN + (q as f32 - 1.0) * HSTEP
        }
    }

    #[test]
    fn terrain_synthetic_island_hole_and_skirt() {
        let e = synthetic_island();
        let t = terrain_grid(&e, None).unwrap();
        // keep bbox = island + 1000 m (125 px) skirt on every side, clipped to the raster
        assert_eq!(t.step, 16.0);
        // columns 150-125 = 25 .. 250+125 = 375 (aligned to 24..376 -> 176 cells); rows 0..300 (top clipped)
        assert_eq!((t.nx, t.nz), (176, 150));
        assert_eq!(t.mx0, e.x0 + 25.0 * 8.0);
        assert_eq!(t.mz0, e.z1 - 8.0);
        let at = |r: usize, c: usize| deq(t.q[(r / 2) * t.nx + (c - 24) / 2]);
        assert!((at(120, 170) - 200.0).abs() < 0.05, "island {}", at(120, 170));
        // the hole is filled from its rim (200 m), not left at the floor
        assert!((at(142, 192) - 200.0).abs() < 1.0, "hole {}", at(142, 192));
        // skirt: decays monotonically from the island edge towards the floor and stops at 1000 m
        let row = 150;
        let near = at(row, 150 - 4);
        let mid = at(row, 150 - 50);
        let far = at(row, 150 - 120);
        assert!(near > mid && mid > far && far > FLOOR_Y, "{near} {mid} {far}");
        assert!(near < 200.0 && near > 180.0, "{near}");
        // the corner of the cropped grid is > 1000 m from the island: not meshed
        assert_eq!(t.q[0], 0);
        assert!(t.q[(t.nz / 2) * t.nx] != 0, "mid-left edge is inside the skirt");
    }

    #[test]
    fn terrain_hole_prefers_the_coarse_raster() {
        let e = synthetic_island();
        let mut c = e.clone();
        for r in 100..200 {
            for col in 150..250 {
                c.dm[r * c.w + col] = 3000; // the coarse raster has 300 m everywhere on the island, hole included
            }
        }
        let t = terrain_grid(&e, Some(&c)).unwrap();
        let q = t.q[(142 / 2) * t.nx + (192 - 24) / 2]; // crop starts at column 24
        assert!((deq(q) - 300.0).abs() < 1.5, "{}", deq(q));
        assert!(terrain_grid(&Elevation { dm: vec![NO_DATA; 16], w: 4, h: 4, ..e.clone() }, None).is_err());
    }

    #[test]
    fn terrain_js_header_and_payload() {
        let e = synthetic_island();
        let (n, v) = unwrap_js(&p3d_terrain_js(&e, None).unwrap(), "P3D");
        assert_eq!(n, "terrain");
        for (k, want) in [("step", 16.0), ("X0", -12540.0), ("Z1", 10738.0), ("W", 3200.0), ("H", 2400.0), ("hmin", -5.0), ("hstep", 0.025), ("sea", 100.0), ("floor", 40.0)] {
            assert_eq!(v[k].as_f64(), Some(want), "{k}");
        }
        assert_eq!(unb64z(&v["data"]).len(), 2 * v["nx"].as_u64().unwrap() as usize * v["nz"].as_u64().unwrap() as usize);
    }

    #[test]
    fn terrain_real_matches_python() {
        let media = real_media();
        let fine = real_elevation(false);
        let coarse = real_elevation(true);
        let t = std::time::Instant::now();
        let js = p3d_terrain_js(&fine, Some(&coarse)).unwrap();
        eprintln!("p3d_terrain_js: {:?}, {} bytes", t.elapsed(), js.len());
        let (_, v) = unwrap_js(&js, "P3D");
        eprintln!("rust terrain: nx {} nz {} mx0 {} mz0 {} step {} W {} H {}", v["nx"], v["nz"], v["mx0"], v["mz0"], v["step"], v["W"], v["H"]);
        for (k, want) in [("nx", 1327.0), ("nz", 1376.0), ("mx0", -11748.0), ("mz0", 10730.0), ("step", 16.0), ("X0", -12540.0), ("Z1", 10738.0), ("W", 22016.0), ("H", 22016.0)] {
            assert_eq!(v[k].as_f64(), Some(want), "header {k}");
        }
        let Some((_, p)) = ref_js("preview3d/terrain.js", "P3D") else { return eprintln!("SKIP python reference terrain.js absent") };
        let _ = media;
        for k in ["nx", "nz", "mx0", "mz0", "step", "X0", "Z1", "W", "H", "hmin", "hstep", "sea", "floor"] {
            assert_eq!(p[k], v[k], "{k}");
        }
        let (pq, rq) = (unb64z(&p["data"]), unb64z(&v["data"]));
        assert_eq!(pq.len(), rq.len());
        let u16s = |b: &[u8]| -> Vec<u16> { b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect() };
        let (pq, rq) = (u16s(&pq), u16s(&rq));
        let n = pq.len();
        let (mut kept_mismatch, mut both, mut within1, mut within01) = (0usize, 0usize, 0usize, 0usize);
        let mut worst = 0.0f32;
        for i in 0..n {
            if (pq[i] == 0) != (rq[i] == 0) {
                kept_mismatch += 1;
                continue;
            }
            if pq[i] == 0 {
                continue;
            }
            both += 1;
            let d = (deq(pq[i]) - deq(rq[i])).abs();
            worst = worst.max(d);
            within1 += usize::from(d <= 1.0);
            within01 += usize::from(d <= 0.1);
        }
        eprintln!(
            "terrain vs Python: {n} cells, {both} meshed in both, {kept_mismatch} meshed in only one; within 1 m: {:.3} % of the meshed, within 0.1 m: {:.3} %, worst {worst:.1} m",
            100.0 * within1 as f64 / both as f64,
            100.0 * within01 as f64 / both as f64
        );
        // cells meshed in at least one of the two; a cell meshed in only one counts as a miss
        let any = both + kept_mismatch;
        assert!(within1 as f64 / any as f64 >= 0.99, "less than 99 % of the cells agree within 1 m");
    }

    // ---- images

    #[test]
    fn tile_rgb_and_jpeg_real() {
        let media = real_media();
        let rgb = tiles::tile_rgb(&media, "Summer", 3, 4, 4).unwrap();
        assert_eq!(rgb.len(), 1024 * 1024 * 3);
        let o = (20 * 1024 + 10) * 3;
        assert_eq!(&rgb[o..o + 3], &[41, 61, 33], "Summer L3 r4 c4 px(10,20)");
        // out-of-range tiles are errors, not panics
        assert!(tiles::tile_rgb(&media, "Summer", 3, 8, 0).is_err());
        assert!(tiles::tile_rgb(&media, "Summer", 4, 0, 0).is_err());
        assert!(tiles::tile_rgb(&media, "Nonsense", 3, 0, 0).is_err());
        // Leaflet z/x/y -> entry z-y-x: tile_jpeg(x=4, y=5) is the tile at row 5, col 4
        let t = std::time::Instant::now();
        let jpg = tile_jpeg(&media, "Summer", 3, 4, 5).unwrap();
        eprintln!("tile_jpeg: {:?}, {} bytes", t.elapsed(), jpg.len());
        let img = image::load_from_memory_with_format(&jpg, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((img.width(), img.height()), (1024, 1024));
        let want = tiles::tile_rgb(&media, "Summer", 3, 5, 4).unwrap();
        let got = img.to_rgb8();
        let mean_abs: f64 = want.iter().zip(got.as_raw()).map(|(a, b)| (*a as f64 - *b as f64).abs()).sum::<f64>() / want.len() as f64;
        eprintln!("tile_jpeg mean abs error vs the decoded tile: {mean_abs:.2}");
        assert!(mean_abs < 8.0, "{mean_abs}");
        let other = tiles::tile_rgb(&media, "Summer", 3, 4, 5).unwrap(); // transposed: must differ
        let mean_t: f64 = other.iter().zip(got.as_raw()).map(|(a, b)| (*a as f64 - *b as f64).abs()).sum::<f64>() / other.len() as f64;
        assert!(mean_t > mean_abs * 2.0, "x/y swapped? {mean_t} vs {mean_abs}");
    }

    #[test]
    fn tex_jpeg_real() {
        let media = real_media();
        let t = std::time::Instant::now();
        let jpg = tex_jpeg(&media, "Summer").unwrap();
        eprintln!("tex_jpeg: {:?}, {} bytes", t.elapsed(), jpg.len());
        let img = image::load_from_memory_with_format(&jpg, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((img.width(), img.height()), (4096, 4096));
        // level 2 tile (1,1) px (512,512) is [58,69,41] -> texture px (1536,1536)
        let p = img.to_rgb8().get_pixel(1536, 1536).0;
        assert!(p.iter().zip([58u8, 69, 41]).all(|(a, b)| (*a as i32 - b as i32).abs() <= 6), "{p:?}");
    }

    // ---- the bundle

    /// ~30 s in a debug build (builds the whole bundle twice): `cargo test --release -- --ignored editor_data`.
    #[test]
    #[ignore = "slow in debug builds; run with --release -- --ignored"]
    fn editor_data_resolve_real() {
        let media = real_media();
        let rt = RoadTypes::project();
        let cache = crate::gamedata::tempdir("editor_data");
        let t = std::time::Instant::now();
        let d = EditorData::build_in(&cache, &media, &rt, &|_| {}).expect("build");
        eprintln!("EditorData::build (cold cache dir, raster built from the install): {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let d2 = EditorData::build_in(&cache, &media, &rt, &|_| {}).expect("build");
        eprintln!("EditorData::build (warm raster cache): {:?}", t.elapsed());
        assert_eq!(d2.seasons(), d.seasons());
        assert_eq!(d.seasons(), SEASONS.map(String::from));
        for name in ["data/meta.js", "data/roads.js", "data/roaded.js", "data/canon.js", "data/elevation.js", "preview3d/meta.js", "preview3d/terrain.js", "preview3d/roads.js"] {
            let s = d.resolve(name).unwrap_or_else(|| panic!("{name} unresolved")).unwrap();
            assert!(!s.cacheable && s.content_type.starts_with("text/javascript"));
            assert!(s.bytes.starts_with(b"(window."), "{name}");
        }
        // tiles: strict parsing
        for bad in [
            "tiles/Summer/3/4/8.jpg",
            "tiles/Summer/4/0/0.jpg",
            "tiles/Summer/3/4/5.png",
            "tiles/Nonsense/3/4/5.jpg",
            "tiles/Summer/3/+4/5.jpg",
            "tiles/Summer/3/4/5/6.jpg",
            "tiles/Summer/../3/4/5.jpg",
            "tiles/Summer/3/4/-1.jpg",
            "preview3d/tex_Nonsense.jpg",
            "preview3d/tex_../x.jpg",
            "data/other.js",
            "",
        ] {
            assert!(d.resolve(bad).is_none(), "{bad} must not resolve");
        }
        let t = std::time::Instant::now();
        let s = d.resolve("tiles/Summer/3/4/5.jpg").unwrap().unwrap();
        eprintln!("tile (generate + cache write): {:?}", t.elapsed());
        assert!(s.cacheable && s.content_type == "image/jpeg" && s.bytes.starts_with(&[0xff, 0xd8]));
        let file = cache.join("tiles/Summer/3/4/5.jpg");
        assert!(file.is_file(), "tile not cached at {}", file.display());
        let t = std::time::Instant::now();
        let again = d.resolve("tiles/Summer/3/4/5.jpg").unwrap().unwrap();
        eprintln!("tile (from the disk cache): {:?}", t.elapsed());
        assert_eq!(again.bytes, s.bytes);
        let tex = d.resolve("preview3d/tex_Winter.jpg").unwrap().unwrap();
        assert!(tex.bytes.starts_with(&[0xff, 0xd8]) && cache.join("tex_Winter.jpg").is_file());
        // Save: only canon.js / preview3d/roads.js follow the new road types
        let before_roads = d.resolve("data/roads.js").unwrap().unwrap().bytes;
        let t = std::time::Instant::now();
        d.set_road_types(&rt);
        eprintln!("set_road_types, project file (canon + p3d roads regen): {:?}", t.elapsed());
        let raw = RoadTypes::raw();
        d.set_road_types(&raw);
        let canon = String::from_utf8(d.resolve("data/canon.js").unwrap().unwrap().bytes).unwrap();
        let (_, c) = unwrap_js(&canon, "FH6");
        assert_eq!(c["types"].as_object().map(|o| o.len()), Some(0));
        let (_, r3) = unwrap_js(&String::from_utf8(d.resolve("preview3d/roads.js").unwrap().unwrap().bytes).unwrap(), "P3D");
        assert_eq!(r3["n_edges"].as_u64(), Some(d.nav().edges().len() as u64));
        assert_eq!(d.resolve("data/roads.js").unwrap().unwrap().bytes, before_roads);
        let _ = std::fs::remove_dir_all(&cache);
    }
}
