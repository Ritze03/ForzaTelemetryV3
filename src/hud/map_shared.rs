//! Map markers drawn identically by the Dashboard Map widget and the HUD Minimap: the own
//! arrow, co-op teammate arrows / edge markers / names, breadcrumb trails and shared
//! waypoints. *Why one implementation:* the user wants the HUD to look like the Dashboard
//! map, and a single drawing path means the two can't drift apart (as `draw_compass`, in
//! `hud/minimap.rs`, already does for the compass).
//!
//! Everything is drawn through a [`MapCanvas`]: the painter, the [`Camera`] (the same one the
//! shared layer renderer uses, so a tilted map's trails, teammates and waypoints sit on the
//! tilted roads; pitch 0 is the plain [`crate::minimap::MapView`] mapping), the screen bounds, a
//! size factor (`s`: 1.0 on the Dashboard, the HUD's design→screen scale) and a fade alpha
//! (`a`: 1.0 on the Dashboard, the HUD's show/hide fade). Markers stay upright (arrows are not
//! tilted); only their positions, and the trail widths, follow the perspective.

use std::time::Instant;

use egui::{pos2, vec2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};

use crate::maprender::gl3d::{Marker3d, GROUND_BELOW_M};
use crate::maprender::Camera;
use crate::minimap::Trail;

/// Where and how to draw. Cheap to build per frame.
pub struct MapCanvas<'a> {
    pub p: &'a Painter,
    /// The map's camera; its `centre` is where the car is on screen.
    pub cam: &'a Camera,
    /// Bounds markers stay in: the Dashboard widget rect, the HUD pill's rect.
    pub rect: Rect,
    /// Trail widths shrink towards a tilted map's far edge (`TiltCfg::taper`).
    pub taper: bool,
    /// Size factor for strokes, arrows and text.
    pub s: f32,
    /// Alpha applied to every colour.
    pub a: f32,
    /// Prefix of a paused player's name (the Dashboard has the icon font, the HUD doesn't).
    pub pause_glyph: &'static str,
}

impl MapCanvas<'_> {
    /// World (x, z) → screen through the camera. A point that is not in front of a tilted camera
    /// (far behind the car) goes far off-screen in its flat direction, so the edge markers still
    /// point the right way.
    pub fn to_screen(&self, wx: f32, wz: f32) -> Pos2 {
        // `Camera::project` is the plane maths of a flat / tilted camera and the terrain-surface
        // point of a 3D one (phase K), so teammates, trails and waypoints sit on the relief.
        self.cam.project(wx, wz).unwrap_or_else(|| self.off_screen(wx, wz))
    }
    /// Far off-screen in the flat direction of (x, z) from the car (for points the camera cannot
    /// project).
    fn off_screen(&self, wx: f32, wz: f32) -> Pos2 {
        let [ox, oy] = self.cam.view.world_to_offset(wx, wz);
        self.cam.centre + vec2(ox, oy).normalized() * 1.0e5
    }
    /// [`MapCanvas::to_screen`] for something that stands at a real height `y` (a teammate's
    /// telemetry height): in the 3D view it is projected at that height (minus the car-centre
    /// offset, like the own car marker: road level), so a teammate in a tunnel shows down at the
    /// tunnel and not on the hill above it. `None` height, a non-finite one, and every flat /
    /// tilted camera give exactly [`MapCanvas::to_screen`]. *Why:* D77 did this for the own car;
    /// teammates were still projected onto the terrain surface.
    pub fn to_screen_at(&self, wx: f32, wz: f32, y: Option<f32>) -> Pos2 {
        if let (Some(_), Some(y)) = (&self.cam.relief, y.filter(|y| y.is_finite())) {
            let ground = y - GROUND_BELOW_M;
            if let Some((p, _)) = self.cam.project3(wx, ground, wz) {
                return p;
            }
            return self.off_screen(wx, wz);
        }
        self.to_screen(wx, wz)
    }
    fn c(&self, c: Color32) -> Color32 {
        c.gamma_multiply(self.a)
    }
    /// Is `at` inside the bounds, `margin` in? The bounds are `rect`, or with `round` the circle
    /// inscribed in it (the round Minimap).
    fn within(&self, at: Pos2, margin: f32, round: bool) -> bool {
        if round {
            (at - self.rect.center()).length() <= self.rect.width().min(self.rect.height()) / 2.0 - margin
        } else {
            self.rect.shrink(margin).contains(at)
        }
    }
    /// `d` (a point relative to the centre) pulled onto the bounds `margin` in when it is beyond
    /// them. Rectangles clamp to the box along the ray, circles to the radius.
    fn pin(&self, d: Vec2, margin: f32, round: bool) -> Vec2 {
        if round {
            let r = (self.rect.width().min(self.rect.height()) / 2.0 - margin).max(0.0);
            return if d.length() > r { d.normalized() * r } else { d };
        }
        let half = self.rect.size() * 0.5 - Vec2::splat(margin);
        let kx = if d.x.abs() > 0.01 { half.x / d.x.abs() } else { f32::INFINITY };
        let ky = if d.y.abs() > 0.01 { half.y / d.y.abs() } else { f32::INFINITY };
        d * kx.min(ky).min(1.0)
    }
    fn black(&self, alpha: u8) -> Color32 {
        self.c(Color32::from_black_alpha(alpha))
    }
}

/// Half-width of the heading arrow in canvas units (`s` = 1).
const ARROW: f32 = 7.0;

/// The arrow's three points at `at`, turned `angle` radians clockwise (0 = apex up): apex,
/// right, left.
pub fn arrow_points(at: Pos2, angle: f32, s: f32) -> [Pos2; 3] {
    let u = ARROW * s;
    let (sa, ca) = angle.sin_cos();
    let rr = |vx: f32, vy: f32| pos2(at.x + vx * ca - vy * sa, at.y + vx * sa + vy * ca);
    [rr(0.0, -u * 1.4), rr(u, u * 0.6), rr(-u, u * 0.6)]
}

fn arrow(cv: &MapCanvas, at: Pos2, angle: f32, col: Color32) {
    cv.p.add(egui::Shape::convex_polygon(
        arrow_points(at, angle, cv.s).to_vec(),
        cv.c(col),
        Stroke::new(1.5 * cv.s, cv.black(255)),
    ));
}

/// The own car: the heading arrow at the view centre, `angle` = `MapView::arrow_angle(raw yaw)`.
/// `col` is the player's co-op colour in a session, white otherwise.
pub fn draw_own_arrow(cv: &MapCanvas, angle: f32, col: Color32) {
    arrow(cv, cv.cam.centre, angle, col);
}

/// A co-op teammate to draw. A paused one is passed at its last known spot (the caller keeps
/// that; a paused packet sits at the world origin).
#[derive(Clone, Debug)]
pub struct Remote {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub z: f32,
    /// The telemetry height in metres (a paused one: its last known). Used by the 3D view to put
    /// the marker where the car is (a tunnel) instead of on the terrain surface; `None` = on the
    /// terrain. Flat and tilted views ignore it.
    pub y: Option<f32>,
    pub yaw: f32,
    pub colour: Color32,
    pub paused: bool,
}

/// How a trail fades: by age or by distance behind the player, whichever comes first.
#[derive(Clone, Copy, Debug)]
pub struct TrailFade {
    pub secs: f32,
    pub metres: f32,
}

impl TrailFade {
    pub fn new(secs: f32, metres: f32) -> Self {
        Self { secs: secs.max(0.5), metres: metres.max(1.0) }
    }

    /// Alpha (0..=255) of the segment ending at a point recorded `age` seconds ago whose
    /// start is `dist` metres from the trail's head.
    pub fn alpha(&self, age: f32, dist: f32) -> u8 {
        let tf = (1.0 - age / self.secs).clamp(0.0, 1.0);
        let df = (1.0 - dist / self.metres).clamp(0.0, 1.0);
        (tf.min(df) * 220.0) as u8
    }
}

/// A player's breadcrumb trail in their colour, faint (old / far behind) to solid (recent).
pub fn draw_trail(cv: &MapCanvas, pts: &Trail, col: Color32, fade: TrailFade, now: Instant) {
    draw_trail_in(cv, pts, col, fade, now, false);
}

/// [`draw_trail`]; with `round` the segments are cut to the circle inscribed in `cv.rect` (the
/// round Minimap: a painter clip rect can only cut a rectangle).
pub fn draw_trail_in(cv: &MapCanvas, pts: &Trail, col: Color32, fade: TrailFade, now: Instant, round: bool) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let head = pts[n - 1]; // the player's current position
    for i in 1..n {
        let (ax, az) = (pts[i - 1].x, pts[i - 1].z);
        let (bx, bz, bt) = (pts[i].x, pts[i].z, pts[i].t);
        let alpha = fade.alpha(now.saturating_duration_since(bt).as_secs_f32(), (ax - head.x).hypot(az - head.z));
        if alpha < 4 {
            continue;
        }
        let c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);
        let (mut a, mut b) = (cv.to_screen(ax, az), cv.to_screen(bx, bz));
        if round {
            let (c, r) = (cv.rect.center(), cv.rect.width().min(cv.rect.height()) / 2.0);
            match clip_segment_circle(a, b, c, r) {
                Some((ca, cb)) => (a, b) = (ca, cb),
                None => continue,
            }
        }
        let k = if cv.taper { cv.cam.depth_scale_at_row((a.y + b.y) * 0.5).clamp(0.4, 1.5) } else { 1.0 };
        cv.p.line_segment([a, b], Stroke::new(2.0 * cv.s * k, cv.c(c)));
    }
}

/// A player's trail for the 3D scene (D77): the same segments, colour and fade as
/// [`draw_trail`], but at the recorded heights, drawn by the GL renderer as a ribbon (so one that
/// went through a tunnel stays in it instead of running over the hill). Segments that have faded
/// out are left out; `None` when nothing is left.
pub fn trail_3d(pts: &Trail, col: Color32, fade: TrailFade, now: Instant) -> Option<crate::maprender::gl3d::Trail3d> {
    let n = pts.len();
    if n < 2 {
        return None;
    }
    let head = pts[n - 1];
    let segs: Vec<crate::maprender::gl3d::TrailSeg> = (1..n)
        .filter_map(|i| {
            let (a, b) = (pts[i - 1], pts[i]);
            let alpha = fade.alpha(now.saturating_duration_since(b.t).as_secs_f32(), (a.x - head.x).hypot(a.z - head.z));
            (alpha >= 4).then(|| crate::maprender::gl3d::TrailSeg { a: [a.x, a.y, a.z], b: [b.x, b.y, b.z], alpha: alpha as f32 / 255.0 })
        })
        .collect();
    (!segs.is_empty()).then(|| crate::maprender::gl3d::Trail3d { segs, colour: col })
}

/// The part of segment `a`-`b` inside the circle (`c`, `r`), or `None` when it misses it.
pub fn clip_segment_circle(a: Pos2, b: Pos2, c: Pos2, r: f32) -> Option<(Pos2, Pos2)> {
    let (d, f) = (b - a, a - c);
    let qa = d.dot(d);
    if qa < 1e-9 {
        return (f.length() <= r).then_some((a, b));
    }
    // |f + t d|^2 = r^2, t in [0, 1].
    let (qb, qc) = (f.dot(d), f.dot(f) - r * r);
    let disc = qb * qb - qa * qc;
    if disc < 0.0 {
        return None;
    }
    let sq = disc.sqrt();
    let (t0, t1) = (((-qb - sq) / qa).max(0.0), ((-qb + sq) / qa).min(1.0));
    (t0 <= t1).then(|| (a + d * t0, a + d * t1))
}

/// Text with a 4-direction dark shadow (the Dashboard's label style).
fn shadowed(cv: &MapCanvas, pos: Pos2, align: egui::Align2, text: &str, font: FontId, col: Color32) {
    let shadow = cv.black(200);
    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
        cv.p.text(pos + vec2(dx, dy) * cv.s.max(0.5), align, text, font.clone(), shadow);
    }
    cv.p.text(pos, align, text, font, cv.c(col));
}

fn distance_text(m: f32) -> String {
    if m >= 1000.0 {
        format!("{:.1}km", m / 1000.0)
    } else {
        format!("{m:.0}m")
    }
}

/// The colour a teammate is drawn in: their co-op colour, grey while paused.
fn remote_colour(r: &Remote) -> Color32 {
    if r.paused {
        crate::theme::steel(170)
    } else {
        r.colour
    }
}

/// The teammates for the 3D scene (D89): one [`Marker3d`] of `kind` (the map's Car marker choice,
/// the own car's) per teammate that [`draw_remotes_in`] would draw as an arrow on the map, i.e. the
/// ones inside the bounds; the others have the edge pointer only. At the telemetry position,
/// height and yaw (a teammate without a height stands on the terrain), in their colour (grey while
/// paused). Empty for a flat / tilted camera. Hand the result to `MarkerScene::mates`, and draw the
/// labels with `draw_remotes_in(.., flat_body = false)`.
pub fn remote_markers_3d(cv: &MapCanvas, remotes: &[Remote], kind: crate::maprender::cfg::MarkerStyle, round: bool) -> Vec<Marker3d> {
    let Some(rel) = cv.cam.relief.as_ref() else { return Vec::new() };
    remotes
        .iter()
        .filter(|r| cv.within(cv.to_screen_at(r.x, r.z, r.y), 8.0 * cv.s, round))
        .map(|r| {
            let y = r.y.filter(|y| y.is_finite()).unwrap_or_else(|| rel.terrain.height(r.x, r.z) + GROUND_BELOW_M);
            Marker3d { pos: [r.x, y, r.z], yaw: r.yaw, kind, colour: remote_colour(r) }
        })
        .collect()
}

/// Teammates relative to the car at `car` (world x, z): on the map a heading arrow with the
/// name above (labels nudged apart), off the map a small pointer on the edge with the distance.
/// Paused ones are grey with the pause glyph. `map_yaw` is the view's yaw (arrows turn by
/// `yaw - map_yaw`). `flat_body`: draw the arrow (flat / tilted maps, and 3D while the scene is
/// not ready); `false` = the scene draws the body ([`remote_markers_3d`]) and only the name and the
/// edge pointer are drawn here.
pub fn draw_remotes(cv: &MapCanvas, remotes: &[Remote], car: (f32, f32), map_yaw: f32, flat_body: bool) {
    draw_remotes_in(cv, remotes, car, map_yaw, false, flat_body);
}

/// [`draw_remotes`]; with `round` the map's bounds are the circle inscribed in `cv.rect` (the
/// pointers pin to the circle, not the box).
pub fn draw_remotes_in(cv: &MapCanvas, remotes: &[Remote], car: (f32, f32), map_yaw: f32, round: bool, flat_body: bool) {
    let s = cv.s;
    // Names are drawn in a second pass so labels of cars close together (racing side by
    // side) can be nudged apart instead of stacking illegibly.
    let mut labels: Vec<(Pos2, String, Color32)> = Vec::new();
    for r in remotes {
        let at = cv.to_screen_at(r.x, r.z, r.y);
        let col = remote_colour(r);
        if cv.within(at, 8.0 * s, round) {
            if flat_body {
                arrow(cv, at, r.yaw - map_yaw, col);
            }
            let label = if r.paused { format!("{} {}", cv.pause_glyph, r.name) } else { r.name.clone() };
            // (a 3D model is at least 20 pt long and its nose points up the screen: the name sits higher)
            let lift = if flat_body { 7.0 * 1.9 } else { 7.0 * 2.5 };
            labels.push((pos2(at.x, at.y - lift * s), label, col));
        } else {
            // Off the map: clamp to the edge and point toward them.
            let c = cv.rect.center();
            let d = at - c;
            let edge = c + cv.pin(d, 10.0 * s, round);
            let (sa, ca) = d.y.atan2(d.x).sin_cos();
            let m = 6.5 * s;
            let rr = |vx: f32, vy: f32| pos2(edge.x + vx * ca - vy * sa, edge.y + vx * sa + vy * ca);
            cv.p.add(egui::Shape::convex_polygon(
                vec![rr(m, 0.0), rr(-m * 0.7, m * 0.7), rr(-m * 0.7, -m * 0.7)],
                cv.c(col),
                Stroke::new(1.0 * s, cv.black(255)),
            ));
            let dist = (r.x - car.0).hypot(r.z - car.1);
            let dpos = edge - d.normalized() * 14.0 * s;
            shadowed(cv, dpos, egui::Align2::CENTER_CENTER, &distance_text(dist), FontId::proportional(10.0 * s), col);
        }
    }
    let font = FontId::proportional(11.0 * s);
    let mut placed: Vec<Pos2> = Vec::new();
    for (mut pos, name, col) in labels {
        while placed.iter().any(|p| (p.x - pos.x).abs() < 46.0 * s && (p.y - pos.y).abs() < 13.0 * s) {
            pos.y += 13.0 * s;
        }
        placed.push(pos);
        shadowed(cv, pos, egui::Align2::CENTER_BOTTOM, &name, font.clone(), col);
    }
}

/// A shared waypoint: a pulsing diamond (clamped to the edge when off the map) with the
/// distance from `car` above it. `colour` is the setter's identity colour, `time` drives the pulse.
pub fn draw_waypoint(cv: &MapCanvas, wp: (f32, f32), colour: Color32, car: (f32, f32), time: f32) {
    draw_waypoint_in(cv, wp, colour, car, time, false);
}

/// [`draw_waypoint`] with `round` bounds (see [`draw_remotes_in`]).
pub fn draw_waypoint_in(cv: &MapCanvas, (wx, wz): (f32, f32), colour: Color32, car: (f32, f32), time: f32, round: bool) {
    let s = cv.s;
    let mut at = cv.to_screen(wx, wz);
    if !cv.within(at, 6.0 * s, round) {
        let d = at - cv.rect.center();
        at = cv.rect.center() + cv.pin(d, 8.0 * s, round);
    }
    // Gentle pulse to draw the eye to the destination.
    let p = (1.0 + 0.16 * (time * 4.0).sin()) * s;
    cv.p.add(egui::Shape::convex_polygon(
        vec![at + vec2(0.0, -9.0 * p), at + vec2(7.0 * p, 0.0), at + vec2(0.0, 9.0 * p), at + vec2(-7.0 * p, 0.0)],
        cv.c(colour),
        Stroke::new(1.5 * s, cv.black(255)),
    ));
    cv.p.circle_filled(at, 2.5 * s, cv.c(Color32::WHITE));
    let dist = (wx - car.0).hypot(wz - car.1);
    shadowed(cv, at + vec2(0.0, -12.0 * s), egui::Align2::CENTER_BOTTOM, &distance_text(dist), FontId::proportional(10.0 * s), colour);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::minimap::Trail;

    #[test]
    fn trail_fades_by_whichever_comes_first() {
        let f = TrailFade::new(10.0, 500.0);
        assert_eq!(f.alpha(0.0, 0.0), 220);
        // Age halfway, distance fresh: age rules. Distance halfway, age fresh: distance rules.
        assert_eq!(f.alpha(5.0, 0.0), 110);
        assert_eq!(f.alpha(0.0, 250.0), 110);
        assert!((f.alpha(5.0, 400.0) as i32 - 44).abs() <= 1);
        // Past either limit: invisible.
        assert_eq!(f.alpha(10.0, 0.0), 0);
        assert_eq!(f.alpha(0.0, 500.0), 0);
        // Degenerate settings are clamped (no division by zero).
        assert_eq!(TrailFade::new(0.0, 0.0).alpha(0.0, 0.0), 220);
    }

    #[test]
    fn arrow_points_apex_up_and_turn_clockwise() {
        let at = pos2(100.0, 50.0);
        let [tip, right, left] = arrow_points(at, 0.0, 1.0);
        assert!((tip - pos2(100.0, 50.0 - 9.8)).length() < 1e-4);
        assert!(right.x > at.x && left.x < at.x);
        // A quarter turn clockwise: the apex points right.
        let [tip, ..] = arrow_points(at, std::f32::consts::FRAC_PI_2, 1.0);
        assert!((tip - pos2(109.8, 50.0)).length() < 1e-3, "{tip:?}");
        // Size scales with `s`.
        let [tip, ..] = arrow_points(at, 0.0, 2.0);
        assert!((tip.y - (50.0 - 19.6)).abs() < 1e-4);
    }

    /// Run `f` with a `MapCanvas` over `rect` on a throwaway egui context; returns what it painted.
    fn with_canvas(cam: &Camera, rect: Rect, taper: bool, f: impl FnOnce(&MapCanvas)) -> Vec<egui::epaint::ClippedShape> {
        let ctx = egui::Context::default();
        let mut f = Some(f);
        ctx.run(egui::RawInput::default(), |ctx| {
            let p = Painter::new(ctx.clone(), egui::LayerId::background(), rect);
            let cv = MapCanvas { p: &p, cam, rect, taper, s: 1.0, a: 1.0, pause_glyph: "||" };
            if let Some(f) = f.take() {
                f(&cv);
            }
        })
        .shapes
    }

    /// Camera parity: pitch 0 maps exactly like the pre-camera `MapView` (centre + world offset),
    /// and a tilted camera puts markers where the layers' projection puts the same world point.
    #[test]
    fn canvas_maps_through_the_camera_flat_like_the_old_mapping_tilted_like_the_layers() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(208.0, 136.0));
        let (car, yaw) = ((1200.0f32, -800.0f32), 0.6);
        let flat = Camera::new(car.0, car.1, yaw, 300.0, rect, rect.center(), 0.0, 1.0);
        let old = crate::minimap::MapView::new(car.0, car.1, yaw, 300.0, 136.0);
        let tilted = Camera::new(car.0, car.1, yaw, 300.0, rect, Camera::tilt_centre(rect, 0.85), 55f32.to_radians(), 200.0);
        for (wx, wz) in [(1200.0f32, -800.0f32), (1300.0, -700.0), (1100.0, -900.0), (1250.0, -650.0)] {
            let [ox, oy] = old.world_to_offset(wx, wz);
            with_canvas(&flat, rect, true, |cv| {
                assert_eq!(cv.to_screen(wx, wz), rect.center() + vec2(ox, oy), "flat {wx},{wz}");
            });
            with_canvas(&tilted, rect, true, |cv| {
                assert_eq!(Some(cv.to_screen(wx, wz)), tilted.project(wx, wz), "tilted {wx},{wz}");
            });
        }
        // Far behind a tilted car (past the eye): off-screen in the flat direction, not NaN.
        with_canvas(&tilted, rect, true, |cv| {
            let behind = cv.to_screen(car.0 - 1.0e6 * yaw.sin(), car.1 - 1.0e6 * yaw.cos());
            assert!(behind.is_finite() && behind.y > rect.bottom() + 1000.0, "{behind:?}");
        });
    }

    #[test]
    fn trails_and_arrows_follow_the_tilted_camera() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(208.0, 136.0));
        let tilted = Camera::new(0.0, 0.0, 0.0, 300.0, rect, Camera::tilt_centre(rect, 0.85), 55f32.to_radians(), 200.0);
        let now = Instant::now();
        // A trail straight ahead of the car, recorded just now: head at 0 m, tail 200 m behind.
        let trail: Trail = (0..5).map(|i| crate::minimap::TrailPt { x: 0.0, y: 0.0, z: -200.0 + 50.0 * i as f32, t: now }).collect();
        let fade = TrailFade::new(10.0, 500.0);
        let widths = |taper| {
            let shapes = with_canvas(&tilted, rect, taper, |cv| draw_trail(cv, &trail, Color32::WHITE, fade, now));
            shapes.iter().filter_map(|s| if let egui::Shape::LineSegment { points, stroke } = &s.shape { Some((points[0], stroke.width)) } else { None }).collect::<Vec<_>>()
        };
        let w = widths(true);
        assert_eq!(w.len(), 4);
        // Segment starts are the projected world points.
        for (i, (start, _)) in w.iter().enumerate() {
            assert!((*start - tilted.project(0.0, -200.0 + 50.0 * i as f32).unwrap()).length() < 1e-3);
        }
        // Behind the car (nearer the viewer) the trail is wider than the plain 2 px; constant off.
        assert!(w[0].1 > 2.0 && w.iter().all(|&(_, wd)| wd > 0.8));
        assert!(widths(false).iter().all(|&(_, wd)| wd == 2.0));
        // The own arrow sits on the camera's car position (the tilt's lowered centre).
        let shapes = with_canvas(&tilted, rect, true, |cv| draw_own_arrow(cv, 0.0, Color32::WHITE));
        let poly = shapes.iter().find_map(|s| if let egui::Shape::Path(p) = &s.shape { Some(p.points.clone()) } else { None }).expect("arrow polygon");
        let c = poly.iter().fold(Vec2::ZERO, |a, q| a + q.to_vec2()) / poly.len() as f32;
        assert!((c.x - tilted.centre.x).abs() < 0.5 && (c.y - tilted.centre.y).abs() < 5.0, "{c:?} vs {:?}", tilted.centre);
    }

    /// D77 for teammates: in 3D a teammate sits at its telemetry height - a tunnel under a hill
    /// shows down at the road, not at the hill's surface - while flat / tilted cameras and a
    /// teammate without a height keep the terrain / plane mapping, and waypoints (no height on
    /// the wire) stay on the terrain.
    #[test]
    fn teammate_in_a_tunnel_is_drawn_at_its_own_height_in_3d_not_on_the_hill() {
        use crate::maprender::terrain::Terrain;
        use crate::maprender::view::Relief;
        use std::sync::Arc;
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(208.0, 136.0));
        let terrain = Arc::new(Terrain::synthetic());
        // The big hill at (-300, 200); the car approaches from the south-east side.
        let (hx, hz) = (-300.0f32, 200.0f32);
        let top = terrain.height(hx, hz);
        let tunnel_y = 100.0f32;
        assert!(top > tunnel_y + 40.0, "the spot is deep under the hill ({top} m)");
        let car = (hx + 150.0, hz - 150.0);
        let cam3 = Camera::new(car.0, car.1, -0.78, 600.0, rect, Camera::tilt_centre(rect, 0.85), 55f32.to_radians(), 200.0)
            .with_relief(Relief::new(terrain.clone(), 1.0, terrain.height(car.0, car.1)));
        let mate = |y: Option<f32>| Remote { id: "kai".into(), name: "Kai".into(), x: hx, z: hz, y, yaw: 0.0, colour: Color32::from_rgb(255, 128, 0), paused: false };
        let arrow_centre = |cam: &Camera, y: Option<f32>| -> Pos2 {
            let shapes = with_canvas(cam, rect, false, |cv| draw_remotes(cv, &[mate(y)], car, 0.0, true));
            let poly = shapes.iter().find_map(|s| if let egui::Shape::Path(p) = &s.shape { Some(p.points.clone()) } else { None }).expect("teammate arrow");
            (poly.iter().fold(Vec2::ZERO, |a, q| a + q.to_vec2()) / poly.len() as f32).to_pos2()
        };
        let want = |y: f32| cam3.project3(hx, y - GROUND_BELOW_M, hz).unwrap().0;
        let on_hill = cam3.project(hx, hz).unwrap();
        let in_tunnel = arrow_centre(&cam3, Some(tunnel_y));
        // The arrow's centroid is within a few px of the projected point.
        assert!((in_tunnel - want(tunnel_y)).length() < 3.0, "{in_tunnel:?} vs {:?}", want(tunnel_y));
        assert!((in_tunnel - on_hill).length() > 15.0, "the tunnel teammate must not sit on the hill: {in_tunnel:?} vs {on_hill:?}");
        // No height: the terrain surface, as before.
        assert!((arrow_centre(&cam3, None) - on_hill).length() < 8.0);
        // Flat and tilted cameras ignore the height.
        let tilted = Camera::new(car.0, car.1, -0.78, 600.0, rect, Camera::tilt_centre(rect, 0.85), 55f32.to_radians(), 200.0);
        let flat = Camera::new(car.0, car.1, -0.78, 600.0, rect, rect.center(), 0.0, 1.0);
        for cam in [&tilted, &flat] {
            let (a, b) = (arrow_centre(cam, Some(tunnel_y)), arrow_centre(cam, None));
            assert!((a - b).length() < 1e-3, "2D / tilted unchanged: {a:?} vs {b:?}");
        }
        // D89: in 3D the body is the scene's model. The marker is where the teammate is - the tunnel's
        // height, not the hill's - heading its yaw, in its colour, of the map's Car marker kind, and
        // the egui side draws no arrow (the name stays).
        use crate::maprender::cfg::MarkerStyle;
        let markers = |cam: &Camera, mates: &[Remote]| -> Vec<Marker3d> {
            let mut out = Vec::new();
            with_canvas(cam, rect, false, |cv| out = remote_markers_3d(cv, mates, MarkerStyle::Sedan, false));
            out
        };
        let mut kai = mate(Some(tunnel_y));
        kai.yaw = 1.25;
        let m = markers(&cam3, std::slice::from_ref(&kai));
        assert_eq!(m, vec![Marker3d { pos: [hx, tunnel_y, hz], yaw: 1.25, kind: MarkerStyle::Sedan, colour: Color32::from_rgb(255, 128, 0) }]);
        // No height: standing on the terrain. Paused: grey, like the flat arrow.
        let m = markers(&cam3, &[mate(None)]);
        assert!((m[0].pos[1] - (top + GROUND_BELOW_M)).abs() < 1e-3, "{:?}", m[0].pos);
        let mut paused = mate(Some(tunnel_y));
        paused.paused = true;
        assert_eq!(markers(&cam3, &[paused.clone()])[0].colour, crate::theme::steel(170));
        // Flat / tilted cameras have no 3D markers; a teammate off the map has the pointer only.
        assert!(markers(&tilted, &[mate(Some(tunnel_y))]).is_empty() && markers(&flat, &[mate(Some(tunnel_y))]).is_empty());
        let mut far = mate(Some(tunnel_y));
        far.x = car.0 + 50_000.0;
        assert!(markers(&cam3, &[far.clone()]).is_empty());
        // `flat_body = false`: no arrow polygon on the map, but the name is still painted; off the
        // map the edge pointer (a polygon) is still drawn.
        let polys = |cv_mates: &[Remote], flat_body: bool| {
            with_canvas(&cam3, rect, false, |cv| draw_remotes(cv, cv_mates, car, 0.0, flat_body)).iter().filter(|s| matches!(s.shape, egui::Shape::Path(_))).count()
        };
        let texts = |flat_body: bool| with_canvas(&cam3, rect, false, |cv| draw_remotes(cv, &[mate(Some(tunnel_y))], car, 0.0, flat_body)).iter().filter(|s| matches!(s.shape, egui::Shape::Text(_))).count();
        assert_eq!(polys(&[mate(Some(tunnel_y))], true), 1);
        assert_eq!(polys(&[mate(Some(tunnel_y))], false), 0, "the 3D body replaces the egui arrow");
        assert!(texts(false) > 0 && texts(false) == texts(true), "the name label is kept");
        assert_eq!(polys(&[far], false), 1, "the edge pointer stays egui");
        // A waypoint at the same spot stays on the terrain surface in 3D.
        let shapes = with_canvas(&cam3, rect, false, |cv| draw_waypoint(cv, (hx, hz), Color32::WHITE, car, 0.0));
        let dot = shapes.iter().find_map(|s| if let egui::Shape::Circle(c) = &s.shape { Some(c.center) } else { None }).unwrap();
        assert!((dot - on_hill).length() < 1.0, "{dot:?} vs {on_hill:?}");
    }

    #[test]
    fn segments_are_cut_to_the_circle() {
        let (c, r) = (pos2(100.0, 100.0), 50.0);
        // Crossing the whole circle: cut at both ends, on the circle.
        let (a, b) = clip_segment_circle(pos2(0.0, 100.0), pos2(200.0, 100.0), c, r).unwrap();
        assert!((a - pos2(50.0, 100.0)).length() < 1e-3 && (b - pos2(150.0, 100.0)).length() < 1e-3, "{a:?} {b:?}");
        // Inside: untouched. One end out: only that end moves. Missing it: nothing.
        let (a, b) = clip_segment_circle(pos2(90.0, 100.0), pos2(110.0, 110.0), c, r).unwrap();
        assert_eq!((a, b), (pos2(90.0, 100.0), pos2(110.0, 110.0)));
        let (a, b) = clip_segment_circle(pos2(100.0, 100.0), pos2(100.0, 300.0), c, r).unwrap();
        assert!(a == pos2(100.0, 100.0) && (b - pos2(100.0, 150.0)).length() < 1e-3);
        assert!(clip_segment_circle(pos2(0.0, 0.0), pos2(200.0, 0.0), c, r).is_none());
        // Beyond the segment's ends: the line crosses the circle, the segment doesn't.
        assert!(clip_segment_circle(pos2(0.0, 100.0), pos2(20.0, 100.0), c, r).is_none());
        // A point-sized segment.
        assert!(clip_segment_circle(pos2(100.0, 100.0), pos2(100.0, 100.0), c, r).is_some());
        assert!(clip_segment_circle(pos2(0.0, 0.0), pos2(0.0, 0.0), c, r).is_none());
    }

    /// Round bounds pin a far teammate / waypoint to the circle, rectangular ones to the box.
    #[test]
    fn pointers_pin_to_the_circle_not_the_box() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 200.0));
        let cam = Camera::new(0.0, 0.0, 0.0, 300.0, rect, rect.center(), 0.0, 1.0);
        let shapes = |round: bool| {
            with_canvas(&cam, rect, false, |cv| {
                // Far to the north-east: the corner direction.
                draw_waypoint_in(cv, (5000.0, 5000.0), Color32::WHITE, (0.0, 0.0), 0.0, round);
            })
        };
        let tip = |round: bool| -> f32 {
            // The pinned diamond's centre is the white dot circle.
            let c = shapes(round).iter().find_map(|s| if let egui::Shape::Circle(c) = &s.shape { Some(c.center) } else { None }).unwrap();
            (c - rect.center()).length()
        };
        // Circle: 8 px in from the radius 100; box: 8 px in along the diagonal, farther out.
        assert!((tip(true) - 92.0).abs() < 0.5, "{}", tip(true));
        assert!(tip(false) > 120.0, "{}", tip(false));
    }

    #[test]
    fn distance_text_switches_to_km() {
        assert_eq!(distance_text(950.4), "950m");
        assert_eq!(distance_text(1500.0), "1.5km");
    }
}
