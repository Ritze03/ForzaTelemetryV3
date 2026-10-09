//! Projection and geometry helpers of the shared map renderer. Pure functions on plain values
//! (no `Painter`), unit-tested.
//!
//! [`Camera`] is the one projection both renderers use: the 2D one here (flat map, optionally
//! tilted) and phase K's GL 3D scene. At pitch 0 it is exactly [`MapView`]'s mapping; with
//! pitch it is a *flat perspective of that map* (D65): the map plane is turned about the
//! horizontal axis through the car and seen by an eye `focal` px above the car, which is what
//! the user's demo does with CSS `perspective(P) rotateX(a)` (that CSS transform is the
//! reference, `docs/features/minimap.md`).
//!
//! **Heights (phase K, D61).** The flat tilt is exactly a pinhole camera looking at a flat
//! world; give the camera a [`Relief`] (terrain + vertical exaggeration + the car's height) and
//! the same camera projects points with heights ([`Camera::project3`]), which is the 3D map's
//! camera: at height 0 it is bit-for-bit the tilt maths (a test pins that to 1e-3 px), so
//! switching Tilted <-> 3D keeps the car on the same screen point at the same ground scale and only
//! adds relief and parallax. With a relief, [`Camera::project`] (the call every layer already
//! uses) projects the *terrain surface point* above (x, z), so POIs, markers and race lines
//! follow the ground with no call-site change. The GL renderer uses [`Camera::view_proj`], which is
//! the same maths as a matrix. See `docs/features/minimap.md` ("3D: data and camera").

use std::sync::Arc;

use egui::epaint::Vertex;
use egui::{pos2, vec2, Mesh, Pos2, Rect};

use super::cfg::TiltCfg;
use super::terrain::Terrain;
use crate::minimap::MapView;

// ── camera ───────────────────────────────────────────────────────────────────────────────────

/// Height of the view the tilt settings are written for: the HUD pill, 136 design px.
pub const REF_VIEW_H: f32 = 136.0;

/// A point must be at least this far (px) in front of the eye to be projected.
const EYE_EPS: f32 = 1.0;
/// The tilted plane ends where things are this small ([`Camera::depth_scale_at_row`]): just short
/// of the horizon, so the map fills the whole view whenever the horizon is outside it, and reaches
/// to within a few px of the horizon when a steep tilt brings it into view. (It used to end a
/// fixed 3.2 view heights ahead, the demo's canvas, which at the HUD's 55 deg left the top fifth
/// of the pill empty and faded the next third.)
pub const FAR_MIN_SCALE: f32 = 0.05;

/// Smallest pitch the eye-clearance rule may reduce to (the tilt config's own lower clamp).
pub const MIN_PITCH: f32 = 5.0 * std::f32::consts::PI / 180.0;

/// The 3D part of a camera: what turns the flat tilt into the relief view.
#[derive(Clone, Debug)]
pub struct Relief {
    pub terrain: Arc<Terrain>,
    /// Vertical exaggeration about y = 0 (`ReliefCfg::exaggeration`); the eye follows the scaled
    /// world, so the picture is that of a world whose heights are simply `exag` times as big.
    pub exag: f32,
    /// Height of the car in metres (un-exaggerated): the telemetry height (+ ~1 m) while
    /// following, the terrain height under the view centre for a panned view.
    pub car_y: f32,
}

impl Relief {
    pub fn new(terrain: Arc<Terrain>, exag: f32, car_y: f32) -> Relief {
        Relief { terrain, exag: if exag.is_finite() { exag.clamp(0.1, 10.0) } else { 1.0 }, car_y }
    }
}

/// Car-centred camera over the map plane. Screen space = egui points.
///
/// **Not `Copy`** since phase K: the optional [`Relief`] holds an `Arc<Terrain>`. *Why not keep
/// it `Copy` and pass the terrain next to it:* `cam.project(x, z)` has ~30 call sites (layers,
/// markers, trails) that must get terrain heights without being touched; with the terrain inside
/// the camera they do. Dropping `Copy` broke nothing: every use was already by reference or a
/// fresh `Camera::from_cfg` (K1 compiled with no call-site edit, inside or outside `maprender`).
#[derive(Clone, Debug)]
pub struct Camera {
    /// The flat mapping (scale, yaw, car position); "plane offsets" below are its offsets.
    pub view: MapView,
    /// Where the car is on screen (the rect centre when flat).
    pub centre: Pos2,
    /// The screen area the camera fills.
    pub rect: Rect,
    /// Tilt in radians; 0 = looking straight down (flat).
    pub pitch: f32,
    /// Eye distance from the plane at the car, in px (the CSS `perspective`).
    pub focal: f32,
    /// `Some` = the 3D relief camera ([`Camera::project3`]); `None` = the flat plane.
    pub relief: Option<Relief>,
}

impl Camera {
    /// `zoom_m` = metres from the centre to the nearest edge of the *untilted* map, as in
    /// [`MapView::new`] (`view_px` = the rect's smaller side).
    #[allow(clippy::too_many_arguments)]
    pub fn new(car_x: f32, car_z: f32, yaw: f32, zoom_m: f32, rect: Rect, centre: Pos2, pitch: f32, focal: f32) -> Camera {
        let view = MapView::new(car_x, car_z, yaw, zoom_m, rect.width().min(rect.height()));
        Camera { view, centre, rect, pitch, focal: focal.max(1.0), relief: None }
    }

    /// The camera of a map for its tilt config: flat when tilt is off, else the car `car_y` of the
    /// way down `rect`, `angle_deg` of pitch (clamped 5..80) and the perspective distance scaled
    /// to the view's height ([`Camera::focal_for`]). The one constructor both maps use.
    pub fn from_cfg(tilt: &TiltCfg, car: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect) -> Camera {
        if tilt.on {
            let pitch = tilt.angle_deg.clamp(5.0, 80.0).to_radians();
            Camera::new(car.0, car.1, yaw, zoom_m, rect, Camera::tilt_centre(rect, tilt.car_y), pitch, Camera::focal_for(tilt.perspective_px, rect))
        } else {
            Camera::new(car.0, car.1, yaw, zoom_m, rect, rect.center(), 0.0, 1.0)
        }
    }

    /// The eye distance (CSS `perspective`) for a view of `rect`'s height: the config value is
    /// the one of the HUD pill (136 px tall, [`REF_VIEW_H`]); a taller view scales it up so its
    /// tilted look is the same picture, only bigger.
    pub fn focal_for(perspective_px: f32, rect: Rect) -> f32 {
        perspective_px * rect.height() / REF_VIEW_H
    }

    /// Size factor of things on screen row `sy`: 1 at the car's row, smaller towards the horizon
    /// (and larger below the car), 1 when flat. The same number as [`Camera::perspective_at`] of
    /// the plane point under that row, in closed form (`k = 1 + dy tan(pitch) / focal`), so a
    /// projected polyline can be tapered without unprojecting.
    pub fn depth_scale_at_row(&self, sy: f32) -> f32 {
        if self.is_flat() {
            return 1.0;
        }
        (1.0 + (sy - self.centre.y) * self.pitch.tan() / self.focal).clamp(0.05, 4.0)
    }

    /// Where the car sits for a tilt config: horizontally centred, `car_y` of the way down.
    pub fn tilt_centre(rect: Rect, car_y: f32) -> Pos2 {
        pos2(rect.center().x, rect.top() + car_y.clamp(0.1, 0.95) * rect.height())
    }

    pub fn is_flat(&self) -> bool {
        self.pitch.abs() < 1e-4
    }

    /// Screen px per metre on the plane at the car.
    pub fn scale(&self) -> f32 {
        self.view.scale
    }

    /// Plane offset (px from the car, y down; what [`MapView::world_to_offset`] returns) → screen.
    /// `None` when the point is not in front of the eye.
    pub fn project_offset(&self, ox: f32, oy: f32) -> Option<Pos2> {
        if self.is_flat() {
            return Some(self.centre + vec2(ox, oy));
        }
        let (s, c) = self.pitch.sin_cos();
        let d = self.focal - oy * s;
        if d < EYE_EPS {
            return None;
        }
        let k = self.focal / d;
        Some(self.centre + vec2(ox * k, oy * c * k))
    }

    /// World (x, z) → screen. Flat plane without a relief; with one, the terrain surface point
    /// above (x, z) projected through the 3D camera ([`Camera::project3`]).
    pub fn project(&self, wx: f32, wz: f32) -> Option<Pos2> {
        match &self.relief {
            None => {
                let [ox, oy] = self.view.world_to_offset(wx, wz);
                self.project_offset(ox, oy)
            }
            Some(r) => self.project3(wx, r.terrain.height(wx, wz), wz).map(|p| p.0),
        }
    }

    /// Screen → plane offset. `None` at or above the horizon.
    pub fn unproject_offset(&self, p: Pos2) -> Option<[f32; 2]> {
        let (sx, sy) = (p.x - self.centre.x, p.y - self.centre.y);
        if self.is_flat() {
            return Some([sx, sy]);
        }
        let (s, c) = self.pitch.sin_cos();
        let den = self.focal * c + sy * s;
        if den < 1e-3 {
            return None;
        }
        let oy = sy * self.focal / den;
        Some([sx * (self.focal - oy * s) / self.focal, oy])
    }

    /// Screen → world (x, z).
    pub fn unproject(&self, p: Pos2) -> Option<[f32; 2]> {
        let [ox, oy] = self.unproject_offset(p)?;
        Some(self.view.offset_to_world(ox, oy))
    }

    /// Metres from the centre to the nearest edge of the untilted map (the "radius" the zoom
    /// settings and the POI zoom limit are in).
    pub fn zoom_m(&self) -> f32 {
        self.rect.width().min(self.rect.height()) / (2.0 * self.view.scale)
    }

    /// Size factor of things standing at plane offset `oy` (px ahead of the car, y down): 1 when
    /// flat, `focal / (focal - oy sin(pitch))` tilted (smaller towards the horizon).
    pub fn perspective_at(&self, oy: f32) -> f32 {
        if self.is_flat() {
            return 1.0;
        }
        let d = self.focal - oy * self.pitch.sin();
        if d < EYE_EPS { 1.0 } else { (self.focal / d).min(4.0) }
    }

    /// The screen row where [`Camera::depth_scale_at_row`] is `k` (tilted only; above the car
    /// for `k < 1`).
    pub fn row_of_depth_scale(&self, k: f32) -> f32 {
        self.centre.y + (k - 1.0) * self.focal / self.pitch.tan().max(1e-6)
    }

    /// The screen row of the plane's far limit ([`FAR_MIN_SCALE`]); above the rect when the
    /// horizon is out of view. Flat: no limit (`f32::MIN`).
    pub fn far_row(&self) -> f32 {
        if self.is_flat() { f32::MIN } else { self.row_of_depth_scale(FAR_MIN_SCALE) }
    }

    /// How far the tilted plane extends ahead of the car (px): to the rect's top edge (a px past
    /// it), or to the far limit when that is lower (the horizon is in view).
    pub fn far_px(&self) -> f32 {
        let y = self.far_row().max(self.rect.top() - 1.0);
        self.unproject_offset(pos2(self.centre.x, y)).map_or(0.0, |o| -o[1])
    }

    /// The part of the plane that can appear on `rect` (plane offsets, y down): the unprojected
    /// rect corners, clamped to the far limit above the horizon and to the eye below.
    pub fn plane_rect(&self) -> Rect {
        let (mut lo, mut hi) = (vec2(f32::MAX, f32::MAX), vec2(f32::MIN, f32::MIN));
        let mut add = |o: [f32; 2]| {
            lo = vec2(lo.x.min(o[0]), lo.y.min(o[1]));
            hi = vec2(hi.x.max(o[0]), hi.y.max(o[1]));
        };
        let (s, _) = self.pitch.sin_cos();
        let near = if self.is_flat() || s <= 1e-4 { f32::MAX } else { 0.9 * self.focal / s };
        let far = self.far_px();
        for corner in [self.rect.left_top(), self.rect.right_top(), self.rect.right_bottom(), self.rect.left_bottom()] {
            let o = match self.unproject_offset(corner) {
                Some(o) if self.is_flat() => o,
                Some(o) if o[1] >= -far => [o[0], o[1].min(near)],
                _ => {
                    // Above the horizon or beyond the far limit: use the far line with the
                    // horizontal scale the perspective gives there.
                    let oy = -far;
                    [(corner.x - self.centre.x) * (self.focal - oy * s) / self.focal, oy]
                }
            };
            add(o);
        }
        Rect::from_min_max(pos2(lo.x, lo.y), pos2(hi.x, hi.y))
    }

    /// Axis-aligned world box `[min_x, min_z, max_x, max_z]` of everything that can appear on
    /// `rect` (plus `margin` metres), for culling chains by their bbox.
    pub fn world_aabb(&self, margin: f32) -> [f32; 4] {
        let pr = self.plane_rect();
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for (ox, oy) in [(pr.min.x, pr.min.y), (pr.max.x, pr.min.y), (pr.max.x, pr.max.y), (pr.min.x, pr.max.y)] {
            let [wx, wz] = self.view.offset_to_world(ox, oy);
            b = [b[0].min(wx), b[1].min(wz), b[2].max(wx), b[3].max(wz)];
        }
        [b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin]
    }
}

// Phase K's 3D camera. The GL scene (`gl3d`, K2) draws with it; the call sites (K3, K4) build it.
// What only they use (`with_relief`, `from_cfg_relief`, `with_eye_clearance`, ...) may stay unused until
// they land; the unit tests exercise all of it.
#[allow(dead_code)]
impl Camera {
    /// The same camera with a relief (3D). Call it only for a tilted camera (`tilt.on`): a flat
    /// 2D map has no use for heights.
    pub fn with_relief(mut self, relief: Relief) -> Camera {
        self.relief = Some(relief);
        self
    }

    /// [`Camera::from_cfg`] for a map that may be in 3D: with `tilt.relief.on` (and `tilt.on`)
    /// and a loaded `terrain` the camera carries the relief (exaggeration from the config);
    /// `car_y` = the car's height in metres, `None` = the terrain height under the car (a
    /// panned view, or no telemetry height). Otherwise exactly [`Camera::from_cfg`], so a map
    /// whose terrain is still loading draws its tilted 2D path.
    pub fn from_cfg_relief(tilt: &TiltCfg, car: (f32, f32), yaw: f32, zoom_m: f32, rect: Rect, terrain: Option<&Arc<Terrain>>, car_y: Option<f32>) -> Camera {
        let cam = Camera::from_cfg(tilt, car, yaw, zoom_m, rect);
        match terrain {
            Some(t) if tilt.on && tilt.relief.on => {
                let r = tilt.relief.sane();
                let y = car_y.filter(|y| y.is_finite()).unwrap_or_else(|| t.height(car.0, car.1));
                cam.with_relief(Relief::new(t.clone(), r.exaggeration, y))
            }
            _ => cam,
        }
    }


    /// World point (x east, **y up in metres**, z north) → (screen, camera depth in px). The 3D
    /// camera: with `(ox, oy)` the plane offset px of (x, z) ([`MapView::world_to_offset`]) and
    /// `h` the height px above the car's plane (`(y - car_y) * exag * scale`),
    ///
    /// ```text
    /// cam    = (ox, oy cos a - h sin a, P - oy sin a - h cos a)      a = pitch, P = focal
    /// screen = centre + cam.xy * P / cam.z
    /// ```
    ///
    /// At `h = 0` this is [`Camera::project_offset`]. `None` when the point is not at least
    /// [`EYE_EPS`] px in front of the eye. Without a relief the car is at y = 0 and `exag` is 1.
    pub fn project3(&self, wx: f32, wy: f32, wz: f32) -> Option<(Pos2, f32)> {
        let (exag, car_y) = self.relief.as_ref().map_or((1.0, 0.0), |r| (r.exag, r.car_y));
        let [ox, oy] = self.view.world_to_offset(wx, wz);
        let h = (wy - car_y) * exag * self.view.scale;
        let (s, c) = self.pitch.sin_cos();
        let cz = self.focal - oy * s - h * c;
        if cz < EYE_EPS {
            return None;
        }
        let k = self.focal / cz;
        Some((self.centre + vec2(ox * k, (oy * c - h * s) * k), cz))
    }

    /// Size factor of things standing on the ground at (x, z): `focal / depth` (1 at the car's
    /// plane position, smaller further away), clamped to 4 like [`Camera::perspective_at`], the
    /// general form of which it is; 0 when the point is behind the eye.
    pub fn k_at(&self, wx: f32, wz: f32) -> f32 {
        match &self.relief {
            None => {
                let [_, oy] = self.view.world_to_offset(wx, wz);
                self.perspective_at(oy)
            }
            Some(r) => self.project3(wx, r.terrain.height(wx, wz), wz).map_or(0.0, |(_, cz)| (self.focal / cz).min(4.0)),
        }
    }

    /// Where the eye is, in world metres (x, y up, z): the car moved back by `P sin a / scale`
    /// and up by `P cos a / scale` (a pinhole of focal length `P` px whose eye is `P / scale` m
    /// from the car, looking at it; the lens shift is the principal point `centre`). `y` is in
    /// real (un-exaggerated) metres. Without a relief the car is at y = 0.
    pub fn eye(&self) -> [f32; 3] {
        let (exag, car_y) = self.relief.as_ref().map_or((1.0, 0.0), |r| (r.exag, r.car_y));
        let (s, c) = self.pitch.sin_cos();
        let (back, up) = (self.focal * s / self.view.scale, self.focal * c / self.view.scale);
        // `ahead` in world (x, z) is (sin yaw, cos yaw), see `MapView::world_to_offset`.
        [self.view.car_x - self.view.sin_yaw * back, car_y + up / exag, self.view.car_z - self.view.cos_yaw * back]
    }

    /// Is the eye above the ground by `margin` metres, and the line of sight from the car to it
    /// free of terrain (blocked by more than 1 m)? The first tenth of the line is not tested
    /// (the car stands on the ground). Always true without a relief.
    pub fn eye_clear(&self, margin: f32) -> bool {
        let Some(r) = &self.relief else { return true };
        let eye = self.eye();
        if eye[1] < r.terrain.height(eye[0], eye[2]) + margin {
            return false;
        }
        let car = [self.view.car_x, r.car_y, self.view.car_z];
        let dist = ((eye[0] - car[0]).powi(2) + (eye[2] - car[2]).powi(2) + (eye[1] - car[1]).powi(2)).sqrt();
        let n = ((dist / r.terrain.grid.res as f32).ceil() as usize).clamp(8, 96);
        (1..n).all(|i| {
            let t = i as f32 / n as f32;
            if t < 0.1 {
                return true;
            }
            let p = [car[0] + (eye[0] - car[0]) * t, car[1] + (eye[1] - car[1]) * t, car[2] + (eye[2] - car[2]) * t];
            p[1] >= r.terrain.height(p[0], p[2]) - 1.0
        })
    }

    /// The **eye-clearance rule** (design §3.2, polish): if the eye would be underground (within
    /// `margin` m of the ground) or the car hidden behind a hill, the camera looks more
    /// top-down — the pitch is lowered (bisection, down to [`MIN_PITCH`]) until it clears. Needed for
    /// 0.1-0.7 % of positions only (measured on the real roads), so it is a safety net; the
    /// caller should ease the pitch change over ~0.5 s rather than snap. Unchanged when it
    /// clears already or without a relief.
    pub fn with_eye_clearance(mut self, margin: f32) -> Camera {
        if self.relief.is_none() || self.is_flat() || self.eye_clear(margin) {
            return self;
        }
        let hi = self.pitch;
        self.pitch = MIN_PITCH.min(hi);
        if !self.eye_clear(margin) {
            return self; // as top-down as the tilt config allows; nothing better to offer
        }
        let (mut ok, mut bad) = (self.pitch, hi);
        for _ in 0..16 {
            let mid = 0.5 * (ok + bad);
            self.pitch = mid;
            if self.eye_clear(margin) {
                ok = mid;
            } else {
                bad = mid;
            }
        }
        self.pitch = ok;
        self
    }

    /// Where the ray through screen point `p` meets the horizontal plane `plane_h` metres
    /// (exaggerated units) above the car, as world (x, z). A ray that never reaches the plane in
    /// front of the eye, or meets it beyond the far limit ([`FAR_MIN_SCALE`]: the 3D scene fades
    /// out where things are smaller than that), ends at the far limit.
    fn ray_ground(&self, p: Pos2, plane_h: f32) -> [f32; 2] {
        let (s, c) = self.pitch.sin_cos();
        let (dx, dy) = ((p.x - self.centre.x) / self.focal, (p.y - self.centre.y) / self.focal);
        let t_far = self.focal / FAR_MIN_SCALE;
        let top = self.focal * c - plane_h * self.view.scale;
        let den = dy * s + c;
        let t = if den > 1e-6 && top > 0.0 { (top / den).min(t_far) } else { t_far };
        // Back from camera coordinates: ox = x, oy = y cos a + (P - z) sin a (plane offset px).
        self.view.offset_to_world(t * dx, t * dy * c + (self.focal - t) * s)
    }

    /// Axis-aligned world box `[min_x, min_z, max_x, max_z]` of the ground that can appear in
    /// `rect` (plus `margin` m), for culling by bbox: the 3D replacement of
    /// [`Camera::world_aabb`]. The four view corners are cast onto the plane at the car's height,
    /// then onto the planes at the lowest and highest terrain found under that first box, so the
    /// hills and valleys that shift the picture are inside it.
    pub fn footprint(&self, margin: f32) -> [f32; 4] {
        let Some(r) = &self.relief else { return self.world_aabb(margin) };
        let corners = [self.rect.left_top(), self.rect.right_top(), self.rect.right_bottom(), self.rect.left_bottom()];
        let aabb = |plane_h: f32| {
            let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for c in corners {
                let [x, z] = self.ray_ground(c, plane_h);
                b = [b[0].min(x), b[1].min(z), b[2].max(x), b[3].max(z)];
            }
            b
        };
        let mut b = aabb(0.0);
        let stride = (((b[2] - b[0]).max(b[3] - b[1]) / r.terrain.grid.res as f32 / 64.0).ceil() as usize).max(1);
        if let Some((lo, hi)) = r.terrain.grid.range_in(b, stride) {
            for h in [lo, hi] {
                let o = aabb((h - r.car_y) * r.exag);
                b = [b[0].min(o[0]), b[1].min(o[1]), b[2].max(o[2]), b[3].max(o[3])];
            }
        }
        [b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin]
    }

    /// Near and far clip distances (camera depth px) of [`Camera::view_proj`]: 0.05 P and 60 P.
    pub fn near_far(&self, ppp: f32) -> (f32, f32) {
        (0.05 * self.focal * ppp, 60.0 * self.focal * ppp)
    }

    /// The camera as a column-major 4x4 for GL, **relative to the car**: `clip = M * (d, 1)` with
    /// `d = (x - car_x, y * exag - car_y * exag, z - car_z)` for a world point (x east, y up in
    /// metres, z north) — see [`Camera::car_exag`]. Subtract the car in the vertex shader (in
    /// f32 that is exact to ~1 mm over the island) rather than folding it into the matrix: the
    /// folded form ([`Camera::view_proj`]) loses precision where the depth is small (see there).
    ///
    /// Output is relative to the camera's `rect` used as the GL viewport (`ppp` = pixels per
    /// point, so the viewport is `rect.size() * ppp`): NDC x/y span the rect, z is the usual
    /// -1..1 over [`Camera::near_far`], `w` is the camera depth in viewport px. Equals
    /// [`Camera::project3`] (tested to 1e-3 px for points beyond the near plane).
    pub fn view_proj_rel(&self, ppp: f32) -> [f32; 16] {
        let rows = self.proj_rows(ppp);
        let mut m = [0.0f32; 16];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                m[c * 4 + r] = *v as f32;
            }
        }
        m
    }

    /// [`Camera::view_proj_rel`] with the car's translation folded in: `clip = M * (x, y * exag, z, 1)`
    /// — **the caller multiplies y by [`Camera::exag`]**. For CPU culling (tile corners against the
    /// clip planes). Its translation column is large (thousands of px), so f32 rounding costs
    /// up to ~0.01 px at the near plane and 1e-4 px beyond ~10 near-plane distances: fine for
    /// culling, too coarse for vertices — draw with the relative form.
    pub fn view_proj(&self, ppp: f32) -> [f32; 16] {
        let mut rows = self.proj_rows(ppp);
        let car = self.car_exag();
        for row in rows.iter_mut() {
            row[3] -= row[0] * car[0] as f64 + row[1] * car[1] as f64 + row[2] * car[2] as f64;
        }
        let mut m = [0.0f32; 16];
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                m[c * 4 + r] = *v as f32;
            }
        }
        m
    }

    /// The car in the units of [`Camera::view_proj_rel`]'s input: `[x, y * exag, z]` (y 0 without
    /// a relief).
    pub fn car_exag(&self) -> [f32; 3] {
        let (exag, car_y) = self.relief.as_ref().map_or((1.0, 0.0), |r| (r.exag, r.car_y));
        [self.view.car_x, car_y * exag, self.view.car_z]
    }

    /// Rows (clip.x, clip.y, clip.z, clip.w) over (dx, dy, dz, 1), d relative to the car, in f64.
    fn proj_rows(&self, ppp: f32) -> [[f64; 4]; 4] {
        let (sy, cy) = (self.view.sin_yaw as f64, self.view.cos_yaw as f64);
        let (sp, cp) = (self.pitch as f64).sin_cos();
        let ppp = ppp as f64;
        let (w, h) = (self.rect.width() as f64 * ppp, self.rect.height() as f64 * ppp);
        let p = self.focal as f64 * ppp;
        let s = self.view.scale as f64 * ppp;
        let (cxp, cyp) = ((self.centre.x - self.rect.min.x) as f64 * ppp, (self.centre.y - self.rect.min.y) as f64 * ppp);
        // cam = A * d + (0, 0, P):
        //   cam.x = s (dx cy - dz sy)
        //   cam.y = -s (dx sy + dz cy) cp - s dy sp
        //   cam.z = P + s (dx sy + dz cy) sp - s dy cp
        let cx = [s * cy, 0.0, -s * sy];
        let cyv = [-s * sy * cp, -s * sp, -s * cy * cp];
        let czv = [s * sy * sp, -s * cp, s * cy * sp];
        let (n, f) = (0.05 * p, 60.0 * p);
        let (a1, b1) = ((f + n) / (f - n), -2.0 * f * n / (f - n));
        let kx = 2.0 * cxp / w - 1.0;
        let ky = 1.0 - 2.0 * cyp / h;
        let (px, py) = (2.0 * p / w, 2.0 * p / h);
        let mut rows = [[0.0f64; 4]; 4];
        for k in 0..3 {
            rows[0][k] = kx * czv[k] + px * cx[k];
            rows[1][k] = ky * czv[k] - py * cyv[k];
            rows[2][k] = a1 * czv[k];
            rows[3][k] = czv[k];
        }
        rows[0][3] = kx * p;
        rows[1][3] = ky * p;
        rows[2][3] = a1 * p + b1;
        rows[3][3] = p;
        rows
    }

    /// Vertical exaggeration (1 without a relief): the factor the world's y is multiplied by
    /// before [`Camera::view_proj`].
    pub fn exag(&self) -> f32 {
        self.relief.as_ref().map_or(1.0, |r| r.exag)
    }
}

/// Do two `[min_x, min_z, max_x, max_z]` boxes overlap?
pub fn bbox_hits(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

// ── thinning ─────────────────────────────────────────────────────────────────────────────────

/// Drop points closer than `min_px` to the last kept one. Keeps the first and the last point,
/// so a thinned polyline still starts and ends where the original does. Idempotent.
pub fn thin(points: &mut Vec<Pos2>, min_px: f32) {
    if points.len() <= 2 || min_px <= 0.0 {
        return;
    }
    let m2 = min_px * min_px;
    let last = *points.last().unwrap();
    let mut w = 1;
    for r in 1..points.len() - 1 {
        if (points[r] - points[w - 1]).length_sq() >= m2 {
            points[w] = points[r];
            w += 1;
        }
    }
    // Attach the end point; drop kept points it would crowd (never the very first), so the
    // result has no tiny stub at the end and thinning it again changes nothing.
    while w > 1 && (last - points[w - 1]).length_sq() < m2 {
        w -= 1;
    }
    points[w] = last;
    w += 1;
    points.truncate(w);
}

// ── polygon clipping + meshes (moved from `hud::minimap`) ────────────────────────────────────

/// Sutherland–Hodgman: `subject` clipped to the convex polygon `clip` (either winding).
/// Used to cut a map shape to the map image when edges aren't mirrored.
pub fn clip_convex(subject: &[Pos2], clip: &[Pos2]) -> Vec<Pos2> {
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

/// Triangle fan from `centre` over the closed `outline`.
pub fn fan(mesh: &mut Mesh, centre: Pos2, outline: &[Pos2], vertex: impl Fn(Pos2) -> Vertex) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(vertex(centre));
    mesh.vertices.extend(outline.iter().map(|&pt| vertex(pt)));
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(base, base + 1 + i, base + 1 + (i + 1) % n);
    }
}

/// Cyrus–Beck: the part of segment `a→b` inside the convex polygon `clip` (either winding).
pub fn clip_segment_convex(a: Pos2, b: Pos2, clip: &[Pos2]) -> Option<(Pos2, Pos2)> {
    let area: f32 = (0..clip.len()).map(|i| clip[i].x * clip[(i + 1) % clip.len()].y - clip[(i + 1) % clip.len()].x * clip[i].y).sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for i in 0..clip.len() {
        let (p, q) = (clip[i], clip[(i + 1) % clip.len()]);
        // Inside = left of p→q for a positive-area polygon: cross(q-p, x-p) * sign >= 0.
        let e = q - p;
        let num = (e.x * (a.y - p.y) - e.y * (a.x - p.x)) * sign;
        let den = (e.x * d.y - e.y * d.x) * sign;
        if den.abs() < 1e-9 {
            if num < 0.0 {
                return None;
            }
        } else {
            let t = -num / den;
            if den > 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
    }
    Some((a + d * t0, a + d * t1))
}

/// A polyline cut to the convex polygon `clip`: the pieces inside it (a polyline that leaves and
/// re-enters becomes several). Used for the HUD pill's rounded corners, where a plain scissor
/// rectangle would let roads poke out into the transparent surround.
pub fn clip_polyline_convex(pts: &[Pos2], clip: &[Pos2]) -> Vec<Vec<Pos2>> {
    let mut out: Vec<Vec<Pos2>> = Vec::new();
    let mut cur: Vec<Pos2> = Vec::new();
    for w in pts.windows(2) {
        match clip_segment_convex(w[0], w[1], clip) {
            Some((p, q)) => {
                if cur.last().is_none_or(|l| (*l - p).length_sq() > 1e-4) {
                    if cur.len() >= 2 {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.clear();
                    cur.push(p);
                }
                cur.push(q);
            }
            None => {
                if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// Is `p` inside the convex polygon `clip` (either winding)?
pub fn inside_convex(p: Pos2, clip: &[Pos2]) -> bool {
    let mut pos = false;
    let mut neg = false;
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let c = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        pos |= c > 0.0;
        neg |= c < 0.0;
        if pos && neg {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimap::MapCalibration;

    fn rect(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(w, h))
    }

    fn cam(yaw: f32, pitch: f32, focal: f32) -> Camera {
        let r = rect(400.0, 300.0);
        let centre = if pitch == 0.0 { r.center() } else { Camera::tilt_centre(r, 0.85) };
        Camera::new(1000.0, -2000.0, yaw, 500.0, r, centre, pitch, focal)
    }

    #[test]
    fn pitch_zero_is_the_map_view_mapping() {
        for yaw in [0.0f32, 0.7, 2.5] {
            let c = cam(yaw, 0.0, 200.0);
            assert!(c.is_flat());
            for (wx, wz) in [(1000.0, -2000.0), (1200.0, -1900.0), (700.0, -2500.0)] {
                let [ox, oy] = c.view.world_to_offset(wx, wz);
                let p = c.project(wx, wz).unwrap();
                assert!((p - (c.centre + vec2(ox, oy))).length() < 1e-4);
                let w = c.unproject(p).unwrap();
                assert!((w[0] - wx).abs() < 0.01 && (w[1] - wz).abs() < 0.01, "{w:?}");
            }
        }
    }

    #[test]
    fn tilt_projection_matches_the_css_perspective_rotate_x() {
        // CSS: perspective(P) rotateX(a) about the car. Plane point (x, y) (y down) →
        // screen (x, y cos a) * P / (P - y sin a).
        let (a, p) = (55f32.to_radians(), 200.0);
        let c = cam(0.0, a, p);
        // 100 px straight ahead (up): the plane offset is (0, -100).
        let s = c.project_offset(0.0, -100.0).unwrap();
        let k = p / (p + 100.0 * a.sin());
        assert!((s.y - (c.centre.y - 100.0 * a.cos() * k)).abs() < 1e-3);
        assert!((s.x - c.centre.x).abs() < 1e-4);
        // 40 px to the right at the same distance is narrower than at the car.
        let r = c.project_offset(40.0, -100.0).unwrap();
        assert!((r.x - c.centre.x - 40.0 * k).abs() < 1e-3);
        // The car itself stays put; things in front shrink towards the horizon at -P/tan(a).
        assert_eq!(c.project_offset(0.0, 0.0).unwrap(), c.centre);
        let far = c.project_offset(0.0, -1.0e6).unwrap();
        assert!((far.y - (c.centre.y - p / a.tan())).abs() < 0.5, "{far:?}");
        // Behind the eye (far below the car) does not project.
        assert!(c.project_offset(0.0, 2.0 * p / a.sin()).is_none());
    }

    #[test]
    fn tilt_round_trips_and_a_known_world_point_lands_where_expected() {
        let c = cam(0.0, 55f32.to_radians(), 200.0);
        // 100 m north of the car at scale 0.3 px/m = 30 px ahead (zoom 500 on a 300 px side).
        let sc = c.scale();
        assert!((sc - 300.0 / 1000.0).abs() < 1e-6);
        let p = c.project(1000.0, -2000.0 + 100.0).unwrap();
        let oy = -100.0 * sc;
        let want = oy * 55f32.to_radians().cos() * 200.0 / (200.0 - oy * 55f32.to_radians().sin());
        assert!((p.y - (c.centre.y + want)).abs() < 1e-3);
        for (wx, wz) in [(1050.0, -1800.0), (900.0, -2100.0), (1000.0, -2000.0)] {
            let p = c.project(wx, wz).unwrap();
            let w = c.unproject(p).unwrap();
            assert!((w[0] - wx).abs() < 0.05 && (w[1] - wz).abs() < 0.05, "{w:?} vs {wx},{wz}");
        }
        // Above the horizon there is no ground.
        assert!(c.unproject(pos2(c.centre.x, c.centre.y - 200.0 / 55f32.to_radians().tan() - 5.0)).is_none());
    }

    #[test]
    fn world_aabb_of_a_rotated_flat_rect() {
        // Half extents in metres: 200 px / 0.3 and 150 px / 0.3.
        let (hw, hh) = (200.0 / 0.3, 150.0 / 0.3);
        for yaw_deg in [0.0f32, 30.0, 90.0] {
            let y = yaw_deg.to_radians();
            let c = cam(y, 0.0, 1.0);
            let b = c.world_aabb(0.0);
            let ex = hw * y.cos().abs() + hh * y.sin().abs();
            let ez = hw * y.sin().abs() + hh * y.cos().abs();
            assert!((b[0] - (1000.0 - ex)).abs() < 0.5 && (b[2] - (1000.0 + ex)).abs() < 0.5, "{yaw_deg}: {b:?}");
            assert!((b[1] - (-2000.0 - ez)).abs() < 0.5 && (b[3] - (-2000.0 + ez)).abs() < 0.5, "{yaw_deg}: {b:?}");
        }
        // A margin grows it on every side.
        let b0 = cam(0.0, 0.0, 1.0).world_aabb(0.0);
        let b1 = cam(0.0, 0.0, 1.0).world_aabb(25.0);
        assert!((b0[0] - b1[0] - 25.0).abs() < 1e-3 && (b1[3] - b0[3] - 25.0).abs() < 1e-3);
    }

    #[test]
    fn tilted_world_aabb_contains_everything_visible() {
        let c = cam(0.4, 55f32.to_radians(), 200.0);
        let b = c.world_aabb(0.0);
        // Every screen pixel below the horizon that unprojects within the far limit is inside.
        for ix in 0..=8 {
            for iy in 0..=8 {
                let p = c.rect.min + vec2(c.rect.width() * ix as f32 / 8.0, c.rect.height() * iy as f32 / 8.0);
                if let Some([ox, oy]) = c.unproject_offset(p) {
                    if oy >= -c.far_px() {
                        let [wx, wz] = c.view.offset_to_world(ox, oy);
                        assert!(wx >= b[0] - 0.5 && wx <= b[2] + 0.5 && wz >= b[1] - 0.5 && wz <= b[3] + 0.5, "{p:?} -> {wx},{wz} not in {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_tilted_plane_reaches_the_top_of_the_pill_or_just_short_of_the_horizon() {
        // The HUD pill (208 x 136) with its tilt config: perspective 200, car 85 % down.
        let r = rect(208.0, 136.0);
        for angle in [30.0f32, 55.0, 60.0, 70.0, 80.0] {
            let t = TiltCfg { on: true, angle_deg: angle, ..Default::default() };
            let c = Camera::from_cfg(&t, (0.0, 0.0), 0.3, 300.0, r);
            let horizon = c.row_of_depth_scale(0.0);
            let far = c.far_row();
            assert!(far > horizon && (c.depth_scale_at_row(far) - FAR_MIN_SCALE).abs() < 1e-4, "{angle}");
            // The far limit's projection is the rect's top, or the far row when that is lower.
            let edge = c.project_offset(0.0, -c.far_px()).unwrap().y;
            assert!((edge - far.max(r.top() - 1.0)).abs() < 0.05, "{angle}: edge {edge} far {far} top {}", r.top());
            // The plane's footprint covers all four corners of the pill below the far row.
            let pr = c.plane_rect();
            let top = far.max(r.top());
            for p in [pos2(r.left(), top + 0.01), pos2(r.right(), top + 0.01), r.left_bottom(), r.right_bottom()] {
                let o = c.unproject_offset(p).unwrap();
                assert!(pr.expand(0.01).contains(pos2(o[0], o[1])), "{angle}: {p:?} -> {o:?} not in {pr:?}");
            }
            // At the default 55 deg (and below) the horizon is out of view: the pill is all map.
            if angle <= 55.0 {
                assert!(far < r.top(), "{angle}: far row {far} inside the pill (top {})", r.top());
            }
            if angle >= 70.0 {
                assert!(far > r.top(), "{angle}: the horizon is in view");
            }
        }
    }

    #[test]
    fn camera_uv_matches_the_map_calibration_when_flat() {
        let c = cam(0.9, 0.0, 1.0);
        let cal = MapCalibration::DEFAULT;
        let p = c.centre + vec2(37.0, -12.0);
        let [u, v] = c.view.uv_at_offset(&cal, [8192, 8192], 37.0, -12.0);
        let w = c.unproject(p).unwrap();
        let uv2 = cal.world_to_uv(w[0], w[1], [8192, 8192]);
        assert!((u - uv2[0]).abs() < 1e-6 && (v - uv2[1]).abs() < 1e-6);
    }

    #[test]
    fn from_cfg_is_flat_without_tilt_and_scales_the_perspective_with_the_view_height() {
        let r = rect(208.0, 136.0);
        let flat = Camera::from_cfg(&TiltCfg::default(), (5.0, 6.0), 0.3, 300.0, r);
        assert!(flat.is_flat() && flat.centre == r.center());
        let same = Camera::new(5.0, 6.0, 0.3, 300.0, r, r.center(), 0.0, 1.0);
        assert_eq!(flat.project(40.0, 50.0), same.project(40.0, 50.0));
        // Tilted: 55 deg, the car 85 % down, perspective 200 px at the pill's 136 px height.
        let t = TiltCfg { on: true, ..Default::default() };
        let c = Camera::from_cfg(&t, (0.0, 0.0), 0.0, 300.0, r);
        assert!((c.pitch - 55f32.to_radians()).abs() < 1e-6 && (c.focal - 200.0).abs() < 1e-3, "{c:?}");
        assert!((c.centre.y - (r.top() + 0.85 * 136.0)).abs() < 1e-3);
        // A view 3.97x taller gets a 3.97x longer eye distance: the same picture, bigger.
        let big = rect(540.0, 540.0);
        let cb = Camera::from_cfg(&t, (0.0, 0.0), 0.0, 300.0, big);
        assert!((cb.focal - 200.0 * 540.0 / 136.0).abs() < 1e-2);
        // The projection of a point at the same relative spot scales with the view.
        let p = c.project(0.0, 300.0).unwrap();
        let pb = cb.project(0.0, 300.0).unwrap();
        assert!(((c.centre.y - p.y) / 136.0 - (cb.centre.y - pb.y) / 540.0).abs() < 1e-3);
        // Out-of-range angles are clamped.
        let steep = Camera::from_cfg(&TiltCfg { on: true, angle_deg: 200.0, ..Default::default() }, (0.0, 0.0), 0.0, 300.0, r);
        assert!((steep.pitch - 80f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn depth_scale_by_row_equals_the_perspective_of_the_plane_point() {
        let c = cam(0.3, 55f32.to_radians(), 200.0);
        for oy in [-300.0f32, -120.0, -10.0, 0.0, 15.0, 60.0] {
            let Some(p) = c.project_offset(0.0, oy) else { continue };
            assert!((c.depth_scale_at_row(p.y) - c.perspective_at(oy)).abs() < 1e-4, "oy {oy}");
        }
        assert_eq!(c.depth_scale_at_row(c.centre.y), 1.0);
        assert!(c.depth_scale_at_row(c.rect.top()) < 1.0 && c.depth_scale_at_row(c.rect.bottom()) > 1.0);
        assert_eq!(cam(0.3, 0.0, 1.0).depth_scale_at_row(0.0), 1.0, "flat");
    }

    // ── 3D relief camera (phase K) ──────────────────────────────────────────────────────────

    /// A tiny deterministic generator for the property tests (no rand crate).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f32) / (1u64 << 31) as f32
        }
        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    fn relief_cam(yaw: f32, pitch: f32, focal: f32, terrain: Terrain, exag: f32, car_y: f32) -> Camera {
        cam(yaw, pitch, focal).with_relief(Relief::new(Arc::new(terrain), exag, car_y))
    }

    /// The 3D camera at height 0 IS the flat tilt (D65): the car stays on the same screen point
    /// at the same ground scale when switching Tilted <-> 3D. Max difference over 2000 random
    /// points, yaws, pitches and eye distances: below 1e-3 px (the prototype measured 1.5e-5).
    #[test]
    fn project3_at_height_zero_equals_project_offset() {
        let mut rng = Lcg(7);
        let mut worst = 0.0f32;
        let mut n = 0;
        for _ in 0..2000 {
            let (yaw, pitch, focal) = (rng.range(0.0, 6.28), rng.range(5.0f32, 80.0).to_radians(), rng.range(60.0, 900.0));
            let plain = cam(yaw, pitch, focal);
            // The car is at 321 m and the points too: height 0 above the car's plane, whatever the exaggeration.
            let c3 = plain.clone().with_relief(Relief::new(Arc::new(Terrain::flat(321.0)), rng.range(0.5, 3.0), 321.0));
            let (wx, wz) = (1000.0 + rng.range(-3000.0, 3000.0), -2000.0 + rng.range(-3000.0, 3000.0));
            let [ox, oy] = plain.view.world_to_offset(wx, wz);
            let a = plain.project_offset(ox, oy);
            let b = c3.project3(wx, 321.0, wz).map(|p| p.0);
            let b2 = c3.project(wx, wz); // flat terrain at the car's height = the same plane
            match (a, b) {
                (Some(a), Some(b)) => {
                    worst = worst.max((a - b).length());
                    worst = worst.max((a - b2.unwrap()).length());
                    n += 1;
                }
                (None, None) => {}
                (a, b) => panic!("visibility differs: {a:?} vs {b:?}"),
            }
        }
        assert!(n > 1500, "{n} comparable points");
        assert!(worst < 1e-3, "worst difference {worst} px");
    }

    /// A plain camera without a relief (the 2D path) projects heights as if the car stood at y = 0.
    #[test]
    fn project3_without_a_relief_uses_the_ground_plane_at_zero() {
        let c = cam(0.4, 55f32.to_radians(), 200.0);
        let p0 = c.project(1100.0, -1900.0).unwrap();
        let p3 = c.project3(1100.0, 0.0, -1900.0).unwrap().0;
        assert!((p0 - p3).length() < 1e-3);
        // Higher things appear higher on a tilted screen; the car's own column rises straight up.
        let up = c.project3(1000.0, 50.0, -2000.0).unwrap().0;
        assert!(up.y < c.centre.y && (up.x - c.centre.x).abs() < 1e-3, "{up:?}");
    }

    /// The matrices the GL renderer / culling get equal the direct path, with exaggeration, a
    /// raised car, a viewport offset and pixels_per_point. The car-relative one (what vertices
    /// use) is exact to 1e-3 px; the folded one (culling) to 0.05 px beyond the near plane.
    #[test]
    fn view_proj_matrices_equal_project3() {
        let mut rng = Lcg(11);
        let (mut worst_rel, mut worst_folded) = (0.0f32, 0.0f32);
        let mut n = 0;
        // The matrix entries are f32 (as uploaded), the products f64: this tests the matrix, not the GPU's rounding.
        let apply = |m: &[f32; 16], v: [f32; 4]| {
            let mut o = [0.0f32; 4];
            for r in 0..4 {
                let mut acc = 0.0f64;
                for k in 0..4 {
                    acc += m[k * 4 + r] as f64 * v[k] as f64;
                }
                o[r] = acc as f32;
            }
            o
        };
        for ppp in [1.0f32, 1.5, 2.0] {
            for _ in 0..600 {
                let (yaw, pitch, focal) = (rng.range(0.0, 6.28), rng.range(5.0f32, 80.0).to_radians(), rng.range(100.0, 600.0));
                let exag = rng.range(0.5, 3.0);
                let car_y = rng.range(-2.0, 900.0);
                let c = relief_cam(yaw, pitch, focal, Terrain::flat(0.0), exag, car_y);
                let (m_rel, m_fold) = (c.view_proj_rel(ppp), c.view_proj(ppp));
                let (wx, wy, wz) = (1000.0 + rng.range(-1500.0, 1500.0), rng.range(-2.0, 1400.0), -2000.0 + rng.range(-1500.0, 1500.0));
                let car = c.car_exag();
                let o_rel = apply(&m_rel, [wx - car[0], wy * c.exag() - car[1], wz - car[2], 1.0]);
                let o_fold = apply(&m_fold, [wx, wy * c.exag(), wz, 1.0]);
                let Some((direct, cz)) = c.project3(wx, wy, wz) else {
                    assert!(o_rel[3] < 1.0 * ppp + 1e-3, "behind the eye but w = {}", o_rel[3]);
                    continue;
                };
                // clip.w is the camera depth in viewport px.
                assert!((o_rel[3] - cz * ppp).abs() < 1e-2 + 1e-5 * cz * ppp, "w {} vs depth {}", o_rel[3], cz * ppp);
                let (nr, fr) = c.near_far(ppp);
                if o_rel[3] < nr {
                    continue; // in front of the near plane: clipped anyway
                }
                let (w, h) = (c.rect.width() * ppp, c.rect.height() * ppp);
                let to_px = |o: [f32; 4]| vec2((o[0] / o[3] + 1.0) * 0.5 * w, (1.0 - o[1] / o[3]) * 0.5 * h);
                let want = (direct - c.rect.min) * ppp;
                if want.x.abs() > 3.0 * w || want.y.abs() > 3.0 * h {
                    continue; // far off screen: f32 relative error in px grows with the distance
                }
                worst_rel = worst_rel.max((to_px(o_rel) - want).length() / ppp);
                worst_folded = worst_folded.max((to_px(o_fold) - want).length() / ppp);
                // NDC depth is inside [-1, 1] between the near and far planes.
                let z = o_rel[2] / o_rel[3];
                assert_eq!(z.abs() <= 1.0 + 1e-4, o_rel[3] <= fr + 1e-3, "z {z} w {}", o_rel[3]);
                n += 1;
            }
        }
        assert!(n > 1000, "{n}");
        assert!(worst_rel < 1e-3, "relative matrix vs direct: {worst_rel} px");
        assert!(worst_folded < 0.05, "folded matrix vs direct: {worst_folded} px");
    }

    #[test]
    fn project_follows_the_terrain_surface_and_hills_rise_on_screen() {
        let t = Terrain::synthetic();
        let car_y = t.height(0.0, 0.0);
        let c = Camera::new(0.0, 0.0, 0.0, 300.0, rect(208.0, 136.0), Camera::tilt_centre(rect(208.0, 136.0), 0.85), 45f32.to_radians(), 200.0)
            .with_relief(Relief::new(Arc::new(t), 1.0, car_y));
        // project(x, z) = project3 at the terrain height.
        let (x, z) = (-300.0, 200.0);
        let h = c.relief.as_ref().unwrap().terrain.height(x, z);
        assert_eq!(c.project(x, z), c.project3(x, h, z).map(|p| p.0));
        // The 220 m hill's top is drawn higher up the screen than the same spot on the plain,
        // and nearer to the eye (bigger icons) than the flat ground there.
        let top = c.project(x, z).unwrap();
        let flat = c.project3(x, car_y, z).unwrap().0;
        assert!(top.y < flat.y - 10.0, "{top:?} vs {flat:?}");
        assert!(c.k_at(x, z) > c.perspective_at(c.view.world_to_offset(x, z)[1]));
        // Exaggeration scales it: twice the heights, twice the lift above the plane.
        let c2 = relief_cam(0.0, 45f32.to_radians(), 200.0, Terrain::synthetic(), 2.0, car_y);
        let c1 = relief_cam(0.0, 45f32.to_radians(), 200.0, Terrain::synthetic(), 1.0, car_y);
        let p = |c: &Camera| c.project3(1000.0, car_y + 40.0, -2000.0 + 150.0).unwrap().0.y;
        assert!(p(&c2) < p(&c1), "{} vs {}", p(&c2), p(&c1));
    }

    #[test]
    fn k_at_is_the_perspective_factor_of_the_ground_point() {
        // Without a relief it is the old perspective_at of the plane point.
        let c = cam(0.3, 55f32.to_radians(), 200.0);
        for (wx, wz) in [(1000.0, -2000.0), (1100.0, -1800.0), (900.0, -2300.0)] {
            let oy = c.view.world_to_offset(wx, wz)[1];
            assert!((c.k_at(wx, wz) - c.perspective_at(oy)).abs() < 1e-5);
        }
        // On a flat relief at the car's height too; 1 at the car; smaller ahead.
        let r = relief_cam(0.3, 55f32.to_radians(), 200.0, Terrain::flat(50.0), 1.0, 50.0);
        assert!((r.k_at(1000.0, -2000.0) - 1.0).abs() < 1e-5);
        assert!(r.k_at(1000.0 + 300.0 * 0.3f32.sin(), -2000.0 + 300.0 * 0.3f32.cos()) < 0.9);
        // Behind the eye: 0.
        assert_eq!(r.k_at(1000.0 - 40_000.0 * 0.3f32.sin(), -2000.0 - 40_000.0 * 0.3f32.cos()), 0.0);
    }

    /// The eye is where the camera depth is zero: `P / scale` m from the car, up `cos a`, back `sin a`.
    #[test]
    fn eye_is_the_pinhole_and_sits_behind_and_above_the_car() {
        let (yaw, pitch) = (0.7f32, 40f32.to_radians());
        let c = relief_cam(yaw, pitch, 200.0, Terrain::flat(0.0), 1.5, 120.0);
        let e = c.eye();
        let d = 200.0 / c.view.scale;
        assert!((e[1] - (120.0 + d * pitch.cos() / 1.5)).abs() < 1e-2, "{e:?}");
        let back = ((e[0] - 1000.0).powi(2) + (e[2] + 2000.0).powi(2)).sqrt();
        assert!((back - d * pitch.sin()).abs() < 1e-2, "{back}");
        // Behind: the eye is opposite to the heading (sin yaw, cos yaw).
        assert!(((e[0] - 1000.0) * yaw.sin() + (e[2] + 2000.0) * yaw.cos()) < 0.0);
        // Camera depth at the eye is zero (clip.w of the matrix).
        let m = c.view_proj_rel(1.0);
        let car = c.car_exag();
        let w = m[3] * (e[0] - car[0]) + m[7] * (e[1] * 1.5 - car[1]) + m[11] * (e[2] - car[2]) + m[15];
        assert!(w.abs() < 0.05, "depth at the eye {w}");
        // Without a relief: car at 0.
        let flat = cam(0.0, pitch, 200.0).eye();
        assert!((flat[1] - 200.0 * pitch.cos() / cam(0.0, pitch, 200.0).view.scale).abs() < 1e-2);
    }

    #[test]
    fn eye_clearance_lowers_the_pitch_only_when_the_eye_would_be_inside_a_hill() {
        let t = Arc::new(Terrain::synthetic());
        // The car at the foot of the 220 m hill (peak at -300, 200), heading south (yaw pi), so the
        // eye 120 m "behind" it lies north, over the hill: at pitch 55 it is underground.
        let (cx, cz) = (-300.0, 40.0);
        let car_y = t.height(cx, cz) + 1.0;
        let mk = |pitch_deg: f32| {
            Camera::new(cx, cz, std::f32::consts::PI, 50.0, rect(208.0, 136.0), Camera::tilt_centre(rect(208.0, 136.0), 0.85), pitch_deg.to_radians(), 200.0)
                .with_relief(Relief::new(t.clone(), 1.0, car_y))
        };
        let blocked = mk(55.0);
        let e = blocked.eye();
        assert!(e[1] < t.height(e[0], e[2]) + 3.0, "the scenario must start blocked: eye {e:?} ground {}", t.height(e[0], e[2]));
        assert!(!blocked.eye_clear(3.0));
        let fixed = blocked.clone().with_eye_clearance(3.0);
        assert!(fixed.eye_clear(3.0), "still blocked at pitch {}", fixed.pitch.to_degrees());
        assert!(fixed.pitch < blocked.pitch && fixed.pitch >= MIN_PITCH, "{}", fixed.pitch.to_degrees());
        // It stops as soon as it clears: not the minimum when a little less is enough.
        assert!(fixed.pitch > MIN_PITCH + 0.01, "{}", fixed.pitch.to_degrees());
        // A camera that clears is returned unchanged, and so is one without a relief.
        let open = Camera::new(500.0, -400.0, 0.0, 50.0, rect(208.0, 136.0), Camera::tilt_centre(rect(208.0, 136.0), 0.85), 55f32.to_radians(), 200.0)
            .with_relief(Relief::new(t.clone(), 1.0, t.height(500.0, -400.0) + 1.0));
        assert!(open.eye_clear(3.0));
        assert_eq!(open.clone().with_eye_clearance(3.0).pitch, open.pitch);
        let plain = cam(0.0, 55f32.to_radians(), 200.0);
        assert_eq!(plain.clone().with_eye_clearance(3.0).pitch, plain.pitch);
        // A hill between the car and an otherwise high eye blocks the line of sight.
        let sight = Camera::new(-300.0, -60.0, std::f32::consts::PI, 50.0, rect(208.0, 136.0), Camera::tilt_centre(rect(208.0, 136.0), 0.85), 70f32.to_radians(), 700.0)
            .with_relief(Relief::new(t.clone(), 1.0, t.height(-300.0, -60.0) + 1.0));
        assert!(sight.eye()[1] > t.height(sight.eye()[0], sight.eye()[2]) + 3.0);
        assert!(!sight.eye_clear(3.0) && sight.with_eye_clearance(3.0).eye_clear(3.0));
    }

    /// Everything the camera can show on `rect` lies inside `footprint` — also over hills.
    #[test]
    fn footprint_contains_every_visible_ground_point() {
        let t = Arc::new(Terrain::synthetic());
        let mut rng = Lcg(3);
        for (zoom, pitch, yaw) in [(300.0f32, 55f32, 0.4f32), (500.0, 40.0, 2.0), (120.0, 70.0, 5.0), (800.0, 25.0, 1.0)] {
            let r = rect(208.0, 136.0);
            let c = Camera::new(-100.0, 50.0, yaw, zoom, r, Camera::tilt_centre(r, 0.85), pitch.to_radians(), 200.0)
                .with_relief(Relief::new(t.clone(), 1.5, t.height(-100.0, 50.0) + 1.0));
            let b = c.footprint(0.0);
            assert!(b[0] < b[2] && b[1] < b[3]);
            let mut inside = 0;
            for _ in 0..4000 {
                let (x, z) = (rng.range(-1000.0, 1000.0), rng.range(-1000.0, 1000.0));
                let Some((p, cz)) = c.project3(x, t.height(x, z), z) else { continue };
                if r.contains(p) && c.focal / cz >= FAR_MIN_SCALE {
                    inside += 1;
                    assert!(x >= b[0] - 1.0 && x <= b[2] + 1.0 && z >= b[1] - 1.0 && z <= b[3] + 1.0, "zoom {zoom} pitch {pitch}: ({x},{z}) -> {p:?} outside {b:?}");
                }
            }
            assert!(inside > 50, "zoom {zoom}: only {inside} visible samples");
            // The margin grows it on every side.
            let m = c.footprint(25.0);
            assert!((b[0] - m[0] - 25.0).abs() < 1e-3 && (m[3] - b[3] - 25.0).abs() < 1e-3);
        }
        // Without a relief it is the flat world box.
        let plain = cam(0.4, 55f32.to_radians(), 200.0);
        assert_eq!(plain.footprint(10.0), plain.world_aabb(10.0));
    }

    #[test]
    fn from_cfg_relief_carries_the_relief_only_when_3d_is_on_and_the_terrain_is_there() {
        let r = rect(208.0, 136.0);
        let t = Arc::new(Terrain::synthetic());
        let mut tilt = TiltCfg { on: true, ..Default::default() };
        // Tilted: no relief, even with a terrain at hand.
        assert!(Camera::from_cfg_relief(&tilt, (0.0, 0.0), 0.0, 300.0, r, Some(&t), None).relief.is_none());
        tilt.relief.on = true;
        tilt.relief.exaggeration = 2.0;
        // 3D but the terrain still loads: the tilted camera (the 2D fallback).
        assert!(Camera::from_cfg_relief(&tilt, (0.0, 0.0), 0.0, 300.0, r, None, None).relief.is_none());
        // 3D with terrain: relief, the config's exaggeration, car height from the terrain unless given.
        let c = Camera::from_cfg_relief(&tilt, (-300.0, 200.0), 0.0, 300.0, r, Some(&t), None);
        let rel = c.relief.as_ref().unwrap();
        assert_eq!(rel.exag, 2.0);
        assert!((rel.car_y - t.height(-300.0, 200.0)).abs() < 1e-4);
        let c = Camera::from_cfg_relief(&tilt, (0.0, 0.0), 0.0, 300.0, r, Some(&t), Some(123.0));
        assert_eq!(c.relief.as_ref().unwrap().car_y, 123.0);
        // The pitch / focal / centre are exactly the tilted camera's.
        let plain = Camera::from_cfg(&tilt, (0.0, 0.0), 0.0, 300.0, r);
        assert_eq!((c.pitch, c.focal, c.centre), (plain.pitch, plain.focal, plain.centre));
        // Off entirely: flat.
        tilt.on = false;
        assert!(Camera::from_cfg_relief(&tilt, (0.0, 0.0), 0.0, 300.0, r, Some(&t), None).relief.is_none());
    }

    #[test]
    fn bbox_hits_basics() {
        let a = [0.0, 0.0, 10.0, 10.0];
        assert!(bbox_hits(&a, &[10.0, 10.0, 20.0, 20.0]));
        assert!(!bbox_hits(&a, &[10.1, 0.0, 20.0, 5.0]));
    }

    fn path(n: usize) -> Vec<Pos2> {
        (0..n).map(|i| pos2(i as f32 * 0.7, (i as f32 * 0.31).sin() * 3.0)).collect()
    }

    #[test]
    fn thin_keeps_first_and_last_and_is_idempotent() {
        for min in [0.5f32, 2.0, 5.0, 40.0] {
            let orig = path(200);
            let mut a = orig.clone();
            thin(&mut a, min);
            assert_eq!(a[0], orig[0]);
            assert_eq!(*a.last().unwrap(), *orig.last().unwrap());
            assert!(a.len() >= 2 && a.len() <= orig.len());
            let mut b = a.clone();
            thin(&mut b, min);
            assert_eq!(a, b, "min {min}");
            // Spacing honours the minimum (a two-point result is just first and last).
            if a.len() > 2 {
                for w in a.windows(2) {
                    assert!((w[1] - w[0]).length() >= min - 1e-4, "min {min}: {w:?}");
                }
            }
        }
        let mut two = vec![pos2(0.0, 0.0), pos2(0.1, 0.0)];
        thin(&mut two, 50.0);
        assert_eq!(two.len(), 2);
        let mut closed = vec![pos2(0.0, 0.0), pos2(0.2, 0.0), pos2(0.4, 0.0)];
        thin(&mut closed, 1.0);
        assert_eq!(closed, vec![pos2(0.0, 0.0), pos2(0.4, 0.0)]);
    }

    fn sq(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<Pos2> {
        vec![pos2(x0, y0), pos2(x1, y0), pos2(x1, y1), pos2(x0, y1)]
    }

    fn area(p: &[Pos2]) -> f32 {
        (0..p.len()).map(|i| p[i].x * p[(i + 1) % p.len()].y - p[(i + 1) % p.len()].x * p[i].y).sum::<f32>().abs() / 2.0
    }

    #[test]
    fn clip_convex_cuts_a_square_to_the_overlap_either_winding() {
        let subject = sq(0.0, 0.0, 10.0, 10.0);
        let clip = sq(5.0, 5.0, 15.0, 15.0);
        assert!((area(&clip_convex(&subject, &clip)) - 25.0).abs() < 1e-3);
        let rev: Vec<Pos2> = clip.iter().rev().copied().collect();
        assert!((area(&clip_convex(&subject, &rev)) - 25.0).abs() < 1e-3);
        assert!((area(&clip_convex(&subject, &sq(-5.0, -5.0, 50.0, 50.0))) - 100.0).abs() < 1e-3);
        assert!(clip_convex(&subject, &sq(20.0, 20.0, 30.0, 30.0)).is_empty());
    }

    #[test]
    fn clip_segment_and_polyline_against_a_convex_polygon() {
        let clip = sq(0.0, 0.0, 10.0, 10.0);
        let (p, q) = clip_segment_convex(pos2(-5.0, 5.0), pos2(15.0, 5.0), &clip).unwrap();
        assert!((p.x - 0.0).abs() < 1e-4 && (q.x - 10.0).abs() < 1e-4);
        assert!(clip_segment_convex(pos2(-5.0, -5.0), pos2(-1.0, 20.0), &clip).is_none());
        let inside = clip_segment_convex(pos2(2.0, 2.0), pos2(3.0, 4.0), &clip).unwrap();
        assert_eq!(inside, (pos2(2.0, 2.0), pos2(3.0, 4.0)));
        // Either winding.
        let rev: Vec<Pos2> = clip.iter().rev().copied().collect();
        assert!(clip_segment_convex(pos2(-5.0, 5.0), pos2(15.0, 5.0), &rev).is_some());
        // Leaves and re-enters: two pieces.
        let pl = [pos2(2.0, 5.0), pos2(20.0, 5.0), pos2(20.0, 8.0), pos2(5.0, 8.0)];
        let parts = clip_polyline_convex(&pl, &clip);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert!((parts[0].last().unwrap().x - 10.0).abs() < 1e-4);
        assert!((parts[1][0].x - 10.0).abs() < 1e-4 && (parts[1].last().unwrap().x - 5.0).abs() < 1e-4);
        // A corner cut by a chamfer polygon.
        let chamfer = vec![pos2(5.0, 0.0), pos2(10.0, 0.0), pos2(10.0, 10.0), pos2(0.0, 10.0), pos2(0.0, 5.0)];
        assert!(clip_polyline_convex(&[pos2(0.5, 0.5), pos2(1.5, 1.5)], &chamfer).is_empty());
        assert!(inside_convex(pos2(5.0, 5.0), &chamfer) && !inside_convex(pos2(1.0, 1.0), &chamfer));
    }

    #[test]
    fn fan_appends_to_an_existing_mesh() {
        let mut m = Mesh::default();
        let v = |p: Pos2| Vertex { pos: p, uv: pos2(0.0, 0.0), color: egui::Color32::WHITE };
        fan(&mut m, pos2(5.0, 5.0), &sq(0.0, 0.0, 10.0, 10.0), v);
        fan(&mut m, pos2(5.0, 5.0), &sq(0.0, 0.0, 10.0, 10.0), v);
        assert_eq!(m.vertices.len(), 10);
        assert_eq!(m.indices.len(), 24);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        assert_eq!(m.indices[12], 5);
    }
}
