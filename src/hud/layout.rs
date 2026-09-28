//! The 3×3 slot layout (D19, D22, D28). Pure: module sizes + cells + screen size → rects.
//!
//! Modules sharing a cell stack **vertically** in the order Map → cluster → race/drift,
//! from the screen edge inward:
//! - top row: top-down from the top margin (map highest);
//! - bottom row: bottom-up from the bottom margin (map lowest);
//! - middle row: the whole stack centred on the screen's vertical middle, map lowest,
//!   growing upward (the tab mockup's `column-reverse; justify-content:center`).
//!
//! Horizontal alignment follows the column: left margin / centred / right margin.
//! Margin and gap are user settings (`OverlayConfig::margin_px` / `gap_px`), in design px
//! scaled like the modules, so a layout keeps its proportions across resolutions and HUD scale.

use egui::{pos2, Rect, Vec2};

use crate::config::HudCell;

/// Default edge margin and stack gap, 1080p design px (the D22 look).
pub const MARGIN: f32 = 44.0;
pub const GAP: f32 = 12.0;

/// A HUD module; the declaration order is the stacking order within a cell.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Module {
    Map,
    Cluster,
    /// The race block or the drift counter (they share `race_cell`).
    Race,
}

/// Place `items` (module, cell, size in design px) on a `screen`-sized surface at `scale`
/// (screen px per design px), `margin` from the edges and `gap` between stacked modules
/// (both design px). Returns one screen rect per item, in input order.
pub fn layout(screen: Vec2, scale: f32, margin: f32, gap: f32, items: &[(Module, HudCell, Vec2)]) -> Vec<Rect> {
    let (margin, gap) = (margin * scale, gap * scale);
    let mut out = vec![Rect::NOTHING; items.len()];
    for cell in HudCell::ALL {
        let mut stack: Vec<usize> = (0..items.len()).filter(|&i| items[i].1 == cell).collect();
        if stack.is_empty() {
            continue;
        }
        stack.sort_by_key(|&i| items[i].0);
        let sizes: Vec<Vec2> = stack.iter().map(|&i| items[i].2 * scale).collect();
        let total = sizes.iter().map(|s| s.y).sum::<f32>() + gap * (sizes.len() - 1) as f32;
        // `y` is the edge the next module starts from; `down` = grows downward.
        let (mut y, down) = match cell.row() {
            0 => (margin, true),
            1 => ((screen.y + total) / 2.0, false),
            _ => (screen.y - margin, false),
        };
        for (&i, size) in stack.iter().zip(sizes) {
            let x = match cell.col() {
                0 => margin,
                1 => (screen.x - size.x) / 2.0,
                _ => screen.x - margin - size.x,
            };
            let top = if down { y } else { y - size.y };
            out[i] = Rect::from_min_size(pos2(x, top), size);
            y = if down { y + size.y + gap } else { y - size.y - gap };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    const SCREEN: Vec2 = Vec2::new(1920.0, 1080.0);
    const MAP: Vec2 = Vec2::new(208.0, 136.0);
    const PILL: Vec2 = Vec2::new(184.0, 46.0);
    const RACE: Vec2 = Vec2::new(196.0, 46.0);

    fn r(x: f32, y: f32, s: Vec2) -> Rect {
        Rect::from_min_size(pos2(x, y), s)
    }

    #[test]
    fn single_module_in_every_cell() {
        let s = PILL;
        let want = [
            r(44.0, 44.0, s),
            r(868.0, 44.0, s),
            r(1692.0, 44.0, s),
            r(44.0, 517.0, s),
            r(868.0, 517.0, s),
            r(1692.0, 517.0, s),
            r(44.0, 990.0, s),
            r(868.0, 990.0, s),
            r(1692.0, 990.0, s),
        ];
        for (cell, want) in HudCell::ALL.into_iter().zip(want) {
            let got = layout(SCREEN, 1.0, MARGIN, GAP, &[(Module::Cluster, cell, s)]);
            assert_eq!(got, vec![want], "{cell:?}");
        }
    }

    #[test]
    fn top_row_stacks_down_map_first() {
        // Input order must not matter: the map is highest regardless.
        let items = [(Module::Race, HudCell::TopRight, RACE), (Module::Cluster, HudCell::TopRight, PILL), (Module::Map, HudCell::TopRight, MAP)];
        let got = layout(SCREEN, 1.0, MARGIN, GAP, &items);
        assert_eq!(got[2], r(1920.0 - 44.0 - 208.0, 44.0, MAP));
        assert_eq!(got[1], r(1920.0 - 44.0 - 184.0, 44.0 + 136.0 + 12.0, PILL));
        assert_eq!(got[0], r(1920.0 - 44.0 - 196.0, 44.0 + 136.0 + 12.0 + 46.0 + 12.0, RACE));
    }

    #[test]
    fn bottom_row_stacks_up_map_lowest() {
        let items = [(Module::Map, HudCell::BottomCenter, MAP), (Module::Cluster, HudCell::BottomCenter, PILL), (Module::Race, HudCell::BottomCenter, RACE)];
        let got = layout(SCREEN, 1.0, MARGIN, GAP, &items);
        assert_eq!(got[0], r(856.0, 1080.0 - 44.0 - 136.0, MAP));
        assert_eq!(got[1], r(868.0, 900.0 - 12.0 - 46.0, PILL));
        assert_eq!(got[2], r(862.0, 842.0 - 12.0 - 46.0, RACE));
    }

    #[test]
    fn middle_row_centred_growing_up() {
        let items = [(Module::Cluster, HudCell::MiddleLeft, PILL), (Module::Map, HudCell::MiddleLeft, MAP)];
        let got = layout(SCREEN, 1.0, MARGIN, GAP, &items);
        // Stack height 136 + 12 + 46 = 194, centred: 443..637. Map at the bottom.
        assert_eq!(got[1], r(44.0, 637.0 - 136.0, MAP));
        assert_eq!(got[0], r(44.0, 443.0, PILL));
        assert!((got[0].top() + got[1].bottom() - 1080.0).abs() < 1e-3);
    }

    #[test]
    fn separate_cells_do_not_interact_and_scale_applies() {
        let items = [(Module::Map, HudCell::BottomLeft, MAP), (Module::Cluster, HudCell::BottomCenter, PILL), (Module::Race, HudCell::TopLeft, RACE)];
        // 1440p (scale 4/3) at 100 %.
        let s = 1440.0 / 1080.0;
        let got = layout(vec2(2560.0, 1440.0), s, MARGIN, GAP, &items);
        let m = 44.0 * s;
        assert!((got[0].left() - m).abs() < 1e-3 && (got[0].bottom() - (1440.0 - m)).abs() < 1e-3);
        assert!((got[0].width() - 208.0 * s).abs() < 1e-3);
        assert!((got[1].center().x - 1280.0).abs() < 1e-3 && (got[1].bottom() - (1440.0 - m)).abs() < 1e-3);
        assert!((got[2].left() - m).abs() < 1e-3 && (got[2].top() - m).abs() < 1e-3);
    }

    #[test]
    fn custom_margin_and_gap_scale_with_the_hud() {
        let items = [(Module::Map, HudCell::TopLeft, MAP), (Module::Cluster, HudCell::TopLeft, PILL), (Module::Race, HudCell::BottomRight, RACE)];
        let got = layout(SCREEN, 1.0, 100.0, 30.0, &items);
        assert_eq!(got[0], r(100.0, 100.0, MAP));
        assert_eq!(got[1], r(100.0, 100.0 + 136.0 + 30.0, PILL));
        assert_eq!(got[2], r(1920.0 - 100.0 - 196.0, 1080.0 - 100.0 - 46.0, RACE));
        // Zero spacing: flush against the edge and each other.
        let got = layout(SCREEN, 1.0, 0.0, 0.0, &items);
        assert_eq!(got[0].min, pos2(0.0, 0.0));
        assert_eq!(got[1].top(), got[0].bottom());
        // Both scale with the surface/HUD scale, like the modules.
        let got = layout(SCREEN * 2.0, 2.0, 100.0, 30.0, &items);
        assert_eq!(got[1], r(200.0, 200.0 + 272.0 + 60.0, PILL * 2.0));
    }
}
