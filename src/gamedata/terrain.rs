//! Terrain elevation of the open world, rasterised at runtime from the user's install; a port
//! of `elevation()` / `raster_height()` in `tools/fh6-extract/extract_terrain.py`. See
//! `docs/game-data/fh6-terrain.md` for the formats and the *why*.
//!
//! The ground heightfield is stored as ~5 400 `scene\tbheightfield\autoterrain_x{X}_z{Z}_{cb|ul}_cluster{N}.i.modelbin`
//! models (512 m cells) inside `Tracks/Brio/GeoChunk0.minizip` (a PGZP archive, see
//! [`super::pgzp`]). Each is a `burG` triangle mesh ([`super::burg`]); they are rasterised
//! into an 8 m grid (the **max** height per pixel, pixel-centre barycentric test), `cb` (the
//! detailed cluster) winning over `ul` where both have data. The coarse whole-island LOD
//! (`scene\uberheightfield\autouberlod_*`, 486 models of 2 048 m cells) fills the same grid as
//! a fallback layer ([`Elevation::build`] with `coarse = true`).
//!
//! **Why 8 m only:** both consumers (the editor's lookup grid, the 3D mesh at 16 m) need no
//! more. **Why cached:** a cold build reads ~215 MB of scattered 40 KB chunks from a 40 GB
//! file (1-2 s on SSD, tens of seconds on a spinning disk), while the raster itself is 15 MB:
//! [`Elevation::load_or_build`] keeps it under `<app_data_dir>/map_editor/cache/`, keyed by
//! the source file's length and mtime. Heavy: call off the UI thread.
//!
//! The cached values are `i16` **decimetres** (`round_ties_even(h * 10)`, `-32768` = no data),
//! exactly the editor's lookup grid.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::burg::{terrain_mesh, Mesh};
use super::install::ci;
use super::pgzp::Pgzp;

/// World extent covered by the raster, m: x from [`X0`] to [`X1`], z from [`Z0`] to [`Z1`]
/// (row 0 = north = [`Z1`]).
pub const X0: f64 = -12540.0;
pub const X1: f64 = 9470.0;
pub const Z0: f64 = -11272.0;
pub const Z1: f64 = 10738.0;
/// Raster resolution, m per pixel.
pub const RES: f64 = 8.0;
/// "No data" in [`Elevation::dm`].
pub const NO_DATA: i16 = i16::MIN;

const CACHE_MAGIC: &[u8; 4] = b"FH6E";
const CACHE_VERSION: u32 = 1;
const CACHE_HEADER: usize = 4 + 4 + 4 + 4 + 4 + 8 + 8 + 8 + 8;

/// An 8 m elevation raster.
#[derive(Clone, Debug)]
pub struct Elevation {
    /// World x of the left edge of column 0.
    pub x0: f64,
    /// World z of the top edge of row 0.
    pub z1: f64,
    /// Pixel size, m.
    pub res: f64,
    pub w: usize,
    pub h: usize,
    /// Row-major heights in decimetres; [`NO_DATA`] where the game has no terrain.
    pub dm: Vec<i16>,
}

/// What a build did (for tests and logs).
#[derive(Clone, Debug, Default)]
pub struct BuildStats {
    /// Model files rasterised.
    pub files: usize,
    pub triangles: usize,
    /// Model files that could not be read or decoded and were skipped.
    pub failed: usize,
}

impl Elevation {
    /// Metres at pixel (`row`, `col`); `None` outside the raster or without data.
    pub fn raw(&self, row: usize, col: usize) -> Option<f32> {
        if row >= self.h || col >= self.w {
            return None;
        }
        match self.dm[row * self.w + col] {
            NO_DATA => None,
            v => Some(v as f32 / 10.0),
        }
    }

    /// World coordinates (x, z) of the centre of pixel (`row`, `col`).
    pub fn centre(&self, row: usize, col: usize) -> (f64, f64) {
        (self.x0 + (col as f64 + 0.5) * self.res, self.z1 - (row as f64 + 0.5) * self.res)
    }

    /// Fraction of pixels that have data.
    pub fn valid_fraction(&self) -> f64 {
        self.dm.iter().filter(|&&v| v != NO_DATA).count() as f64 / self.dm.len().max(1) as f64
    }

    /// Terrain height (m) at world (`x`, `z`): bilinear between the four nearest pixel centres,
    /// pixels without data left out of the weights; `None` when none of them has data. This is
    /// the editor's `terrainY` (`viewer_template.html`), so Rust and JS agree.
    pub fn height(&self, x: f64, z: f64) -> Option<f32> {
        let fx = (x - self.x0) / self.res - 0.5;
        let fz = (self.z1 - z) / self.res - 0.5;
        let (c0, r0) = (fx.floor(), fz.floor());
        let (tx, tz) = (fx - c0, fz - r0);
        let (mut sw, mut sv) = (0.0f64, 0.0f64);
        for dr in 0..2i64 {
            for dc in 0..2i64 {
                let (c, r) = (c0 as i64 + dc, r0 as i64 + dr);
                if c < 0 || r < 0 || c >= self.w as i64 || r >= self.h as i64 {
                    continue;
                }
                let v = self.dm[r as usize * self.w + c as usize];
                if v == NO_DATA {
                    continue;
                }
                let wt = (if dc == 1 { tx } else { 1.0 - tx }) * (if dr == 1 { tz } else { 1.0 - tz });
                sw += wt;
                sv += wt * v as f64 / 10.0;
            }
        }
        (sw > 1e-6).then(|| (sv / sw) as f32)
    }

    /// Load the cached raster of the install, or build (and cache) it. `coarse` selects the
    /// whole-island LOD layer. `progress` gets 0.0..=1.0 (called from worker threads).
    pub fn load_or_build(media: &Path, coarse: bool, progress: &(dyn Fn(f32) + Sync)) -> Result<Elevation, String> {
        Self::load_or_build_in(&cache_dir(), media, coarse, progress)
    }

    /// [`Elevation::load_or_build`] with an explicit cache folder.
    pub fn load_or_build_in(cache: &Path, media: &Path, coarse: bool, progress: &(dyn Fn(f32) + Sync)) -> Result<Elevation, String> {
        let src = geochunk_path(media)?;
        let key = source_key(&src)?;
        let file = cache.join(if coarse { "elevation_coarse_8m.bin" } else { "elevation_8m.bin" });
        if let Some(e) = read_cache(&file, key) {
            progress(1.0);
            return Ok(e);
        }
        let e = Self::build(media, coarse, progress)?;
        if let Err(err) = write_cache(&file, key, &e) {
            eprintln!("elevation cache not written: {err}"); // non-fatal: next start just rebuilds
        }
        Ok(e)
    }

    /// Rasterise from the install (no cache). Multi-threaded; ~2 s on an SSD.
    pub fn build(media: &Path, coarse: bool, progress: &(dyn Fn(f32) + Sync)) -> Result<Elevation, String> {
        Self::build_with_stats(media, coarse, progress).map(|(e, _)| e)
    }

    /// [`Elevation::build`] that also reports the file / triangle counts.
    pub fn build_with_stats(media: &Path, coarse: bool, progress: &(dyn Fn(f32) + Sync)) -> Result<(Elevation, BuildStats), String> {
        progress(0.0);
        let src = geochunk_path(media)?;
        let names = ci(media, "Tracks/Brio/ChunkContentsMiniZip0.txt")
            .filter(|p| p.is_file())
            .ok_or("no ChunkContentsMiniZip0.txt in the install")?;
        let keep = |n: &str| if coarse { parse_name_coarse(n).is_some() } else { parse_name(n).is_some() };
        let pg = Pgzp::open(&src, &names, &keep)?;
        progress(0.03);
        let (g, stats) = rasterise(&pg, coarse, progress)?;
        progress(1.0);
        Ok((g, stats))
    }
}

/// `<app_data_dir>/map_editor/cache`.
pub fn cache_dir() -> PathBuf {
    crate::config::app_data_dir().join("map_editor").join("cache")
}

fn geochunk_path(media: &Path) -> Result<PathBuf, String> {
    ci(media, "Tracks/Brio/GeoChunk0.minizip").filter(|p| p.is_file()).ok_or_else(|| "no GeoChunk0.minizip in the install".to_owned())
}

/// (length, mtime in seconds) of the source archive: a changed game file invalidates the cache.
fn source_key(src: &Path) -> Result<(u64, u64), String> {
    let m = std::fs::metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let secs = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    Ok((m.len(), secs))
}

fn read_cache(path: &Path, key: (u64, u64)) -> Option<Elevation> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = [0u8; CACHE_HEADER];
    f.read_exact(&mut h).ok()?;
    let u32at = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
    let u64at = |o: usize| u64::from_le_bytes(h[o..o + 8].try_into().unwrap());
    if &h[0..4] != CACHE_MAGIC || u32at(4) != CACHE_VERSION {
        return None;
    }
    let (w, hh, res) = (u32at(8) as usize, u32at(12) as usize, f32::from_le_bytes(h[16..20].try_into().unwrap()) as f64);
    let (x0, z1) = (f64::from_le_bytes(h[20..28].try_into().unwrap()), f64::from_le_bytes(h[28..36].try_into().unwrap()));
    if (u64at(36), u64at(44)) != key || w == 0 || hh == 0 || w > 16384 || hh > 16384 {
        return None;
    }
    let mut raw = Vec::with_capacity(w * hh * 2);
    f.take((w * hh * 2 + 1) as u64).read_to_end(&mut raw).ok()?;
    if raw.len() != w * hh * 2 {
        return None;
    }
    let dm = raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    Some(Elevation { x0, z1, res, w, h: hh, dm })
}

fn write_cache(path: &Path, key: (u64, u64), e: &Elevation) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut buf = Vec::with_capacity(CACHE_HEADER + e.dm.len() * 2);
    buf.extend_from_slice(CACHE_MAGIC);
    buf.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    buf.extend_from_slice(&(e.w as u32).to_le_bytes());
    buf.extend_from_slice(&(e.h as u32).to_le_bytes());
    buf.extend_from_slice(&(e.res as f32).to_le_bytes());
    buf.extend_from_slice(&e.x0.to_le_bytes());
    buf.extend_from_slice(&e.z1.to_le_bytes());
    buf.extend_from_slice(&key.0.to_le_bytes());
    buf.extend_from_slice(&key.1.to_le_bytes());
    buf.extend(e.dm.iter().flat_map(|v| v.to_le_bytes()));
    // atomic: write a sibling temp file, then rename over the target
    let tmp = path.with_extension("tmp");
    std::fs::File::create(&tmp)?.write_all(&buf)?;
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------------------------------------ file names

/// Centre of the 512 m cell a model named `x{fx}_z{fz}` belongs to (names carry a 1023-based
/// tile coordinate; cells are 512 m).
pub fn cell_of(fx: i64, fz: i64) -> (f64, f64) {
    (((fx as f64) / 1023.0).round() * 512.0, ((fz as f64) / 1023.0).round() * 512.0)
}

/// `scene\tbheightfield\autoterrain_x{X}_z{Z}_{cb|ul}_cluster{N}.i.modelbin` → `(X, Z, is_cb)`.
pub fn parse_name(n: &str) -> Option<(i64, i64, bool)> {
    let r = n.strip_suffix(".i.modelbin")?;
    let r = r.rsplit_once("tbheightfield\\autoterrain_x")?.1;
    let (x, r) = r.split_once("_z")?;
    let (z, r) = r.split_once('_')?;
    let (kind, cluster) = r.split_once("_cluster")?;
    if cluster.is_empty() || !cluster.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let cb = match kind {
        "cb" => true,
        "ul" => false,
        _ => return None,
    };
    Some((x.parse().ok()?, z.parse().ok()?, cb))
}

/// `scene\uberheightfield\autouberlod_x{X}_z{Z}_cluster{N}.i.modelbin` → `(X, Z)` (the coarse
/// whole-island LOD, 2 048 m cells).
pub fn parse_name_coarse(n: &str) -> Option<(i64, i64)> {
    let r = n.strip_suffix(".i.modelbin")?;
    let r = r.rsplit_once("uberheightfield\\autouberlod_x")?.1;
    let (x, r) = r.split_once("_z")?;
    let (z, cluster) = r.split_once("_cluster")?;
    if cluster.is_empty() || !cluster.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((x.parse().ok()?, z.parse().ok()?))
}

/// Does the cell (`cx`, `cz`) of edge length `size` overlap the raster extent?
fn in_region(cx: f64, cz: f64, size: f64) -> bool {
    !(cx + size < X0 || cx > X1 || cz + size < Z0 || cz > Z1)
}

// ------------------------------------------------------------------------------------------------ rasterising

/// A float grid in the raster's coordinates (NaN = empty); used for the full raster and for
/// the per-model patches.
struct Grid {
    w: usize,
    h: usize,
    res: f64,
    x0: f64,
    z1: f64,
    a: Vec<f32>,
}

impl Grid {
    fn new(w: usize, h: usize, res: f64, x0: f64, z1: f64) -> Grid {
        Grid { w, h, res, x0, z1, a: vec![f32::NAN; w * h] }
    }
}

/// Max-height rasterisation of one mesh into `g`: pixel centres at +0.5, barycentric inside
/// test with 1e-6 slack, the highest triangle wins (bridges / overlapping layers).
fn raster_into(m: &Mesh, g: &mut Grid) {
    let (w, h, res) = (g.w as i64, g.h as i64, g.res);
    for t in &m.t {
        let p = |i: u32| {
            let v = m.v[i as usize];
            ((v[0] as f64 - g.x0) / res, (g.z1 - v[2] as f64) / res, v[1] as f64)
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        let minx = ((a.0.min(b.0).min(c.0) - 0.5).floor() as i64).max(0);
        let maxx = ((a.0.max(b.0).max(c.0) - 0.5).ceil() as i64).min(w - 1);
        let minz = ((a.1.min(b.1).min(c.1) - 0.5).floor() as i64).max(0);
        let maxz = ((a.1.max(b.1).max(c.1) - 0.5).ceil() as i64).min(h - 1);
        if minx > maxx || minz > maxz {
            continue;
        }
        let d = (b.1 - c.1) * (a.0 - c.0) + (c.0 - b.0) * (a.1 - c.1);
        if d.abs() < 1e-9 {
            continue;
        }
        for gz in minz..=maxz {
            for gx in minx..=maxx {
                let (fx, fz) = (gx as f64 + 0.5, gz as f64 + 0.5);
                let l1 = ((b.1 - c.1) * (fx - c.0) + (c.0 - b.0) * (fz - c.1)) / d;
                let l2 = ((c.1 - a.1) * (fx - c.0) + (a.0 - c.0) * (fz - c.1)) / d;
                let l3 = 1.0 - l1 - l2;
                if l1 >= -1e-6 && l2 >= -1e-6 && l3 >= -1e-6 {
                    let y = (l1 * a.2 + l2 * b.2 + l3 * c.2) as f32;
                    let cell = &mut g.a[gz as usize * g.w + gx as usize];
                    if cell.is_nan() || y > *cell {
                        *cell = y;
                    }
                }
            }
        }
    }
}

/// Rasterise every selected model of `pg` (multi-threaded, entries in archive order).
fn rasterise(pg: &Pgzp, coarse: bool, progress: &(dyn Fn(f32) + Sync)) -> Result<(Elevation, BuildStats), String> {
    let w = ((X1 - X0) / RES).ceil() as usize;
    let h = ((Z1 - Z0) / RES).ceil() as usize;
    // (entry index, is_cb) of the models overlapping the extent
    let mut sel: Vec<(usize, bool)> = Vec::new();
    for (i, n) in pg.names() {
        let hit = if coarse { parse_name_coarse(n).map(|(x, z)| (x, z, false)) } else { parse_name(n) };
        if let Some((x, z, cb)) = hit {
            let (cx, cz) = cell_of(x, z);
            if in_region(cx, cz, if coarse { 2048.0 } else { 512.0 }) {
                sel.push((*i, cb));
            }
        }
    }
    if sel.is_empty() {
        return Err("no terrain models found in the archive".into());
    }
    let order = pg.sorted(sel.iter().map(|s| s.0).collect());
    let kinds: HashMap<usize, bool> = sel.into_iter().collect();
    let total = order.len();
    let grids = Mutex::new((Grid::new(w, h, RES, X0, Z1), Grid::new(w, h, RES, X0, Z1))); // (cb, ul)
    let next = AtomicUsize::new(0);
    let (tris, failed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let first_err: Mutex<Option<String>> = Mutex::new(None);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut f = match pg.open_file() {
                    Ok(f) => f,
                    Err(e) => {
                        first_err.lock().unwrap().get_or_insert(e);
                        return;
                    }
                };
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= total {
                        break;
                    }
                    if k % 64 == 0 {
                        progress(0.03 + 0.96 * k as f32 / total as f32);
                    }
                    let i = order[k];
                    let mesh = pg
                        .entry(&mut f, i)
                        .and_then(|d| terrain_mesh(&d, &|n| n.starts_with("GenMaterial_UberMap")).ok_or_else(|| format!("entry {i}: not a terrain model")));
                    let m = match mesh {
                        Ok(m) => m,
                        Err(e) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            first_err.lock().unwrap().get_or_insert(e);
                            continue;
                        }
                    };
                    if m.t.is_empty() {
                        continue;
                    }
                    tris.fetch_add(m.t.len(), Ordering::Relaxed);
                    // Rasterise into a thread-local patch around the mesh, then max-merge under the lock.
                    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
                    for v in &m.v {
                        lo[0] = lo[0].min(v[0]);
                        hi[0] = hi[0].max(v[0]);
                        lo[1] = lo[1].min(v[2]);
                        hi[1] = hi[1].max(v[2]);
                    }
                    let px0 = (((lo[0] as f64 - X0) / RES).floor() as i64 - 1).max(0);
                    let px1 = (((hi[0] as f64 - X0) / RES).ceil() as i64 + 1).min(w as i64 - 1);
                    let pz0 = (((Z1 - hi[1] as f64) / RES).floor() as i64 - 1).max(0);
                    let pz1 = (((Z1 - lo[1] as f64) / RES).ceil() as i64 + 1).min(h as i64 - 1);
                    if px0 > px1 || pz0 > pz1 {
                        continue;
                    }
                    let mut patch = Grid::new((px1 - px0 + 1) as usize, (pz1 - pz0 + 1) as usize, RES, X0 + px0 as f64 * RES, Z1 - pz0 as f64 * RES);
                    raster_into(&m, &mut patch);
                    let mut g = grids.lock().unwrap();
                    let dst = if kinds[&i] { &mut g.0 } else { &mut g.1 };
                    for r in 0..patch.h {
                        for c in 0..patch.w {
                            let y = patch.a[r * patch.w + c];
                            if y.is_nan() {
                                continue;
                            }
                            let cell = &mut dst.a[(pz0 as usize + r) * w + px0 as usize + c];
                            if cell.is_nan() || y > *cell {
                                *cell = y;
                            }
                        }
                    }
                }
            });
        }
    });
    let failed = failed.load(Ordering::Relaxed);
    if failed * 20 > total {
        return Err(format!("{failed} of {total} terrain models failed: {}", first_err.lock().unwrap().clone().unwrap_or_default()));
    }
    let (cb, ul) = grids.into_inner().unwrap();
    let dm = cb
        .a
        .iter()
        .zip(&ul.a)
        .map(|(c, u)| {
            let v = if c.is_nan() { *u } else { *c };
            if v.is_nan() {
                NO_DATA
            } else {
                (v * 10.0).round_ties_even().clamp(-32767.0, 32767.0) as i16
            }
        })
        .collect();
    let stats = BuildStats { files: total - failed, triangles: tris.load(Ordering::Relaxed), failed };
    Ok((Elevation { x0: X0, z1: Z1, res: RES, w, h, dm }, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;

    #[test]
    fn name_parsing() {
        let n = "scene\\tbheightfield\\autoterrain_x-1023_z2046_cb_cluster12.i.modelbin";
        assert_eq!(parse_name(n), Some((-1023, 2046, true)));
        assert_eq!(parse_name("scene\\tbheightfield\\autoterrain_x0_z0_ul_cluster0.i.modelbin"), Some((0, 0, false)));
        assert_eq!(parse_name("scene\\tbheightfield\\autoterrain_x0_z0_zz_cluster0.i.modelbin"), None);
        assert_eq!(parse_name("scene\\tbheightfield\\autoterrain_x0_z0_cb_cluster.i.modelbin"), None);
        assert_eq!(parse_name("scene\\other\\thing.i.modelbin"), None);
        assert_eq!(parse_name_coarse("scene\\uberheightfield\\autouberlod_x2046_z-2046_cluster3.i.modelbin"), Some((2046, -2046)));
        assert_eq!(parse_name_coarse(n), None);
        // 1023-based names -> 512 m cells
        assert_eq!(cell_of(1023, -1023), (512.0, -512.0));
        assert_eq!(cell_of(0, 2046), (0.0, 1024.0));
    }

    #[test]
    fn region_test() {
        assert!(in_region(0.0, 0.0, 512.0));
        assert!(!in_region(10_000.0, 0.0, 512.0));
        assert!(in_region(-12540.0 - 512.0, 0.0, 512.0)); // touching the west edge
        assert!(!in_region(0.0, -12000.0, 512.0));
    }

    /// One triangle (a sloped plane) rasterised at 8 m: pixel centres inside get the plane's
    /// height; outside stay NaN; a second, higher triangle wins (max).
    #[test]
    fn raster_one_triangle() {
        let mut g = Grid::new(10, 10, 8.0, 0.0, 80.0); // x 0..80, z 0..80, row 0 = z 80
        // height = x / 10 (so 0..8 m across the 80 m): vertices at (0,0), (80,0), (0,80) world
        let m = Mesh { v: vec![[0.0, 0.0, 0.0], [80.0, 8.0, 0.0], [0.0, 0.0, 80.0]], t: vec![[0, 1, 2]] };
        raster_into(&m, &mut g);
        // pixel (row 9, col 0): centre x=4, z=4 -> inside (x + z < 80), height 0.4
        assert!((g.a[9 * 10] - 0.4).abs() < 1e-4, "{}", g.a[9 * 10]);
        // pixel (row 0, col 9): centre x=76, z=76 -> outside the hypotenuse
        assert!(g.a[9].is_nan());
        // pixel (row 9, col 8): centre x=68, z=4 -> inside, height 6.8
        assert!((g.a[9 * 10 + 8] - 6.8).abs() < 1e-4);
        let hi = Mesh { v: vec![[0.0, 50.0, 0.0], [80.0, 50.0, 0.0], [0.0, 50.0, 80.0]], t: vec![[0, 1, 2]] };
        raster_into(&hi, &mut g);
        assert!((g.a[9 * 10] - 50.0).abs() < 1e-4);
    }

    fn sample() -> Elevation {
        // 3x3 raster, 8 m, x0 = 0, z1 = 24; heights in dm: row 0: 10 20 30 / row 1: 10 20 NO / row 2: all NO
        let dm = vec![10, 20, 30, 10, 20, NO_DATA, NO_DATA, NO_DATA, NO_DATA];
        Elevation { x0: 0.0, z1: 24.0, res: 8.0, w: 3, h: 3, dm }
    }

    #[test]
    fn bilinear_height() {
        let e = sample();
        assert_eq!(e.raw(0, 1), Some(2.0));
        assert_eq!(e.raw(2, 2), None);
        assert_eq!(e.raw(5, 0), None);
        // exactly at a pixel centre = the pixel's value
        let (x, z) = e.centre(0, 1);
        assert!((e.height(x, z).unwrap() - 2.0).abs() < 1e-5);
        // halfway between centres (0,0) = 1.0 and (0,1) = 2.0
        let (x, z) = e.centre(0, 0);
        assert!((e.height(x + 4.0, z).unwrap() - 1.5).abs() < 1e-5);
        // between (1,1)=2.0 and (1,2)=NO_DATA: the missing pixel is left out of the weights
        let (x, z) = e.centre(1, 1);
        assert!((e.height(x + 4.0, z).unwrap() - 2.0).abs() < 1e-5);
        // the bottom row has no data and is ignored; far outside -> None
        assert_eq!(e.height(-100.0, 0.0), None);
        assert_eq!(e.height(4.0, 4.0), None);
        assert!((e.valid_fraction() - 5.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn cache_roundtrip_and_invalidation() {
        let dir = crate::gamedata::tempdir("elevcache");
        let f = dir.join("sub").join("e.bin");
        let e = sample();
        write_cache(&f, (123, 456), &e).unwrap();
        let back = read_cache(&f, (123, 456)).expect("hit");
        assert_eq!((back.w, back.h, back.x0, back.z1, back.res), (3, 3, 0.0, 24.0, 8.0));
        assert_eq!(back.dm, e.dm);
        assert!(read_cache(&f, (123, 457)).is_none(), "mtime/length mismatch invalidates");
        assert!(read_cache(&f, (124, 456)).is_none());
        std::fs::write(&f, b"FH6E garbage").unwrap();
        assert!(read_cache(&f, (123, 456)).is_none());
        assert!(read_cache(&dir.join("missing.bin"), (1, 2)).is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Reference heights of the Python raster (`extract_terrain.py` output, 8 m,
    /// `/home/mo/fh6-viewer-work/terr_e/elevation.npy`), sampled at the pixel centre of the cell
    /// containing (x, z); the Rust prototype matched that raster to 0.051 m.
    const REFERENCE: [(f64, f64, f32); 10] = [
        (0.0, 0.0, 198.561),
        (1000.0, 2000.0, 127.067),
        (-4000.0, 6800.0, 504.822),
        (2956.0, 1051.0, 277.128),
        (-4229.0, -5248.0, 342.127),
        (5000.0, -3000.0, 90.011),
        (100.0, -100.0, 175.346),
        (-2000.0, 500.0, 375.795),
        (6000.0, 4000.0, 91.244),
        (-6000.0, -2000.0, 234.842),
    ];

    fn check_reference(e: &Elevation) {
        assert_eq!((e.w, e.h), (2752, 2752));
        for (x, z, want) in REFERENCE {
            let (c, r) = (((x - e.x0) / e.res).floor() as usize, ((e.z1 - z) / e.res).floor() as usize);
            let got = e.raw(r, c).unwrap_or_else(|| panic!("no data at ({x},{z})"));
            assert!((got - want).abs() < 0.5, "({x},{z}): got {got}, want {want}");
        }
        // the one missing 512 m cell and open sea have no data
        for (x, z) in [(3830.0, -4870.0), (-8000.0, 8000.0)] {
            let (c, r) = (((x - e.x0) / e.res).floor() as usize, ((e.z1 - z) / e.res).floor() as usize);
            assert_eq!(e.raw(r, c), None, "({x},{z}) should have no data");
        }
        assert!((e.valid_fraction() - 0.6029).abs() < 0.0005, "valid fraction {}", e.valid_fraction());
    }

    /// Full cold build against the real install, compared with the Python reference values.
    /// Multi-threaded and reads ~215 MB: fast in release, slow in debug (decode + raster), so
    /// ignored by default: `cargo test --release -- --ignored real_install`.
    #[test]
    #[ignore = "reads ~215 MB of the install and rasterises 5.9 M triangles; run with --release -- --ignored"]
    fn real_install_elevation_reference() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_elevation_reference: FH6 install not found");
            return;
        };
        let t = std::time::Instant::now();
        let (e, stats) = Elevation::build_with_stats(&media, false, &|_| {}).expect("build");
        eprintln!("elevation build {:?}: {stats:?}", t.elapsed());
        assert_eq!(stats.triangles, 5_852_162);
        assert_eq!((stats.files, stats.failed), (5431, 0));
        check_reference(&e);
        // cache roundtrip through a temp dir: second call is a cache hit with identical data
        let dir = crate::gamedata::tempdir("elevreal");
        let a = Elevation::load_or_build_in(&dir, &media, false, &|_| {}).unwrap();
        let t = std::time::Instant::now();
        let b = Elevation::load_or_build_in(&dir, &media, false, &|_| {}).unwrap();
        eprintln!("cache hit in {:?}", t.elapsed());
        assert_eq!(a.dm, e.dm);
        assert_eq!(a.dm, b.dm);
        std::fs::remove_dir_all(dir).ok();
    }

    /// The coarse LOD layer: 486 models, 844 322 triangles.
    #[test]
    #[ignore = "reads 22 MB of the install; run with --release -- --ignored"]
    fn real_install_coarse_reference() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_coarse_reference: FH6 install not found");
            return;
        };
        let (e, stats) = Elevation::build_with_stats(&media, true, &|_| {}).expect("build");
        eprintln!("coarse: {stats:?}, valid {:.4}", e.valid_fraction());
        assert_eq!((stats.files, stats.triangles, stats.failed), (486, 844_322, 0));
        assert_eq!((e.w, e.h), (2752, 2752));
    }
}
