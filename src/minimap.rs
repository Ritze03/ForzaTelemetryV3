//! Minimap season-image loading and world↔UV / heading-up maths, shared by the Dashboard
//! map widget and the HUD overlay's minimap.
//!
//! Everything here is UI-thread independent: plain values in, plain values out, no
//! `&ForzaApp`. The overlay thread can call it with a `ForzaPacket` on its own `egui::Context`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::gamedata::tiles;
pub use crate::gamedata::tiles::MapLoadError;

// ── Season detection ──────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

pub const ALL_SEASONS: [Season; 4] = [Season::Spring, Season::Summer, Season::Autumn, Season::Winter];

/// The season FH6's overworld map is currently showing. Wall-clock based (not from packets):
/// the skin rotates weekly from a fixed epoch.
pub fn current_season() -> Season {
    // Spring started 2025-06-12 14:30:00 UTC (7:30 AM PDT). Unix: 1749738600.
    // Cycle repeats weekly: Spring → Summer → Autumn → Winter.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let secs = now - 1_749_738_600_i64;
    if secs < 0 {
        return Season::Spring;
    }
    match (secs / 604_800) % 4 {
        0 => Season::Spring,
        1 => Season::Summer,
        2 => Season::Autumn,
        _ => Season::Winter,
    }
}

pub fn season_display_name(season: Season) -> &'static str {
    match season {
        Season::Spring => "Spring",
        Season::Summer => "Summer",
        Season::Autumn => "Autumn",
        Season::Winter => "Winter",
    }
}

// ── Season image loading (on-disk cache keyed by season + quality) ─

/// Size of the full map image the calibration is expressed in (the install's level 3). Cached
/// images may be smaller; this is always the stored "original" size.
const MAP_ORIG_SIZE: [u32; 2] = [8192, 8192];

/// On-disk cache file for `season` at `quality_pct` (20–100 % of the 8192² source).
pub fn map_cache_path(season: Season, quality_pct: u32) -> std::path::PathBuf {
    crate::config::app_data_dir()
        .join("map_cache")
        .join(format!(
            "{}_q{}.bin",
            season_display_name(season).to_lowercase(),
            quality_pct
        ))
}

fn try_load_map_cache(path: &std::path::Path) -> Option<(egui::ColorImage, [u32; 2])> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 16 {
        return None;
    }
    let orig_w = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let orig_h = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let w = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
    let h = u32::from_le_bytes(data[12..16].try_into().ok()?) as usize;
    if data.len() != 16 + w * h * 4 {
        return None;
    }
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], &data[16..]);
    Some((color_image, [orig_w, orig_h]))
}

fn write_map_cache(path: &std::path::Path, orig: [u32; 2], scaled: [u32; 2], rgba: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut buf = Vec::with_capacity(16 + rgba.len());
    buf.extend_from_slice(&orig[0].to_le_bytes());
    buf.extend_from_slice(&orig[1].to_le_bytes());
    buf.extend_from_slice(&scaled[0].to_le_bytes());
    buf.extend_from_slice(&scaled[1].to_le_bytes());
    buf.extend_from_slice(rgba);
    // Write to a unique temp file, then rename: the Dashboard loader and the overlay thread
    // may build/read the same file concurrently, and a reader must never see a partial file.
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&tmp, buf).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Builds the binary cache file for `season` from the user's FH6 install (map tiles, see
/// `gamedata::tiles`). No-ops if the cache already exists, so caches from older builds keep
/// working without an install. Does NOT return the image data. ~60 ms (level 3, parallel) in a
/// release build, plus the cache write: call off the UI/render thread.
///
/// Quality → source: ≥ 99.9 % → level 3 (8192²); exactly 50 % (the HUD's, and a Dashboard set to
/// 50 %) → level 2 (4096²) directly, since resizing level 3 takes ~1.7 s; any other value →
/// level 3 resized with a triangle filter. The stored original size is always 8192² whatever the
/// cached size: `MapCalibration` works in 8192-px space.
pub fn decode_and_cache_season(season: Season, quality: f32) -> Result<(), MapLoadError> {
    let cache_file = map_cache_path(season, quality.round() as u32);
    if cache_file.exists() {
        return Ok(());
    }
    let media = crate::gamedata::install::find_media(None).ok_or(MapLoadError::NoInstall)?;
    let name = season_display_name(season);
    let (rgba, w, h) = if quality >= 99.9 {
        let (px, side) = tiles::load_mosaic(&media, name, 3)?;
        (px, side as u32, side as u32)
    } else if (quality - 50.0).abs() < 0.5 {
        let (px, side) = tiles::load_mosaic(&media, name, 2)?;
        (px, side as u32, side as u32)
    } else {
        let (px, side) = tiles::load_mosaic(&media, name, 3)?;
        let n = ((side as f32 * quality / 100.0) as u32).max(1);
        let full = image::RgbaImage::from_raw(side as u32, side as u32, px)
            .ok_or_else(|| MapLoadError::Decode("mosaic size".into()))?;
        let small = image::imageops::resize(&full, n, n, image::imageops::FilterType::Triangle);
        (small.into_raw(), n, n)
    };
    write_map_cache(&cache_file, MAP_ORIG_SIZE, [w, h], &rgba);
    Ok(())
}

/// Loads a season's map. Always reads from cache (building it first if needed).
/// Returns the (possibly downscaled) image and the ORIGINAL image size in px; the
/// calibration (`MapCalibration`) is in original-image pixels, so UV maths needs the latter.
/// `Err` when there is no cache and the FH6 install can't supply the tiles.
pub fn load_map_color_image(season: Season, quality: f32) -> Result<(egui::ColorImage, [u32; 2]), MapLoadError> {
    decode_and_cache_season(season, quality)?;
    let path = map_cache_path(season, quality.round() as u32);
    try_load_map_cache(&path).ok_or_else(|| MapLoadError::Decode(format!("map cache {} is unreadable", path.display())))
}

// ── HUD overlay image ─────────────────────────────────────────────

/// Quality (% of the 8192² source) of the overlay's map copy: 50 % → 4096².
pub const OVERLAY_MAP_QUALITY: f32 = 50.0;

/// Texture options for the overlay's map: linear + trilinear mipmaps (egui_glow generates the
/// mip chain on upload), so the minified, rotating map doesn't shimmer.
pub const OVERLAY_MAP_TEXTURE_OPTIONS: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::MirroredRepeat,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

/// The 4096² (the install's level-2 tiles) map image for `season`, plus the original
/// image size for `MapCalibration::world_to_uv`. Thread-safe.
///
/// Cached per season on disk (`map_cache/<season>_q50.bin`, shared with a Dashboard set to 50 %
/// quality), not in RAM: the caller uploads it with `ctx.load_texture(.., OVERLAY_MAP_TEXTURE_OPTIONS)`
/// and drops it, so no 64 MiB CPU copy lingers. First call per season writes the cache
/// from the install's level-2 tiles (~60 ms in a release build); later calls read the 64 MiB
/// cache (~35 ms warm). Either way it's heavy: call it off the render path / before showing the
/// surface. `Err` without an install (and no cache).
pub fn overlay_map_image(season: Season) -> Result<(egui::ColorImage, [u32; 2]), MapLoadError> {
    load_map_color_image(season, OVERLAY_MAP_QUALITY)
}

// ── Calibration & world↔UV ────────────────────────────────────────

/// World-metres ↔ map-image-pixel transform (in ORIGINAL image pixels):
/// `pixel_x = (world_x - origin_x) * px_per_m`, `pixel_y = (origin_z - world_z) * px_per_m`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MapCalibration {
    pub px_per_m: f32,
    pub origin_x: f32,
    pub origin_z: f32,
}

impl MapCalibration {
    /// Derived from in-game reference points; mirrors the `AppConfig` defaults.
    pub const DEFAULT: Self = Self { px_per_m: 0.3722, origin_x: -12540.0, origin_z: 10738.0 };

    /// From the user's `minimap_px_per_m` / `minimap_world_origin_x/z` config fields.
    pub fn from_config(cfg: &crate::config::AppConfig) -> Self {
        Self {
            px_per_m: cfg.minimap_px_per_m,
            origin_x: cfg.minimap_world_origin_x,
            origin_z: cfg.minimap_world_origin_z,
        }
    }

    /// World (x, z) metres → texture UV. May fall outside [0,1] off the map.
    pub fn world_to_uv(&self, wx: f32, wz: f32, orig_size: [u32; 2]) -> [f32; 2] {
        [
            (wx - self.origin_x) * self.px_per_m / orig_size[0] as f32,
            (self.origin_z - wz) * self.px_per_m / orig_size[1] as f32,
        ]
    }

    /// Texture UV → world (x, z) metres. Inverse of `world_to_uv`.
    #[allow(dead_code)] // only the round-trip test uses it today
    pub fn uv_to_world(&self, u: f32, v: f32, orig_size: [u32; 2]) -> [f32; 2] {
        [
            self.origin_x + u * orig_size[0] as f32 / self.px_per_m,
            self.origin_z - v * orig_size[1] as f32 / self.px_per_m,
        ]
    }

    /// The whole image's four corners as (world x, world z, uv), clockwise from top-left
    /// (TL, TR, BR, BL). For drawing the map without mirroring past its edges.
    pub fn image_corners(&self, orig_size: [u32; 2]) -> [(f32, f32, [f32; 2]); 4] {
        let map_world_w = orig_size[0] as f32 / self.px_per_m;
        let map_world_h = orig_size[1] as f32 / self.px_per_m;
        let (ox, oz) = (self.origin_x, self.origin_z);
        [
            (ox,               oz,               [0.0, 0.0]),
            (ox + map_world_w, oz,               [1.0, 0.0]),
            (ox + map_world_w, oz - map_world_h, [1.0, 1.0]),
            (ox,               oz - map_world_h, [0.0, 1.0]),
        ]
    }
}

// ── Heading-up view (world ↔ screen offset) ───────────────────────

/// A car-centred, rotated, zoomed view of the map. Screen offsets are pixels from the view
/// centre with y pointing down.
/// Convention: yaw = 0 → north (+Z) is screen-up; positive yaw rotates clockwise viewed
/// from above. `yaw` is the MAP rotation (0 for north-up, the car's heading for heading-up).
#[derive(Clone, Copy, Debug)]
pub struct MapView {
    pub car_x: f32,
    pub car_z: f32,
    pub yaw: f32,
    /// Screen pixels per world metre.
    pub scale: f32,
    pub sin_yaw: f32,
    pub cos_yaw: f32,
}

impl MapView {
    /// `zoom_m` = metres visible from the view centre to its nearest edge; `view_px` = the
    /// view's smaller dimension (width.min(height)) in pixels.
    pub fn new(car_x: f32, car_z: f32, yaw: f32, zoom_m: f32, view_px: f32) -> Self {
        let zoom = zoom_m.max(1.0);
        Self {
            car_x,
            car_z,
            yaw,
            scale: view_px / (2.0 * zoom),
            sin_yaw: yaw.sin(),
            cos_yaw: yaw.cos(),
        }
    }

    /// World (x, z) → screen offset [dx, dy] from the view centre (rotated by the map yaw).
    pub fn world_to_offset(&self, wx: f32, wz: f32) -> [f32; 2] {
        let dx = wx - self.car_x;
        let dz = wz - self.car_z;
        [
            (dx * self.cos_yaw - dz * self.sin_yaw) * self.scale,
            -(dx * self.sin_yaw + dz * self.cos_yaw) * self.scale,
        ]
    }

    /// Screen offset from the view centre → world (x, z). Inverse of `world_to_offset`.
    pub fn offset_to_world(&self, sx: f32, sy: f32) -> [f32; 2] {
        let inv_scale = 1.0 / self.scale;
        [
            self.car_x + (sx * self.cos_yaw - sy * self.sin_yaw) * inv_scale,
            self.car_z - (sx * self.sin_yaw + sy * self.cos_yaw) * inv_scale,
        ]
    }

    /// Texture UV under a screen offset: for building map meshes of any shape (quad, fan, pill).
    pub fn uv_at_offset(&self, cal: &MapCalibration, orig_size: [u32; 2], sx: f32, sy: f32) -> [f32; 2] {
        let [wx, wz] = self.offset_to_world(sx, sy);
        cal.world_to_uv(wx, wz, orig_size)
    }

    /// Unit screen direction [x, y] of world-north (for the compass).
    pub fn north_dir(&self) -> [f32; 2] {
        [-self.sin_yaw, -self.cos_yaw]
    }

    /// Screen rotation of the car arrow for the car's raw `pkt.yaw` (0 = pointing up).
    pub fn arrow_angle(&self, car_raw_yaw: f32) -> f32 {
        car_raw_yaw - self.yaw
    }
}

// ── Yaw / zoom easing ─────────────────────────────────────────────

/// Speed under which the car counts as stopping (km/h) and how long it must stay under it
/// (s) before the map zooms out / eases to north.
pub const STOPPED_KMH: f32 = 5.0;
pub const STOPPED_SECS: f32 = 1.5;

/// Returns the yaw angle the minimap should orient to.
/// If `use_movement_dir` and the car is moving, derives heading from velocity vector.
pub fn target_yaw(pkt: &crate::packet::ForzaPacket, use_movement_dir: bool) -> f32 {
    if use_movement_dir && pkt.speed > 1.0 {
        pkt.yaw + f32::atan2(pkt.velocity_x, pkt.velocity_z)
    } else {
        pkt.yaw
    }
}

/// Linearly interpolates between two angles, taking the shortest arc.
pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut diff = (b - a).rem_euclid(TAU);
    if diff > PI {
        diff -= TAU;
    }
    a + diff * t
}

/// One frame of smooth map rotation toward `target` (shortest arc). `dt` in seconds.
pub fn ease_yaw(current: f32, target: f32, dt: f32) -> f32 {
    lerp_angle(current, target, ease_t(dt))
}

/// The map rotation's per-frame lerp factor (rate 6/s, `dt` capped at 0.1 s).
fn ease_t(dt: f32) -> f32 {
    (6.0 * dt.min(0.1)).min(1.0)
}

/// Wrap an angle into (-PI, PI].
pub fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let w = (a + PI).rem_euclid(TAU) - PI;
    if w <= -PI { w + TAU } else { w }
}

/// "Look-around": the right-stick angle (radians) for a stick vector (x right, y up,
/// post-deadzone), measured clockwise from stick-up. Up = 0, right = +90 deg, down = 180 deg.
/// `(0, 0)` = no look.
/// Angle only, not scaled by deflection: the deadzone already gates it, and scaling the angle
/// by magnitude would make a half-pushed stick point at the wrong direction.
pub fn look_offset(stick: (f32, f32)) -> f32 {
    if stick.0 == 0.0 && stick.1 == 0.0 {
        0.0
    } else {
        f32::atan2(stick.0, stick.1)
    }
}

/// Right-stick look-around: owns the map's **view yaw** while the stick is in play, on top of
/// the map's own rotation (its "base" yaw: 0 in north-up, the eased heading-up yaw otherwise).
/// Both maps keep one of these and draw with the yaw [`LookAround::step`] returns.
///
/// - **Held** (option on, stick deflected): the view is `heading + eased θ`, `θ =
///   look_offset(stick)`, `heading` the heading-up yaw ([`target_yaw`]), i.e. the direction
///   the stick points **relative to the car** comes to the top (like the game's camera). The
///   offset is kept *relative to the heading*, so the base yaw doesn't enter at all: "north up
///   when stopped", "Smooth rotation" or the north-up lock can do what they like underneath
///   and the view doesn't move (it follows the car's heading, exactly, and the stick).
/// - **Released** (or option off): the offset is kept *relative to the base yaw* and decays
///   to 0 at the map-rotation rate (6/s), so the view eases from wherever it is to the mode's
///   own orientation and then simply *is* the base yaw again (no added lag once settled).
///
/// On each switch the offset is re-referenced from the previous frame's view, so the view is
/// continuous. On release it is split so the total turn takes the short way to the base's
/// target (`base_target`): the base eases there by its own shortest arc, the offset carries the
/// rest (unwrapped, up to ±2π) and decays linearly, not by shortest arc.
///
/// Why one view state instead of the first version's `base + eased offset toward (heading +
/// θ − base)`: when the base moved (north-up-when-stopped easing it to 0, or a snap with
/// "Smooth rotation" off), the offset's target moved by minus that change in the same frame,
/// but the offset only chased it at 6/s, so the sum swung toward north and then back to the
/// stick (the reported "jolt", up to ~37 % of the base's swing). The held view never reads the
/// base now.
#[derive(Clone, Copy, Debug, Default)]
pub struct LookAround {
    /// Held: offset from the heading (wrapped). Released: offset from the base yaw (decaying).
    off: f32,
    held: bool,
    /// The offset's goal: θ while held, 0 released.
    goal: f32,
    /// The view yaw of the last step (None before the first).
    view: Option<f32>,
}

impl LookAround {
    /// One frame. `heading` = the heading-up yaw ([`target_yaw`]); `base` = the map's own yaw
    /// this frame (what it shows without the stick); `base_target` = where that base is easing
    /// to (0 north-up / stopped-north, else the heading). Returns the view yaw, wrapped.
    pub fn step(&mut self, stick: (f32, f32), enabled: bool, heading: f32, base: f32, base_target: f32, dt: f32) -> f32 {
        let held = enabled && !(stick.0 == 0.0 && stick.1 == 0.0);
        let prev = self.view.unwrap_or(base);
        if held {
            if !self.held {
                self.off = wrap_angle(prev - heading);
            }
            self.goal = look_offset(stick);
            self.off = wrap_angle(ease_yaw(self.off, self.goal, dt));
        } else {
            if self.held {
                self.off = wrap_angle(prev - base_target) - wrap_angle(base - base_target);
            }
            self.goal = 0.0;
            self.off -= self.off * ease_t(dt);
            if self.off.abs() < 1e-4 {
                self.off = 0.0;
            }
        }
        self.held = held;
        let view = wrap_angle(if held { heading + self.off } else { base + self.off });
        self.view = Some(view);
        view
    }

    /// The view yaw of the last [`Self::step`], or `base` before the first.
    pub fn view_yaw(&self, base: f32) -> f32 {
        self.view.unwrap_or(base)
    }

    /// True while the look offset is still easing (the HUD redraws while animating).
    pub fn easing(&self) -> bool {
        let left = if self.held { wrap_angle(self.goal - self.off) } else { self.off };
        left.abs() > 1e-3
    }
}

/// One frame of smooth zoom toward `target_m`. `dt` in seconds.
pub fn ease_zoom(current_m: f32, target_m: f32, dt: f32) -> f32 {
    let lerp_t = (3.0 * dt.min(0.1)).min(1.0);
    current_m * (1.0 - lerp_t) + target_m * lerp_t
}

// ── Co-op trails (shared by the Dashboard map and the HUD Minimap) ─────────

/// One player's recent world path: `(x, z, recorded_at)`, oldest first. Drawn by
/// `hud::map_shared::draw_trail`.
pub type Trail = VecDeque<(f32, f32, Instant)>;

/// Append `(x, z)` to `trail` the way both maps record it: only after `MIN_MOVE` metres of
/// travel, a jump of `TELEPORT` metres (fast-travel / reset) starts a fresh trail, points
/// older than `max_age` are dropped and the length is capped.
pub fn trail_push(trail: &mut Trail, x: f32, z: f32, now: Instant, max_age: Duration) {
    const MIN_MOVE: f32 = 4.0;
    const MAX_PTS: usize = 400;
    const TELEPORT: f32 = 300.0;
    match trail.back() {
        Some(&(px, pz, _)) => {
            let moved = (px - x).hypot(pz - z);
            if moved >= TELEPORT {
                trail.clear();
                trail.push_back((x, z, now));
            } else if moved >= MIN_MOVE {
                trail.push_back((x, z, now));
            }
        }
        None => trail.push_back((x, z, now)),
    }
    while trail.front().is_some_and(|&(_, _, t)| now.duration_since(t) > max_age) {
        trail.pop_front();
    }
    if trail.len() > MAX_PTS {
        trail.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    const ORIG: [u32; 2] = [8192, 8192];

    fn close(a: [f32; 2], b: [f32; 2], eps: f32) -> bool {
        (a[0] - b[0]).abs() < eps && (a[1] - b[1]).abs() < eps
    }

    #[test]
    fn world_uv_round_trip() {
        let cal = MapCalibration::DEFAULT;
        for (wx, wz) in [(0.0, 0.0), (-12540.0, 10738.0), (1234.5, -6789.0), (8000.0, 3000.0)] {
            let [u, v] = cal.world_to_uv(wx, wz, ORIG);
            assert!(close(cal.uv_to_world(u, v, ORIG), [wx, wz], 0.05));
        }
    }

    #[test]
    fn calibration_origin_and_far_corner() {
        let cal = MapCalibration::DEFAULT;
        // World origin is pixel (0,0).
        assert!(close(cal.world_to_uv(-12540.0, 10738.0, ORIG), [0.0, 0.0], 1e-6));
        // The far corner is 8192 px / 0.3722 px/m ≈ 22009.7 m east and south.
        let span = 8192.0 / 0.3722;
        assert!(close(cal.world_to_uv(-12540.0 + span, 10738.0 - span, ORIG), [1.0, 1.0], 1e-4));
        // image_corners agrees with world_to_uv.
        for (wx, wz, uv) in cal.image_corners(ORIG) {
            assert!(close(cal.world_to_uv(wx, wz, ORIG), uv, 1e-4));
        }
        // North (+Z) is up in the image (smaller v).
        assert!(cal.world_to_uv(0.0, 100.0, ORIG)[1] < cal.world_to_uv(0.0, 0.0, ORIG)[1]);
    }

    #[test]
    fn view_round_trip() {
        let view = MapView::new(500.0, -300.0, 0.7, 1500.0, 164.0);
        for (wx, wz) in [(500.0, -300.0), (900.0, 100.0), (-2000.0, 50.0)] {
            let [sx, sy] = view.world_to_offset(wx, wz);
            assert!(close(view.offset_to_world(sx, sy), [wx, wz], 0.05));
        }
        // Car is at the centre.
        assert!(close(view.world_to_offset(500.0, -300.0), [0.0, 0.0], 1e-4));
    }

    #[test]
    fn north_up_view() {
        // yaw 0, 1 px per metre: north is up, east is right.
        let view = MapView::new(0.0, 0.0, 0.0, 50.0, 100.0);
        assert!(close(view.world_to_offset(0.0, 10.0), [0.0, -10.0], 1e-5));
        assert!(close(view.world_to_offset(10.0, 0.0), [10.0, 0.0], 1e-5));
        assert!(close(view.north_dir(), [0.0, -1.0], 1e-6));
    }

    #[test]
    fn heading_up_rotated_90() {
        // Car heading east (yaw +90°): heading-up map puts east at screen-up, north at left.
        let view = MapView::new(0.0, 0.0, FRAC_PI_2, 50.0, 100.0);
        assert!(close(view.world_to_offset(10.0, 0.0), [0.0, -10.0], 1e-4));
        assert!(close(view.world_to_offset(0.0, 10.0), [-10.0, 0.0], 1e-4));
        assert!(close(view.north_dir(), [-1.0, 0.0], 1e-6));
        // Car arrow points up when its raw yaw equals the map yaw.
        assert!(view.arrow_angle(FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn uv_at_offset_centre_is_car() {
        let cal = MapCalibration::DEFAULT;
        let view = MapView::new(1000.0, 2000.0, 1.3, 1500.0, 164.0);
        assert!(close(view.uv_at_offset(&cal, ORIG, 0.0, 0.0), cal.world_to_uv(1000.0, 2000.0, ORIG), 1e-5));
    }

    #[test]
    fn look_offset_follows_stick_angle() {
        assert_eq!(look_offset((0.0, 0.0)), 0.0);
        assert_eq!(look_offset((0.0, 0.8)), 0.0); // up = no change
        assert!((look_offset((0.5, 0.0)) - FRAC_PI_2).abs() < 1e-6); // right
        assert!((look_offset((-0.5, 0.0)) + FRAC_PI_2).abs() < 1e-6); // left
        assert!((look_offset((0.0, -1.0)).abs() - std::f32::consts::PI).abs() < 1e-6); // down
        // Magnitude doesn't matter.
        assert_eq!(look_offset((0.1, 0.1)), look_offset((0.9, 0.9)));
    }

    const DT: f32 = 1.0 / 60.0;

    /// Step a look for `secs` at 60 fps against a fixed heading / base (the base settled at its
    /// target). Returns the last view yaw.
    fn settle(look: &mut LookAround, stick: (f32, f32), heading: f32, base: f32, secs: f32) -> f32 {
        let mut view = look.view_yaw(base);
        for _ in 0..(secs * 60.0) as usize {
            view = look.step(stick, true, heading, base, base, DT);
        }
        view
    }

    fn ang_close(a: f32, b: f32) -> bool {
        wrap_angle(a - b).abs() < 1e-2
    }

    /// A map like the Dashboard's / HUD's: the base-yaw rule (north-up lock, "north up when
    /// stopped", "Smooth rotation") under a [`LookAround`].
    struct Sim {
        north_up: bool,
        smooth: bool,
        north_when_stopped: bool,
        base: f32,
        look: LookAround,
    }

    impl Sim {
        fn new(north_up: bool, smooth: bool, north_when_stopped: bool, heading: f32) -> Self {
            Self { north_up, smooth, north_when_stopped, base: if north_up { 0.0 } else { heading }, look: LookAround::default() }
        }
        fn frame(&mut self, heading: f32, stopped: bool, stick: (f32, f32)) -> f32 {
            let stopped_north = self.north_when_stopped && stopped;
            let target = if self.north_up || stopped_north { 0.0 } else { heading };
            self.base = if self.north_up {
                0.0
            } else if self.smooth || stopped_north {
                ease_yaw(self.base, target, DT)
            } else {
                heading
            };
            self.look.step(stick, true, heading, self.base, target, DT)
        }
    }

    #[test]
    fn look_returns_to_base_and_respects_enabled() {
        let mut look = LookAround::default();
        // Heading-up (base = heading): the view is heading + stick angle.
        let v = settle(&mut look, (1.0, 0.0), 0.8, 0.8, 2.0);
        assert!(ang_close(v, 0.8 + FRAC_PI_2), "{v}");
        assert!(!look.easing());
        let v = settle(&mut look, (0.0, 0.0), 0.8, 0.8, 2.0); // released
        assert!(ang_close(v, 0.8), "{v}");
        assert_eq!(look.off, 0.0, "a settled release must hand the view back to the base exactly");
        // Option off: the stick is ignored, the view is the base.
        let mut look = LookAround::default();
        assert!((look.step((1.0, 0.0), false, 0.8, 0.3, 0.3, 0.016) - 0.3).abs() < 1e-6);
    }

    /// The first north-up bug: the stick turned the view by its angle from *north* (stick
    /// right = east at the top), so the result was off by the car's heading. Now the stick
    /// picks a direction relative to the car in both modes, and the views agree.
    #[test]
    fn look_is_car_relative_in_north_up_and_heading_up() {
        use std::f32::consts::PI;
        let (car_x, car_z) = (1000.0, -500.0);
        for heading in [0.0, FRAC_PI_2, 2.5, -1.2, PI] {
            for stick in [(1.0, 0.0), (0.0, 1.0), (-0.7, -0.7), (0.0, -1.0), (-1.0, 0.2)] {
                let theta = look_offset(stick);
                let mut views = Vec::new();
                for base in [0.0 /* north-up */, heading /* heading-up */] {
                    let yaw = settle(&mut LookAround::default(), stick, heading, base, 3.0);
                    let view = MapView::new(car_x, car_z, yaw, 400.0, 200.0);
                    // View yaw = car heading + stick angle.
                    assert!(ang_close(view.yaw, heading + theta), "h {heading} s {stick:?} base {base}: {}", view.yaw);
                    // The car stays at the view centre (the pivot).
                    assert!(close(view.world_to_offset(car_x, car_z), [0.0, 0.0], 1e-3));
                    // The world direction the stick points at, relative to the car, is at the top.
                    let d = heading + theta;
                    let [sx, sy] = view.world_to_offset(car_x + 100.0 * d.sin(), car_z + 100.0 * d.cos());
                    assert!(sx.abs() < 1.0 && sy < 0.0, "h {heading} s {stick:?} base {base}: ({sx}, {sy})");
                    // The car arrow is turned by minus the stick angle (stick right: car points left).
                    assert!(ang_close(view.arrow_angle(heading), -theta), "{}", view.arrow_angle(heading));
                    // The compass points at world north: a point due north lies along north_dir.
                    let [nx, ny] = view.world_to_offset(car_x, car_z + 100.0);
                    let [cx, cy] = view.north_dir();
                    let len = (nx * nx + ny * ny).sqrt();
                    assert!((nx / len - cx).abs() < 1e-3 && (ny / len - cy).abs() < 1e-3);
                    views.push(view.yaw);
                }
                assert!(ang_close(views[0], views[1]), "north-up and heading-up look views differ: {views:?}");
            }
        }
    }

    #[test]
    fn look_north_up_release_eases_back_to_north() {
        // Car heading east, north-up, stick right: the car's right (south) comes to the top.
        let mut look = LookAround::default();
        let v = settle(&mut look, (1.0, 0.0), FRAC_PI_2, 0.0, 3.0);
        assert!(ang_close(v, std::f32::consts::PI), "{v}");
        assert!(close(MapView::new(0.0, 0.0, v, 50.0, 100.0).world_to_offset(0.0, -10.0), [0.0, -10.0], 0.2));
        // Released: back to north-up.
        let v = settle(&mut look, (0.0, 0.0), FRAC_PI_2, 0.0, 3.0);
        assert!(v.abs() < 1e-2, "{v}");
    }

    #[test]
    fn look_stays_wrapped_while_the_car_circles() {
        // North-up, stick held up while the heading winds through several turns.
        let mut look = LookAround::default();
        for i in 0..2000 {
            let heading = i as f32 * 0.02; // ~6.4 turns
            let v = look.step((0.0, 1.0), true, heading, 0.0, 0.0, DT);
            let pi = std::f32::consts::PI + 1e-4;
            assert!(v.abs() <= pi && look.off.abs() <= pi, "{v} {}", look.off);
        }
    }

    /// The reported jolt: stick held right while driving heading-up, then the car stops and
    /// "north up when stopped" eases the base to north underneath. The view must stay at
    /// heading + 90 deg the whole time (it used to swing toward north by up to ~37 % of the
    /// base's turn, then back). Released while stopped: a monotone ease to north. Then
    /// driving off with the stick held again: no jump either; released: eases to the heading.
    #[test]
    fn look_held_ignores_north_up_when_stopped() {
        for smooth in [true, false] {
            let heading = 0.8;
            let right = (1.0, 0.0);
            let want = wrap_angle(heading + FRAC_PI_2);
            let mut sim = Sim::new(false, smooth, true, heading);
            for _ in 0..180 {
                sim.frame(heading, false, right); // settle while driving
            }
            assert!(ang_close(sim.look.view_yaw(0.0), want));
            // Stops: the base eases to north; the held view doesn't move at all.
            for i in 0..180 {
                let v = sim.frame(heading, true, right);
                assert!(wrap_angle(v - want).abs() < 1e-3, "smooth {smooth} frame {i}: view {v}, want {want}");
            }
            assert!(sim.base.abs() < 1e-2, "the base did go north underneath: {}", sim.base);
            // Released while stopped: monotone ease to north, never overshooting.
            let mut prev = sim.look.view_yaw(0.0);
            for _ in 0..240 {
                let v = sim.frame(heading, true, (0.0, 0.0));
                assert!(v.abs() < 1e-4 || (v.abs() <= prev.abs() + 1e-6 && v.signum() == prev.signum()), "{prev} -> {v}");
                prev = v;
            }
            assert!(prev.abs() < 1e-2, "{prev}");
            // Stick held again, then the car drives off (the base eases or snaps back to the
            // heading underneath): the held view stays put.
            for _ in 0..180 {
                sim.frame(heading, true, right);
            }
            for i in 0..120 {
                let v = sim.frame(heading, false, right);
                assert!(wrap_angle(v - want).abs() < 1e-3, "smooth {smooth} drive-off frame {i}: {v}");
            }
            // Released while driving: eases to the heading.
            let mut prev = sim.look.view_yaw(0.0);
            for _ in 0..240 {
                let v = sim.frame(heading, false, (0.0, 0.0));
                assert!(wrap_angle(v - heading).abs() <= wrap_angle(prev - heading).abs() + 1e-6, "{prev} -> {v}");
                prev = v;
            }
            assert!(ang_close(prev, heading), "{prev}");
        }
    }

    /// Every mode (north-up lock, heading-up, smooth on/off, north-up-when-stopped on/off),
    /// stick held while the car turns, stops and drives off: once settled, the view is exactly
    /// heading + θ every frame, and it never moves more than the heading did.
    #[test]
    fn look_held_follows_only_heading_in_every_mode() {
        let left = (-1.0, 0.0);
        let theta = look_offset(left);
        for north_up in [false, true] {
            for smooth in [false, true] {
                for nws in [false, true] {
                    let mut sim = Sim::new(north_up, smooth, nws, 0.3);
                    for _ in 0..180 {
                        sim.frame(0.3, false, left);
                    }
                    let mut prev = (0.3f32, sim.look.view_yaw(0.0));
                    for i in 0..600 {
                        // Turning for 4 s, stopped for 3 s, then off again.
                        let heading = 0.3 + (i.min(240) as f32) * 0.01;
                        let stopped = (240..420).contains(&i);
                        let v = sim.frame(heading, stopped, left);
                        let tag = format!("north_up {north_up} smooth {smooth} nws {nws} frame {i}");
                        assert!(wrap_angle(v - (heading + theta)).abs() < 2e-3, "{tag}: {v}");
                        let moved = wrap_angle(v - prev.1).abs();
                        assert!(moved <= wrap_angle(heading - prev.0).abs() + 1e-3, "{tag}: moved {moved}");
                        prev = (heading, v);
                    }
                }
            }
        }
    }

    /// Released mid-way through the base's ease to north (the base is still turning): the view
    /// takes the short way to north, not base arc + offset arc round the long way.
    #[test]
    fn look_release_takes_the_short_way_while_the_base_still_turns() {
        let heading = 2.6; // ~149 deg; stick right -> view ~ -121 deg
        let mut sim = Sim::new(false, true, true, heading);
        for _ in 0..180 {
            sim.frame(heading, false, (1.0, 0.0));
        }
        for _ in 0..3 {
            sim.frame(heading, true, (1.0, 0.0)); // the base has just started its ease to north
        }
        assert!(sim.base > 1.5, "base still far from north: {}", sim.base);
        let start = sim.look.view_yaw(0.0);
        let (mut prev, mut travelled) = (start, 0.0);
        for _ in 0..300 {
            let v = sim.frame(heading, true, (0.0, 0.0));
            travelled += wrap_angle(v - prev);
            prev = v;
        }
        assert!(prev.abs() < 1e-2, "{prev}");
        assert!((travelled + start).abs() < 1e-2 && travelled.abs() <= std::f32::consts::PI, "start {start}, travelled {travelled}");
    }

    #[test]
    fn wrap_angle_range() {
        use std::f32::consts::PI;
        for (a, w) in [(0.0, 0.0), (PI, PI), (-PI, PI), (3.0 * PI, PI), (2.0 * PI + 0.5, 0.5), (-0.5, -0.5)] {
            assert!((wrap_angle(a) - w).abs() < 1e-5, "{a} -> {}", wrap_angle(a));
        }
    }

    #[test]
    fn lerp_angle_shortest_arc() {
        use std::f32::consts::PI;
        // From 170° to -170° goes through 180°, not back through 0.
        let a = 170f32.to_radians();
        let b = -170f32.to_radians();
        let mid = lerp_angle(a, b, 0.5);
        assert!((mid - PI).abs() < 1e-4);
    }
}
