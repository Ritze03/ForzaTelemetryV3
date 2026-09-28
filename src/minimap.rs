//! Minimap season-image loading and world↔UV / heading-up maths, shared by the Dashboard
//! map widget and the HUD overlay's minimap.
//!
//! Everything here is UI-thread independent: plain values in, plain values out, no
//! `&ForzaApp`. The overlay thread can call it with a `ForzaPacket` on its own `egui::Context`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    // Spring started 2026-06-12 14:30:00 UTC (7:30 AM PDT). Unix: 1749738600.
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

fn season_map_bytes(season: Season) -> &'static [u8] {
    match season {
        Season::Spring => include_bytes!("../assets/maps/spring.jpg"),
        Season::Summer => include_bytes!("../assets/maps/summer.jpg"),
        Season::Autumn => include_bytes!("../assets/maps/autumn.jpg"),
        Season::Winter => include_bytes!("../assets/maps/winter.jpg"),
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

/// Decodes the JPEG for `season` and writes the binary cache file.
/// No-ops if the cache already exists. Does NOT return the image data.
/// Heavy (8192² JPEG decode + resize, ~2 s in release): call off the UI/render thread.
pub fn decode_and_cache_season(season: Season, quality: f32) {
    let quality_pct = quality.round() as u32;
    let cache_file = map_cache_path(season, quality_pct);
    if cache_file.exists() {
        return;
    }
    let bytes = season_map_bytes(season);
    let Ok(img) = image::load_from_memory(bytes) else {
        return;
    };
    let orig_size = [img.width(), img.height()];
    let rgba = if quality >= 99.9 {
        img.into_rgba8()
    } else {
        let nw = ((orig_size[0] as f32 * quality / 100.0) as u32).max(1);
        let nh = ((orig_size[1] as f32 * quality / 100.0) as u32).max(1);
        img.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
            .into_rgba8()
    };
    let (w, h) = rgba.dimensions();
    write_map_cache(&cache_file, orig_size, [w, h], rgba.as_raw());
}

/// Loads a season's map. Always reads from cache (building it first if needed).
/// Returns the (possibly downscaled) image and the ORIGINAL image size in px; the
/// calibration (`MapCalibration`) is in original-image pixels, so UV maths needs the latter.
pub fn load_map_color_image(season: Season, quality: f32) -> Option<(egui::ColorImage, [u32; 2])> {
    decode_and_cache_season(season, quality);
    try_load_map_cache(&map_cache_path(season, quality.round() as u32))
}

// ── HUD overlay image ─────────────────────────────────────────────

/// Quality (% of the 8192² source) of the overlay's map copy: 50 % → 4096².
pub const OVERLAY_MAP_QUALITY: f32 = 50.0;

/// Texture options for the overlay's map: linear + trilinear mipmaps (egui_glow generates the
/// mip chain on upload), so the minified, rotating map doesn't shimmer.
pub const OVERLAY_MAP_TEXTURE_OPTIONS: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

/// The 4096² (downscaled 2× with a triangle filter) map image for `season`, plus the original
/// image size for `MapCalibration::world_to_uv`. Thread-safe.
///
/// Cached per season on disk (`map_cache/<season>_q50.bin`, shared with a Dashboard set to 50 %
/// quality), not in RAM: the caller uploads it with `ctx.load_texture(.., OVERLAY_MAP_TEXTURE_OPTIONS)`
/// and drops it, so no 64 MiB CPU copy lingers. First call per season decodes the JPEG and
/// writes the cache (~2.3 s in a release build); later calls read the 64 MiB cache (~35 ms
/// warm). Either way it's heavy: call it off the render path / before showing the surface.
pub fn overlay_map_image(season: Season) -> Option<(egui::ColorImage, [u32; 2])> {
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
    let lerp_t = (6.0 * dt.min(0.1)).min(1.0);
    lerp_angle(current, target, lerp_t)
}

/// One frame of smooth zoom toward `target_m`. `dt` in seconds.
pub fn ease_zoom(current_m: f32, target_m: f32, dt: f32) -> f32 {
    let lerp_t = (3.0 * dt.min(0.1)).min(1.0);
    current_m * (1.0 - lerp_t) + target_m * lerp_t
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
    fn lerp_angle_shortest_arc() {
        use std::f32::consts::PI;
        // From 170° to -170° goes through 180°, not back through 0.
        let a = 170f32.to_radians();
        let b = -170f32.to_radians();
        let mid = lerp_angle(a, b, 0.5);
        assert!((mid - PI).abs() < 1e-4);
    }
}
