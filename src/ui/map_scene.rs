//! The map scene both full maps draw: the Dashboard's Map widget and the Map tab's viewer
//! (D67). Extracted from `dashboard::show_minimap_widget` so the viewer does not fork it.
//!
//! One function, [`draw`], paints everything: the shared renderer (`maprender::draw_base` +
//! `draw_layers`, D61) and the markers (`hud::map_shared`) over `rect`. What differs per caller is
//! [`Scene`]: the layer config, where the view is centred, its rotation and zoom. The Dashboard
//! centres on the car; the viewer may be panned away from it.
//!
//! **3D (phase K, K4).** A map whose view mode is 3D (`TiltCfg::view_mode`) draws through the shared
//! GL renderer (`maprender::gl3d`) instead of the 2D base and roads: [`Map3d`] is the eframe-side
//! owner of its one `Gl3dHandle` (the Dashboard map and the viewer share it: they are never on
//! screen together and the renderer's FBO is transient), [`draw`] follows the renderer's call shape
//! and falls back to the tilted 2D map (the relief-less camera) until the scene is `Ready` and for
//! good when the context cannot do 3D.

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::time::Duration;

use egui::{pos2, vec2, Color32, Pos2, Rect, Stroke, Ui, Vec2};

use crate::app::ForzaApp;
use crate::i18n::tr;
use crate::maprender::cfg::MapLayerConfig;
use crate::maprender::data::MapLayers;
use crate::maprender::paint2d::Parts;
use crate::maprender::terrain::Terrain;
use crate::maprender::{Camera, MapTex, RaceSel};

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

/// Time constant (s) of the panned 3D camera's height easing towards the terrain under the view
/// centre ([`eased_car_y`]).
const HEIGHT_EASE_S: f32 = 0.2;

/// The relief camera's height for a *panned* view this frame: `prev` (last frame's) eased towards
/// `target` (the terrain under the view centre), `dt` seconds on. `None` = no previous frame in 3D:
/// straight to the target.
///
/// *Why eased, not the plain terrain height (the first K4 version):* the camera is a pivot over
/// the ground plane through the car, so its height shifts the whole picture. With the pivot at the
/// terrain under the centre, moving the centre up a slope lifts the pivot at the same time and
/// moves the picture several times further than the drag (measured: 4x at exaggeration 3 on a
/// 80 % slope (synthetic hill)), and down a slope the sinking pivot cancels the movement on screen (the map barely
/// moves under a drag: "panning does not work"); on top of that the picture lurches vertically as
/// the pivot jumps from the telemetry height to the terrain's on the first drag frame. Eased with a short time constant, the pivot changes by a
/// small fraction of the height difference per frame: a drag moves the map with the pointer (the
/// pan solves for it, see [`Ground::centre_over`]) and the new height settles in smoothly after.
fn eased_car_y(prev: Option<f32>, target: f32, dt: f32) -> f32 {
    match prev.filter(|p| p.is_finite()) {
        Some(p) => p + (target - p) * (1.0 - (-dt.clamp(0.0, 0.25) / HEIGHT_EASE_S).exp()),
        None => target,
    }
}

/// How a map looked on its last frame when that frame was drawn in **3D** (`draw` leaves it in the
/// egui context, keyed by the map's `Ui`; [`ManualView::interact`] reads it). Pan and zoom need it
/// because the relief camera is not the flat one: the picture shows the terrain *surface*, up to
/// hundreds of metres above or below the plane through the car that `Camera::unproject` stops at,
/// so a gesture worked out on that plane slides the map against the pointer (and grabbing a
/// mountain top above the plane's horizon grabs nothing at all).
#[derive(Clone)]
struct Ground {
    terrain: Arc<Terrain>,
    /// The drawn camera's `Relief::car_y` (telemetry height while following, else the terrain's).
    car_y: f32,
    /// `Context::cumulative_pass_nr` of that frame: older than a few passes = the map is not
    /// being drawn (another tab, still loading), so what it showed no longer counts.
    pass: u64,
}

impl Ground {
    fn id(ui: &Ui) -> egui::Id {
        ui.id().with("map_scene_ground")
    }

    /// What `ui`'s map showed last frame, if that was a 3D scene.
    fn of(ui: &Ui) -> Option<Ground> {
        let now = ui.ctx().cumulative_pass_nr();
        let g: Option<Ground> = ui.ctx().data(|d| d.get_temp::<Option<Ground>>(Self::id(ui))).flatten();
        g.filter(|g| g.pass + 4 >= now)
    }

    /// `draw`: leave this frame's look (`None` = not drawn in 3D) for the next frame's input.
    fn publish(ui: &Ui, cam: Option<&Camera>) {
        let g = cam.and_then(|c| c.relief.as_ref()).map(|r| Ground {
            terrain: r.terrain.clone(),
            car_y: r.car_y,
            pass: ui.ctx().cumulative_pass_nr(),
        });
        ui.ctx().data_mut(|d| d.insert_temp(Self::id(ui), g));
    }

    /// The camera `draw` builds for a view over this ground. `car_y` `None` = a panned view:
    /// its height is the one eased from this ground's towards the terrain under `centre`, `dt`
    /// s on ([`eased_car_y`], the very formula `draw` uses); `Some` = the telemetry-based height
    /// of a following one (or the drawn camera's own).
    fn camera(&self, layers: &MapLayerConfig, centre: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect, car_y: Option<f32>, dt: f32) -> Camera {
        let y = car_y.unwrap_or_else(|| eased_car_y(Some(self.car_y), self.terrain.height(centre.0, centre.1), dt));
        Camera::from_cfg_relief(&layers.tilt, centre, yaw, zoom_m, rect, Some(&self.terrain), Some(y))
    }

    /// The view centre that puts the world point `anchor` (on the terrain) under screen point
    /// `at` for this yaw / zoom, with the camera built the way a panned view's is: its height is
    /// the terrain's under the centre, so the camera rises and sinks as the centre moves over hills
    /// and there is no closed form (a fixed-point iteration on the picked ground point oscillates
    /// on steep ground). Newton's method on the anchor's *screen* error instead, the 2x2 Jacobian by
    /// finite differences in the centre, damped so a step never makes the error worse. Never worse
    /// than `start`; `None` = the anchor cannot be shown at all.
    fn centre_over(&self, layers: &MapLayerConfig, start: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect, dt: f32, at: Pos2, anchor: [f32; 2]) -> Option<(f32, f32)> {
        let miss = |c: (f32, f32)| -> Option<Vec2> {
            let cam = self.camera(layers, c, yaw, zoom_m, rect, None, dt);
            let y = self.terrain.height(anchor[0], anchor[1]);
            cam.project3(anchor[0], y, anchor[1]).map(|(s, _)| s - at)
        };
        let (mut c, mut e) = (start, miss(start)?);
        const H: f32 = 2.0; // metres, for the finite differences
        for _ in 0..24 {
            if e.length() < 0.1 {
                break;
            }
            let (ex, ez) = (miss((c.0 + H, c.1))?, miss((c.0, c.1 + H))?);
            let (jx, jz) = ((ex - e) / H, (ez - e) / H); // screen px per metre of centre x / z
            let det = jx.x * jz.y - jz.x * jx.y;
            if det.abs() < 1e-9 {
                break;
            }
            // Solve [jx jz] d = -e.
            let d = ((-e.x * jz.y + e.y * jz.x) / det, (-e.y * jx.x + e.x * jx.y) / det);
            let mut stepped = false;
            for damp in [1.0_f32, 0.5, 0.25, 0.1, 0.04, 0.015] {
                let next = (c.0 + d.0 * damp, c.1 + d.1 * damp);
                if let Some(ne) = miss(next) {
                    if ne.length() < e.length() {
                        (c, e, stepped) = (next, ne, true);
                        break;
                    }
                }
            }
            if !stepped {
                break;
            }
        }
        (c.0.is_finite() && c.1.is_finite()).then_some(c)
    }

    /// 3D [`panned`]: the *terrain point* grabbed at `prev` ends up under `now`.
    fn panned(&self, layers: &MapLayerConfig, centre: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect, dt: f32, prev: Pos2, now: Pos2) -> (f32, f32) {
        let drawn = self.camera(layers, centre, yaw, zoom_m, rect, Some(self.car_y), dt);
        pick(&drawn, prev)
            .and_then(|anchor| self.centre_over(layers, centre, yaw, zoom_m, rect, dt, now, anchor))
            .unwrap_or(centre)
    }

    /// 3D [`zoomed_at`]: the terrain point under `at` stays under it.
    fn zoomed_at(&self, layers: &MapLayerConfig, centre: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect, dt: f32, at: Pos2, factor: f32) -> ((f32, f32), f32) {
        let new = (zoom_m * factor).clamp(ZOOM_MIN_M, ZOOM_MAX_M);
        let drawn = self.camera(layers, centre, yaw, zoom_m, rect, Some(self.car_y), dt);
        let c = pick(&drawn, at).and_then(|anchor| self.centre_over(layers, centre, yaw, new, rect, dt, at, anchor));
        (c.unwrap_or(centre), new)
    }
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
    /// car stays put, nothing to anchor to); once panned it zooms around the cursor. On a map
    /// drawn in 3D both anchor on the terrain surface under the pointer ([`Ground`]).
    pub fn interact(&mut self, ui: &Ui, resp: &egui::Response, v: &ViewIn) {
        let ground = Ground::of(ui);
        let dt = ui.input(|i| i.stable_dt);
        let (centre, zoom) = self.view(v.car, v.base_zoom_m);
        if resp.dragged_by(egui::PointerButton::Primary) && resp.drag_delta() != Vec2::ZERO {
            if let Some(now) = resp.interact_pointer_pos() {
                let prev = now - resp.drag_delta();
                let new = match &ground {
                    Some(g) => g.panned(v.layers, centre, v.yaw, zoom, v.rect, dt, prev, now),
                    None => panned(&camera(v.layers, centre, v.yaw, zoom, v.rect), centre, prev, now),
                };
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
                        let (c, z) = match &ground {
                            Some(g) => g.zoomed_at(v.layers, centre, v.yaw, zoom, v.rect, dt, at, factor),
                            None => zoomed_at(v.layers, centre, v.yaw, zoom, v.rect, at, factor),
                        };
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

/// The north compass's scale for a map over `rect`: the HUD design size (1.0) scaled with the
/// widget and clamped so it stays proportionate.
fn compass_scale(rect: Rect) -> f32 {
    (rect.width().min(rect.height()) / 200.0).clamp(0.8, 1.6)
}

/// Where [`draw`] paints the compass over a map of `rect` (`hud::minimap::draw_compass`: disc of
/// radius 11 at (18, 18) in HUD units, scaled by [`compass_scale`]). It sits top left, where the
/// "Follow car" buttons do, so those step aside while it is on (the Dashboard's
/// [`follow_button`] and the viewer's own button both use this).
pub fn compass_rect(rect: Rect) -> Rect {
    let s = compass_scale(rect);
    Rect::from_min_max(rect.min + vec2(7.0, 7.0) * s, rect.min + vec2(29.0, 29.0) * s)
}

/// The small "Follow car" button the Dashboard map shows in its corner while its view is manual
/// (the viewer's is the same: only while manual). True = pressed (the caller resets the view). Right of
/// the compass when that is on: [`draw`] leaves whether it painted one in the egui context,
/// keyed by the map's `Ui`, so the caller needs no extra argument.
pub fn follow_button(ui: &mut Ui, rect: Rect) -> bool {
    let compass = ui.ctx().data(|d| d.get_temp::<bool>(compass_id(ui))).unwrap_or(false);
    let dx = if compass { (compass_rect(rect).right() + 6.0 - rect.left()).max(8.0) } else { 8.0 };
    let at = Rect::from_min_size(rect.left_top() + vec2(dx, 8.0), vec2((rect.width().min(220.0) - dx).max(0.0), 26.0));
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

fn compass_id(ui: &Ui) -> egui::Id {
    ui.id().with("map_scene_compass")
}

// ── 3D (phase K, K4) ─────────────────────────────────────────────────────────────────────────

/// "Retry a failed 3D context when the user brings a 3D map back up": true on the frame a 3D map
/// appears (the previous frame drew none) while the context is failed. *Why edge-triggered:* the
/// failure is sticky per context, so a retry costs a fresh probe (and, for "too slow", two more
/// seconds of slow frames); doing it every frame would never settle. A toggle in the settings
/// page takes the map off the screen for the frames in between, so toggling Flat -> 3D, or just
/// coming back to the map, is the edge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct RetryGate {
    was_3d: bool,
}

impl RetryGate {
    /// One frame: did a map draw in 3D mode last frame, is the context failed? True = destroy the
    /// context's state now so the next callback starts afresh.
    fn step(&mut self, wants_3d: bool, failed: bool) -> bool {
        let edge = wants_3d && !self.was_3d;
        self.was_3d = wants_3d;
        edge && failed
    }
}

/// The eframe side's 3D state, a field of `ForzaApp`: the **one** `Gl3dHandle` of the window's GL
/// context (Dashboard map and Map-tab viewer share it) and the bookkeeping around it. Where the
/// platform has no GL 3D (`gl3d` is Linux + Windows) it is an empty shell and 3D maps draw as
/// Tilted.
#[derive(Default)]
pub struct Map3d {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    handle: crate::maprender::gl3d::Gl3dHandle,
    /// A map drew in 3D mode this frame (set by [`draw`], read at the next frame's start).
    drew: Cell<bool>,
    gate: RetryGate,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl Map3d {
    /// Start of an `update`, GL context current: retry a failed context when a 3D map has just
    /// come back (see [`RetryGate`]). `gl` is `None` without a glow backend.
    pub fn begin_frame(&mut self, gl: Option<&eframe::glow::Context>) {
        let failed = self.handle.status().failure().is_some();
        if self.gate.step(self.drew.take(), failed) {
            if let Some(gl) = gl {
                self.retry(gl);
            }
        }
    }

    /// Forget the old failure (its status line too) and free the GL objects: the next callback
    /// probes again.
    fn retry(&self, gl: &eframe::glow::Context) {
        if let Some(why) = self.handle.status().failure() {
            crate::maprender::gl3d::clear_failure_if(why);
        }
        self.handle.destroy(gl);
    }

    /// `ForzaApp::on_exit(Some(gl))`: free the renderer's GL objects with the context current
    /// (idempotent; dropping a `Gl3d` without it only warns).
    pub fn destroy(&self, gl: &eframe::glow::Context) {
        self.handle.destroy(gl);
    }

    /// The tilted 2D map is still wanted under the scene (not `Ready`, or failed for good).
    fn wants_underlay(&self) -> bool {
        self.handle.wants_underlay()
    }

    /// Queue the 3D scene over `cam.rect` (which must carry a relief) and keep frames coming
    /// while the staged init runs.
    #[allow(clippy::too_many_arguments)]
    fn add_scene(
        &self,
        painter: &egui::Painter,
        cam: &Camera,
        sc: &Scene,
        map: Option<MapTex>,
        cal: crate::minimap::MapCalibration,
        data: Option<&Arc<MapLayers>>,
        sel: &RaceSel,
        trails: Vec<crate::maprender::gl3d::Trail3d>,
    ) {
        use crate::maprender::gl3d;
        let lc = sc.layers;
        let Some(relief) = &cam.relief else { return };
        // The road mesh only exists for roads that are drawn; it builds on its own thread.
        let mesh = data.filter(|_| lc.roads.on).and_then(|d| crate::maprender::store::road_mesh(d, &relief.terrain));
        // The in-race focus (D66), exactly when the 2D path would apply it (`draw_layers_parts`):
        // other roads muted, hidden or gone. The race lines themselves do not depend on it (D88).
        let focusing = data.is_some_and(|d| sel.focus_line().is_some_and(|l| l < d.races.lines.len()));
        let focus = data
            .filter(|_| focusing && lc.race_lines.focus.other_roads != crate::maprender::cfg::OtherRoads::Normal)
            .and_then(|d| sel.road_focus(d))
            .map(|focus| gl3d::Focus3d { focus, cfg: lc.race_lines.focus });
        // Every race line the mode draws is part of the scene (D88), so terrain and overpasses
        // hide it like the roads; its mesh builds on its own thread.
        let race = data.and_then(|d| sel.race_draw(d, &lc.race_lines)).map(|draw| {
            let mesh = crate::maprender::store::race_mesh(&draw, &relief.terrain);
            gl3d::Race3d { draw, mesh, cfg: lc.race_lines }
        });
        gl3d::add_scene(
            painter,
            &self.handle,
            gl3d::Scene3d {
                cam: cam.clone(),
                mesh,
                map,
                cal,
                look: (&lc.image).into(),
                mirror: sc.mirror,
                a: 1.0,
                s: 1.0,
                corner_radius: 0.0,
                relief: lc.tilt.relief,
                roads: lc.roads.clone(),
                focus,
                race,
                trails,
            },
        );
        if self.handle.busy() {
            painter.ctx().request_repaint();
        }
    }

    /// Queue the own car over `cam.rect` (D77 / D78), after the vectors over the scene.
    fn add_marker(&self, painter: &egui::Painter, cam: &Camera, marker: crate::maprender::gl3d::Marker3d) {
        use crate::maprender::gl3d;
        gl3d::add_marker(painter, &self.handle, gl3d::MarkerScene { cam: cam.clone(), marker, a: 1.0, s: 1.0, corner_radius: 0.0 });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
impl Map3d {
    fn wants_underlay(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn add_scene(&self, _: &egui::Painter, _: &Camera, _: &Scene, _: Option<MapTex>, _: crate::minimap::MapCalibration, _: Option<&Arc<MapLayers>>, _: &RaceSel, _: Vec<crate::maprender::gl3d::Trail3d>) {}

    fn add_marker(&self, _: &egui::Painter, _: &Camera, _: crate::maprender::gl3d::Marker3d) {}
}

/// May a map go 3D on this platform? Windows only with the user's opt-in (`map_3d_windows`: its GL
/// path is untested by the developers); everywhere else yes.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn allowed_3d(windows_flag: bool) -> bool {
    !cfg!(windows) || windows_flag
}

/// The 3D camera for this scene, when the map is in 3D mode and the terrain is loaded; `None`
/// draws the plain camera (Flat / Tilted, a terrain still loading or missing, or a platform
/// without GL 3D). Polls the terrain store, which starts its lazy load: this is the only place
/// the eframe side asks for it, so it loads only while a map is *in 3D and on screen*.
///
/// `car_y`: while the view follows the car, the telemetry height + 1 m (the camera sits above the
/// car, not above the terrain mesh under it); a panned view, or no race on, eases (`prev_car_y`
/// = last frame's, `dt` s) towards the terrain height under the view centre ([`eased_car_y`]).
/// The `bool` is "the height has settled": false = keep the frames coming.
fn relief_camera(app: &ForzaApp, sc: &Scene, rect: Rect, prev_car_y: Option<f32>, dt: f32) -> Option<(Camera, bool)> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        use crate::maprender::cfg::ViewMode;
        use crate::maprender::store::{self, TerrainStatus};
        if sc.layers.tilt.view_mode() != ViewMode::Relief || !allowed_3d(app.config.overlay.map_3d_windows) {
            return None;
        }
        app.map3d.drew.set(true);
        let TerrainStatus::Ready(terrain) = store::terrain() else {
            return None; // the status line in the View mode card says why
        };
        let following = sc.centre == (app.minimap_cached_car_x, app.minimap_cached_car_z);
        let telemetry_y = following
            .then(|| app.telemetry.latest.as_ref().filter(|p| p.is_race_on != 0).map(|p| p.position_y + 1.0))
            .flatten();
        let ground = terrain.height(sc.centre.0, sc.centre.1);
        let car_y = telemetry_y.unwrap_or_else(|| eased_car_y(prev_car_y, ground, dt));
        let settled = telemetry_y.is_some() || (car_y - ground).abs() < 0.25;
        Some((Camera::from_cfg_relief(&sc.layers.tilt, sc.centre, sc.yaw, sc.zoom_m, rect, Some(&terrain), Some(car_y)), settled))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, sc, rect);
        None
    }
}

/// Where the ray through screen point `p` meets the horizontal plane `h` px (scene px: metres x
/// `scale` x exaggeration) above the camera's ground plane, as world (x, z). The inverse of
/// `Camera::project3` for a fixed height: `h = 0` is `Camera::unproject`.
fn unproject_at_height(cam: &Camera, p: Pos2, h: f32) -> Option<[f32; 2]> {
    let (s, c) = cam.pitch.sin_cos();
    let f = cam.focal;
    let (dx, dy) = ((p.x - cam.centre.x) / f, (p.y - cam.centre.y) / f);
    let den = c + dy * s;
    if den < 1e-4 {
        return None; // at or above the horizon
    }
    let oy = (dy * (f - h * c) + h * s) / den;
    let cz = f - oy * s - h * c;
    if cz < 1e-3 {
        return None; // behind the eye
    }
    Some(cam.view.offset_to_world(dx * cz, oy))
}

/// Where the view ray through screen point `p` first meets the terrain surface, as world (x, z):
/// the ray is the line of points `unproject_at_height` gives for every height, walked from the eye
/// down in coarse steps until it is below the surface, then bisected. `None` = it never gets
/// there (it points up, or leaves the terrain's depth range). Needs a relief camera.
fn terrain_hit(cam: &Camera, p: Pos2) -> Option<[f32; 2]> {
    let r = cam.relief.as_ref()?;
    let k = cam.view.scale * r.exag;
    let top = cam.focal * cam.pitch.cos() * 0.999; // the eye's height above the car's plane, px
    let bottom = (-200.0 - r.car_y) * k; // 200 m below sea level: under any terrain
    if !(top > bottom) {
        return None;
    }
    // Terrain height above the ray (px): > 0 = the ray point is still above the surface.
    let above = |h: f32| -> Option<([f32; 2], f32)> {
        let at = unproject_at_height(cam, p, h)?;
        Some((at, h - (r.terrain.height(at[0], at[1]) - r.car_y) * k))
    };
    const STEPS: usize = 384;
    let mut last = (top, above(top)?.1);
    for i in 1..=STEPS {
        let h = top + (bottom - top) * i as f32 / STEPS as f32;
        let Some((_, d)) = above(h) else { return None };
        if d <= 0.0 {
            let (mut hi, mut lo) = (last.0, h);
            for _ in 0..28 {
                let mid = 0.5 * (hi + lo);
                match above(mid) {
                    Some((_, d)) if d > 0.0 => hi = mid,
                    Some(_) => lo = mid,
                    None => break,
                }
            }
            return above(0.5 * (hi + lo)).map(|(at, _)| at);
        }
        last = (h, d);
    }
    None
}

/// The world point (x, z) under screen point `p` for a click: on the flat plane for a flat or
/// tilted camera; in 3D the point of the *terrain surface* the pointer is over ([`terrain_hit`];
/// where that finds none, a few rounds of re-casting onto the plane at the height found, which
/// converge for any slope gentler than the view ray). `Camera::unproject` alone would land on the
/// plane at the car's height, up to hundreds of metres off on a hillside.
pub fn pick(cam: &Camera, p: Pos2) -> Option<[f32; 2]> {
    let mut at = cam.unproject(p)?;
    if let Some(r) = &cam.relief {
        if let Some(hit) = terrain_hit(cam, p) {
            return Some(hit);
        }
        let k = cam.view.scale * r.exag;
        for _ in 0..6 {
            let h = (r.terrain.height(at[0], at[1]) - r.car_y) * k;
            match unproject_at_height(cam, p, h) {
                Some(next) => at = next,
                None => break,
            }
        }
    }
    Some(at)
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
    let flat_cam = crate::maprender::Camera::from_cfg(&lc.tilt, sc.centre, yaw, sc.zoom_m, rect);

    // 3D (K4): `relief` = the 3D camera once the terrain is loaded. The scene is drawn by the GL
    // renderer (`add_scene`); until it is `Ready`, and for good when the context failed, the tilted
    // 2D map (`flat_cam`) is drawn under it AND over it (POIs, markers): everything on screen then
    // agrees with the 2D picture. `cam` is the one the markers and the caller use.
    let prev_car_y = Ground::of(ui).map(|g| g.car_y);
    let relief = relief_camera(app, sc, rect, prev_car_y, ui.input(|i| i.stable_dt));
    if relief.as_ref().is_some_and(|(_, settled)| !settled) {
        ui.ctx().request_repaint(); // the camera height is still easing
    }
    let relief = relief.map(|(c, _)| c);
    let underlay = relief.is_none() || app.map3d.wants_underlay();
    let cam = match &relief {
        Some(r) if !underlay => r.clone(),
        _ => flat_cam,
    };
    let view = cam.view;
    // What the next frame's pan / zoom needs to know about this one (see `Ground`).
    Ground::publish(ui, relief.as_ref().filter(|_| !underlay));

    let painter = ui.painter_at(rect);
    let outline = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    if !lc.image.on || tilted {
        // Vectors-only look, or the sky above a tilted map's far edge.
        painter.rect_filled(rect, 0.0, crate::maprender::style::MAP_BACKING);
    }
    let tex = crate::maprender::MapTex { id: texture.id(), orig_size: app.minimap_orig_size, winter: false };
    if lc.image.on && underlay {
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
    let layers = lc.wants_layers().then(crate::maprender::layers);
    if layers.as_ref().is_some_and(|l| l.status == crate::maprender::LayerStatus::Loading) {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    let data = layers.as_ref().and_then(|l| l.data.as_ref());
    let mut sel = sc.race_sel.borrow_mut();
    if let Some(d) = data {
        let in_race = app.telemetry.latest.as_ref().is_some_and(|p| p.race_position != 0);
        sel.update(&d.races, &lc.race_lines, (car_x, car_z), app.minimap_cached_raw_yaw, in_race);
    }
    let sel: &RaceSel = &sel;
    let icons = data.and_then(|d| app.minimap_icons.borrow_mut().ensure(ui.ctx(), d.icons.as_ref()));
    let layer_pass = |cam: &Camera, over_3d: bool| {
        if let Some(data) = data {
            let cx = crate::maprender::LayerCtx {
                p: &painter,
                cam,
                s: 1.0,
                a: 1.0,
                car: (car_x, car_z),
                corner_clip: None,
                icons: icons.as_deref(),
                race_sel: sel,
                week: None,
            };
            if over_3d {
                crate::maprender::paint2d::draw_layers_parts(&cx, data, lc, Parts::OVER_3D);
            } else {
                crate::maprender::draw_layers(&cx, data, lc);
            }
        }
    };
    if underlay {
        layer_pass(&cam, false);
    }

    // Breadcrumb trails: each player's recent path fades from faint (old) to solid (recent) in
    // their identity colour; the own trail is recorded solo too and is then white, like the own
    // arrow (which uses the player's co-op colour, colour only, no name, in a session).
    let in_session = app.coop.role() != crate::coop::Role::Off;
    let local_col = if in_session { crate::ui::coop::hue_color(app.config.coop_hue) } else { Color32::WHITE };
    let remotes = app.coop.remote_players();
    let trail_now = std::time::Instant::now();
    let fade = crate::hud::map_shared::TrailFade::new(cfg.coop_trail_fade_secs, cfg.coop_trail_fade_m);
    let trails: Vec<(&crate::minimap::Trail, Color32)> = app
        .minimap_trails
        .get("local")
        .map(|t| (t, local_col))
        .into_iter()
        .chain(
            remotes
                .iter()
                .filter(|(_, pkt)| !pkt.is_paused()) // paused teammate: don't draw their line
                .filter_map(|(info, _)| app.minimap_trails.get(&info.id).map(|t| (t, crate::ui::coop::hue_color(info.hue)))),
        )
        .collect();
    if let Some(r) = &relief {
        // D77: in 3D the trails are part of the scene, at their recorded heights.
        let trails3d = trails.iter().filter_map(|(t, c)| crate::hud::map_shared::trail_3d(t, *c, fade, trail_now)).collect();
        app.map3d.add_scene(&painter, r, sc, lc.image.on.then_some(tex), cal, data, sel, trails3d);
    }
    if !underlay {
        layer_pass(&cam, true); // the roads are in the scene; race lines and POIs over it
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

    // The flat trails (drawn behind the car arrows) unless the 3D scene has them (D77).
    if underlay {
        for (tr, c) in &trails {
            crate::hud::map_shared::draw_trail(&cv, tr, *c, fade, trail_now);
        }
    }

    // Remote co-op players: identity colour + name. Paused players stop broadcasting a valid
    // position; show them at their last-known spot in grey instead of at the world origin.
    let mates: Vec<crate::hud::map_shared::Remote> = remotes
        .iter()
        .filter_map(|(info, pkt)| {
            let paused = pkt.is_paused();
            let (x, y, z, yaw) = if paused {
                let s = app.coop_last_pos.get(&info.id)?; // never seen at a valid spot — nothing to show
                (s.x, s.y, s.z, s.yaw)
            } else {
                (pkt.position_x, pkt.position_y, pkt.position_z, pkt.yaw)
            };
            Some(crate::hud::map_shared::Remote {
                id: info.id.clone(),
                name: info.name.clone(),
                x,
                z,
                y: Some(y),
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
    if underlay {
        let car_at = cv.to_screen(car_x, car_z);
        painter.add(egui::Shape::convex_polygon(
            crate::hud::map_shared::arrow_points(car_at, view.arrow_angle(app.minimap_cached_raw_yaw), 1.0).to_vec(),
            local_col,
            Stroke::new(1.5, Color32::BLACK),
        ));
    } else if let Some(rel) = relief.as_ref().and_then(|r| r.relief.as_ref()) {
        // D77 / D78: in 3D the GL car at the telemetry position and height (in a tunnel: down at
        // its road), on top of the POIs and race lines like the flat arrow. Without a live
        // position it stands on the terrain.
        let live = app.telemetry.latest.as_ref().filter(|p| p.is_race_on != 0 && !p.is_paused());
        let y = live.map_or_else(|| rel.terrain.height(car_x, car_z), |p| p.position_y);
        let marker = crate::maprender::gl3d::Marker3d { pos: [car_x, y, car_z], yaw: app.minimap_cached_raw_yaw, kind: lc.tilt.relief.marker, colour: local_col };
        app.map3d.add_marker(&painter, &cam, marker);
    }

    let time = ui.input(|i| i.time) as f32;
    for (_pid, wx, wz, hue) in app.coop.waypoints() {
        crate::hud::map_shared::draw_waypoint(&cv, (wx, wz), crate::ui::coop::hue_color(hue), (car_x, car_z), time);
    }

    // North compass: shared with the HUD Minimap (`hud::minimap::draw_compass`), scaled
    // with the widget (HUD design size = 1.0) and clamped so it stays proportionate.
    ui.ctx().data_mut(|d| d.insert_temp(compass_id(ui), sc.compass));
    if sc.compass {
        let s = compass_scale(rect);
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
        mv.touch(Some(30.0));
        mv.centre = Some((5.0, 5.0));
        assert!(!mv.tick(&ctx, Some(30.0), 100.0));
        assert!(!mv.tick(&ctx, Some(30.0), 105.0));
        mv.reset();
        assert_eq!(mv, ManualView::default());
    }

    // ── 3D (K4) ──────────────────────────────────────────────────────────────────────────────

    use crate::maprender::terrain::Terrain;

    /// A Dashboard-style config in 3D (tilt + relief on).
    fn layers_3d() -> MapLayerConfig {
        let mut l = MapLayerConfig::dashboard();
        l.tilt.on = true;
        l.tilt.relief.on = true;
        l
    }

    /// The relief camera the draw code builds, over the synthetic hills (the big one is at
    /// (-300, 200)), the car `car_y` m up.
    fn cam_3d(layers: &MapLayerConfig, centre: (f32, f32), yaw: f32, zoom: f32, car_y: f32) -> Camera {
        let t = Arc::new(Terrain::synthetic());
        Camera::from_cfg_relief(&layers.tilt, centre, yaw, zoom, VIEW, Some(&t), Some(car_y))
    }

    /// D72 in 3D: pan and zoom move over the ground plane at the car's height, and that plane is
    /// the one `Camera::unproject` and the relief-less camera (`camera()`, which `interact`
    /// builds) share with the 3D camera. So the grabbed point stays under the pointer, and the
    /// gestures need no 3D special case.
    #[test]
    fn pan_and_zoom_anchor_on_the_ground_plane_in_3d() {
        let layers = layers_3d();
        for yaw in [0.0_f32, 1.0, -2.2] {
            let centre = (-250.0, 150.0);
            let cam = cam_3d(&layers, centre, yaw, 400.0, 260.0);
            assert!(cam.relief.is_some());
            // The interaction camera (relief-less) agrees with the 3D one about the plane.
            let flat = camera(&layers, centre, yaw, 400.0, VIEW);
            for p in [pos2(400.0, 300.0), pos2(120.0, 500.0), pos2(700.0, 450.0)] {
                let (a, b) = (cam.unproject(p).unwrap(), flat.unproject(p).unwrap());
                assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3, "yaw {yaw}: {a:?} vs {b:?}");
            }
            // Pan: the plane point grabbed at `prev` ends under `now` in the next 3D frame.
            let (prev, now) = (pos2(400.0, 420.0), pos2(470.0, 470.0));
            let grabbed = cam.unproject(prev).unwrap();
            let new_centre = panned(&flat, centre, prev, now);
            let after = cam_3d(&layers, new_centre, yaw, 400.0, 260.0);
            let (under, _) = after.project3(grabbed[0], 260.0, grabbed[1]).expect("visible");
            assert!((under - now).length() < 0.2, "yaw {yaw}: {under:?} vs {now:?}");
            // Zoom at the cursor: the plane point under it stays.
            let at = pos2(560.0, 380.0);
            let (c2, z2) = zoomed_at(&layers, centre, yaw, 400.0, VIEW, at, 0.6);
            let before = cam.unproject(at).unwrap();
            let zoomed = cam_3d(&layers, c2, yaw, z2, 260.0);
            let (under, _) = zoomed.project3(before[0], 260.0, before[1]).expect("visible");
            assert!((under - at).length() < 0.2, "yaw {yaw}: {under:?} vs {at:?}");
        }
    }

    // ── pan / zoom on the terrain surface (3D) ──────────────────────────────────────────────

    /// A 3D map's last frame over the synthetic hills, the camera at `car_y`.
    fn ground(car_y: f32) -> Ground {
        Ground { terrain: Arc::new(Terrain::synthetic()), car_y, pass: 0 }
    }

    /// One frame at 60 Hz.
    const DT: f32 = 1.0 / 60.0;

    /// Where the terrain point `at` (x, z) shows in the 3D frame `draw` makes for `centre`: the
    /// panned view's camera, its height eased towards the terrain's under the centre.
    fn shows_at(g: &Ground, layers: &MapLayerConfig, centre: (f32, f32), yaw: f32, zoom: f32, at: [f32; 2]) -> Pos2 {
        let cam = g.camera(layers, centre, yaw, zoom, VIEW, None, DT);
        cam.project3(at[0], g.terrain.height(at[0], at[1]), at[1]).expect("visible").0
    }

    /// The bug: pan worked out on the car-height plane (`panned`, a relief-less camera) slides the
    /// map against the pointer in 3D, because the picture shows the terrain surface. The 3D pan
    /// keeps the grabbed terrain point under the pointer, frame after frame (the camera height
    /// follows the terrain under the centre, so the view moves while the centre does), from the
    /// following view (telemetry height) and the panned one, at any yaw.
    #[test]
    fn pan_keeps_the_grabbed_terrain_point_under_the_pointer_in_3d() {
        let layers = layers_3d();
        let t = Terrain::synthetic();
        let mut worst_plane = 0.0_f32;
        for yaw in [0.0_f32, 1.0, -2.2] {
            // The car beside the 220 m hill (-300, 200); the view starts on it, following.
            let car = (-250.0, 80.0);
            let mut g = ground(t.height(car.0, car.1) + 1.0);
            let mut centre = car;
            let mut now = pos2(400.0, 420.0);
            for step in 0..14 {
                let prev = now;
                now += vec2(14.0, -9.0); // a drag up and to the right, over the hill
                let drawn = g.camera(&layers, centre, yaw, 400.0, VIEW, Some(g.car_y), DT);
                let anchor = pick(&drawn, prev).expect("ground under the pointer");
                let plane = panned(&camera(&layers, centre, yaw, 400.0, VIEW), centre, prev, now);
                let new = g.panned(&layers, centre, yaw, 400.0, VIEW, DT, prev, now);
                assert!(new != centre, "yaw {yaw} step {step}: the view must move");
                let under = shows_at(&g, &layers, new, yaw, 400.0, anchor);
                assert!((under - now).length() < 0.5, "yaw {yaw} step {step}: the grabbed point shows at {under:?}, pointer at {now:?}");
                worst_plane = worst_plane.max((shows_at(&g, &layers, plane, yaw, 400.0, anchor) - now).length());
                centre = new;
                g.car_y = g.camera(&layers, centre, yaw, 400.0, VIEW, None, DT).relief.unwrap().car_y; // the next frame's camera
            }
        }
        assert!(worst_plane > 5.0, "the plane pan was meant to miss on these hills (worst {worst_plane} px)");
    }

    /// Wheel zoom in 3D keeps the terrain point under the cursor under it, zooming in and out.
    #[test]
    fn zoom_keeps_the_terrain_point_under_the_cursor_in_3d() {
        let layers = layers_3d();
        let t = Terrain::synthetic();
        for yaw in [0.0_f32, 0.8] {
            let centre = (-270.0, 120.0);
            let g = ground(t.height(centre.0, centre.1));
            for (at, factor) in [(pos2(560.0, 380.0), 0.6), (pos2(300.0, 450.0), 1.7), (pos2(420.0, 250.0), 0.8)] {
                let drawn = g.camera(&layers, centre, yaw, 500.0, VIEW, Some(g.car_y), DT);
                let anchor = pick(&drawn, at).expect("ground under the cursor");
                let (c, z) = g.zoomed_at(&layers, centre, yaw, 500.0, VIEW, DT, at, factor);
                assert!((z - 500.0 * factor).abs() < 1e-3);
                let under = shows_at(&g, &layers, c, yaw, z, anchor);
                assert!((under - at).length() < 0.5, "yaw {yaw} x{factor}: {under:?} vs {at:?}");
            }
            // The radius stays clamped.
            let (_, z) = g.zoomed_at(&layers, centre, yaw, 60.0, VIEW, DT, pos2(400.0, 300.0), 0.1);
            assert_eq!(z, ZOOM_MIN_M);
        }
    }

    /// The easing of the panned camera's height: no previous frame = the target at once; else a
    /// fraction per frame that grows with `dt` and never overshoots.
    #[test]
    fn the_panned_camera_height_eases_towards_the_terrain() {
        assert_eq!(eased_car_y(None, 300.0, DT), 300.0);
        assert_eq!(eased_car_y(Some(f32::NAN), 300.0, DT), 300.0);
        let (a, b) = (eased_car_y(Some(100.0), 300.0, DT), eased_car_y(Some(100.0), 300.0, 0.05));
        assert!(a > 100.0 && a < b && b < 300.0, "{a} {b}");
        // A long frame is capped, so a hitch does not teleport the camera.
        assert!(eased_car_y(Some(100.0), 300.0, 5.0) < 300.0);
        // It converges.
        let mut y = Some(0.0);
        for _ in 0..300 {
            y = Some(eased_car_y(y, 300.0, DT));
        }
        assert!((y.unwrap() - 300.0).abs() < 0.25);
    }

    /// The reason for the easing: with the pivot at the terrain under the centre, the picture
    /// moves several times too far uphill and hardly at all downhill (the sinking pivot cancels the
    /// drag). With it, a drag in either direction on a steep slope moves the grabbed point
    /// under the pointer, and the view really moves.
    #[test]
    fn dragging_up_and_down_a_slope_moves_the_map_in_3d() {
        let mut layers = layers_3d();
        layers.tilt.relief.exaggeration = 3.0;
        let t = Terrain::synthetic();
        // On the south flank of the 220 m hill (-300, 200): north is uphill.
        let car = (-300.0, 20.0);
        let g = ground(t.height(car.0, car.1) + 1.0);
        for (prev, now) in [(pos2(400.0, 330.0), pos2(400.0, 380.0)), (pos2(400.0, 380.0), pos2(400.0, 330.0))] {
            let anchor = pick(&g.camera(&layers, car, 0.0, 400.0, VIEW, Some(g.car_y), DT), prev).unwrap();
            let centre = g.panned(&layers, car, 0.0, 400.0, VIEW, DT, prev, now);
            assert!((shows_at(&g, &layers, centre, 0.0, 400.0, anchor) - now).length() < 0.5, "{prev:?} -> {now:?}");
            assert!((centre.1 - car.1).abs() > 5.0, "the view must have moved: {centre:?}");
        }
    }

    /// A pointer over no ground (above the horizon of the plane) leaves the view where it is.
    #[test]
    fn pan_over_the_sky_does_nothing_in_3d() {
        let layers = layers_3d();
        let g = ground(100.0);
        let c = (-250.0, 80.0);
        assert_eq!(g.panned(&layers, c, 0.0, 400.0, VIEW, DT, pos2(400.0, -3000.0), pos2(420.0, -3000.0)), c);
    }

    /// `draw` leaves the 3D look for the next frame's input, and clears it when it draws flat.
    #[test]
    fn the_3d_look_is_handed_from_draw_to_interact() {
        let layers = layers_3d();
        let cam = cam_3d(&layers, (-250.0, 150.0), 0.0, 400.0, 260.0);
        let flat = camera(&layers, (-250.0, 150.0), 0.0, 400.0, VIEW);
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(Ground::of(ui).is_none(), "nothing drawn yet");
                Ground::publish(ui, Some(&cam));
                assert_eq!(Ground::of(ui).map(|g| g.car_y), Some(260.0));
                Ground::publish(ui, Some(&flat));
                assert!(Ground::of(ui).is_none(), "a flat frame clears it");
                Ground::publish(ui, Some(&cam));
                Ground::publish(ui, None);
                assert!(Ground::of(ui).is_none());
            });
        });
    }

    /// The compass box is where `draw_compass` paints (disc of radius 11 at (18, 18) HUD units).
    #[test]
    fn the_compass_rect_covers_the_drawn_compass() {
        for (w, h) in [(160.0_f32, 100.0_f32), (800.0, 600.0), (3000.0, 1500.0)] {
            let r = Rect::from_min_size(pos2(10.0, 20.0), vec2(w, h));
            let s = compass_scale(r);
            let c = compass_rect(r);
            assert!((c.center() - (r.min + vec2(18.0, 18.0) * s)).length() < 1e-3);
            assert!((c.width() - 22.0 * s).abs() < 1e-3);
        }
    }

    /// `unproject_at_height` is the inverse of `project3` at a fixed height, and `h = 0` is the
    /// plane `unproject` knows.
    #[test]
    fn unproject_at_height_inverts_project3() {
        let layers = layers_3d();
        let cam = cam_3d(&layers, (-250.0, 150.0), 0.7, 500.0, 240.0);
        let k = cam.view.scale * cam.relief.as_ref().unwrap().exag;
        for p in [pos2(400.0, 300.0), pos2(150.0, 520.0), pos2(650.0, 380.0)] {
            let flat = cam.unproject(p).unwrap();
            let zero = unproject_at_height(&cam, p, 0.0).unwrap();
            assert!((flat[0] - zero[0]).abs() < 1e-2 && (flat[1] - zero[1]).abs() < 1e-2, "{flat:?} vs {zero:?}");
            for y in [100.0_f32, 180.0, 310.0] {
                let w = unproject_at_height(&cam, p, (y - 240.0) * k).unwrap();
                let (back, _) = cam.project3(w[0], y, w[1]).unwrap();
                assert!((back - p).length() < 0.05, "y {y}: {back:?} vs {p:?}");
            }
        }
    }

    /// A click in 3D lands on the terrain surface under the pointer (the waypoint goes where the
    /// user points, on a hillside too), not on the plane at the car's height.
    #[test]
    fn a_click_in_3d_picks_the_terrain_surface() {
        let layers = layers_3d();
        let t = Arc::new(Terrain::synthetic());
        // Looking at the 220 m hill from the south-east, the car at its foot.
        for yaw in [0.0_f32, 0.8] {
            let cam = cam_3d(&layers, (-250.0, 80.0), yaw, 500.0, t.height(-250.0, 80.0) + 1.0);
            let mut slopes = 0;
            for p in [pos2(400.0, 330.0), pos2(330.0, 260.0), pos2(470.0, 200.0), pos2(300.0, 400.0)] {
                let at = pick(&cam, p).expect("on the ground");
                let (back, _) = cam.project3(at[0], t.height(at[0], at[1]), at[1]).unwrap();
                assert!((back - p).length() < 0.5, "yaw {yaw}: picked {at:?} shows at {back:?}, not {p:?}");
                let flat = cam.unproject(p).unwrap();
                if (flat[0] - at[0]).hypot(flat[1] - at[1]) > 10.0 {
                    slopes += 1; // the plane would have been off by metres here
                }
            }
            assert!(slopes >= 1, "yaw {yaw}: the test points must include slopes");
        }
        // Without a relief it is the plane.
        let flat = camera(&layers, (0.0, 0.0), 0.3, 500.0, VIEW);
        assert_eq!(pick(&flat, pos2(300.0, 400.0)), flat.unproject(pos2(300.0, 400.0)));
        // A camera with a relief over level ground picks the plane too.
        let level = Arc::new(Terrain::flat(150.0));
        let cam = Camera::from_cfg_relief(&layers.tilt, (0.0, 0.0), 0.3, 500.0, VIEW, Some(&level), Some(150.0));
        let (a, b) = (pick(&cam, pos2(300.0, 400.0)).unwrap(), cam.unproject(pos2(300.0, 400.0)).unwrap());
        assert!((a[0] - b[0]).abs() < 1e-2 && (a[1] - b[1]).abs() < 1e-2);
    }

    /// Windows runs 3D only with the opt-in flag; the other platforms never ask for it. The one
    /// flag (`OverlayConfig::map_3d_windows`) is off by default and is the HUD's too.
    #[test]
    fn windows_needs_the_opt_in_for_3d() {
        assert!(allowed_3d(true));
        assert_eq!(allowed_3d(false), !cfg!(windows));
        assert!(!crate::config::OverlayConfig::default().map_3d_windows);
        let o: crate::config::OverlayConfig = serde_json::from_str("{}").unwrap();
        assert!(!o.map_3d_windows, "a config saved before the flag existed");
        assert_eq!(crate::hud::minimap::wants_3d(&crate::config::OverlayConfig { map_layers: layers_3d(), ..Default::default() }), !cfg!(windows));
        assert!(crate::hud::minimap::wants_3d(&crate::config::OverlayConfig { map_layers: layers_3d(), map_3d_windows: true, ..Default::default() }));
    }

    /// The retry fires once per appearance of a 3D map, and only for a failed context.
    #[test]
    fn a_failed_context_is_retried_when_a_3d_map_comes_back() {
        let mut g = RetryGate::default();
        // Healthy: never.
        assert!(!g.step(true, false) && !g.step(true, false));
        // Fails while the map is up: no retry in the same run of frames ...
        assert!(!g.step(true, true));
        assert!(!g.step(true, true));
        assert!(!g.step(true, true));
        // ... the user leaves (settings page, Flat, another tab) and brings it back: once.
        assert!(!g.step(false, true));
        assert!(g.step(true, true));
        assert!(!g.step(true, true), "still failed after the retry: wait for the next appearance");
        // A recovered context needs nothing on the next appearance.
        assert!(!g.step(false, false));
        assert!(!g.step(true, false));
    }

    /// A fresh `Map3d` (no GL touched yet) wants the 2D underlay, has nothing to retry, and a
    /// frame without any 3D map leaves it so.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn a_fresh_map3d_wants_the_underlay_and_survives_frames_without_gl() {
        let mut m = Map3d::default();
        assert!(m.wants_underlay());
        m.begin_frame(None);
        m.drew.set(true);
        m.begin_frame(None); // a rising edge, but not failed: nothing to do
        assert!(m.wants_underlay());
    }

    /// The exit path (`on_exit(Some(gl))` -> `Map3d::destroy`) on a real context: safe on a
    /// renderer that never drew, and twice.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    #[ignore = "needs an EGL device"]
    fn destroy_on_exit_is_safe_without_and_after_a_scene() {
        use crate::overlay::gl::{Flavour, Headless};
        let hl = match Headless::new_with(Flavour::Default, None) {
            Ok(h) => h,
            Err(e) => return eprintln!("SKIP: no EGL device: {e}"),
        };
        let m = Map3d::default();
        m.destroy(&hl.glow);
        m.destroy(&hl.glow);
        assert!(m.wants_underlay());
    }
}
