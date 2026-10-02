//! Map markers drawn identically by the Dashboard Map widget and the HUD Minimap: the own
//! arrow, co-op teammate arrows / edge markers / names, breadcrumb trails and shared
//! waypoints. *Why one implementation:* the user wants the HUD to look like the Dashboard
//! map, and a single drawing path means the two can't drift apart (as `draw_compass`, in
//! `hud/minimap.rs`, already does for the compass).
//!
//! Everything is drawn through a [`MapCanvas`]: the painter, the [`MapView`] transform, the
//! screen centre / bounds, a size factor (`s`: 1.0 on the Dashboard, the HUD's design→screen
//! scale) and a fade alpha (`a`: 1.0 on the Dashboard, the HUD's show/hide fade).

use std::time::Instant;

use egui::{pos2, vec2, Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};

use crate::minimap::{MapView, Trail};

/// Where and how to draw. Cheap to build per frame.
pub struct MapCanvas<'a> {
    pub p: &'a Painter,
    pub view: &'a MapView,
    /// Screen position of the car (the view centre).
    pub centre: Pos2,
    /// Bounds markers stay in: the Dashboard widget rect, the HUD pill's rect.
    pub rect: Rect,
    /// Size factor for strokes, arrows and text.
    pub s: f32,
    /// Alpha applied to every colour.
    pub a: f32,
    /// Prefix of a paused player's name (the Dashboard has the icon font, the HUD doesn't).
    pub pause_glyph: &'static str,
}

impl MapCanvas<'_> {
    pub fn to_screen(&self, wx: f32, wz: f32) -> Pos2 {
        let [ox, oy] = self.view.world_to_offset(wx, wz);
        self.centre + vec2(ox, oy)
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
    arrow(cv, cv.centre, angle, col);
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
        cv.p.line_segment([cv.to_screen(ax, az), cv.to_screen(bx, bz)], Stroke::new(2.0 * cv.s, cv.c(c)));
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

    #[test]
    fn trail_fades_by_whichever_comes_first() {
        let f = TrailFade::new(10.0, 500.0);
        assert_eq!(f.alpha(0.0, 0.0), 220);
        // Age halfway, distance fresh: age rules. Distance halfway, age fresh: distance rules.
        assert_eq!(f.alpha(5.0, 0.0), 110);
        assert_eq!(f.alpha(0.0, 250.0), 110);
        assert_eq!(f.alpha(5.0, 400.0), (0.2_f32 * 220.0) as u8);
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

    #[test]
    fn distance_text_switches_to_km() {
        assert_eq!(distance_text(950.4), "950m");
        assert_eq!(distance_text(1500.0), "1.5km");
    }
}
