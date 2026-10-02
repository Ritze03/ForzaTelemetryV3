//! D26 notifications: short pill messages ("Gearbox: ON") stacked at one 3×3 anchor.
//!
//! [`stack_layout`] is pure (anchor + sizes → rects) and carries the stacking rules; [`draw`]
//! picks the live notifications, fades them and draws each as a small plate pill in the
//! Drive cluster's look (plate backing, Big Shoulders text, coloured status dot).

use egui::{pos2, vec2, Painter, Rect, Vec2};

use super::col;
use super::fonts::W800;
use super::prims::{self, Cells, TextStyle, Xf};
use crate::config::HudCell;
use crate::overlay::snapshot::{HudSnapshot, NotifKind, Notification};

/// How long a notification lives, seconds (fade-in and fade-out included).
pub const TTL_SECS: f64 = 2.5;
const FADE_IN_SECS: f64 = 0.15;
const FADE_OUT_SECS: f64 = 0.45;
/// At most this many are drawn (the newest ones).
pub const MAX_VISIBLE: usize = 5;
/// Gap between stacked pills, design px.
const SPACING: f32 = 8.0;
const H: f32 = 38.0;
const TEXT: TextStyle = TextStyle { family: W800, size: 22.0, tracking: 0.4, cells: Cells::Off, shadow: false };

/// Place a vertical stack of boxes at `anchor` on an `area`-sized surface. `sizes` are
/// **newest first**; the result is one rect per size, in the same order. Never side by side.
///
/// - top row: grows downward from the top margin, newest on top, older below;
/// - bottom row: grows upward from the bottom margin, newest at the bottom, older above
///   (so in both rows the newest is the one nearest the screen edge);
/// - middle row, left / right: top to bottom, newest on top, the block centred on the
///   screen's vertical middle;
/// - dead centre: starts at the vertical middle and grows downward, newest on top.
///
/// Horizontally: left margin / centred / right margin by the anchor's column.
pub fn stack_layout(anchor: HudCell, sizes: &[Vec2], area: Vec2, margin: f32, spacing: f32) -> Vec<Rect> {
    let total = sizes.iter().map(|s| s.y).sum::<f32>() + spacing * sizes.len().saturating_sub(1) as f32;
    let (col, row) = (anchor.col(), anchor.row());
    // Top of the first (newest) rect, and whether the following ones go down or up.
    let (mut y, down) = match (row, col) {
        (0, _) => (margin, true),
        (1, 1) => (area.y / 2.0, true),
        (1, _) => ((area.y - total) / 2.0, true),
        _ => (area.y - margin, false),
    };
    sizes
        .iter()
        .map(|&s| {
            let x = match col {
                0 => margin,
                1 => (area.x - s.x) / 2.0,
                _ => area.x - margin - s.x,
            };
            let top = if down { y } else { y - s.y };
            y = if down { y + s.y + spacing } else { y - s.y - spacing };
            Rect::from_min_size(pos2(x, top), s)
        })
        .collect()
}

/// Opacity of a notification `age` seconds old: quick fade-in, fade-out at the end of its life.
pub fn alpha(age: f64) -> f32 {
    if !(0.0..TTL_SECS).contains(&age) {
        return 0.0;
    }
    ((age / FADE_IN_SECS).min(1.0).min((TTL_SECS - age) / FADE_OUT_SECS)).clamp(0.0, 1.0) as f32
}

/// The notifications to show at `now`: alive ones, newest first, at most [`MAX_VISIBLE`].
pub fn live(all: &[Notification], now: f64) -> Vec<&Notification> {
    let mut v: Vec<&Notification> = all.iter().filter(|n| (0.0..TTL_SECS).contains(&(now - n.created))).collect();
    v.sort_by(|a, b| b.created.total_cmp(&a.created).then(b.id.cmp(&a.id)));
    v.truncate(MAX_VISIBLE);
    v
}

fn dot(kind: NotifKind) -> egui::Color32 {
    match kind {
        NotifKind::On => col::BEST,
        NotifKind::Off => col::RED,
        NotifKind::Info => col::SHIFT,
    }
}

/// Draw the stack. `fade` is the HUD's global alpha; `s` the design→screen scale. Returns
/// true while any notification is alive (the overlay keeps its frame timer running).
pub fn draw(p: &Painter, screen: Rect, snap: &HudSnapshot, now: f64, fade: f32, s: f32) -> bool {
    let cfg = &*snap.cfg;
    if !cfg.notif_on {
        return false;
    }
    let items = live(&snap.notifications, now);
    if items.is_empty() {
        return false;
    }
    let runs: Vec<_> = items.iter().map(|n| prims::layout(p, s, &n.text, &TEXT)).collect();
    // Design-px sizes: dot area 34 + text + right padding 18.
    let sizes: Vec<Vec2> = runs.iter().map(|r| vec2(34.0 + r.width / s + 18.0, H)).collect();
    let rects = stack_layout(cfg.notif_cell, &sizes, screen.size() / s, cfg.margin_px, SPACING);
    for ((n, run), r) in items.iter().zip(&runs).zip(rects) {
        let xf = Xf { o: screen.min + r.min.to_vec2() * s, s, a: fade * alpha(now - n.created) };
        prims::rounded(p, &xf, [0.0, 0.0, r.width(), H], [H / 2.0; 4], col::plate(cfg.plate_opacity));
        prims::rounded(p, &xf, [14.0, H / 2.0 - 5.0, 10.0, 10.0], [5.0; 4], dot(n.kind));
        prims::draw_run(p, &xf, run, xf.p(34.0, 27.0), false, col::INK);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Vec2 = vec2(1920.0, 1080.0);
    const M: f32 = 40.0;
    const GAP: f32 = 8.0;

    /// Three boxes of different widths, newest first.
    fn sizes() -> Vec<Vec2> {
        vec![vec2(200.0, 38.0), vec2(260.0, 38.0), vec2(180.0, 38.0)]
    }

    fn at(cell: HudCell) -> Vec<Rect> {
        stack_layout(cell, &sizes(), AREA, M, GAP)
    }

    /// Consecutive rects are separated vertically (never side by side).
    fn assert_vertical_no_overlap(r: &[Rect]) {
        for w in r.windows(2) {
            assert!(
                w[0].max.y + GAP <= w[1].min.y + 0.01 || w[1].max.y + GAP <= w[0].min.y + 0.01,
                "overlap or side-by-side: {w:?}"
            );
        }
    }

    #[test]
    fn top_row_newest_on_top_older_below() {
        for c in [HudCell::TopLeft, HudCell::TopCenter, HudCell::TopRight] {
            let r = at(c);
            assert_eq!(r[0].min.y, M, "{c:?}");
            assert!(r[0].min.y < r[1].min.y && r[1].min.y < r[2].min.y, "{c:?}");
            assert_eq!(r[1].min.y, r[0].max.y + GAP);
            assert_vertical_no_overlap(&r);
        }
    }

    #[test]
    fn bottom_row_newest_at_the_bottom_older_above() {
        for c in [HudCell::BottomLeft, HudCell::BottomCenter, HudCell::BottomRight] {
            let r = at(c);
            assert_eq!(r[0].max.y, AREA.y - M, "{c:?}");
            assert!(r[0].min.y > r[1].min.y && r[1].min.y > r[2].min.y, "{c:?}");
            assert_eq!(r[1].max.y, r[0].min.y - GAP);
            assert_vertical_no_overlap(&r);
        }
    }

    #[test]
    fn middle_sides_top_to_bottom_newest_on_top_block_centred() {
        for c in [HudCell::MiddleLeft, HudCell::MiddleRight] {
            let r = at(c);
            assert!(r[0].min.y < r[1].min.y && r[1].min.y < r[2].min.y, "{c:?}");
            let (top, bottom) = (r[0].min.y, r[2].max.y);
            assert!(((top + bottom) / 2.0 - AREA.y / 2.0).abs() < 0.01, "{c:?} not centred");
            assert_vertical_no_overlap(&r);
        }
    }

    #[test]
    fn dead_centre_starts_at_the_middle_and_grows_down() {
        let r = at(HudCell::Center);
        assert_eq!(r[0].min.y, AREA.y / 2.0);
        assert!(r[0].min.y < r[1].min.y && r[1].min.y < r[2].min.y);
        assert_eq!(r[1].min.y, r[0].max.y + GAP);
        assert_vertical_no_overlap(&r);
    }

    #[test]
    fn horizontal_follows_the_column() {
        for cell in HudCell::ALL {
            for (r, s) in at(cell).iter().zip(sizes()) {
                match cell.col() {
                    0 => assert_eq!(r.min.x, M),
                    1 => assert_eq!(r.center().x, AREA.x / 2.0),
                    _ => assert_eq!(r.max.x, AREA.x - M),
                }
                assert_eq!(r.size(), s);
            }
        }
    }

    #[test]
    fn no_items_no_rects() {
        assert!(stack_layout(HudCell::Center, &[], AREA, M, GAP).is_empty());
    }

    #[test]
    fn fade_curve_and_live_selection() {
        assert_eq!(alpha(-0.1), 0.0);
        assert_eq!(alpha(0.0), 0.0);
        assert_eq!(alpha(1.0), 1.0);
        assert!(alpha(TTL_SECS - 0.2) < 1.0 && alpha(TTL_SECS - 0.2) > 0.0);
        assert_eq!(alpha(TTL_SECS), 0.0);
        let mk = |id, created| Notification { id, text: String::new(), kind: NotifKind::Info, created };
        let all: Vec<_> = (0..8).map(|i| mk(i, 10.0 + i as f64 * 0.1)).chain([mk(99, 1.0)]).collect();
        let v = live(&all, 10.8);
        assert_eq!(v.len(), MAX_VISIBLE);
        assert!(v.windows(2).all(|w| w[0].created >= w[1].created), "newest first");
        assert_eq!(v[0].id, 7);
    }
}
