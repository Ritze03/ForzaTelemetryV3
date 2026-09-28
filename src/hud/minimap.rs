//! M2′ minimap (pill frame, no scale bar), 208 × 136. Heading-up season map clipped to a
//! rounded rect: a triangle-fan mesh whose per-vertex UVs come from
//! [`MapView::uv_at_offset`] (affine, so interpolating UVs across the fan is exact).
//!
//! The season image is loaded on a helper thread ([`MapLoader`]); until it arrives the frame
//! draws over a plain backing.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use egui::epaint::Vertex;
use egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, TextureHandle, TextureId, Vec2};

use super::col;
use super::prims::{self, Xf};
use crate::minimap::{self as mm, MapCalibration, MapView, Season};
use crate::overlay::snapshot::HudSnapshot;

pub const SIZE: Vec2 = vec2(208.0, 136.0);
const RADIUS: f32 = 22.0;
/// How often the wall-clock season is re-checked, seconds.
const SEASON_CHECK_SECS: f64 = 60.0;

/// An uploaded season map: texture plus the original image size its calibration is in.
#[derive(Clone, Copy, Debug)]
pub struct MapTex {
    pub id: TextureId,
    pub orig_size: [u32; 2],
    pub winter: bool,
}

type Loaded = Option<(egui::ColorImage, [u32; 2])>;

/// Loads the overlay's season map off the render thread (2.3 s cold) and uploads it on the
/// render thread; the CPU copy is dropped right after `load_texture`.
#[derive(Default)]
pub struct MapLoader {
    tex: Option<(TextureHandle, [u32; 2], Season)>,
    pending: Option<(Receiver<Loaded>, Season)>,
    /// Season last asked for (loaded, loading or failed) and when that was checked.
    wanted: Option<Season>,
    checked_at: Option<f64>,
}

impl MapLoader {
    /// Poll: start a load when the season changed (checked once a minute), upload a finished
    /// one. Only loads while the minimap module is on (`want`). Returns the current texture.
    pub fn poll(&mut self, ctx: &egui::Context, now: f64, want: bool) -> Option<MapTex> {
        if want && self.checked_at.is_none_or(|t| now - t >= SEASON_CHECK_SECS) {
            self.checked_at = Some(now);
            let season = mm::current_season();
            if self.wanted != Some(season) && self.pending.is_none() {
                self.wanted = Some(season);
                let (tx, rx) = mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("hud-map".into())
                    .spawn(move || {
                        let _ = tx.send(mm::overlay_map_image(season));
                    });
                match spawned {
                    Ok(_) => self.pending = Some((rx, season)),
                    Err(e) => eprintln!("overlay: map loader thread: {e}"),
                }
            }
        }
        if let Some((rx, season)) = &self.pending {
            match rx.try_recv() {
                Ok(Some((img, orig))) => {
                    // ponytail: the 64 MiB upload + mip generation lands in one visible frame
                    // (tens of ms, once per season). Upgrade: upload before the surface is
                    // mapped, or in tiles over several frames.
                    let handle = ctx.load_texture("hud-map", img, mm::OVERLAY_MAP_TEXTURE_OPTIONS);
                    self.tex = Some((handle, orig, *season));
                    self.pending = None;
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {
                    // ponytail: a failed load waits for the next season. Upgrade: retry on
                    // the next minute check if the cache ever turns out to be flaky.
                    eprintln!("overlay: season map failed to load");
                    self.pending = None;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        self.tex.as_ref().map(|(h, orig, s)| MapTex { id: h.id(), orig_size: *orig, winter: *s == Season::Winter })
    }

    /// Use `handle` as the map and never load the real one (the PNG harness's synthetic map).
    #[cfg(test)]
    pub fn set(&mut self, handle: TextureHandle, orig_size: [u32; 2], season: Season) {
        self.tex = Some((handle, orig_size, season));
        self.wanted = Some(season);
        self.checked_at = Some(f64::INFINITY);
    }
}

/// Eased view state (yaw and zoom follow the car smoothly, like the Dashboard map).
#[derive(Default, Debug)]
pub struct MapAnim {
    /// `now` of the previous step (for dt).
    last: Option<f64>,
    yaw: Option<f32>,
    zoom: Option<f32>,
    slow_since: Option<f64>,
}

impl MapAnim {
    #[cfg(test)]
    pub fn zoom(&self) -> Option<f32> {
        self.zoom
    }

    /// Advance to `now` toward the packet's heading and the driving/stopped zoom. Returns
    /// true while still easing.
    fn step(&mut self, snap: &HudSnapshot, now: f64) -> bool {
        let dt = self.last.map_or(0.0, |t| (now - t).clamp(0.0, 0.1) as f32);
        self.last = Some(now);
        let (pkt, cfg) = (&snap.pkt, &*snap.cfg);
        let kmh = pkt.speed * 3.6;
        let stopped = if kmh >= mm::STOPPED_KMH {
            self.slow_since = None;
            false
        } else {
            now - *self.slow_since.get_or_insert(now) >= mm::STOPPED_SECS as f64
        };
        let target_zoom = if stopped { cfg.zoom_stopped_m } else { cfg.zoom_driving_m };
        let target_yaw = mm::target_yaw(pkt, false);
        let yaw = self.yaw.map_or(target_yaw, |y| mm::ease_yaw(y, target_yaw, dt));
        // Under 5 km/h but not yet stopped for 1.5 s: hold the zoom (as the Dashboard does).
        let zoom = match self.zoom {
            None => target_zoom,
            Some(z) if stopped || kmh >= mm::STOPPED_KMH => mm::ease_zoom(z, target_zoom, dt),
            Some(z) => z,
        };
        self.yaw = Some(yaw);
        self.zoom = Some(zoom);
        let yaw_left = (mm::lerp_angle(yaw, target_yaw, 1.0) - yaw).abs() > 1e-3;
        let zoom_left = (zoom - target_zoom).abs() > 0.5 && (stopped || kmh >= mm::STOPPED_KMH);
        yaw_left || zoom_left
    }
}

/// A co-op teammate on the map (not wired yet: the positions live UI-side and need their
/// own path to the overlay thread). The seam `draw` takes today, always called empty.
#[derive(Clone, Debug)]
pub struct Teammate {
    pub x: f32,
    pub z: f32,
}

/// Draw M2′. Returns true while the view is still easing.
pub fn draw(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, anim: &mut MapAnim, map: Option<MapTex>, teammates: &[Teammate]) -> bool {
    let animating = anim.step(snap, now);
    let (yaw, zoom) = (anim.yaw.unwrap_or(0.0), anim.zoom.unwrap_or(snap.cfg.zoom_driving_m));
    let (w, h) = (SIZE.x, SIZE.y);
    let centre = xf.p(w / 2.0, h / 2.0);
    let view = MapView::new(snap.pkt.position_x, snap.pkt.position_z, yaw, zoom, xf.l(w.min(h)));

    // The map: fan over the frame's rounded rect, inset 0.5 px so the 3 px border (drawn
    // after, feathered) covers the mesh's hard edge.
    let outline = prims::rounded_points(xf.rect(0.5, 0.5, w - 1.0, h - 1.0), [xf.l(RADIUS - 0.5); 4]);
    match map {
        Some(tex) => {
            let cal = calibration(snap);
            let mut mesh = Mesh::with_texture(tex.id);
            let white = xf.c(Color32::WHITE);
            let uv = |pt: Pos2| {
                let [u, v] = view.uv_at_offset(&cal, tex.orig_size, pt.x - centre.x, pt.y - centre.y);
                pos2(u, v)
            };
            fan(&mut mesh, centre, &outline, |pt| Vertex { pos: pt, uv: uv(pt), color: white });
            p.add(mesh);
            // Darken so the white marker reads over bright maps (winter more).
            let tint = xf.c(if tex.winter { col::MAP_TINT_WINTER } else { col::MAP_TINT });
            p.add(egui::Shape::convex_polygon(outline.clone(), tint, egui::Stroke::NONE));
        }
        None => {
            p.add(egui::Shape::convex_polygon(outline.clone(), xf.c(col::plate(snap.cfg.plate_opacity)), egui::Stroke::NONE));
        }
    }

    // TODO(co-op): teammate markers (arrow 12 × 14 + name, #F2A93B, spec sheet "Co-op
    // marker") once teammates reach the overlay thread.
    let _ = teammates;

    // Compass (D12): disc at (18, 18) r 11, two-colour needle 16 × 6 pointing to world north.
    if snap.cfg.compass {
        let c = xf.p(18.0, 18.0);
        p.circle_filled(c, xf.l(11.0), xf.c(col::COMPASS));
        let [nx, ny] = view.north_dir();
        let n = vec2(nx, ny);
        let side = vec2(-ny, nx) * xf.l(3.0);
        let tip = n * xf.l(8.0);
        p.add(egui::Shape::convex_polygon(vec![c + tip, c + side, c - side], xf.c(col::NORTH), egui::Stroke::NONE));
        p.add(egui::Shape::convex_polygon(vec![c - tip, c - side, c + side], xf.c(col::INK), egui::Stroke::NONE));
    }

    // Car marker, fixed apex up: 14 × 17, white over a 2.2 px dark stroke (canvas strokes
    // first and fills over it, so only the outer 1.1 px of the stroke shows).
    let tri = [[0.0, -9.0], [7.0, 8.0], [-7.0, 8.0]].map(|[x, y]| [w / 2.0 + x, h / 2.0 + y]);
    let pts: Vec<Pos2> = tri.iter().map(|&[x, y]| xf.p(x, y)).collect();
    p.add(egui::Shape::convex_polygon(pts.clone(), xf.c(col::MARKER_EDGE), egui::Stroke::new(xf.l(2.2), xf.c(col::MARKER_EDGE))));
    p.add(egui::Shape::convex_polygon(pts, xf.c(Color32::WHITE), egui::Stroke::NONE));

    prims::rounded_border(p, xf, [0.0, 0.0, w, h], RADIUS, 3.0, col::FRAME);
    animating
}

/// The snapshot's calibration, or the built-in default if it's unset (a zeroed snapshot).
fn calibration(snap: &HudSnapshot) -> MapCalibration {
    let m = snap.minimap;
    if m.px_per_m > 0.0 {
        MapCalibration { px_per_m: m.px_per_m, origin_x: m.origin_x, origin_z: m.origin_z }
    } else {
        MapCalibration::DEFAULT
    }
}

/// Triangle fan from `centre` over the closed `outline`.
fn fan(mesh: &mut Mesh, centre: Pos2, outline: &[Pos2], vertex: impl Fn(Pos2) -> Vertex) {
    mesh.vertices.push(vertex(centre));
    mesh.vertices.extend(outline.iter().map(|&pt| vertex(pt)));
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
}
