//! Projection and geometry helpers of the shared map renderer. Pure functions on plain values
//! (no `Painter`), unit-tested.
//!
//! [`Camera`] is the one projection both renderers use: the 2D one here (flat map, optionally
//! tilted) and phase K's GL 3D scene. At pitch 0 it is exactly [`MapView`]'s mapping; with
//! pitch it is a *flat perspective of that map* (D65): the map plane is turned about the
//! horizontal axis through the car and seen by an eye `focal` px above the car, which is what
//! the user's demo does with CSS `perspective(P) rotateX(a)` (that CSS transform is the
//! reference, `docs/features/minimap.md`). Heights are not part of it yet (phase K adds `y`).

use egui::epaint::Vertex;
use egui::{pos2, vec2, Mesh, Pos2, Rect};

use super::cfg::TiltCfg;
use crate::minimap::MapView;

// ── camera ───────────────────────────────────────────────────────────────────────────────────

/// Height of the view the tilt settings are written for: the HUD pill, 136 design px.
pub const REF_VIEW_H: f32 = 136.0;

/// A point must be at least this far (px) in front of the eye to be projected.
const EYE_EPS: f32 = 1.0;
/// How far ahead of the car (in multiples of the view height, plane px) the tilted map ends:
/// the demo's canvas is 4 view heights tall with the car 80 % down it, so 3.2.
pub const FAR_VIEW_HEIGHTS: f32 = 3.2;

/// Car-centred camera over the map plane. Screen space = egui points.
#[derive(Clone, Copy, Debug)]
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
}

impl Camera {
    /// `zoom_m` = metres from the centre to the nearest edge of the *untilted* map, as in
    /// [`MapView::new`] (`view_px` = the rect's smaller side).
    #[allow(clippy::too_many_arguments)]
    pub fn new(car_x: f32, car_z: f32, yaw: f32, zoom_m: f32, rect: Rect, centre: Pos2, pitch: f32, focal: f32) -> Camera {
        let view = MapView::new(car_x, car_z, yaw, zoom_m, rect.width().min(rect.height()));
        Camera { view, centre, rect, pitch, focal: focal.max(1.0) }
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

    /// World (x, z) → screen.
    pub fn project(&self, wx: f32, wz: f32) -> Option<Pos2> {
        let [ox, oy] = self.view.world_to_offset(wx, wz);
        self.project_offset(ox, oy)
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

    /// How far the tilted plane extends ahead of the car (px).
    pub fn far_px(&self) -> f32 {
        FAR_VIEW_HEIGHTS * self.rect.height()
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
