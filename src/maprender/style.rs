//! What things look like: road draw order, the zoom-dependent line width, dash patterns, and
//! the POI category table (id, colour, marker shape, the `PoiKind` it shows).
//!
//! These are *data* colours (road types, POI categories), so they live here and in
//! `cfg.rs` defaults rather than in `theme.rs` (CLAUDE.md: chrome uses the theme's role
//! tokens, data colours live with the data).

use egui::Color32;

use super::cfg::{DashStyle, Rgb, RoadsCfg};
use crate::gamedata::poi::PoiKind;

// ── roads ────────────────────────────────────────────────────────────────────────────────────

/// Road type slots in draw order, bottom to top: asphalt ends on top (the demo's `ORDER`),
/// edges without a type (slot 0) first. The turnaround slot (9) is not here: never drawn (D52).
pub const ROAD_DRAW_ORDER: [usize; 8] = [
    0, // unset
    5, // crosscountry
    3, // other
    2, // offroad
    4, // trail
    6, // tunnel
    1, // road
    8, // highway
];

/// Edges without a type: grey dashed, so a nav that does not match the road-type data shows.
pub const UNSET_COLOR: Rgb = Rgb::hex(0x8d96a3);
pub const UNSET_ALPHA: f32 = 0.6;
pub const UNSET_WIDTH: f32 = 0.7;

/// Below this many px per metre dashes are sub-pixel: draw solid (design §4.2).
pub const DASH_MIN_PX_PER_M: f32 = 0.02;

/// A line's thinnest drawn width, px (the demo's `max(0.7, …)`).
pub const MIN_LINE_PX: f32 = 0.7;

/// Screen px between kept vertices of a road polyline (`view::thin`).
pub const THIN_PX: f32 = 2.0;

/// Base road width in px for a view with `px_per_m` screen px per metre (the demo's
/// `roadBase`): `clamp(px_per_m * metres, min_px, max_px)` when it scales with the zoom, else
/// the fixed `base_px`.
pub fn road_base_px(c: &RoadsCfg, px_per_m: f32) -> f32 {
    if c.scale_with_zoom {
        (px_per_m * c.metres).clamp(c.min_px, c.max_px.max(c.min_px))
    } else {
        c.base_px
    }
}

/// Width factor of the race road (D80, `RouteStyle::Road`) on the roads' width rule, 2D and 3D.
/// *Why the road rule and not the track's own half-width:* the rule keeps every road readable at
/// the HUD's and the Viewer's zooms (a 12 m track is a hairline at 3 km and a slab at 150 m);
/// a little wider than a highway (1.44) so the race road covers whatever road it runs on, the
/// other road's casing showing as its edge.
pub const RACE_ROAD_WIDTH: f32 = 1.5;

/// Final line width of a type: `max(0.7, base * type factor)`.
pub fn line_px(base: f32, factor: f32) -> f32 {
    (base * factor).max(MIN_LINE_PX)
}

/// Dash and gap lengths in px for a line of width `w` (the demo's `DASH * max(1, w*0.6)`), or
/// `None` for solid.
pub fn dash_pattern(d: DashStyle, w: f32) -> Option<(f32, f32)> {
    let (a, g) = match d {
        DashStyle::None => return None,
        DashStyle::Dashed => (6.0, 4.0),
        DashStyle::Short => (2.0, 3.0),
        DashStyle::Dotted => (1.0, 4.0),
    };
    let k = (w * 0.6).max(1.0);
    Some((a * k, g * k))
}

/// Behind the satellite image when it is off (vectors only) or does not cover the widget (the
/// sky above a tilted map): the demo's `dash.bg`.
pub const MAP_BACKING: Color32 = Color32::from_rgb(0x14, 0x1a, 0x21);

// ── POIs ─────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Circle,
    Diamond,
    Square,
    Ring,
}

/// One POI category. `id` is the name used in the config (the demo's category names).
pub struct PoiCat {
    pub id: &'static str,
    /// The reader's kind. `None` for a category that is not a kind of its own: the current
    /// treasure chest (`treasure_chest_current`) is one of the `TreasureChest` /
    /// `TreasureChestBoard` items, picked by the week (`PoiLayer::current_chest`).
    pub kind: Option<PoiKind>,
    /// Whose game icon the category draws when it is not `kind`'s own.
    pub icon: Option<PoiKind>,
    pub color: Rgb,
    pub shape: Shape,
}

impl PoiCat {
    /// The kind whose game icon this category shows.
    pub fn icon_kind(&self) -> Option<PoiKind> {
        self.icon.or(self.kind)
    }
}

const fn cat(id: &'static str, kind: Option<PoiKind>, color: u32, shape: Shape) -> PoiCat {
    PoiCat { id, kind, icon: None, color: Rgb::hex(color), shape }
}

use PoiKind as K;
use Shape::{Circle, Diamond, Ring, Square};

/// Every category, colours from the demo. The order is the lookup index (`PoiLayer::cat`), not
/// stored anywhere: configs hold the ids.
pub static POI_CATS: &[PoiCat] = &[
    cat("race_pin", Some(K::RacePin), 0xff9f43, Circle),
    cat("touge_event", Some(K::TougeEvent), 0xc084fc, Circle),
    cat("landmark", Some(K::Landmark), 0xe5e7eb, Ring),
    cat("barn_find", Some(K::BarnFind), 0xfbbf24, Diamond),
    cat("barn_find_hint", Some(K::BarnFindHint), 0xd97706, Ring),
    cat("car_meet", Some(K::CarMeet), 0x2dd4bf, Circle),
    cat("drag_meet", Some(K::DragMeet), 0xf472b6, Circle),
    cat("drag_meet_finish", Some(K::DragMeetFinish), 0xf9a8d4, Square),
    cat("estate", Some(K::Estate), 0x60a5fa, Square),
    cat("estate_entrance", Some(K::EstateEntrance), 0x93c5fd, Square),
    cat("fast_travel", Some(K::FastTravel), 0x22d3ee, Diamond),
    cat("festival_site", Some(K::FestivalSite), 0xf59e0b, Circle),
    cat("house", Some(K::House), 0x60a5fa, Square),
    cat("aftermarket_spot", Some(K::AftermarketSpot), 0xa78bfa, Circle),
    cat("aftermarket_board", Some(K::AftermarketBoard), 0xa78bfa, Square),
    cat("horizon_job", Some(K::HorizonJob), 0x38bdf8, Circle),
    cat("horizon_story", Some(K::HorizonStory), 0xfb923c, Circle),
    cat("job_activation", Some(K::JobActivation), 0x38bdf8, Ring),
    cat("story_activation", Some(K::StoryActivation), 0xfb923c, Ring),
    cat("special_event", Some(K::SpecialEvent), 0xfacc15, Diamond),
    cat("rush_event", Some(K::RushEvent), 0xfacc15, Circle),
    cat("showcase", Some(K::Showcase), 0xe879f9, Diamond),
    cat("treasure_car", Some(K::TreasureCar), 0xfde047, Circle),
    cat("upsell", Some(K::Upsell), 0xa78bfa, Diamond),
    cat("pinata", Some(K::Pinata), 0xf472b6, Circle),
    cat("eliminator_spawn", Some(K::Eliminator), 0xef4444, Circle),
    cat("parking_area", Some(K::Parking), 0x64748b, Square),
    cat("creature_zone", Some(K::CreatureZone), 0x86efac, Ring),
    cat("flag_rush_flag", Some(K::FlagRushFlag), 0x4ade80, Diamond),
    cat("treasure_chest_board", Some(K::TreasureChestBoard), 0xfde047, Square),
    cat("treasure_chest", Some(K::TreasureChest), 0xfde047, Diamond),
    PoiCat { id: "treasure_chest_current", kind: None, icon: Some(K::TreasureChest), color: Rgb::hex(0xfde047), shape: Diamond },
    cat("xp_board", Some(K::XpBoard), 0xa3e635, Square),
    cat("mascot", Some(K::Mascot), 0xf0abfc, Circle),
    cat("speed_trap", Some(K::SpeedTrap), 0x38bdf8, Diamond),
    cat("speed_zone", Some(K::SpeedZone), 0x38bdf8, Square),
    cat("trailblazer", Some(K::Trailblazer), 0x34d399, Square),
    cat("drift_zone", Some(K::DriftZone), 0xfb923c, Square),
    cat("danger_sign", Some(K::DangerSign), 0xf87171, Diamond),
];

/// The current treasure chest is drawn this much bigger than the other POIs.
pub const CURRENT_CHEST_SCALE: f32 = 1.5;

/// Gate lines (speed zone / trailblazer / drift zone / speed trap): line width in px at scale 1,
/// and the shortest drawn length (a gate is ~10-20 m wide: sub-pixel at 5 km, so it is
/// stretched to at least this, about its midpoint).
pub const GATE_PX: f32 = 2.5;
pub const GATE_CASING_PX: f32 = 1.5;
pub const GATE_MIN_LEN_PX: f32 = 7.0;

// ── tilt ─────────────────────────────────────────────────────────────────────────────────────

/// The tilted map fades into the backing at its far edge over this much depth scale above
/// `view::FAR_MIN_SCALE` (alpha 0 there, 1 at `FAR_MIN_SCALE + FAR_FADE_DEPTH`). A band just
/// under the horizon, so it only shows when a steep tilt brings the horizon into view (about
/// 14 px of the pill at 55 deg, all of it above the pill). It used to be 30 % of the view height
/// below a fixed far line (the demo's `H * 0.3`), which faded the top half of the pill.
pub const FAR_FADE_DEPTH: f32 = 0.1;

/// Depth taper: line widths scale with the perspective factor at their screen row, quantised to
/// this many bands (one `Shape` per band piece).
pub const TAPER_BANDS: usize = 8;

/// POIs shrink with the perspective but never below this share of their size (else far icons
/// become specks).
pub const POI_MIN_K: f32 = 0.4;

/// Tilted: POIs further away than where things are this small are not drawn (the current
/// treasure chest still is). The plane reaches nearly to the horizon, and the far strip would
/// otherwise be a pile of minimum-size icons; this is about where the plane used to end at the
/// HUD's 55 deg (the top ~18 px of the pill stay icon-free).
pub const POI_FAR_K: f32 = 0.3;

/// The category index of a config id.
pub fn cat_index(id: &str) -> Option<usize> {
    POI_CATS.iter().position(|c| c.id == id)
}

/// Bit mask over [`POI_CATS`] of the enabled category ids (unknown ids ignored).
pub fn cat_mask(ids: &[String]) -> u64 {
    ids.iter().filter_map(|i| cat_index(i)).fold(0u64, |m, i| m | (1u64 << i))
}

// ── race lines ───────────────────────────────────────────────────────────────────────────────

/// Start dot of a point-to-point sprint.
pub const START_DOT: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
pub const START_DOT_OUTLINE: Color32 = Color32::from_rgb(0x08, 0x10, 0x0a);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::roadtypes::RoadType;
    use crate::maprender::cfg::MapLayerConfig;

    #[test]
    fn style_table_covers_every_road_type_and_turnaround_is_never_drawn() {
        let c = MapLayerConfig::default();
        for t in RoadType::ALL {
            match t {
                RoadType::Turnaround => {
                    assert!(c.roads.styles.get(t).is_none());
                    assert!(!ROAD_DRAW_ORDER.contains(&(t.index() as usize)), "turnaround slot is drawn");
                }
                _ => {
                    assert!(c.roads.styles.get(t).is_some(), "{t:?}");
                    assert!(ROAD_DRAW_ORDER.contains(&(t.index() as usize)) || t == RoadType::Jump, "{t:?} missing from the draw order");
                }
            }
        }
        // Jump lines are arrows drawn after the chains, not part of the chain order.
        assert!(!ROAD_DRAW_ORDER.contains(&(RoadType::Jump.index() as usize)));
        // Asphalt on top: road before highway, both after the sand types.
        let pos = |t: RoadType| ROAD_DRAW_ORDER.iter().position(|&s| s == t.index() as usize).unwrap();
        assert!(pos(RoadType::Offroad) < pos(RoadType::Road) && pos(RoadType::Road) < pos(RoadType::Highway));
        assert_eq!(ROAD_DRAW_ORDER[0], 0);
        let mut seen = ROAD_DRAW_ORDER.to_vec();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), ROAD_DRAW_ORDER.len());
    }

    /// The demo's `roadBase` (index.html:278) evaluated by hand for the user's export
    /// (`metres 10, minPx 1, maxPx 10, basePx 3`): 3 zooms on a 420 px view.
    #[test]
    fn width_function_matches_the_demo_at_three_zooms() {
        let c = RoadsCfg::default();
        let k = |zoom_m: f32| 420.0 / (2.0 * zoom_m); // MapView::new: view_px / (2 * zoom)
        // 5 km: 0.042 px/m * 10 = 0.42 → clamped up to minPx 1.
        assert_eq!(road_base_px(&c, k(5000.0)), 1.0);
        // 300 m: 0.7 px/m * 10 = 7 inside the clamp.
        assert!((road_base_px(&c, k(300.0)) - 7.0).abs() < 1e-5);
        // 100 m: 2.1 * 10 = 21 → clamped down to maxPx 10.
        assert_eq!(road_base_px(&c, k(100.0)), 10.0);
        // Per type: max(0.7, base * factor), road 1.0 / highway 1.44 / offroad 0.875 at base 1.
        assert_eq!(line_px(1.0, 1.0), 1.0);
        assert!((line_px(1.0, 1.44) - 1.44).abs() < 1e-6);
        assert_eq!(line_px(1.0, 0.5), 0.7);
        // Fixed width when it does not scale with the zoom.
        let fixed = RoadsCfg { scale_with_zoom: false, ..RoadsCfg::default() };
        assert_eq!(road_base_px(&fixed, 123.0), 3.0);
    }

    #[test]
    fn dash_patterns_follow_the_demo() {
        assert_eq!(dash_pattern(DashStyle::None, 3.0), None);
        assert_eq!(dash_pattern(DashStyle::Dashed, 1.0), Some((6.0, 4.0)));
        let (a, g) = dash_pattern(DashStyle::Dashed, 5.0).unwrap();
        assert!((a - 18.0).abs() < 1e-5 && (g - 12.0).abs() < 1e-5);
        assert_eq!(dash_pattern(DashStyle::Dotted, 1.0), Some((1.0, 4.0)));
    }

    #[test]
    fn poi_categories_are_unique_and_default_on_names_all_resolve() {
        for (i, c) in POI_CATS.iter().enumerate() {
            assert!(POI_CATS[..i].iter().all(|o| o.id != c.id), "duplicate {}", c.id);
            if let Some(k) = c.kind {
                assert!(POI_CATS[..i].iter().all(|o| o.kind != Some(k)), "kind {k:?} twice");
            }
        }
        assert!(POI_CATS.len() <= 64);
        for n in crate::maprender::cfg::POI_DEFAULT_ON {
            assert!(cat_index(n).is_some(), "{n}");
        }
        // Danger signs are a kind of their own now; the current chest is not (it is one of the
        // chests, picked by the week) but shows the chest's icon.
        let by = |id: &str| &POI_CATS[cat_index(id).unwrap()];
        assert_eq!(by("danger_sign").kind, Some(PoiKind::DangerSign));
        assert_eq!((by("treasure_chest_current").kind, by("treasure_chest_current").icon_kind()), (None, Some(PoiKind::TreasureChest)));
        assert_eq!(by("treasure_chest").icon_kind(), Some(PoiKind::TreasureChest));
        let m = cat_mask(&["house".into(), "nope".into(), "danger_sign".into()]);
        assert_eq!(m.count_ones(), 2);
        assert_ne!(m & (1 << cat_index("house").unwrap()), 0);
    }
}
