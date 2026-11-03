//! Drawing primitives in 1080p design space. Every shape is an epaint convex polygon (so it
//! gets epaint's 1 px edge feathering as anti-aliasing) or a galley; no custom shaders.
//!
//! Colours are straight (unmultiplied) CSS values from the mockup, built with
//! [`rgba`]; `Color32` stores them premultiplied, and egui_glow blends premultiplied in gamma
//! space like the browser, so they carry over 1:1.

use std::sync::Arc;

use egui::epaint::{PathShape, PathStroke};
use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Color32, FontId, Galley, Painter, Pos2, Rect, Shape, Stroke};

/// `rgba(r,g,b,a)` from CSS, `a` in 0..=1.
pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, (a * 255.0 + 0.5) as u8)
}

/// Design space → screen: `o` is the widget's top-left on screen, `s` the scale
/// (`surface_h / 1080 × cfg.scale`), `a` the widget's alpha (global fade).
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    pub o: Pos2,
    pub s: f32,
    pub a: f32,
}

impl Xf {
    pub fn p(&self, x: f32, y: f32) -> Pos2 {
        self.o + vec2(x, y) * self.s
    }
    pub fn l(&self, v: f32) -> f32 {
        v * self.s
    }
    pub fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), vec2(w, h) * self.s)
    }
    /// A colour with the widget alpha applied.
    pub fn c(&self, c: Color32) -> Color32 {
        c.gamma_multiply(self.a)
    }
    /// The same transform with its alpha multiplied by `a` (a fading layer).
    pub fn fade(&self, a: f32) -> Self {
        Self { a: self.a * a, ..*self }
    }
}

/// Outline of a rounded rect, clockwise from the top-left corner's arc. `r` = radii
/// `[nw, ne, se, sw]` in screen px, each clamped to half the shorter side.
pub fn rounded_points(rect: Rect, r: [f32; 4]) -> Vec<Pos2> {
    rounded_points_n(rect, r, 12)
}

/// [`rounded_points`] with up to `max_n` segments per 90° arc (the default 12 is 7.5° steps; a
/// big circle, like the round Minimap, needs finer ones to stay round: a 4-point-per-octant arc
/// of radius 150 sags 0.3 px).
pub fn rounded_points_n(rect: Rect, r: [f32; 4], max_n: usize) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let max_r = rect.width().min(rect.height()) / 2.0;
    // Corner centres and the start angle of each 90° arc (screen y down, angle from +x).
    let corners = [
        (pos2(rect.left(), rect.top()), 1.0, 1.0, PI),
        (pos2(rect.right(), rect.top()), -1.0, 1.0, PI + FRAC_PI_2),
        (pos2(rect.right(), rect.bottom()), -1.0, -1.0, 0.0),
        (pos2(rect.left(), rect.bottom()), 1.0, -1.0, FRAC_PI_2),
    ];
    let mut pts = Vec::with_capacity(4 * 13);
    for (i, (corner, sx, sy, a0)) in corners.into_iter().enumerate() {
        let rad = r[i].clamp(0.0, max_r);
        if rad < 0.01 {
            pts.push(corner);
            continue;
        }
        let c = corner + vec2(sx * rad, sy * rad);
        // ~one point per 7.5°: chord error at r 23 is 0.05 px.
        let n = ((rad * 0.5).ceil() as usize).clamp(3, max_n.max(3));
        for k in 0..=n {
            let a = a0 + FRAC_PI_2 * k as f32 / n as f32;
            pts.push(c + rad * vec2(a.cos(), a.sin()));
        }
    }
    // A full pill's arcs meet end to start: drop the duplicates (zero-length edges).
    pts.dedup_by(|a, b| a.distance(*b) < 0.01);
    if pts.len() > 1 && pts[0].distance(pts[pts.len() - 1]) < 0.01 {
        pts.pop();
    }
    pts
}

/// Filled rounded rect in design px; `r` = `[nw, ne, se, sw]` design px.
pub fn rounded(p: &Painter, xf: &Xf, [x, y, w, h]: [f32; 4], r: [f32; 4], fill: Color32) {
    let pts = rounded_points(xf.rect(x, y, w, h), r.map(|v| xf.l(v)));
    p.add(Shape::convex_polygon(pts, xf.c(fill), Stroke::NONE));
}

/// Inside border of a rounded rect (CSS `border`): a closed stroke centred `width/2` inside.
/// `max_n` = arc segments per quarter ([`rounded_points_n`]; 12 is the usual).
pub fn rounded_border(p: &Painter, xf: &Xf, [x, y, w, h]: [f32; 4], r: f32, width: f32, col: Color32, max_n: usize) {
    let half = width / 2.0;
    let rect = xf.rect(x + half, y + half, w - width, h - width);
    let pts = rounded_points_n(rect, [xf.l((r - half).max(0.0)); 4], max_n);
    p.add(PathShape::closed_line(pts, PathStroke::new(xf.l(width), xf.c(col))));
}

/// Filled convex polygon from design-space points.
pub fn poly(p: &Painter, xf: &Xf, pts: &[[f32; 2]], fill: Color32) {
    let pts = pts.iter().map(|&[x, y]| xf.p(x, y)).collect();
    p.add(Shape::convex_polygon(pts, xf.c(fill), Stroke::NONE));
}

/// Circle outline of `width` design px centred on radius `r` (SVG stroke semantics; epaint
/// strokes circles *outside* their radius, so the radius is pulled in by half the width).
pub fn ring(p: &Painter, xf: &Xf, c: [f32; 2], r: f32, width: f32, col: Color32) {
    p.circle_stroke(xf.p(c[0], c[1]), xf.l(r - width / 2.0), Stroke::new(xf.l(width), xf.c(col)));
}

/// Point at `deg` (0° = 12 o'clock, clockwise) and radius `r` around `(cx, cy)`, design px.
pub fn polar(cx: f32, cy: f32, deg: f32, r: f32) -> [f32; 2] {
    let a = deg.to_radians();
    [cx + r * a.sin(), cy - r * a.cos()]
}

/// Annular sector as one convex quad (the mockup's SVG polygon: straight chords; at r 52
/// over 8.45° the chord sags 0.14 px, invisible).
pub fn sector(p: &Painter, xf: &Xf, c: [f32; 2], deg: [f32; 2], ri: f32, ro: f32, fill: Color32) {
    let [cx, cy] = c;
    let pts = [
        polar(cx, cy, deg[0], ri),
        polar(cx, cy, deg[0], ro),
        polar(cx, cy, deg[1], ro),
        polar(cx, cy, deg[1], ri),
    ];
    poly(p, xf, &pts, fill);
}

// ── Text ─────────────────────────────────────────────────────────────────────

/// Horizontal anchor of a text run at its x.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Anchor {
    Left,
    Center,
    Right,
}

/// How digits are spaced. Big Shoulders' digits are proportional and egui has no `tnum`, so
/// numbers that change use fixed cells, each digit centred in its cell (no jitter).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cells {
    /// Natural advances (static words).
    Off,
    /// Cells as wide as the widest digit at this size.
    Widest,
    /// Cells of this many design px (the spec's 6 px rpm-label cells).
    Fixed(f32),
}

/// A text run in design px: font size, letter spacing (px, between glyphs, like CSS
/// `letter-spacing` minus the trailing one) and digit cells.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub family: &'static str,
    pub size: f32,
    pub tracking: f32,
    pub cells: Cells,
    /// Mockup `.sh`: the glyphs drawn again `rgba(0,0,0,.5)` at +2 px y, underneath.
    pub shadow: bool,
}

pub const SHADOW: Color32 = rgba(0, 0, 0, 0.5);

/// One laid-out run: glyph pieces with their x offsets (screen px from the run's start) and
/// the baseline offset from a piece's top.
pub struct Run {
    pieces: Vec<(f32, Arc<Galley>)>,
    pub width: f32,
}

fn job(text: &str, font: FontId, tracking: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(text, 0.0, TextFormat { font_id: font, extra_letter_spacing: tracking, color: Color32::WHITE, ..Default::default() });
    job
}

/// Baseline of a single-row galley, px below its top.
fn baseline(g: &Galley) -> f32 {
    g.rows.first().and_then(|r| r.glyphs.first().map(|gl| r.pos.y + gl.pos.y)).unwrap_or(0.0)
}

/// Lay out `text` at scale `s` (screen px).
pub fn layout(p: &Painter, s: f32, text: &str, st: &TextStyle) -> Run {
    let font = FontId::new(st.size * s, egui::FontFamily::Name(st.family.into()));
    let tracking = st.tracking * s;
    let cell = match st.cells {
        Cells::Off => None,
        Cells::Fixed(w) => Some(w * s),
        Cells::Widest => Some(p.fonts_mut(|f| ('0'..='9').map(|c| f.glyph_width(&font, c)).fold(0.0, f32::max))),
    };
    let Some(cell) = cell else {
        let g = p.layout_job(job(text, font, tracking));
        let width = g.size().x;
        return Run { pieces: vec![(0.0, g)], width };
    };
    let mut pieces = Vec::with_capacity(text.len());
    let mut x = 0.0;
    for (i, ch) in text.chars().enumerate() {
        if i > 0 {
            x += tracking;
        }
        let mut buf = [0u8; 4];
        let g = p.layout_job(job(ch.encode_utf8(&mut buf), font.clone(), 0.0));
        let adv = g.size().x;
        if ch.is_ascii_digit() {
            pieces.push((x + (cell - adv) / 2.0, g));
            x += cell;
        } else {
            pieces.push((x, g));
            x += adv;
        }
    }
    Run { pieces, width: x }
}

/// Widest of `chars` in style `st` at scale `s`, screen px (a fixed cell for a letter that
/// varies, like the drive-mode letter).
pub fn widest(p: &Painter, s: f32, st: &TextStyle, chars: &str) -> f32 {
    let font = FontId::new(st.size * s, egui::FontFamily::Name(st.family.into()));
    p.fonts_mut(|f| chars.chars().map(|c| f.glyph_width(&font, c)).fold(0.0, f32::max))
}

/// Draw `text` with its baseline at design `(x, y)`, anchored per `anchor`. Returns the run's
/// width in design px (for placing what follows it).
#[allow(clippy::too_many_arguments)]
pub fn text(p: &Painter, xf: &Xf, x: f32, y: f32, anchor: Anchor, text: &str, st: &TextStyle, color: Color32) -> f32 {
    let run = layout(p, xf.s, text, st);
    let at = xf.p(x, y);
    let x0 = match anchor {
        Anchor::Left => at.x,
        Anchor::Center => at.x - run.width / 2.0,
        Anchor::Right => at.x - run.width,
    };
    draw_run(p, xf, &run, pos2(x0, at.y), st.shadow, color);
    run.width / xf.s
}

/// Draw a laid-out run with its start at `pen` (screen px; `pen.y` = baseline).
pub fn draw_run(p: &Painter, xf: &Xf, run: &Run, pen: Pos2, shadow: bool, color: Color32) {
    if shadow {
        let sh = xf.c(SHADOW);
        for (dx, g) in &run.pieces {
            let pos = pos2(pen.x + dx, pen.y - baseline(g) + xf.l(2.0));
            p.galley_with_override_text_color(pos, g.clone(), sh);
        }
    }
    let col = xf.c(color);
    for (dx, g) in &run.pieces {
        let pos = pos2(pen.x + dx, pen.y - baseline(g));
        p.galley_with_override_text_color(pos, g.clone(), col);
    }
}

// ── Number formatting ────────────────────────────────────────────────────────

/// `1234567` → `"1,234,567"`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Lap time `m:ss.mmm`; not running (≤ 0 or not finite) → `–:––.–––` like the mockup.
pub fn lap_time(t: f32) -> String {
    if !(t.is_finite() && t > 0.0) {
        return "\u{2013}:\u{2013}\u{2013}.\u{2013}\u{2013}\u{2013}".into();
    }
    let ms = (t as f64 * 1000.0).round() as u64;
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// Lap delta `−0.42` / `+0.42` (true minus sign, like the mockup).
pub fn delta(d: f32) -> String {
    format!("{}{:.2}", if d < 0.0 { '\u{2212}' } else { '+' }, d.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(4039), "4,039");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(lap_time(41.273), "0:41.273");
        assert_eq!(lap_time(64.882), "1:04.882");
        assert_eq!(lap_time(0.0), "–:––.–––");
        assert_eq!(delta(-0.42), "−0.42");
        assert_eq!(delta(0.3), "+0.30");
    }

    #[test]
    fn rounded_points_stay_in_rect() {
        let r = Rect::from_min_size(pos2(10.0, 20.0), vec2(60.0, 46.0));
        let pts = rounded_points(r, [23.0, 4.0, 4.0, 23.0]);
        assert!(pts.iter().all(|p| r.expand(1e-3).contains(*p)));
        // Left side is a half circle: its leftmost point is at mid-height.
        let left = pts.iter().fold(f32::MAX, |m, p| m.min(p.x));
        assert!((left - 10.0).abs() < 1e-3);
    }
}
