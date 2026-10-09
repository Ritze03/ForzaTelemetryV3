//! M2′ minimap (pill frame, no scale bar), 208 × 136. Heading-up season map clipped to a
//! rounded rect, drawn by the shared renderer (`maprender`, D61): `draw_base` for the image, then
//! `draw_layers` for roads / race lines / POIs, both through one [`Camera`] (flat, or tilted by
//! default) and cut to the pill's rounded corners.
//!
//! **3D (phase K, K3).** With the Map tab's View mode on *3D* the image and roads come from the
//! shared GL scene (`maprender::gl3d`) instead: [`draw`] adds it as a paint callback over the pill
//! and draws the vectors that stay on egui (race lines, POIs), the markers, compass and border
//! over it. Its inputs reach [`draw`] through [`MapAnim`] ([`Scene3dIn`], set per frame by
//! `overlay::render`) because `draw`'s signature is fixed. Until the scene is `Ready`, and for
//! good when it failed, the tilted 2D map is drawn instead (`Gl3dHandle::wants_underlay`).
//!
//! The season image is loaded on a helper thread ([`MapLoader`]); until it arrives the frame
//! draws over the plate (none by default). The layer data (`maprender::layers()`) and the POI
//! icons reach [`draw`] through [`MapAnim`], set per frame by `overlay::render`.
//!
//! The own arrow, co-op teammates, trails and waypoints are drawn by `hud::map_shared`, the
//! same code as the Dashboard map. The co-op state they need (teammates at their last known
//! spot, per-player trail buffers) lives in [`CoopLayer`], fed on the overlay thread.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use egui::{vec2, Color32, Painter, TextureHandle, Vec2};

use super::col;
use super::map_shared::{self, MapCanvas, Remote, TrailFade};
use super::prims::{self, Xf};
use crate::maprender::data::MapLayers;
use crate::maprender::paint2d::{draw_layers_parts, CornerClip, IconAtlas, Parts};
use crate::maprender::{draw_base, BaseParams, Camera, LayerCtx, RaceSel};
use crate::minimap::{self as mm, MapCalibration, Season, Trail};
use crate::overlay::snapshot::HudSnapshot;

pub const SIZE: Vec2 = vec2(208.0, 136.0);
const RADIUS: f32 = 22.0;
/// How often the wall-clock season is re-checked, seconds.
const SEASON_CHECK_SECS: f64 = 60.0;

// The uploaded season map is shared with the Dashboard map (`maprender::MapTex`).
pub use crate::maprender::MapTex;

type Loaded = Result<(egui::ColorImage, [u32; 2]), mm::MapLoadError>;

/// Loads the overlay's season map off the render thread (~0.06 s cold at HUD quality 50, which reads pyramid level 2 directly) and uploads it on the
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
            let r = rx.try_recv();
            match r {
                Ok(Ok((img, orig))) => {
                    // ponytail: the 64 MiB upload + mip generation lands in one visible frame
                    // (tens of ms, once per season). Upgrade: upload before the surface is
                    // mapped, or in tiles over several frames.
                    let handle = ctx.load_texture("hud-map", img, mm::OVERLAY_MAP_TEXTURE_OPTIONS);
                    self.tex = Some((handle, orig, *season));
                    self.pending = None;
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    // A failed load is normal without an FH6 install (and instant). Forget the
                    // attempt so the once-a-minute season check retries, which also picks up an
                    // install set later in Setup.
                    if let Ok(Err(e)) = &r {
                        eprintln!("overlay: season map failed to load: {e}");
                    }
                    self.wanted = None;
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

/// Everything the 3D branch of [`draw`] needs besides the config and the packet: this context's
/// GL handle, the terrain (the camera's relief) and the newest road mesh. Set per frame by the
/// overlay renderer only while the Minimap is in 3D mode and the terrain is loaded
/// ([`MapAnim::set_scene3d`]); without it [`draw`] is the 2D map, flat or tilted.
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Clone)]
pub struct Scene3dIn {
    pub gl3d: crate::maprender::gl3d::Gl3dHandle,
    pub terrain: Arc<crate::maprender::terrain::Terrain>,
    /// `store::road_mesh`; `None` while it builds = terrain only.
    pub mesh: Option<Arc<crate::maprender::mesh3d::RoadMesh>>,
}

/// Is the Minimap to be drawn in 3D? View mode *3D*, and not on Windows: the Windows overlay path
/// (WGL context, offscreen FBO + readback) cannot be tested by the developers, so there 3D is
/// treated as Tilted until a tester has run it (K3; a config opt-in is proposed in
/// `docs/features/overlay.md`).
pub fn wants_3d(cfg: &crate::config::OverlayConfig) -> bool {
    cfg.minimap_on && cfg.map_layers.tilt.view_mode() == crate::maprender::cfg::ViewMode::Relief && !cfg!(windows)
}

/// The car's height for the 3D camera: the telemetry height plus about a metre (the roof, so the
/// camera clears the road it drives on) while driving; `None` (the terrain under the car) when
/// the game is not sending real positions (paused packet, no race on, not connected).
fn car_height(snap: &HudSnapshot) -> Option<f32> {
    let p = &snap.pkt;
    (snap.connected && p.is_race_on != 0 && !p.is_paused()).then_some(p.position_y + 1.0)
}

/// Eased view state (yaw and zoom follow the car smoothly, like the Dashboard map).
#[derive(Default)]
pub struct MapAnim {
    /// `now` of the previous step (for dt).
    last: Option<f64>,
    yaw: Option<f32>,
    zoom: Option<f32>,
    /// Right-stick look-around; owns the drawn view yaw (`view`).
    look: mm::LookAround,
    /// The yaw the map is drawn with: `yaw` under the look-around.
    view: f32,
    slow_since: Option<f64>,
    /// The shared layer data (roads, POIs, race lines) from `maprender::layers()`, set per frame by
    /// the overlay renderer ([`MapAnim::set_layers`], via `Hud::set_layers`); `None` draws the
    /// image alone. Lives here because `draw`'s signature is fixed (the PNG harness and tests
    /// call it) and this is the minimap's per-frame state.
    layers: Option<Arc<MapLayers>>,
    /// This context's uploaded POI icons ([`MapAnim::set_icons`]).
    icons: Option<Arc<IconAtlas>>,
    /// Which race lines to draw (one selector per map, `maprender::RaceSel`).
    race_sel: RaceSel,
    /// The 3D scene's inputs; `None` = the 2D map ([`Scene3dIn`]).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    scene3d: Option<Scene3dIn>,
}

impl MapAnim {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn set_scene3d(&mut self, scene: Option<Scene3dIn>) {
        self.scene3d = scene;
    }

    /// The 3D scene still needs frames (staged init, an upload waiting its turn).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub fn scene3d_busy(&self) -> bool {
        self.scene3d.as_ref().is_some_and(|s| s.gl3d.busy())
    }

    pub fn set_layers(&mut self, layers: Option<Arc<MapLayers>>) {
        self.layers = layers;
    }

    pub fn set_icons(&mut self, icons: Option<Arc<IconAtlas>>) {
        self.icons = icons;
    }

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
        // Look-around is relative to the car's heading (the heading-up yaw) in every mode, and
        // while held it ignores the base yaw (see `minimap::LookAround`).
        let heading = mm::target_yaw(pkt, cfg.map_use_movement_dir);
        self.view = self.look.step(snap.look_stick, cfg.map_look_stick, heading, yaw, target_yaw, dt);
        self.yaw = Some(yaw);
        self.zoom = Some(zoom);
        let yaw_left = (mm::lerp_angle(yaw, target_yaw, 1.0) - yaw).abs() > 1e-3;
        let zoom_left = (zoom - target_zoom).abs() > 0.5 && (stopped || kmh >= mm::STOPPED_KMH);
        yaw_left || zoom_left || self.look.easing()
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

/// The co-op layer of the Minimap: who to draw, their trails, the shared waypoints. The own
/// trail is kept without a session too (the solo trail, drawn white).
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
    /// Refresh from this frame's co-op state. Anything the effective config turns off leaves
    /// the matching part empty (and forgets its history). Without a session only the own
    /// trail is kept (solo trail, drawn white); it survives a session starting or ending.
    pub fn update(&mut self, input: Option<CoopInput>, snap: &HudSnapshot, now: Instant) {
        let cfg = &*snap.cfg;
        self.now = Some(now);
        if !cfg.minimap_on {
            *self = Self { now: self.now, ..Self::default() };
            return;
        }
        let max_age = Duration::from_secs_f32(cfg.coop_trail_fade_secs.max(0.5));
        let mut present: HashSet<String> = HashSet::new();

        // Own trail, in a session or solo: only while driving (not paused, game connected),
        // like the Dashboard. "Show trails" off drops it in both cases.
        let pkt = &snap.pkt;
        if cfg.coop_trails {
            if snap.connected && pkt.is_race_on != 0 && !pkt.is_paused() {
                mm::trail_push(self.trails.entry("local".into()).or_default(), pkt.position_x, pkt.position_z, now, max_age);
            }
            present.insert("local".into());
        }

        let Some(input) = input.filter(|i| i.in_session) else {
            let own = self.trails.remove("local").filter(|_| cfg.coop_trails);
            *self = Self { now: self.now, ..Self::default() };
            if let Some(tr) = own {
                self.trails.insert("local".into(), tr);
            }
            return;
        };
        self.in_session = true;

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

/// Draw M2′. Returns true while the view is still easing.
///
/// Layers: satellite image ([`draw_base`]) → roads / race lines / POIs ([`draw_layers_parts`], from
/// `anim.layers`, cut to the pill's rounded corners) → markers (`map_shared`) → compass → frame.
/// The camera (flat or tilted, `OverlayConfig::map_layers.tilt`) is shared by all of them.
///
/// **In 3D** (`anim` has a [`Scene3dIn`] and the scene is `Ready`) the image and the roads are the
/// GL scene's, added as a paint callback over the plate; the tint, the race lines and POIs
/// (`Parts::OVER_3D`, no occlusion by hills), the markers, compass and border follow over it, all
/// through the relief camera (`Camera::project` lands on the terrain). *Why the tint comes after:*
/// it darkens the image so the white marker reads, and a callback is opaque, so a tint drawn
/// before it would be hidden (it also dims the GL roads a little, 12 % in summer).
pub fn draw(p: &Painter, xf: &Xf, snap: &HudSnapshot, now: f64, anim: &mut MapAnim, map: Option<MapTex>, coop: &CoopLayer) -> bool {
    let animating = anim.step(snap, now);
    let (yaw, zoom) = (anim.view, anim.zoom.unwrap_or(snap.cfg.zoom_driving_m));
    let (w, h) = (SIZE.x, SIZE.y);
    let cfg = &*snap.cfg;
    let lc = &cfg.map_layers;
    let car = (snap.pkt.position_x, snap.pkt.position_z);
    let rect = xf.rect(0.0, 0.0, w, h);
    let cam2d = Camera::from_cfg(&lc.tilt, car, yaw, zoom, rect);
    // The relief camera, when 3D is on and the terrain is there; `ready` = the scene is drawing
    // (otherwise the 2D map is the underlay, see `Gl3dHandle::wants_underlay`).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let three = anim.scene3d.clone().and_then(|sc| {
        let cam = Camera::from_cfg_relief(&lc.tilt, car, yaw, zoom, rect, Some(&sc.terrain), car_height(snap));
        cam.relief.is_some().then_some((sc, cam))
    });
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let cam3: Option<&Camera> = three.as_ref().filter(|(sc, _)| !sc.gl3d.wants_underlay()).map(|(_, c)| c);
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let cam3: Option<&Camera> = None;
    // What markers and vectors project through.
    let cam = cam3.unwrap_or(&cam2d);
    let view = cam.view;

    // The map shape: the frame's rounded rect, inset 0.5 px so the 3 px border (drawn after,
    // feathered) covers the mesh's hard edge.
    let outline = prims::rounded_points(xf.rect(0.5, 0.5, w - 1.0, h - 1.0), [xf.l(RADIUS - 0.5); 4]);
    let plate = xf.c(col::plate(cfg.map_plate_opacity));
    if plate.a() > 0 {
        p.add(egui::Shape::convex_polygon(outline.clone(), plate, egui::Stroke::NONE));
    }
    // Darken so the white marker reads over bright maps (winter more), as strongly as the image shows.
    let tint = |tex: MapTex| {
        let c = xf.c(if tex.winter { col::MAP_TINT_WINTER } else { col::MAP_TINT }).gamma_multiply(lc.image.opacity.clamp(0.0, 1.0));
        p.add(egui::Shape::convex_polygon(outline.clone(), c, egui::Stroke::NONE));
    };
    let image = map.filter(|_| lc.image.on);
    if cam3.is_none() {
        if let Some(tex) = image {
            draw_base(
                p,
                &BaseParams {
                    cam: &cam2d,
                    cal: calibration(snap),
                    tex,
                    outline: &outline,
                    // Mirror on: the texture wraps (MirroredRepeat), so the whole pill is textured and
                    // the reflected continuation shows past the edge. Off: the shape is cut to the
                    // image and the plate shows outside it, like the Dashboard.
                    mirror: cfg.map_mirror_edges,
                    look: (&lc.image).into(),
                    a: xf.a,
                    // The far edge of a tilted map fades out into the plate, or the game when there is none.
                    far_fade: true,
                },
            );
            tint(tex);
        }
    }

    // Race selection and the in-race focus (shared by the 2D roads and the GL ones).
    let MapAnim { layers, icons, race_sel, .. } = anim;
    let data = layers.as_deref().filter(|_| lc.wants_layers());
    let picked = data.map(|d| &*race_sel.update(&d.races, &lc.race_lines, car, snap.pkt.yaw, snap.pkt.race_position != 0));

    // The 3D scene, over the plate (and, until it is Ready, over the 2D underlay).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    if let Some((sc, c3)) = &three {
        use crate::maprender::cfg::OtherRoads;
        use crate::maprender::gl3d::{add_scene, Focus3d, Scene3d};
        let focus = data.zip(picked).and_then(|(d, rs)| {
            let focusing = rs.focus_line().is_some_and(|l| l < d.races.lines.len());
            (focusing && lc.race_lines.focus.other_roads != OtherRoads::Normal).then(|| rs.road_focus(d)).flatten()
        });
        add_scene(
            &p.with_clip_rect(rect),
            &sc.gl3d,
            Scene3d {
                cam: c3.clone(),
                mesh: sc.mesh.clone(),
                map: image,
                cal: calibration(snap),
                look: (&lc.image).into(),
                mirror: cfg.map_mirror_edges,
                a: xf.a,
                s: xf.s,
                corner_radius: xf.l(RADIUS),
                relief: lc.tilt.relief,
                roads: lc.roads,
                focus: focus.map(|focus| Focus3d { focus, cfg: lc.race_lines.focus }),
            },
        );
    }
    if cam3.is_some() {
        if let Some(tex) = image {
            tint(tex);
        }
    }

    // Roads (2D only), race lines, POIs: the shared renderer, vectors cut to the pill's rounded
    // corners (safe = the rect shrunk by the corner radius, which is wholly inside the pill).
    if let (Some(data), Some(picked)) = (data, picked) {
        let lp = p.with_clip_rect(rect);
        draw_layers_parts(
            &LayerCtx {
                p: &lp,
                cam,
                s: xf.s,
                a: xf.a,
                car,
                corner_clip: Some(CornerClip { poly: &outline, safe: rect.shrink(xf.l(RADIUS)) }),
                icons: icons.as_deref(),
                race_sel: picked,
                week: None,
            },
            data,
            lc,
            if cam3.is_some() { Parts::OVER_3D } else { Parts::ALL },
        );
    }

    // Markers: the Dashboard map's drawing code (`map_shared`), clipped to the pill's inner rect.
    let inner = xf.rect(3.0, 3.0, w - 6.0, h - 6.0);
    let mp = p.with_clip_rect(inner);
    let cv = MapCanvas { p: &mp, cam, rect: inner, taper: lc.tilt.taper, s: xf.s, a: xf.a, pause_glyph: "||" };
    let hue = |h: f32| crate::ui::coop::hue_color(h);
    let at = coop.now.unwrap_or_else(Instant::now);
    let fade = TrailFade::new(cfg.coop_trail_fade_secs, cfg.coop_trail_fade_m);
    // Own arrow and trail: the player's co-op colour in a session, white otherwise (as the
    // Dashboard).
    let own = if coop.in_session { hue(snap.coop_hue) } else { Color32::WHITE };
    if let Some(tr) = coop.trails.get("local") {
        map_shared::draw_trail(&cv, tr, own, fade, at);
    }
    for t in coop.teammates.iter().filter(|t| !t.paused) {
        if let Some(tr) = coop.trails.get(&t.id) {
            map_shared::draw_trail(&cv, tr, t.colour, fade, at);
        }
    }
    map_shared::draw_remotes(&cv, &coop.teammates, car, view.yaw);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimap::MapView;

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
    fn coop_layer_records_teammates_in_a_session_and_the_own_trail_always() {
        let cfg = crate::config::OverlayConfig::default();
        let mut layer = CoopLayer::default();
        let t0 = Instant::now();
        // Not in a session: only the own (solo) trail is recorded; no teammates or waypoints.
        layer.update(Some(input(false, vec![(0.0, 0.0, false)])), &driving_snap(cfg.clone(), -10.0, 0.0), t0);
        assert!(!layer.in_session && layer.teammates.is_empty() && layer.waypoints.is_empty());
        assert_eq!(layer.trails.keys().collect::<Vec<_>>(), ["local"]);
        // No co-op handle at all: the same.
        layer.update(None, &driving_snap(cfg.clone(), -20.0, 0.0), t0);
        assert_eq!((layer.trails.len(), layer.trails["local"].len()), (1, 2));

        // In a session: own + teammate trails grow with movement (4 m spacing), waypoint kept.
        // The solo points carry over into the session.
        for i in 0..3 {
            let x = i as f32 * 10.0;
            layer.update(Some(input(true, vec![(x, 5.0, false)])), &driving_snap(cfg.clone(), x, 0.0), t0);
        }
        assert!(layer.in_session);
        assert_eq!(layer.trails["local"].len(), 5);
        assert_eq!(layer.trails["p0"].len(), 3);
        assert_eq!((layer.teammates.len(), layer.waypoints.len()), (1, 1));

        // The teammate pauses (packet at the origin): drawn at the last spot, no new trail point.
        layer.update(Some(input(true, vec![(0.0, 0.0, true)])), &driving_snap(cfg.clone(), 20.0, 0.0), t0);
        let t = &layer.teammates[0];
        assert!(t.paused && (t.x, t.z) == (20.0, 5.0), "{t:?}");
        assert_eq!(layer.trails["p0"].len(), 3);

        // Session ends: teammates, their trails and waypoints are forgotten; the own trail stays.
        layer.update(None, &driving_snap(cfg.clone(), 20.0, 0.0), t0);
        assert!(!layer.in_session && layer.teammates.is_empty() && layer.waypoints.is_empty());
        assert_eq!((layer.trails.len(), layer.trails["local"].len()), (1, 5));

        // "Show trails" off: the solo trail goes too.
        let off = crate::config::OverlayConfig { coop_trails: false, ..Default::default() };
        layer.update(None, &driving_snap(off, 30.0, 0.0), t0);
        assert!(layer.trails.is_empty());
        // Minimap module off: nothing.
        layer.update(None, &driving_snap(cfg.clone(), 40.0, 0.0), t0);
        let hidden = crate::config::OverlayConfig { minimap_on: false, ..Default::default() };
        layer.update(None, &driving_snap(hidden, 50.0, 0.0), t0);
        assert!(layer.trails.is_empty());
    }

    /// The HUD runs the same look-around as the Dashboard: in north-up and heading-up alike the
    /// stick turns the view to `heading + stick angle` (car-relative), then eases back on release.
    #[test]
    fn map_anim_look_is_car_relative_in_north_up() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let view_yaw = |a: &MapAnim| a.view;
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

    /// The reported jolt, through the HUD's own `MapAnim`: stick held right, the car stops and
    /// "north up when stopped" engages after 1.5 s. The view must not move (it used to swing
    /// toward north, then back); released, it eases monotonically to north.
    #[test]
    fn map_anim_held_look_ignores_north_up_when_stopped() {
        use std::f32::consts::FRAC_PI_2;
        for smooth in [true, false] {
            let cfg = crate::config::OverlayConfig { map_north_up_when_stopped: true, map_look_stick: true, map_smooth_rotation: smooth, ..Default::default() };
            let mut s = driving_snap(cfg, 0.0, 0.0);
            s.pkt.speed = 30.0;
            s.pkt.yaw = 0.8;
            s.look_stick = (1.0, 0.0);
            let want = mm::wrap_angle(0.8 + FRAC_PI_2);
            let mut anim = MapAnim::default();
            let mut t = 0.0;
            for _ in 0..240 {
                anim.step(&s, t);
                t += 1.0 / 60.0;
            }
            s.pkt.speed = 0.0; // stops: after 1.5 s the base eases to north
            for i in 0..300 {
                anim.step(&s, t);
                t += 1.0 / 60.0;
                assert!(mm::wrap_angle(anim.view - want).abs() < 1e-3, "smooth {smooth} frame {i}: {}", anim.view);
            }
            assert!(anim.yaw.unwrap().abs() < 1e-2, "base went north underneath: {:?}", anim.yaw);
            s.look_stick = (0.0, 0.0);
            let mut prev = anim.view;
            for _ in 0..300 {
                anim.step(&s, t);
                t += 1.0 / 60.0;
                assert!(anim.view.abs() <= prev.abs() + 1e-6, "{prev} -> {}", anim.view);
                prev = anim.view;
            }
            assert!(prev.abs() < 1e-2 && !anim.step(&s, t), "{prev}");
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
