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
        self.cam.project(wx, wz).unwrap_or_else(|| {
            let [ox, oy] = self.cam.view.world_to_offset(wx, wz);
            self.cam.centre + vec2(ox, oy).normalized() * 1.0e5
        })
    }
    fn c(&self, c: Color32) -> Color32 {
        c.gamma_multiply(self.a)
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
    let n = pts.len();
    if n < 2 {
        return;
    }
    let (hx, hz, _) = pts[n - 1]; // head = the player's current position
    for i in 1..n {
        let (ax, az, _) = pts[i - 1];
        let (bx, bz, bt) = pts[i];
        let alpha = fade.alpha(now.saturating_duration_since(bt).as_secs_f32(), (ax - hx).hypot(az - hz));
        if alpha < 4 {
            continue;
        }
        let c = Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), alpha);
        let (a, b) = (cv.to_screen(ax, az), cv.to_screen(bx, bz));
        let k = if cv.taper { cv.cam.depth_scale_at_row((a.y + b.y) * 0.5).clamp(0.4, 1.5) } else { 1.0 };
        cv.p.line_segment([a, b], Stroke::new(2.0 * cv.s * k, cv.c(c)));
    }
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

/// Teammates relative to the car at `car` (world x, z): on the map a heading arrow with the
/// name above (labels nudged apart), off the map a small pointer on the edge with the distance.
/// Paused ones are grey with the pause glyph. `map_yaw` is the view's yaw (arrows turn by
/// `yaw - map_yaw`).
pub fn draw_remotes(cv: &MapCanvas, remotes: &[Remote], car: (f32, f32), map_yaw: f32) {
    let s = cv.s;
    // Names are drawn in a second pass so labels of cars close together (racing side by
    // side) can be nudged apart instead of stacking illegibly.
    let mut labels: Vec<(Pos2, String, Color32)> = Vec::new();
    for r in remotes {
        let at = cv.to_screen(r.x, r.z);
        let col = if r.paused { crate::theme::steel(170) } else { r.colour };
        if cv.rect.shrink(8.0 * s).contains(at) {
            arrow(cv, at, r.yaw - map_yaw, col);
            let label = if r.paused { format!("{} {}", cv.pause_glyph, r.name) } else { r.name.clone() };
            labels.push((pos2(at.x, at.y - 7.0 * s * 1.9), label, col));
        } else {
            // Off the map: clamp to the edge and point toward them.
            let c = cv.rect.center();
            let d = at - c;
            let half = cv.rect.size() * 0.5 - Vec2::splat(10.0 * s);
            let kx = if d.x.abs() > 0.01 { half.x / d.x.abs() } else { f32::INFINITY };
            let ky = if d.y.abs() > 0.01 { half.y / d.y.abs() } else { f32::INFINITY };
            let edge = c + d * kx.min(ky).min(1.0);
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
pub fn draw_waypoint(cv: &MapCanvas, (wx, wz): (f32, f32), colour: Color32, car: (f32, f32), time: f32) {
    let s = cv.s;
    let mut at = cv.to_screen(wx, wz);
    if !cv.rect.shrink(6.0 * s).contains(at) {
        let d = at - cv.rect.center();
        let half = cv.rect.size() * 0.5 - Vec2::splat(8.0 * s);
        let kx = if d.x.abs() > 0.01 { half.x / d.x.abs() } else { f32::INFINITY };
        let ky = if d.y.abs() > 0.01 { half.y / d.y.abs() } else { f32::INFINITY };
        at = cv.rect.center() + d * kx.min(ky).min(1.0);
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
        let trail: Trail = (0..5).map(|i| (0.0, -200.0 + 50.0 * i as f32, now)).collect();
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

    #[test]
    fn distance_text_switches_to_km() {
        assert_eq!(distance_text(950.4), "950m");
        assert_eq!(distance_text(1500.0), "1.5km");
    }
}
