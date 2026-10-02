//! M2′ minimap (pill frame, no scale bar), 208 × 136. Heading-up season map clipped to a
//! rounded rect: a triangle-fan mesh whose per-vertex UVs come from
//! [`MapView::uv_at_offset`] (affine, so interpolating UVs across the fan is exact).
//!
//! The season image is loaded on a helper thread ([`MapLoader`]); until it arrives the frame
//! draws over a plain backing.
//!
//! The own arrow, co-op teammates, trails and waypoints are drawn by `hud::map_shared`, the
//! same code as the Dashboard map. The co-op state they need (teammates at their last known
//! spot, per-player trail buffers) lives in [`CoopLayer`], fed on the overlay thread.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::epaint::Vertex;
use egui::{pos2, vec2, Color32, Mesh, Painter, Pos2, TextureHandle, TextureId, Vec2};

use super::col;
use super::map_shared::{self, MapCanvas, Remote, TrailFade};
use super::prims::{self, Xf};
use crate::minimap::{self as mm, MapCalibration, MapView, Season, Trail};
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
    /// Eased right-stick look-around offset, added to `yaw` when drawing.
    look: f32,
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
        let target_yaw = map_target_yaw(pkt, cfg, stopped);
        // Ease-to-north always animates; otherwise honour "Smooth rotation".
        let smooth = cfg.map_smooth_rotation || (cfg.map_north_up_when_stopped && stopped);
        let yaw = match self.yaw {
            Some(y) if smooth => mm::ease_yaw(y, target_yaw, dt),
            _ => target_yaw,
        };
        // Under 5 km/h but not yet stopped for 1.5 s: hold the zoom (as the Dashboard does).
        let zoom = match self.zoom {
            None => target_zoom,
            Some(z) if stopped || kmh >= mm::STOPPED_KMH => mm::ease_zoom(z, target_zoom, dt),
            Some(z) => z,
        };
        // Look-around is relative to the car's heading (the heading-up yaw) in every mode.
        let heading = mm::target_yaw(pkt, cfg.map_use_movement_dir);
        self.look = mm::ease_look(self.look, snap.look_stick, cfg.map_look_stick, heading, yaw, dt);
        self.yaw = Some(yaw);
        self.zoom = Some(zoom);
        let look_target = if cfg.map_look_stick { mm::look_target(snap.look_stick, heading, yaw) } else { 0.0 };
        let look_left = (mm::lerp_angle(self.look, look_target, 1.0) - self.look).abs() > 1e-3;
        let yaw_left = (mm::lerp_angle(yaw, target_yaw, 1.0) - yaw).abs() > 1e-3;
        let zoom_left = (zoom - target_zoom).abs() > 0.5 && (stopped || kmh >= mm::STOPPED_KMH);
        yaw_left || zoom_left || look_left
    }
}

/// The yaw the map rotates to: 0 (north-up) when locked or (heading-up, opted in) stopped;
/// else the car's heading, or its movement direction if enabled.
fn map_target_yaw(pkt: &crate::packet::ForzaPacket, cfg: &crate::config::OverlayConfig, stopped: bool) -> f32 {
    if cfg.map_north_up || (cfg.map_north_up_when_stopped && stopped) {
        0.0
    } else {
        mm::target_yaw(pkt, cfg.map_use_movement_dir)
    }
}

/// What the overlay thread reads from the co-op session each frame (see
/// `overlay::render::coop_input`). Plain data so [`CoopLayer::update`] is testable.
pub struct CoopInput {
    pub in_session: bool,
    pub remotes: Vec<(crate::coop::PlayerInfo, crate::packet::ForzaPacket)>,
    /// `(world_x, world_z, hue)` of the shared waypoints.
    pub waypoints: Vec<(f32, f32, f32)>,
}

/// The co-op layer of the Minimap: who to draw, their trails, the shared waypoints.
///
/// **Why the HUD keeps its own trail buffers** (instead of the Dashboard's
/// `ForzaApp::minimap_trails`, which the UI thread fills): the UI loop stops while the game
/// covers the window, which is exactly when the HUD is in use, so the overlay thread records
/// them itself from the snapshot's packet and `CoopReader::remote_players`, with the same
/// recording rules (`minimap::trail_push`) as the Dashboard.
#[derive(Default)]
pub struct CoopLayer {
    pub in_session: bool,
    /// Teammates to draw (paused ones at their last known spot).
    pub teammates: Vec<Remote>,
    pub waypoints: Vec<(f32, f32, f32)>,
    /// Trails by player: `"local"` or the co-op player id.
    pub trails: HashMap<String, Trail>,
    /// Last position/heading from an unpaused packet, per player id.
    last_pos: HashMap<String, (f32, f32, f32)>,
    /// When `update` last ran (the trail fade's "now").
    now: Option<Instant>,
}

impl CoopLayer {
    /// Refresh from this frame's co-op state. Anything the effective config turns off, or a
    /// session that isn't running, leaves the matching part empty (and forgets its history).
    pub fn update(&mut self, input: Option<CoopInput>, snap: &HudSnapshot, now: Instant) {
        let cfg = &*snap.cfg;
        self.now = Some(now);
        let Some(input) = input.filter(|i| i.in_session && cfg.minimap_on) else {
            *self = Self { now: self.now, ..Self::default() };
            return;
        };
        self.in_session = true;
        let max_age = Duration::from_secs_f32(cfg.coop_trail_fade_secs.max(0.5));
        let mut present: HashSet<String> = HashSet::new();

        // Own trail: only while driving (not paused, game connected), like the Dashboard.
        let pkt = &snap.pkt;
        if cfg.coop_trails {
            if snap.connected && pkt.is_race_on != 0 && !pkt.is_paused() {
                mm::trail_push(self.trails.entry("local".into()).or_default(), pkt.position_x, pkt.position_z, now, max_age);
            }
            present.insert("local".into());
        }

        self.teammates.clear();
        if cfg.coop_teammates {
            for (info, rp) in &input.remotes {
                let paused = rp.is_paused();
                if !paused {
                    self.last_pos.insert(info.id.clone(), (rp.position_x, rp.position_z, rp.yaw));
                    if cfg.coop_trails {
                        mm::trail_push(self.trails.entry(info.id.clone()).or_default(), rp.position_x, rp.position_z, now, max_age);
                    }
                }
                if cfg.coop_trails {
                    present.insert(info.id.clone()); // a paused player keeps their trail (undrawn)
                }
                // A paused packet sits at the world origin; draw their last known spot, or
                // nothing if they were never seen at a valid one.
                let pos = if paused { self.last_pos.get(&info.id).copied() } else { Some((rp.position_x, rp.position_z, rp.yaw)) };
                if let Some((x, z, yaw)) = pos {
                    self.teammates.push(Remote {
                        id: info.id.clone(),
                        name: info.name.clone(),
                        x,
                        z,
                        yaw,
                        colour: crate::ui::coop::hue_color(info.hue),
                        paused,
                    });
                }
            }
        }
        self.trails.retain(|k, _| present.contains(k));
        let alive: HashSet<&String> = input.remotes.iter().map(|(i, _)| &i.id).collect();
        self.last_pos.retain(|k, _| alive.contains(k));
        self.waypoints = if cfg.coop_waypoints { input.waypoints } else { Vec::new() };
    }
}

/// Compass (D12): disc at (18, 18) r 11, two-colour needle 16 x 6 pointing to world north.
/// `north` is the unit screen direction of world-north (`MapView::north_dir`). Shared with
/// the Dashboard map so the two compasses can't drift apart.
pub fn draw_compass(p: &Painter, xf: &Xf, north: [f32; 2]) {
    let c = xf.p(18.0, 18.0);
    p.circle_filled(c, xf.l(11.0), xf.c(col::COMPASS));
    let n = vec2(north[0], north[1]);
    let side = vec2(-n.y, n.x) * xf.l(3.0);
    let tip = n * xf.l(8.0);
    p.add(egui::Shape::convex_polygon(vec![c + tip, c + side, c - side], xf.c(col::NORTH), egui::Stroke::NONE));
    p.add(egui::Shape::convex_polygon(vec![c - tip, c - side, c + side], xf.c(col::INK), egui::Stroke::NONE));
}

/// Sutherland–Hodgman: `subject` clipped to the convex polygon `clip` (either winding).
/// Used to cut the pill to the map image when edges aren't mirrored.
fn clip_convex(subject: &[Pos2], clip: &[Pos2]) -> Vec<Pos2> {
    let cross = |a: Pos2, b: Pos2, p: Pos2| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let area: f32 = (0..clip.len()).map(|i| cross(Pos2::ZERO, clip[i], clip[(i + 1) % clip.len()])).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut out = subject.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let input = std::mem::take(&mut out);
        for j in 0..input.len() {
            let (cur, prev) = (input[j], input[(j + input.len() - 1) % input.len()]);
            let (dc, dp) = (cross(a, b, cur) * sign, cross(a, b, prev) * sign);
            if (dc >= 0.0) != (dp >= 0.0) {
                out.push(prev + (cur - prev) * (dp / (dp - dc)));
            }
            if dc >= 0.0 {
                out.push(cur);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// Draw M2′. Returns true while the view is still easing.
pub fn draw(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, anim: &mut MapAnim, map: Option<MapTex>, coop: &CoopLayer) -> bool {
    let animating = anim.step(snap, now);
    let (yaw, zoom) = (anim.yaw.unwrap_or(0.0) + anim.look, anim.zoom.unwrap_or(snap.cfg.zoom_driving_m));
    let (w, h) = (SIZE.x, SIZE.y);
    let centre = xf.p(w / 2.0, h / 2.0);
    let view = MapView::new(snap.pkt.position_x, snap.pkt.position_z, yaw, zoom, xf.l(w.min(h)));

    // The map: fan over the frame's rounded rect, inset 0.5 px so the 3 px border (drawn
    // after, feathered) covers the mesh's hard edge.
    let outline = prims::rounded_points(xf.rect(0.5, 0.5, w - 1.0, h - 1.0), [xf.l(RADIUS - 0.5); 4]);
    let plate = |p: &Painter| {
        p.add(egui::Shape::convex_polygon(outline.clone(), xf.c(col::plate(snap.cfg.plate_opacity)), egui::Stroke::NONE));
    };
    match map {
        Some(tex) => {
            let cal = calibration(snap);
            // Mirror on: the texture wraps (MirroredRepeat), so the whole pill is textured and
            // the reflected continuation shows past the edge. Off: the pill is cut to the image
            // and the plate shows outside it, like the Dashboard.
            let (shape, hub) = if snap.cfg.map_mirror_edges {
                (outline.clone(), centre)
            } else {
                plate(p);
                let quad: Vec<Pos2> = cal
                    .image_corners(tex.orig_size)
                    .iter()
                    .map(|&(wx, wz, _)| {
                        let [ox, oy] = view.world_to_offset(wx, wz);
                        centre + vec2(ox, oy)
                    })
                    .collect();
                let cut = clip_convex(&outline, &quad);
                let hub = cut.iter().fold(Vec2::ZERO, |a, q| a + q.to_vec2()) / cut.len().max(1) as f32;
                (cut, hub.to_pos2())
            };
            if shape.len() >= 3 {
                let mut mesh = Mesh::with_texture(tex.id);
                let white = xf.c(Color32::WHITE);
                let uv = |pt: Pos2| {
                    let [u, v] = view.uv_at_offset(&cal, tex.orig_size, pt.x - centre.x, pt.y - centre.y);
                    pos2(u, v)
                };
                fan(&mut mesh, hub, &shape, |pt| Vertex { pos: pt, uv: uv(pt), color: white });
                p.add(mesh);
            }
            // Darken so the white marker reads over bright maps (winter more).
            let tint = xf.c(if tex.winter { col::MAP_TINT_WINTER } else { col::MAP_TINT });
            p.add(egui::Shape::convex_polygon(outline.clone(), tint, egui::Stroke::NONE));
        }
        None => plate(p),
    }

    // Markers: the Dashboard map's drawing code (`map_shared`), clipped to the pill's inner rect.
    let inner = xf.rect(3.0, 3.0, w - 6.0, h - 6.0);
    let mp = p.with_clip_rect(inner);
    let cv = MapCanvas { p: &mp, view: &view, centre, rect: inner, s: xf.s, a: xf.a, pause_glyph: "||" };
    let (car, cfg) = ((snap.pkt.position_x, snap.pkt.position_z), &*snap.cfg);
    let hue = |h: f32| crate::ui::coop::hue_color(h);
    let at = coop.now.unwrap_or_else(Instant::now);
    let fade = TrailFade::new(cfg.coop_trail_fade_secs, cfg.coop_trail_fade_m);
    if let Some(tr) = coop.trails.get("local") {
        map_shared::draw_trail(&cv, tr, hue(snap.coop_hue), fade, at);
    }
    for t in coop.teammates.iter().filter(|t| !t.paused) {
        if let Some(tr) = coop.trails.get(&t.id) {
            map_shared::draw_trail(&cv, tr, t.colour, fade, at);
        }
    }
    map_shared::draw_remotes(&cv, &coop.teammates, car, view.yaw);

    // Own arrow: the player's co-op colour in a session, white otherwise (as the Dashboard).
    let own = if coop.in_session { hue(snap.coop_hue) } else { Color32::WHITE };
    map_shared::draw_own_arrow(&cv, view.arrow_angle(snap.pkt.yaw), own);
    for &(x, z, hue_deg) in &coop.waypoints {
        map_shared::draw_waypoint(&cv, (x, z), hue(hue_deg), car, now as f32);
    }

    if cfg.compass {
        draw_compass(p, xf, view.north_dir());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compass_north_follows_map_rotation() {
        let up = MapView::new(0.0, 0.0, 0.0, 400.0, 136.0).north_dir();
        assert!(up[0].abs() < 1e-6 && (up[1] + 1.0).abs() < 1e-6, "{up:?}");
        // Map turned a quarter clockwise: north swings to the left of the screen.
        let q = MapView::new(0.0, 0.0, std::f32::consts::FRAC_PI_2, 400.0, 136.0).north_dir();
        assert!((q[0] + 1.0).abs() < 1e-6 && q[1].abs() < 1e-6, "{q:?}");
    }

    #[test]
    fn target_yaw_follows_north_up_and_stopped_options() {
        let mut pkt = crate::packet::ForzaPacket::default();
        pkt.yaw = 0.8;
        let mut cfg = crate::config::OverlayConfig::default();
        // Default: heading-up on the raw yaw, stopped or not.
        assert_eq!(map_target_yaw(&pkt, &cfg, false), 0.8);
        assert_eq!(map_target_yaw(&pkt, &cfg, true), 0.8);
        cfg.map_north_up_when_stopped = true;
        assert_eq!(map_target_yaw(&pkt, &cfg, false), 0.8);
        assert_eq!(map_target_yaw(&pkt, &cfg, true), 0.0);
        cfg.map_north_up_when_stopped = false;
        cfg.map_north_up = true;
        assert_eq!(map_target_yaw(&pkt, &cfg, false), 0.0);
        // North-up: the car arrow turns by its raw yaw; heading-up it stays at 0.
        let north = MapView::new(0.0, 0.0, 0.0, 400.0, 136.0);
        assert_eq!(north.arrow_angle(pkt.yaw), 0.8);
        let head = MapView::new(0.0, 0.0, map_target_yaw(&pkt, &crate::config::OverlayConfig::default(), false), 400.0, 136.0);
        assert_eq!(head.arrow_angle(pkt.yaw), 0.0);
    }

    #[test]
    fn clip_convex_cuts_a_square_to_the_overlap_either_winding() {
        let sq = |x0: f32, y0: f32, x1: f32, y1: f32| vec![pos2(x0, y0), pos2(x1, y0), pos2(x1, y1), pos2(x0, y1)];
        let area = |p: &[Pos2]| (0..p.len()).map(|i| p[i].x * p[(i + 1) % p.len()].y - p[(i + 1) % p.len()].x * p[i].y).sum::<f32>().abs() / 2.0;
        let subject = sq(0.0, 0.0, 10.0, 10.0);
        let clip = sq(5.0, 5.0, 20.0, 20.0);
        assert!((area(&clip_convex(&subject, &clip)) - 25.0).abs() < 1e-3);
        let mut rev = clip.clone();
        rev.reverse();
        assert!((area(&clip_convex(&subject, &rev)) - 25.0).abs() < 1e-3);
        // Fully inside: unchanged; disjoint: empty.
        assert!((area(&clip_convex(&subject, &sq(-5.0, -5.0, 50.0, 50.0))) - 100.0).abs() < 1e-3);
        assert!(clip_convex(&subject, &sq(20.0, 20.0, 30.0, 30.0)).is_empty());
    }

    fn input(in_session: bool, remotes: Vec<(f32, f32, bool)>) -> CoopInput {
        let remotes = remotes
            .into_iter()
            .enumerate()
            .map(|(i, (x, z, paused))| {
                let mut p = crate::packet::ForzaPacket::default();
                p.position_x = x;
                p.position_z = z;
                p.position_y = if paused { 0.0 } else { 1.0 };
                p.is_race_on = if paused { 0 } else { 1 };
                p.engine_max_rpm = if paused { 0.0 } else { 8000.0 };
                (crate::coop::PlayerInfo { id: format!("p{i}"), name: format!("P{i}"), hue: 100.0 }, p)
            })
            .collect();
        CoopInput { in_session, remotes, waypoints: vec![(1.0, 2.0, 30.0)] }
    }

    fn driving_snap(cfg: crate::config::OverlayConfig, x: f32, z: f32) -> HudSnapshot {
        let mut s = HudSnapshot { cfg: std::sync::Arc::new(cfg), connected: true, ..Default::default() };
        s.pkt.position_x = x;
        s.pkt.position_z = z;
        s.pkt.position_y = 1.0; // not the all-zero "paused" packet
        s.pkt.is_race_on = 1;
        s.pkt.engine_max_rpm = 8000.0;
        s
    }

    #[test]
    fn coop_layer_records_own_and_teammate_trails_only_in_a_session() {
        let cfg = crate::config::OverlayConfig::default();
        let mut layer = CoopLayer::default();
        let t0 = Instant::now();
        // Not in a session: nothing is recorded or drawn.
        layer.update(Some(input(false, vec![(0.0, 0.0, false)])), &driving_snap(cfg.clone(), 0.0, 0.0), t0);
        assert!(!layer.in_session && layer.trails.is_empty() && layer.teammates.is_empty() && layer.waypoints.is_empty());

        // In a session: own + teammate trails grow with movement (4 m spacing), waypoint kept.
        for i in 0..3 {
            let x = i as f32 * 10.0;
            layer.update(Some(input(true, vec![(x, 5.0, false)])), &driving_snap(cfg.clone(), x, 0.0), t0);
        }
        assert!(layer.in_session);
        assert_eq!(layer.trails["local"].len(), 3);
        assert_eq!(layer.trails["p0"].len(), 3);
        assert_eq!((layer.teammates.len(), layer.waypoints.len()), (1, 1));

        // The teammate pauses (packet at the origin): drawn at the last spot, no new trail point.
        layer.update(Some(input(true, vec![(0.0, 0.0, true)])), &driving_snap(cfg.clone(), 20.0, 0.0), t0);
        let t = &layer.teammates[0];
        assert!(t.paused && (t.x, t.z) == (20.0, 5.0), "{t:?}");
        assert_eq!(layer.trails["p0"].len(), 3);

        // Session ends: everything is forgotten.
        layer.update(None, &driving_snap(cfg, 20.0, 0.0), t0);
        assert!(!layer.in_session && layer.trails.is_empty() && layer.teammates.is_empty());
    }

    /// The HUD runs the same look-around as the Dashboard: in north-up and heading-up alike the
    /// stick turns the view to `heading + stick angle` (car-relative), then eases back on release.
    #[test]
    fn map_anim_look_is_car_relative_in_north_up() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let view_yaw = |a: &MapAnim| a.yaw.unwrap() + a.look;
        let ang = |a: f32, b: f32| mm::wrap_angle(a - b).abs() < 1e-2;
        for north_up in [true, false] {
            let cfg = crate::config::OverlayConfig { map_north_up: north_up, map_look_stick: true, map_smooth_rotation: false, ..Default::default() };
            let mut s = driving_snap(cfg, 0.0, 0.0);
            s.pkt.speed = 30.0;
            s.pkt.yaw = FRAC_PI_2; // heading east
            s.look_stick = (1.0, 0.0); // look right
            let mut anim = MapAnim::default();
            let mut t = 0.0;
            for _ in 0..240 {
                anim.step(&s, t);
                t += 1.0 / 60.0;
            }
            // The car's right (south) is at the top: view yaw = heading + 90 deg = 180 deg.
            assert!(ang(view_yaw(&anim), PI), "north_up {north_up}: {}", view_yaw(&anim));
            assert!(!anim.step(&s, t), "settled look must stop animating");
            s.look_stick = (0.0, 0.0);
            for _ in 0..240 {
                t += 1.0 / 60.0;
                anim.step(&s, t);
            }
            let rest = if north_up { 0.0 } else { FRAC_PI_2 };
            assert!(ang(view_yaw(&anim), rest), "north_up {north_up}: {}", view_yaw(&anim));
        }
    }

    #[test]
    fn coop_layer_follows_the_effective_toggles() {
        let mut layer = CoopLayer::default();
        let t0 = Instant::now();
        let cfg = crate::config::OverlayConfig { coop_trails: false, coop_teammates: false, coop_waypoints: false, ..Default::default() };
        layer.update(Some(input(true, vec![(0.0, 0.0, false)])), &driving_snap(cfg, 0.0, 0.0), t0);
        assert!(layer.in_session && layer.trails.is_empty() && layer.teammates.is_empty() && layer.waypoints.is_empty());
    }
}
