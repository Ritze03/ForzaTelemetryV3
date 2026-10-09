//! The 3D renderer's terrain on the CPU (phase K, K1): a **filled** 8 m height grid ([`HeightGrid`]),
//! built from the game's [`Elevation`] rasters, plus the hole fill / sea skirt that the map
//! editor's 3D page already uses (moved here from `mapedit::data`, which calls them back).
//!
//! # Why a second grid next to [`Elevation`]
//!
//! [`Elevation`] is the raw raster: 39.7 % of its pixels have no data (open sea plus 374 small
//! holes). A renderer needs a height for every pixel, so [`build_height_grid`] fills them:
//!
//! * **holes** (no-data regions of at most [`SEA_MIN_PX`] px, 4-connected) come from the coarse
//!   `uberheightfield` raster where it has data, else are interpolated from the rim
//!   ([`fill_holes`]);
//! * **open sea** (the larger no-data regions) becomes a *skirt*: the height of the nearest land
//!   decaying to the fill height over [`SKIRT_M`] ([`apply_skirt`]). The editor's page fills with
//!   [`FLOOR_Y`] (40 m, a sea floor under a translucent plane); the 3D map fills with
//!   [`SEA_Y`] = 100 m, the real sea level (`docs/game-data/fh6-terrain.md`), so open water is
//!   a flat surface at the right height that carries the satellite's water tint and there is no
//!   cliff at the coast. *Why not the editor's floor:* the HUD would need a second, translucent
//!   sea mesh and shows a drop-off the game does not have.
//!
//! The grid is `u16` in 0.1 m steps from -10 m (`h = q * 0.1 - 10`, range -10..6543 m, the
//! island is -1.8..1473 m): 15 MB for 2752 x 2752, the same bytes the GPU gets as an `R16UI`
//! texture (K2), so CPU lookups ([`Terrain::height`], bilinear between pixel centres exactly as
//! the shader does) and the picture agree.
//!
//! Pure CPU, no GL, no egui.

#![allow(dead_code)] // phase K: consumed by the GL renderer (K2) and the call sites (K3, K4); tested here

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::gamedata::terrain::{Elevation, NO_DATA};

/// Metres of smooth skirt beyond the island data.
pub const SKIRT_M: f32 = 1000.0;
/// No-data regions larger than this (px, 4-connected) are open sea, the rest holes.
pub const SEA_MIN_PX: usize = 20_000;
/// Sea level of the world (FH6's water surface), m: the fill height of the 3D map's open sea.
pub const SEA_Y: f32 = 100.0;
/// The map editor's 3D page fills open sea with this (a floor under a translucent plane).
pub const FLOOR_Y: f32 = 40.0;
/// `h = q * H_STEP + H_OFF`.
pub const H_OFF: f32 = -10.0;
pub const H_STEP: f32 = 0.1;

// ── hole fill + skirt (shared with the map editor) ───────────────────────────────────────────

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
pub(crate) fn fill_holes(h: &mut [f32], w: usize, hh: usize, coarse: Option<&Elevation>) -> Vec<bool> {
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
pub(crate) fn nearest_land(sea: &[bool], w: usize, hh: usize) -> Vec<[i16; 2]> {
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

/// Open sea beyond the data: every `sea` pixel gets the nearest land height decaying (squared
/// ramp) to `fill` over [`SKIRT_M`], a smooth skirt with neither cliff nor pit. `keep`, when
/// given, is cleared for the pixels further than [`SKIRT_M`] from land (the editor meshes only
/// the kept part). Does nothing without sea pixels.
pub(crate) fn apply_skirt(h: &mut [f32], sea: &[bool], w: usize, hh: usize, res: f64, fill: f32, mut keep: Option<&mut [bool]>) {
    if !sea.iter().any(|&s| s) {
        return;
    }
    let off = nearest_land(sea, w, hh);
    for i in 0..w * hh {
        if !sea[i] {
            continue;
        }
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        let [dx, dy] = off[i];
        let (nx, ny) = (x + dx as isize, y + dy as isize);
        let dist = ((dx as f32).powi(2) + (dy as f32).powi(2)).sqrt() * res as f32;
        let near = if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < hh { h[ny as usize * w + nx as usize] } else { f32::NAN };
        let wt = (1.0 - dist / SKIRT_M).clamp(0.0, 1.0).powi(2);
        h[i] = if near.is_nan() { fill } else { fill + (near - fill) * wt };
        if let Some(k) = keep.as_deref_mut() {
            k[i] = dist <= SKIRT_M;
        }
    }
}

// ── the grid ─────────────────────────────────────────────────────────────────────────────────

/// A filled height raster: every pixel has a height. Row 0 = north (`z1`), as [`Elevation`].
#[derive(Clone)]
pub struct HeightGrid {
    pub w: usize,
    pub h: usize,
    /// World x of the left edge of column 0.
    pub x0: f64,
    /// World z of the top edge of row 0.
    pub z1: f64,
    /// Pixel size, m.
    pub res: f64,
    /// Row-major; `height = q * H_STEP + H_OFF`.
    pub q: Vec<u16>,
}

impl std::fmt::Debug for HeightGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeightGrid {{ {}x{} @ {} m, x0 {}, z1 {} }}", self.w, self.h, self.res, self.x0, self.z1)
    }
}

impl HeightGrid {
    /// Quantise a metre grid.
    pub fn from_metres(h: &[f32], w: usize, hh: usize, x0: f64, z1: f64, res: f64) -> HeightGrid {
        let q = h.iter().map(|&v| (((v - H_OFF) / H_STEP).round()).clamp(0.0, 65535.0) as u16).collect();
        HeightGrid { w, h: hh, x0, z1, res, q }
    }

    /// Height of pixel (`col`, `row`), edge-clamped.
    pub fn at_px(&self, col: isize, row: isize) -> f32 {
        let c = col.clamp(0, self.w as isize - 1) as usize;
        let r = row.clamp(0, self.h as isize - 1) as usize;
        self.q[r * self.w + c] as f32 * H_STEP + H_OFF
    }

    /// Height at world (`x`, `z`): bilinear between the four nearest pixel centres, edge-clamped
    /// outside the raster. The same maths as the vertex shader's `texelFetch` + manual bilinear.
    pub fn height(&self, x: f32, z: f32) -> f32 {
        let fx = (x as f64 - self.x0) / self.res - 0.5;
        let fz = (self.z1 - z as f64) / self.res - 0.5;
        let (c0, r0) = (fx.floor(), fz.floor());
        let (tx, tz) = ((fx - c0) as f32, (fz - r0) as f32);
        let (c0, r0) = (c0 as isize, r0 as isize);
        let a = self.at_px(c0, r0) * (1.0 - tx) + self.at_px(c0 + 1, r0) * tx;
        let b = self.at_px(c0, r0 + 1) * (1.0 - tx) + self.at_px(c0 + 1, r0 + 1) * tx;
        a * (1.0 - tz) + b * tz
    }

    /// World extent `[min_x, min_z, max_x, max_z]`.
    pub fn bounds(&self) -> [f32; 4] {
        [
            self.x0 as f32,
            (self.z1 - self.h as f64 * self.res) as f32,
            (self.x0 + self.w as f64 * self.res) as f32,
            self.z1 as f32,
        ]
    }

    /// Lowest and highest height in the box `[min_x, min_z, max_x, max_z]` (raster pixels whose
    /// centres lie in it, sampled every `stride` px; the box is clamped to the raster). `None`
    /// for an empty box.
    pub fn range_in(&self, bbox: [f32; 4], stride: usize) -> Option<(f32, f32)> {
        let stride = stride.max(1);
        let c0 = (((bbox[0] as f64 - self.x0) / self.res).floor().max(0.0)) as usize;
        let c1 = ((((bbox[2] as f64 - self.x0) / self.res).ceil()).min(self.w as f64 - 1.0)).max(0.0) as usize;
        let r0 = (((self.z1 - bbox[3] as f64) / self.res).floor().max(0.0)) as usize;
        let r1 = ((((self.z1 - bbox[1] as f64) / self.res).ceil()).min(self.h as f64 - 1.0)).max(0.0) as usize;
        if c0 > c1 || r0 > r1 || (bbox[0] as f64) > self.x0 + self.w as f64 * self.res || (bbox[2] as f64) < self.x0 {
            return None;
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        let mut r = r0;
        while r <= r1 {
            let mut c = c0;
            while c <= c1 {
                let v = self.q[r * self.w + c] as f32 * H_STEP + H_OFF;
                lo = lo.min(v);
                hi = hi.max(v);
                c += stride;
            }
            r += stride;
        }
        (lo <= hi).then_some((lo, hi))
    }
}

/// Fill the raster (holes from `coarse` / interpolation, open sea = skirt decaying to
/// [`SEA_Y`]) and quantise. `Err` for an empty raster or one without any data.
pub fn build_height_grid(e: &Elevation, coarse: Option<&Elevation>) -> Result<HeightGrid, String> {
    let (w, hh) = (e.w, e.h);
    if w == 0 || hh == 0 || e.dm.len() != w * hh {
        return Err("empty elevation raster".into());
    }
    let mut h: Vec<f32> = e.dm.iter().map(|&v| if v == NO_DATA { f32::NAN } else { v as f32 / 10.0 }).collect();
    if h.iter().all(|v| v.is_nan()) {
        return Err("the elevation raster has no data".into());
    }
    let sea = fill_holes(&mut h, w, hh, coarse);
    apply_skirt(&mut h, &sea, w, hh, e.res, SEA_Y, None);
    for v in h.iter_mut() {
        if v.is_nan() {
            *v = SEA_Y; // a hole nothing could fill (no neighbours at all)
        }
    }
    Ok(HeightGrid::from_metres(&h, w, hh, e.x0, e.z1, e.res))
}

// ── the shared handle ────────────────────────────────────────────────────────────────────────

static REV: AtomicU64 = AtomicU64::new(1);

/// What the 3D maps share: the filled grid plus a revision (unique per built terrain in this
/// process) for "is it the same terrain" checks in caches. Shared as `Arc<Terrain>`.
#[derive(Clone, Debug)]
pub struct Terrain {
    pub grid: HeightGrid,
    pub rev: u64,
}

impl Terrain {
    pub fn from_grid(grid: HeightGrid) -> Terrain {
        Terrain { grid, rev: REV.fetch_add(1, Ordering::Relaxed) }
    }

    /// Build from the install: the cached fine raster (built and cached when missing, ~2 s cold on
    /// an SSD) and the coarse one that fills its holes (a failure there only means interpolated
    /// holes). `progress` reports the fine raster's build. Heavy: call off the UI thread.
    pub fn load(media: &Path, progress: &(dyn Fn(f32) + Sync)) -> Result<Terrain, String> {
        Self::load_in(&crate::gamedata::terrain::cache_dir(), media, progress)
    }

    /// [`Terrain::load`] with an explicit cache folder.
    pub fn load_in(cache: &Path, media: &Path, progress: &(dyn Fn(f32) + Sync)) -> Result<Terrain, String> {
        let fine = Elevation::load_or_build_in(cache, media, false, progress)?;
        let coarse = match Elevation::load_or_build_in(cache, media, true, &|_| {}) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("3D terrain: no coarse raster, holes are interpolated: {e}");
                None
            }
        };
        Ok(Terrain::from_grid(build_height_grid(&fine, coarse.as_ref())?))
    }

    /// Terrain height (m) at world (`x`, `z`), see [`HeightGrid::height`].
    #[inline]
    pub fn height(&self, x: f32, z: f32) -> f32 {
        self.grid.height(x, z)
    }

    /// A small hand-made terrain for tests and the PNG harness (no install): flat ground at
    /// [`SEA_Y`] with three Gaussian hills — a 220 m peak at (-300, 200) with sigma 140 m, an 80 m
    /// one at (350, -250) with sigma 90 m, a 45 m one at (50, 450) with sigma 60 m — on a
    /// 2048 m square around the origin at 8 m.
    #[cfg(test)]
    pub fn synthetic() -> Terrain {
        let (w, res) = (256usize, 8.0f64);
        let (x0, z1) = (-(w as f64) * res / 2.0, w as f64 * res / 2.0);
        let hills = [(-300.0f32, 200.0f32, 220.0f32, 140.0f32), (350.0, -250.0, 80.0, 90.0), (50.0, 450.0, 45.0, 60.0)];
        let mut h = vec![SEA_Y; w * w];
        for r in 0..w {
            for c in 0..w {
                let (x, z) = ((x0 + (c as f64 + 0.5) * res) as f32, (z1 - (r as f64 + 0.5) * res) as f32);
                for (hx, hz, a, s) in hills {
                    let d2 = (x - hx).powi(2) + (z - hz).powi(2);
                    h[r * w + c] += a * (-d2 / (2.0 * s * s)).exp();
                }
            }
        }
        Terrain::from_grid(HeightGrid::from_metres(&h, w, w, x0, z1, res))
    }

    /// A flat terrain at `y` over the same extent as [`Terrain::synthetic`] (tests of the h = 0
    /// camera parity).
    #[cfg(test)]
    pub fn flat(y: f32) -> Terrain {
        let (w, res) = (64usize, 32.0f64);
        Terrain::from_grid(HeightGrid::from_metres(&vec![y; w * w], w, w, -(w as f64) * res / 2.0, w as f64 * res / 2.0, res))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elev(w: usize, h: usize, f: impl Fn(usize, usize) -> i16) -> Elevation {
        Elevation { x0: 0.0, z1: h as f64 * 8.0, res: 8.0, w, h, dm: (0..w * h).map(|i| f(i / w, i % w)).collect() }
    }

    #[test]
    fn height_is_bilinear_between_pixel_centres_and_edge_clamped() {
        // 4 x 4, height = 10 * col (m): a ramp along x.
        let g = HeightGrid::from_metres(&(0..16).map(|i| 10.0 * (i % 4) as f32).collect::<Vec<_>>(), 4, 4, 0.0, 32.0, 8.0);
        // Pixel centres are at x = 4, 12, 20, 28.
        assert!((g.height(4.0, 16.0) - 0.0).abs() < 0.06);
        assert!((g.height(12.0, 16.0) - 10.0).abs() < 0.06);
        assert!((g.height(8.0, 20.0) - 5.0).abs() < 0.06);
        // Outside: clamped to the edge column.
        assert!((g.height(-500.0, 16.0) - 0.0).abs() < 0.06 && (g.height(900.0, 16.0) - 30.0).abs() < 0.06);
        // Quantised to 0.1 m.
        let q = HeightGrid::from_metres(&[1.234], 1, 1, 0.0, 8.0, 8.0);
        assert!((q.height(4.0, 4.0) - 1.2).abs() < 1e-4);
    }

    #[test]
    fn sea_becomes_a_skirt_decaying_to_sea_level_and_holes_are_filled() {
        // 400 x 100: land (50 m) on the left quarter with a 3 x 3 hole, no data (sea) elsewhere.
        let e = elev(400, 100, |r, c| if c >= 100 || ((10..13).contains(&r) && (10..13).contains(&c)) { NO_DATA } else { 500 });
        let g = build_height_grid(&e, None).unwrap();
        assert_eq!((g.w, g.h), (400, 100));
        // The hole is interpolated from its rim: 50 m.
        assert!((g.at_px(11, 11) - 50.0).abs() < 0.2, "{}", g.at_px(11, 11));
        // Land untouched.
        assert!((g.at_px(50, 50) - 50.0).abs() < 0.06);
        // Sea next to land starts at the land height (one pixel = 8 m out: 100 - 50 * 0.984) ...
        assert!((g.at_px(100, 50) - 50.8).abs() < 0.2, "{}", g.at_px(100, 50));
        // ... and is exactly sea level from SKIRT_M (125 px) on.
        assert!((g.at_px(99 + 126, 50) - SEA_Y).abs() < 0.06 && (g.at_px(399, 50) - SEA_Y).abs() < 0.06);
        // Monotone away from the coast.
        let row: Vec<f32> = (99..230).step_by(10).map(|c| g.at_px(c, 50)).collect();
        assert!(row.windows(2).all(|w| w[1] >= w[0] - 0.06), "{row:?}");
    }

    #[test]
    fn the_skirt_fill_height_is_a_parameter_sea_level_here_the_floor_in_the_editor() {
        // One land pixel (50 m) then three sea pixels 500 m apart: 500 / 1000 / 1500 m from land.
        for fill in [SEA_Y, FLOOR_Y] {
            let mut h = vec![50.0, f32::NAN, f32::NAN, f32::NAN];
            let sea = [false, true, true, true];
            let mut keep = vec![true; 4];
            apply_skirt(&mut h, &sea, 4, 1, 500.0, fill, Some(&mut keep));
            assert_eq!(h[0], 50.0);
            assert!((h[1] - (fill + (50.0 - fill) * 0.25)).abs() < 1e-4, "{h:?}"); // (1 - 0.5)^2
            assert!((h[2] - fill).abs() < 1e-4 && (h[3] - fill).abs() < 1e-4, "{h:?}");
            assert_eq!(keep, vec![true, true, true, false], "kept up to SKIRT_M from land");
        }
        // No land anywhere: nothing to ramp from, the caller's fallback fill applies.
        let mut a = vec![f32::NAN; 4];
        apply_skirt(&mut a, &[true; 4], 2, 2, 8.0, SEA_Y, None);
        assert!(a.iter().all(|&v| (v - SEA_Y).abs() < 1e-4), "{a:?}");
    }

    #[test]
    fn empty_and_dataless_rasters_are_errors() {
        assert!(build_height_grid(&elev(0, 0, |_, _| 0), None).is_err());
        assert!(build_height_grid(&elev(4, 4, |_, _| NO_DATA), None).is_err());
    }

    #[test]
    fn synthetic_terrain_has_its_hills_over_a_flat_sea_level_plain() {
        let t = Terrain::synthetic();
        assert!((t.height(1000.0, -1000.0) - SEA_Y).abs() < 0.2);
        assert!((t.height(-300.0, 200.0) - (SEA_Y + 220.0)).abs() < 1.0);
        assert!((t.height(350.0, -250.0) - (SEA_Y + 80.0)).abs() < 1.0);
        let (lo, hi) = t.grid.range_in(t.grid.bounds(), 1).unwrap();
        assert!((lo - SEA_Y).abs() < 0.2 && hi > SEA_Y + 215.0 && hi < SEA_Y + 225.0, "{lo} {hi}");
        assert_ne!(Terrain::synthetic().rev, t.rev, "every terrain has its own revision");
    }

    #[test]
    fn range_in_finds_the_extremes_of_a_box() {
        let t = Terrain::synthetic();
        let (lo, hi) = t.grid.range_in([-400.0, 100.0, -200.0, 300.0], 1).unwrap();
        assert!(hi > SEA_Y + 200.0 && lo > SEA_Y + 20.0, "{lo} {hi}");
        assert!(t.grid.range_in([5000.0, 5000.0, 6000.0, 6000.0], 1).is_none());
    }

    /// The real install's raster through the real cache (skipped without an install): the filled
    /// grid is 2752 x 2752 over the island's -1.8..1473 m, open sea is exactly sea level, and
    /// the fill leaves the data pixels alone.
    #[test]
    fn real_install_terrain() {
        let Some(media) = crate::gamedata::install::find_media(None) else {
            eprintln!("SKIP real_install_terrain: FH6 install not found");
            return;
        };
        let t0 = std::time::Instant::now();
        let cache = crate::gamedata::terrain::cache_dir();
        let fine = Elevation::load_or_build_in(&cache, &media, false, &|_| {}).expect("fine raster");
        let coarse = Elevation::load_or_build_in(&cache, &media, true, &|_| {}).expect("coarse raster");
        let t = Terrain::from_grid(build_height_grid(&fine, Some(&coarse)).unwrap());
        eprintln!("real terrain: {} ms (caches warm: 0.2-0.4 s release, measured 415 ms in a test run alongside others)", t0.elapsed().as_millis());
        let g = &t.grid;
        assert_eq!((g.w, g.h), (2752, 2752));
        assert_eq!((g.x0, g.z1, g.res), (-12540.0, 10738.0, 8.0));
        let (lo, hi) = g.range_in(g.bounds(), 1).unwrap();
        assert!((lo - -1.8).abs() < 0.2, "lowest {lo}");
        assert!((hi - 1473.0).abs() < 2.0, "highest {hi}");
        // Open sea in the corners: exactly sea level (q rounds 100.0 m to a whole 0.1 step).
        for (c, r) in [(0usize, 0usize), (2751, 0), (0, 2751), (2751, 2751)] {
            assert!((g.at_px(c as isize, r as isize) - SEA_Y).abs() < 0.06, "corner {c},{r}: {}", g.at_px(c as isize, r as isize));
        }
        // Data pixels are the raster's (to the 0.1 m quantisation both share).
        let mut checked = 0;
        for i in (0..fine.dm.len()).step_by(997) {
            if fine.dm[i] != NO_DATA {
                let want = fine.dm[i] as f32 / 10.0;
                assert!((g.q[i] as f32 * H_STEP + H_OFF - want).abs() < 0.06, "pixel {i}");
                checked += 1;
            }
        }
        assert!(checked > 1000, "{checked}");
        // No no-data left: every pixel has a plausible height.
        assert!(g.q.iter().all(|&q| q > 0), "an unfilled pixel (q = 0)");
        // The world coordinate of a known road spot (the editor's reference: land at the start area).
        let h = t.height(0.0, 0.0);
        assert!(h > -2.0 && h < 1500.0, "{h}");
    }
}
