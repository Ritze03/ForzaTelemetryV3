//! The map scene both full maps draw: the Dashboard's Map widget and the Map tab's viewer
//! (D67). Extracted from `dashboard::show_minimap_widget` so the viewer does not fork it.
//!
//! One function, [`draw`], paints everything: the shared renderer (`maprender::draw_base` +
//! `draw_layers`, D61) and the markers (`hud::map_shared`) over `rect`. What differs per caller is
//! [`Scene`]: the layer config, where the view is centred, its rotation and zoom. The Dashboard
//! centres on the car; the viewer may be panned away from it.

use std::cell::RefCell;
use std::time::Duration;

use egui::{pos2, vec2, Color32, Rect, Stroke, Ui, Vec2};

use crate::app::ForzaApp;
use crate::i18n::tr;
use crate::maprender::cfg::MapLayerConfig;
use crate::maprender::{Camera, RaceSel};

/// What one map draws with.
pub struct Scene<'a> {
    pub layers: &'a MapLayerConfig,
    /// The world point (x, z) at the view centre (the car, for a following view).
    pub centre: (f32, f32),
    /// The map's rotation: 0 = north up, else the heading for heading-up.
    pub yaw: f32,
    /// Metres from the view centre to the nearest edge.
    pub zoom_m: f32,
    pub mirror: bool,
    pub compass: bool,
    /// This map's own race-line selection state (the Dashboard and the viewer each keep one).
    pub race_sel: &'a RefCell<RaceSel>,
}

// ── manual pan and zoom (D72) ────────────────────────────────────────────────────────────────

/// Zoom limits of a manual view, as the radius (metres from the centre to the nearest edge):
/// close enough to see single lanes, far enough to see the whole map in a wide window.
pub const ZOOM_MIN_M: f32 = 50.0;
pub const ZOOM_MAX_M: f32 = 8000.0;

/// The car counts as driving from this speed (m/s, ~11 km/h) …
pub const DRIVE_MS: f32 = 3.0;
/// … and as stopped below this one (~7 km/h). Between the two nothing changes: the gap is the
/// hysteresis that keeps a crawl or a nudge around the threshold from flipping the state.
pub const STOPPED_MS: f32 = 2.0;
/// The car must stay above [`DRIVE_MS`] this long (s) before the view snaps back, so a short
/// nudge (a bump, a tap on the throttle) does not take the map away from the user.
pub const RESET_DELAY_S: f64 = 0.6;

/// "Has the player started driving again?" for a manual view. It resets only after the car has
/// been seen stopped since the view went manual, then holds [`DRIVE_MS`] for [`RESET_DELAY_S`].
/// *Why "seen stopped":* panning while already driving (a passenger, a quick look ahead) must not
/// snap back a moment later; it snaps back the next time the car stops and drives off.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriveGate {
    stopped_seen: bool,
    driving_since: Option<f64>,
}

impl DriveGate {
    /// The view just went manual at `speed` (m/s; `None` = no telemetry, counts as stopped).
    fn arm(speed: Option<f32>) -> Self {
        Self { stopped_seen: speed.is_none_or(|v| v < DRIVE_MS), driving_since: None }
    }

    /// One frame at `now` (s). True = reset the view. No telemetry (`None`) changes nothing
    /// except the pending delay: the manual view stays while the game is not sending.
    pub fn step(&mut self, speed: Option<f32>, now: f64) -> bool {
        let Some(v) = speed else {
            self.driving_since = None;
            return false;
        };
        if v < STOPPED_MS {
            self.stopped_seen = true;
            self.driving_since = None;
            return false;
        }
        if v >= DRIVE_MS && self.stopped_seen {
            let since = *self.driving_since.get_or_insert(now);
            return now - since >= RESET_DELAY_S;
        }
        false // in the hysteresis band (or driving since before the pan): keep waiting
    }

    /// A reset is counting down (the caller keeps repainting so it fires without input).
    pub fn pending(&self) -> bool {
        self.driving_since.is_some()
    }
}

/// A map's temporary manual view: the user's pan (`centre`) and zoom (`zoom_m`) over the map's
/// own base view (the car, the configured zoom). Shared by the Dashboard map and the Map tab
/// viewer so they behave alike. `None` = that part follows the base.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ManualView {
    pub centre: Option<(f32, f32)>,
    pub zoom_m: Option<f32>,
    gate: DriveGate,
}

/// What [`ManualView::interact`] needs to know about this frame's map.
pub struct ViewIn<'a> {
    pub layers: &'a MapLayerConfig,
    pub yaw: f32,
    pub rect: Rect,
    /// The car's position and the base zoom the view falls back to.
    pub car: (f32, f32),
    pub base_zoom_m: f32,
    /// Packet speed in m/s; `None` = nothing received.
    pub speed: Option<f32>,
    /// `ui.input(|i| i.time)`.
    pub now: f64,
}

pub fn camera(layers: &MapLayerConfig, centre: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect) -> Camera {
    Camera::from_cfg(&layers.tilt, centre, yaw, zoom_m, rect)
}

/// The view centre after dragging the pointer from `prev` to `now`: the world point grabbed at
/// `prev` ends up under `now`.
pub fn panned(cam: &Camera, centre: (f32, f32), prev: egui::Pos2, now: egui::Pos2) -> (f32, f32) {
    match (cam.unproject(prev), cam.unproject(now)) {
        (Some([ax, az]), Some([bx, bz])) => (centre.0 + ax - bx, centre.1 + az - bz),
        _ => centre,
    }
}

/// (centre, radius) after zooming the radius by `factor`, keeping the world point under `at` where
/// it is on screen (cursor-anchored zoom).
pub fn zoomed_at(
    layers: &MapLayerConfig,
    centre: (f32, f32),
    yaw: f32,
    zoom_m: f32,
    rect: Rect,
    at: egui::Pos2,
    factor: f32,
) -> ((f32, f32), f32) {
    let new = (zoom_m * factor).clamp(ZOOM_MIN_M, ZOOM_MAX_M);
    let before = camera(layers, centre, yaw, zoom_m, rect).unproject(at);
    let after = camera(layers, centre, yaw, new, rect).unproject(at);
    match (before, after) {
        (Some([bx, bz]), Some([ax, az])) => ((centre.0 + bx - ax, centre.1 + bz - az), new),
        _ => (centre, new),
    }
}

/// The radius factor of this frame's wheel and pinch input (> 1 zooms out).
pub fn zoom_factor(scroll_y: f32, pinch: f32) -> f32 {
    (-scroll_y * 0.002).exp() / pinch.max(0.05)
}

impl ManualView {
    /// Is any part of the view manual?
    pub fn is_manual(&self) -> bool {
        self.centre.is_some() || self.zoom_m.is_some()
    }

    /// Back to the base view (follow the car, configured zoom).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Freeze the view where it is (the viewer's "Follow car" switched off without a drag).
    pub fn pin_centre(&mut self, at: (f32, f32), speed: Option<f32>) {
        self.touch(speed);
        self.centre = Some(at);
    }

    /// The effective (centre, radius): the manual parts over the base.
    pub fn view(&self, car: (f32, f32), base_zoom_m: f32) -> ((f32, f32), f32) {
        (self.centre.unwrap_or(car), self.zoom_m.unwrap_or(base_zoom_m))
    }

    /// The user is about to change the view: arm the reset-on-driving gate if it was not manual.
    fn touch(&mut self, speed: Option<f32>) {
        if !self.is_manual() {
            self.gate = DriveGate::arm(speed);
        }
    }

    /// Per frame: advance the reset-on-driving gate. True = it just reset the view.
    pub fn tick(&mut self, ctx: &egui::Context, speed: Option<f32>, now: f64) -> bool {
        if !self.is_manual() {
            return false;
        }
        if self.gate.step(speed, now) {
            self.reset();
            return true;
        }
        if self.gate.pending() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        false
    }

    /// One frame of input on the map's response: a drag pans, the wheel (or a pinch) zooms, then
    /// the reset gate runs. While the view follows the car the wheel zooms around the car (the
    /// car stays put, nothing to anchor to); once panned it zooms around the cursor.
    pub fn interact(&mut self, ui: &Ui, resp: &egui::Response, v: &ViewIn) {
        let (centre, zoom) = self.view(v.car, v.base_zoom_m);
        if resp.dragged_by(egui::PointerButton::Primary) && resp.drag_delta() != Vec2::ZERO {
            if let Some(now) = resp.interact_pointer_pos() {
                let cam = camera(v.layers, centre, v.yaw, zoom, v.rect);
                let new = panned(&cam, centre, now - resp.drag_delta(), now);
                self.touch(v.speed);
                self.centre = Some(new);
            }
        }
        if resp.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = zoom_factor(scroll, pinch);
            if factor != 1.0 {
                let (centre, zoom) = self.view(v.car, v.base_zoom_m);
                let anchor = resp.hover_pos().filter(|_| self.centre.is_some());
                let (new_centre, new_zoom) = match anchor {
                    Some(at) => {
                        let (c, z) = zoomed_at(v.layers, centre, v.yaw, zoom, v.rect, at, factor);
                        (Some(c), z)
                    }
                    None => (self.centre, (zoom * factor).clamp(ZOOM_MIN_M, ZOOM_MAX_M)),
                };
                self.touch(v.speed);
                self.centre = new_centre;
                self.zoom_m = Some(new_zoom);
            }
        }
        self.tick(ui.ctx(), v.speed, v.now);
    }
}

/// The small "Follow car" button the Dashboard map shows in its corner while its view is manual
/// (the viewer has its own, always there). True = pressed (the caller resets the view).
pub fn follow_button(ui: &mut Ui, rect: Rect) -> bool {
    let at = Rect::from_min_size(rect.left_top() + vec2(8.0, 8.0), vec2(rect.width().min(220.0) - 8.0, 26.0));
    let mut hit = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(at).layout(egui::Layout::left_to_right(egui::Align::Min)),
        |ui| {
            hit = ui
                .add(crate::theme::secondary_button(format!("{}  {}", crate::icons::CROSSHAIRS, tr("Follow car"))))
                .clicked();
        },
    );
    hit
}

/// The map image's texture, or (painting a status into `rect` and returning `None`) why there is
/// none yet: a failed load (no install, unreadable, undecodable) or the loading spinner.
pub fn texture_or_status<'a>(ui: &mut Ui, app: &'a ForzaApp, rect: Rect) -> Option<&'a egui::TextureHandle> {
    let Some(texture) = &app.minimap_texture else {
        let center = rect.center();
        // The last load failed (no FH6 install to read the tiles from): say so instead of
        // spinning. Retried by Reload Map and when the install folder changes (`app.rs`).
        if let Some(err) = &app.minimap_error {
            use crate::minimap::MapLoadError as E;
            let (label, sub) = match err {
                E::NoInstall => (
                    tr("Map needs your Forza Horizon 6 install"),
                    tr("Set it in Setup → Game Install").to_string(),
                ),
                E::NotReadable(_) | E::MissingZip(_) | E::Decode(_) => {
                    (tr("Map could not be loaded"), err.to_string())
                }
            };
            let p = ui.painter_at(rect);
            p.text(
                center + vec2(0.0, -4.0),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(13.0),
                crate::theme::TEXT_DIM,
            );
            p.text(
                center + vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                sub,
                egui::FontId::proportional(11.0),
                crate::theme::TEXT_FAINT,
            );
            return None;
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        // Spinner — identical position to the regular "Loading map…" screen
        ui.put(
            egui::Rect::from_center_size(center + vec2(0.0, -16.0), Vec2::splat(32.0)),
            egui::Spinner::new().size(24.0),
        );
        let p = ui.painter_at(rect);
        let (label, sub) = match &app.minimap_cache_progress {
            Some(in_progress) if !in_progress.is_empty() => {
                let names = in_progress.join(", ");
                (tr("Creating Map Cache"), Some(format!("{}: {}…", tr("Processing"), names)))
            }
            _ => (tr("Loading map…"), None),
        };
        p.text(
            center + vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.0),
            crate::theme::TEXT_DIM,
        );
        if let Some(sub_text) = sub {
            p.text(
                center + vec2(0.0, 28.0),
                egui::Align2::CENTER_CENTER,
                sub_text,
                egui::FontId::proportional(11.0),
                crate::theme::TEXT_FAINT,
            );
        }
        return None;
    };
    Some(texture)
}

/// Draw the whole map scene into `rect` (the texture is `texture_or_status`'s): image, roads, POIs
/// and race lines, trails, teammates, the own arrow, shared waypoints, compass and the co-op player
/// list. Returns the camera, for hit-testing clicks (`Camera::unproject`).
pub fn draw(ui: &mut Ui, app: &ForzaApp, rect: Rect, texture: &egui::TextureHandle, sc: &Scene) -> Camera {
    let cfg = &app.config;
    let lc = sc.layers;
    let cal = crate::minimap::MapCalibration::from_config(cfg);

    // The car's real position: distances, near-only layers and the own arrow use it, whatever the
    // view is centred on.
    let car_x = app.minimap_cached_car_x;
    let car_z = app.minimap_cached_car_z;
    let yaw = sc.yaw;

    // The shared renderer's camera (`maprender`): metres visible from the view centre to the nearest edge
    // (zoom); rotates world displacement into car-relative screen space (see
    // `minimap::MapView` for the conventions). Tilted (a config option, no UI yet), the car sits
    // lower in the widget and the map is seen in perspective; the perspective distance scales
    // with the widget's height (`Camera::focal_for`), so it looks like the HUD's at any size.
    let tilted = lc.tilt.on;
    let cam = crate::maprender::Camera::from_cfg(&lc.tilt, sc.centre, yaw, sc.zoom_m, rect);
    let view = cam.view;

    let painter = ui.painter_at(rect);
    let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    if !lc.image.on || tilted {
        // Vectors-only look, or the sky above a tilted map's far edge.
        painter.rect_filled(rect, 0.0, crate::maprender::style::MAP_BACKING);
    }
    if lc.image.on {
        let tex = crate::maprender::MapTex { id: texture.id(), orig_size: app.minimap_orig_size, winter: false };
        crate::maprender::draw_base(&painter, &crate::maprender::BaseParams {
            cam: &cam,
            cal,
            tex,
            outline: &outline,
            mirror: sc.mirror,
            look: (&lc.image).into(),
            a: 1.0,
            far_fade: true,
        });
    }

    // Roads, jump lines, race lines and POIs from the shared store (loaded on its own thread;
    // nothing is requested while every layer is off, and without an install the map is the
    // image alone, as before).
    if lc.wants_layers() {
        let l = crate::maprender::layers();
        if l.status == crate::maprender::LayerStatus::Loading {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
        if let Some(data) = &l.data {
            let icons = app.minimap_icons.borrow_mut().ensure(ui.ctx(), data.icons.as_ref());
            let in_race = app.telemetry.latest.as_ref().is_some_and(|p| p.race_position != 0);
            let mut sel = sc.race_sel.borrow_mut();
            let picked = sel.update(&data.races, &lc.race_lines, (car_x, car_z), app.minimap_cached_raw_yaw, in_race);
            crate::maprender::draw_layers(
                &crate::maprender::LayerCtx {
                    p: &painter,
                    cam: &cam,
                    s: 1.0,
                    a: 1.0,
                    car: (car_x, car_z),
                    corner_clip: None,
                    icons: icons.as_deref(),
                    race_sel: picked,
                    week: None,
                },
                data,
                lc,
            );
        }
    }

    // Markers (trails, teammates, own arrow, waypoints) come from `hud::map_shared`, the same
    // code the HUD Minimap draws with.
    let cv = crate::hud::map_shared::MapCanvas {
        p: &painter,
        cam: &cam,
        rect,
        taper: lc.tilt.taper,
        s: 1.0,
        a: 1.0,
        pause_glyph: crate::icons::PAUSE,
    };

    // Breadcrumb trails (drawn behind the car arrows). Each player's recent path fades from
    // faint (old) to solid (recent) in their identity colour; the own trail is recorded solo
    // too and is then white, like the own arrow.
    let in_session = app.coop.role() != crate::coop::Role::Off;
    let local_col = if in_session { crate::ui::coop::hue_color(app.config.coop_hue) } else { Color32::WHITE };
    let remotes = app.coop.remote_players();
    if !app.minimap_trails.is_empty() {
        let now = std::time::Instant::now();
        let fade = crate::hud::map_shared::TrailFade::new(cfg.coop_trail_fade_secs, cfg.coop_trail_fade_m);
        if let Some(tr) = app.minimap_trails.get("local") {
            crate::hud::map_shared::draw_trail(&cv, tr, local_col, fade, now);
        }
        for (info, pkt) in &remotes {
            if pkt.is_paused() {
                continue; // paused teammate — don't draw their line
            }
            if let Some(tr) = app.minimap_trails.get(&info.id) {
                crate::hud::map_shared::draw_trail(&cv, tr, crate::ui::coop::hue_color(info.hue), fade, now);
            }
        }
    }

    // Remote co-op players: identity colour + name. Paused players stop broadcasting a valid
    // position; show them at their last-known spot in grey instead of at the world origin.
    let mates: Vec<crate::hud::map_shared::Remote> = remotes
        .iter()
        .filter_map(|(info, pkt)| {
            let paused = pkt.is_paused();
            let (x, z, yaw) = if paused {
                let s = app.coop_last_pos.get(&info.id)?; // never seen at a valid spot — nothing to show
                (s.x, s.z, s.yaw)
            } else {
                (pkt.position_x, pkt.position_z, pkt.yaw)
            };
            Some(crate::hud::map_shared::Remote {
                id: info.id.clone(),
                name: info.name.clone(),
                x,
                z,
                yaw,
                colour: crate::ui::coop::hue_color(info.hue),
                paused,
            })
        })
        .collect();
    crate::hud::map_shared::draw_remotes(&cv, &mates, (car_x, car_z), yaw);

    // Local car indicator: triangle rotated to show heading relative to map orientation.
    // Uses the player's co-op colour (colour only, no name) when in a session, else white.
    // Drawn where the car is on screen: the view centre, unless the Map tab was panned away.
    let car_at = cv.to_screen(car_x, car_z);
    painter.add(egui::Shape::convex_polygon(
        crate::hud::map_shared::arrow_points(car_at, view.arrow_angle(app.minimap_cached_raw_yaw), 1.0).to_vec(),
        local_col,
        Stroke::new(1.5, Color32::BLACK),
    ));

    let time = ui.input(|i| i.time) as f32;
    for (_pid, wx, wz, hue) in app.coop.waypoints() {
        crate::hud::map_shared::draw_waypoint(&cv, (wx, wz), crate::ui::coop::hue_color(hue), (car_x, car_z), time);
    }

    // North compass: shared with the HUD Minimap (`hud::minimap::draw_compass`), scaled
    // with the widget (HUD design size = 1.0) and clamped so it stays proportionate.
    if sc.compass {
        let s = (rect.width().min(rect.height()) / 200.0).clamp(0.8, 1.6);
        let xf = crate::hud::prims::Xf { o: rect.min, s, a: 1.0 };
        crate::hud::minimap::draw_compass(&painter, &xf, view.north_dir());
    }

    // On-map co-op player list. Fixed-width, space-padded columns so the panel
    // never reflows (which would flicker). Front marker is a dot, or the ⏸ glyph
    // (in the player's colour) when paused.
    if cfg.coop_map_playerlist && app.coop.role() != crate::coop::Role::Off {
        let unit = if cfg.use_mph { "mph" } else { "km/h" };
        // (hue colour, paused, row text, class, PI). The class column is drawn as a
        // label image (assets/labels) after the text, so it's excluded from the text.
        let mut rows: Vec<(Color32, bool, String, i32, i32)> = Vec::new();
        let mut push_row = |hue: f32, name: &str, speed_ms: f32, gear: u8, class: i32, pi: i32, dist: f32, is_self: bool, paused: bool| {
            // Name: 12 cells, left-aligned, ellipsised if longer.
            let mut s = if name.chars().count() > 12 {
                name.chars().take(11).collect::<String>() + "…"
            } else {
                format!("{name:<12}")
            };
            if cfg.coop_list_distance {
                let d = if is_self {
                    String::new()
                } else if dist >= 1000.0 {
                    format!("{:.1}km", dist / 1000.0)
                } else {
                    format!("{dist:.0}m")
                };
                s += &format!(" {d:>6}"); // reserves up to "99.9km"
            }
            if cfg.coop_list_speed {
                let disp = if cfg.use_mph { speed_ms * 2.236_94 } else { speed_ms * 3.6 };
                s += &format!(" {disp:>3.0}{unit}");
            }
            if cfg.coop_list_gear {
                let g = match gear {
                    0 => "R".to_string(),
                    11 => "N".to_string(),
                    g => g.to_string(),
                };
                s += &format!(" G{g:<2}"); // "G10" / "G9 " / "GN " / "GR "
            }
            rows.push((crate::ui::coop::hue_color(hue), paused, s, class, pi));
        };
        if let Some(p) = &app.telemetry.latest {
            // Our own class/PI come from the cache so a local pause doesn't blank them.
            push_row(cfg.coop_hue, &cfg.coop_name, p.speed, p.gear, app.cached_car_class, app.cached_car_pi, 0.0, true, p.is_paused());
        }
        for (info, pkt) in app.coop.remote_players() {
            let paused = pkt.is_paused();
            let last = app.coop_last_pos.get(&info.id);
            let (px, pz) = if paused {
                last.map(|s| (s.x, s.z))
                    .unwrap_or((pkt.position_x, pkt.position_z))
            } else {
                (pkt.position_x, pkt.position_z)
            };
            let dist = ((px - car_x).powi(2) + (pz - car_z).powi(2)).sqrt();
            // PI 0 = empty (paused game transmits zeros) — fall back to the last
            // real class/PI we saw from this player.
            let (cl, pi) = if pkt.car_performance_index == 0 {
                last.map(|s| (s.car_class, s.pi))
                    .unwrap_or((pkt.car_class, pkt.car_performance_index))
            } else {
                (pkt.car_class, pkt.car_performance_index)
            };
            push_row(info.hue, &info.name, pkt.speed, pkt.gear, cl, pi, dist, false, paused);
        }
        if !rows.is_empty() {
            let font = egui::FontId::monospace(11.0);
            let (icon_x, text_x, row_h, pad) = (9.0_f32, 19.0_f32, 17.0_f32, 5.0_f32);
            // Class label sized to the row with headroom; native art is 111×40.
            let native = app.labels.class_size(0, 1.0);
            let class_scale = (row_h - 2.0) / native.y;
            let class_gap = 6.0;
            let class_w = if cfg.coop_list_class { native.x * class_scale + class_gap } else { 0.0 };
            let galleys: Vec<(Color32, bool, std::sync::Arc<egui::Galley>, i32, i32)> = rows
                .iter()
                .map(|(c, paused, s, cl, pi)| (*c, *paused, painter.layout_no_wrap(s.clone(), font.clone(), Color32::WHITE), *cl, *pi))
                .collect();
            let text_w = galleys.iter().map(|(_, _, g, _, _)| g.size().x).fold(0.0, f32::max);
            let w = text_x + text_w + class_w + pad;
            let h = pad * 2.0 + row_h * galleys.len() as f32;
            let origin = rect.right_top() + vec2(-w - 6.0, 6.0);
            let panel = egui::Rect::from_min_size(origin, vec2(w, h));
            painter.rect_filled(panel, 4.0, Color32::from_black_alpha(160));
            for (i, (c, paused, g, cl, pi)) in galleys.into_iter().enumerate() {
                let cy = panel.top() + pad + row_h * i as f32 + row_h * 0.5;
                let icon_pos = pos2(panel.left() + icon_x, cy);
                if paused {
                    painter.text(icon_pos, egui::Align2::CENTER_CENTER, crate::icons::PAUSE,
                        egui::FontId::monospace(10.0), c);
                } else {
                    painter.circle_filled(icon_pos, 4.0, c);
                }
                painter.galley(pos2(panel.left() + text_x, cy - g.size().y * 0.5), g, Color32::WHITE);
                if cfg.coop_list_class {
                    let cx0 = panel.left() + text_x + text_w + class_gap;
                    let lbl = app.labels.class_size(cl, class_scale);
                    app.labels.paint_class(&painter, cl, pi, pos2(cx0, cy - lbl.y * 0.5), class_scale);
                }
            }
        }
    }
    cam
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn approx(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.05 && (a.1 - b.1).abs() < 0.05
    }

    const VIEW: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(800.0, 600.0));

    /// Dragging keeps the grabbed map point under the pointer, north-up and turned, flat and
    /// tilted.
    #[test]
    fn panning_keeps_the_grabbed_point_under_the_pointer() {
        for tilt in [false, true] {
            for yaw in [0.0_f32, 1.0, -2.2] {
                let mut layers = MapLayerConfig::dashboard();
                layers.tilt.on = tilt;
                let centre = (120.0, -340.0);
                let cam = camera(&layers, centre, yaw, 900.0, VIEW);
                let (prev, now) = (pos2(400.0, 300.0), pos2(470.0, 340.0));
                let grabbed = cam.unproject(prev).expect("on the plane");
                let new_centre = panned(&cam, centre, prev, now);
                let after = camera(&layers, new_centre, yaw, 900.0, VIEW);
                let under = after.project(grabbed[0], grabbed[1]).expect("visible");
                assert!((under - now).length() < 0.2, "tilt {tilt} yaw {yaw}: {under:?} vs {now:?}");
            }
        }
    }

    /// Zooming at the cursor keeps the world point under it; the radius stays within its limits.
    #[test]
    fn zoom_is_anchored_at_the_cursor_and_clamped() {
        let layers = MapLayerConfig::dashboard();
        let at = pos2(650.0, 120.0);
        for factor in [0.5_f32, 0.9, 1.3] {
            let (centre, zoom) = zoomed_at(&layers, (10.0, 20.0), 0.7, 1000.0, VIEW, at, factor);
            assert!((zoom - 1000.0 * factor).abs() < 0.01);
            let before = camera(&layers, (10.0, 20.0), 0.7, 1000.0, VIEW).unproject(at).unwrap();
            let after = camera(&layers, centre, 0.7, zoom, VIEW).unproject(at).unwrap();
            assert!(approx((before[0], before[1]), (after[0], after[1])), "{before:?} {after:?}");
        }
        assert_eq!(zoomed_at(&layers, (0.0, 0.0), 0.0, 100.0, VIEW, at, 0.01).1, ZOOM_MIN_M);
        assert_eq!(zoomed_at(&layers, (0.0, 0.0), 0.0, 7000.0, VIEW, at, 50.0).1, ZOOM_MAX_M);
    }

    #[test]
    fn wheel_up_zooms_in_and_pinch_out_zooms_in() {
        assert!(zoom_factor(50.0, 1.0) < 1.0);
        assert!(zoom_factor(-50.0, 1.0) > 1.0);
        assert!(zoom_factor(0.0, 1.5) < 1.0);
        assert_eq!(zoom_factor(0.0, 1.0), 1.0);
    }

    /// A manual view made while stopped snaps back once the car drives off (after the delay), and
    /// not before.
    #[test]
    fn manual_view_resets_when_the_player_drives_off() {
        let mut g = DriveGate::arm(Some(0.0));
        assert!(!g.step(Some(0.0), 0.0));
        // Driving off: waits out the delay, then resets.
        assert!(!g.step(Some(8.0), 1.0));
        assert!(g.pending());
        assert!(!g.step(Some(12.0), 1.0 + RESET_DELAY_S - 0.05));
        assert!(g.step(Some(12.0), 1.0 + RESET_DELAY_S + 0.01));
    }

    /// A nudge (a bump, a tap on the throttle) must not take the map away: dropping below the
    /// stopped threshold cancels the countdown, and the band between the thresholds neither
    /// starts nor cancels it.
    #[test]
    fn a_nudge_does_not_reset_the_view() {
        let mut g = DriveGate::arm(Some(0.0));
        assert!(!g.step(Some(4.0), 0.0)); // starts the countdown
        assert!(!g.step(Some(0.5), 0.3)); // stopped again: cancelled
        assert!(!g.pending());
        assert!(!g.step(Some(4.0), 0.4)); // a fresh countdown from here
        assert!(!g.step(Some(4.0), 0.4 + RESET_DELAY_S - 0.05));
        // Hysteresis band: 2.5 m/s neither starts nor cancels.
        let mut g = DriveGate::arm(Some(0.0));
        for t in 0..20 {
            assert!(!g.step(Some(2.5), t as f64));
        }
        assert!(!g.pending());
        assert!(!g.step(Some(3.5), 30.0));
        assert!(!g.step(Some(2.5), 30.3)); // dipping into the band keeps the countdown ...
        assert!(g.step(Some(3.5), 30.7)); // ... so it still fires on time
    }

    /// Panning while already driving does not snap back; it does once the car has stopped and
    /// driven off again. No telemetry never resets.
    #[test]
    fn panning_while_driving_waits_for_the_next_stop() {
        let mut g = DriveGate::arm(Some(20.0));
        for t in 0..10 {
            assert!(!g.step(Some(20.0), t as f64), "still driving since before the pan");
        }
        assert!(!g.step(Some(0.0), 10.0)); // stops
        assert!(!g.step(Some(15.0), 11.0));
        assert!(g.step(Some(15.0), 11.0 + RESET_DELAY_S + 0.01));
        // No packets: the view stays, and a running countdown is dropped.
        let mut g = DriveGate::arm(None);
        assert!(!g.step(Some(15.0), 0.0));
        assert!(!g.step(None, 5.0));
        assert!(!g.pending());
        for t in 0..10 {
            assert!(!g.step(None, 10.0 + t as f64));
        }
    }

    /// `ManualView`: effective view = manual parts over the base; reset clears them; ticking
    /// resets only a manual view.
    #[test]
    fn manual_view_overrides_the_base_and_resets() {
        let ctx = egui::Context::default();
        let mut mv = ManualView::default();
        assert!(!mv.is_manual());
        assert_eq!(mv.view((1.0, 2.0), 900.0), ((1.0, 2.0), 900.0));
        mv.touch(Some(0.0));
        mv.centre = Some((50.0, 60.0));
        assert!(mv.is_manual());
        assert_eq!(mv.view((1.0, 2.0), 900.0), ((50.0, 60.0), 900.0));
        mv.zoom_m = Some(300.0);
        assert_eq!(mv.view((1.0, 2.0), 900.0), ((50.0, 60.0), 300.0));
        // Stopped: stays. Drives off: resets after the delay.
        assert!(!mv.tick(&ctx, Some(0.0), 0.0));
        assert!(!mv.tick(&ctx, Some(10.0), 1.0));
        assert!(mv.is_manual());
        assert!(mv.tick(&ctx, Some(10.0), 1.0 + RESET_DELAY_S + 0.01));
        assert!(!mv.is_manual());
        assert_eq!(mv.view((1.0, 2.0), 900.0), ((1.0, 2.0), 900.0));
        // A view made while driving keeps its first arming until it is reset.
        mv.pin_centre((5.0, 5.0), Some(30.0));
        assert!(!mv.tick(&ctx, Some(30.0), 100.0));
        assert!(!mv.tick(&ctx, Some(30.0), 105.0));
        mv.reset();
        assert_eq!(mv, ManualView::default());
    }
}
