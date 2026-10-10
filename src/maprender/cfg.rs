//! Config of the shared map renderer: what the Dashboard map (`AppConfig::minimap_layers`) and
//! the HUD minimap (`OverlayConfig::map_layers`) draw on top of / instead of the satellite
//! image. Plain serde types, no egui state; the defaults are the user's own settings of 2026-10-08 (D71; before: the demo export, D62).
//!
//! *Why one struct for both maps:* D61 wants one renderer with different parameters. The two
//! constructors, [`MapLayerConfig::dashboard`] (the `Default`) and [`MapLayerConfig::hud`], are
//! the only places the maps differ.
//!
//! *Why colours are `"#rrggbb"` strings and categories are names:* the settings travel in
//! preset / profile JSON that people read and edit by hand, and a name list stays valid when a
//! later game-data task adds a POI kind (an unknown name is simply ignored).

use egui::Color32;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::gamedata::roadtypes::RoadType;

// ── colours ──────────────────────────────────────────────────────────────────────────────────

/// An opaque colour, written as `"#rrggbb"`. A value that does not parse becomes magenta
/// instead of failing the whole config load (the colour is then obviously wrong, not lost).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub const fn hex(v: u32) -> Rgb {
        Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        u32::from_str_radix(h, 16).ok().map(Rgb::hex)
    }
    /// With alpha 0..=1 (straight, converted to egui's premultiplied form).
    pub fn color(self, alpha: f32) -> Color32 {
        let [r, g, b] = self.0;
        Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let [r, g, b] = self.0;
        s.serialize_str(&format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Rgb, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(v.as_str().and_then(Rgb::parse).unwrap_or(Rgb([255, 0, 255])))
    }
}

// ── roads ────────────────────────────────────────────────────────────────────────────────────

/// How a road type's line is broken up. Pattern lengths are in px at width 1.0 and grow with
/// the line (the demo's `DASH`), see `style::dash_pattern`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum DashStyle {
    #[default]
    None,
    Dashed,
    Short,
    Dotted,
}

/// One road type's look.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct RoadTypeStyle {
    pub on: bool,
    pub color: Rgb,
    /// Width factor on top of the zoom-dependent base width ([`RoadsCfg::base_px`]).
    pub width: f32,
    pub dash: DashStyle,
    pub alpha: f32,
    /// Dark outline under the line so it reads on the satellite image.
    pub casing: bool,
    pub casing_color: Rgb,
}

impl RoadTypeStyle {
    const fn new(color: u32, width: f32, dash: DashStyle, alpha: f32, casing: u32) -> Self {
        Self { on: true, color: Rgb::hex(color), width, dash, alpha, casing: true, casing_color: Rgb::hex(casing) }
    }
}

impl Default for RoadTypeStyle {
    fn default() -> Self {
        Self::new(0xffffff, 1.0, DashStyle::None, 1.0, 0x000000)
    }
}

/// The "by type" preset of the demo (the editor's type colours): each type its own colour with
/// a darker outline of the same hue. Turnarounds have no entry: they are never drawn (D52).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(from = "RoadStylesPartial")]
pub struct RoadStyles {
    pub road: RoadTypeStyle,
    pub highway: RoadTypeStyle,
    pub offroad: RoadTypeStyle,
    pub other: RoadTypeStyle,
    pub trail: RoadTypeStyle,
    pub crosscountry: RoadTypeStyle,
    pub tunnel: RoadTypeStyle,
    pub jump: RoadTypeStyle,
}

/// A style as written in a file: whatever is missing keeps *that type's* default (a plain
/// `#[serde(default)]` would fill it from the white placeholder instead).
#[derive(Deserialize, Default)]
#[serde(default)]
struct PartialStyle {
    on: Option<bool>,
    color: Option<Rgb>,
    width: Option<f32>,
    dash: Option<DashStyle>,
    alpha: Option<f32>,
    casing: Option<bool>,
    casing_color: Option<Rgb>,
}

impl PartialStyle {
    fn onto(self, mut b: RoadTypeStyle) -> RoadTypeStyle {
        b.on = self.on.unwrap_or(b.on);
        b.color = self.color.unwrap_or(b.color);
        b.width = self.width.unwrap_or(b.width);
        b.dash = self.dash.unwrap_or(b.dash);
        b.alpha = self.alpha.unwrap_or(b.alpha);
        b.casing = self.casing.unwrap_or(b.casing);
        b.casing_color = self.casing_color.unwrap_or(b.casing_color);
        b
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RoadStylesPartial {
    road: PartialStyle,
    highway: PartialStyle,
    offroad: PartialStyle,
    other: PartialStyle,
    trail: PartialStyle,
    crosscountry: PartialStyle,
    tunnel: PartialStyle,
    jump: PartialStyle,
}

impl From<RoadStylesPartial> for RoadStyles {
    fn from(p: RoadStylesPartial) -> Self {
        let d = RoadStyles::default();
        Self {
            road: p.road.onto(d.road),
            highway: p.highway.onto(d.highway),
            offroad: p.offroad.onto(d.offroad),
            other: p.other.onto(d.other),
            trail: p.trail.onto(d.trail),
            crosscountry: p.crosscountry.onto(d.crosscountry),
            tunnel: p.tunnel.onto(d.tunnel),
            jump: p.jump.onto(d.jump),
        }
    }
}

impl Default for RoadStyles {
    fn default() -> Self {
        use DashStyle::{Dashed, None as Solid};
        Self {
            road: RoadTypeStyle::new(0x38bdf8, 1.0, Solid, 1.0, 0x06222f),
            highway: RoadTypeStyle::new(0xfbbf24, 1.44, Solid, 1.0, 0x1c1403),
            offroad: RoadTypeStyle::new(0xff8c1a, 0.875, Solid, 1.0, 0x3a1c00),
            other: RoadTypeStyle::new(0x4ade80, 0.81, Solid, 1.0, 0x06260f),
            trail: RoadTypeStyle::new(0xfacc15, 0.75, Dashed, 1.0, 0x3a3000),
            crosscountry: RoadTypeStyle::new(0xa78bfa, 0.75, Solid, 1.0, 0x1e1240),
            tunnel: RoadTypeStyle::new(0xe2e8f0, 0.94, Solid, 0.85, 0x0b0e14),
            jump: RoadTypeStyle::new(0xf43f5e, 0.875, Dashed, 1.0, 0x3a0610),
        }
    }
}

impl RoadStyles {
    /// The style of `t`; `None` for the turnaround, which is never drawn.
    pub fn get(&self, t: RoadType) -> Option<&RoadTypeStyle> {
        Some(match t {
            RoadType::Road => &self.road,
            RoadType::Highway => &self.highway,
            RoadType::Offroad => &self.offroad,
            RoadType::Other => &self.other,
            RoadType::Trail => &self.trail,
            RoadType::Crosscountry => &self.crosscountry,
            RoadType::Tunnel => &self.tunnel,
            RoadType::Jump => &self.jump,
            RoadType::Turnaround => return None,
        })
    }
}

/// Roads by type, drawn over the image.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct RoadsCfg {
    pub on: bool,
    pub styles: RoadStyles,
    /// Width follows the zoom: `clamp(px_per_metre * metres, min_px, max_px)`; off = the fixed
    /// `base_px`. (The demo's `road.scaleZoom / metres / minPx / maxPx / basePx`.)
    pub scale_with_zoom: bool,
    pub metres: f32,
    pub min_px: f32,
    pub max_px: f32,
    pub base_px: f32,
    /// Extra px of the casing on top of the line width, and its alpha.
    pub casing_px: f32,
    pub casing_alpha: f32,
}

impl Default for RoadsCfg {
    fn default() -> Self {
        Self {
            on: true,
            styles: RoadStyles::default(),
            scale_with_zoom: true,
            metres: 10.0,
            min_px: 1.0,
            max_px: 10.0,
            base_px: 3.0,
            casing_px: 1.4,
            casing_alpha: 1.0,
        }
    }
}

// ── points of interest ───────────────────────────────────────────────────────────────────────

/// Category names switched on by default on the Dashboard map (D71: the user's own selection;
/// category ids see `style::POI_CATS`).
pub const POI_DEFAULT_ON: &[&str] = &[
    "barn_find",
    "car_meet",
    "festival_site",
    "house",
    "aftermarket_spot",
    "aftermarket_board",
    "speed_trap",
    "speed_zone",
    "trailblazer",
    "drift_zone",
    "danger_sign",
    "treasure_chest_current",
];

/// The HUD minimap's default categories (D71): the ones that matter at a glance in a 300 m view.
pub const POI_HUD_DEFAULT_ON: &[&str] =
    &["festival_site", "house", "speed_trap", "speed_zone", "trailblazer", "drift_zone", "danger_sign"];

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct PoisCfg {
    pub on: bool,
    /// Icon (or fallback marker) size in px at scale 1.
    pub size_px: f32,
    /// Hidden while the view's radius (metres from centre to edge) is above this.
    pub max_zoom_m: f32,
    /// Only POIs within `radius_m` of the car.
    pub near_only: bool,
    pub radius_m: f32,
    /// Draw the gate line of speed zones / trailblazers / drift zones (needs the gate ends from
    /// the game-data reader; see `docs/features/minimap.md`).
    pub gates: bool,
    /// Enabled category ids (`style::POI_CATS`); unknown names are ignored.
    pub categories: Vec<String>,
}

impl Default for PoisCfg {
    fn default() -> Self {
        Self {
            on: true,
            size_px: 32.0,
            max_zoom_m: 10000.0,
            near_only: false,
            radius_m: 2000.0,
            gates: true,
            categories: POI_DEFAULT_ON.iter().map(|s| s.to_string()).collect(),
        }
    }
}

// ── race lines ───────────────────────────────────────────────────────────────────────────────

/// Which race lines to draw.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum RaceLineMode {
    Off,
    /// The race the car is in (best effort, see `racesel`).
    #[default]
    Current,
    /// The line nearest to the car (within [`RaceCfg::radius_m`]).
    Nearest,
    /// Every line within [`RaceCfg::radius_m`] of the car.
    Near,
    /// All lines in view.
    All,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct RaceCfg {
    pub mode: RaceLineMode,
    pub radius_m: f32,
    pub width_px: f32,
    /// One colour for every race, circuit or sprint (D88). *Why (the user, 2026-10-10):* "Sprints
    /// and circuits should have the same color". `circuit_color` is the old key (old configs keep
    /// their circuit colour; the old `sprint_color` key is ignored on load).
    #[serde(alias = "circuit_color")]
    pub color: Rgb,
    pub alpha: f32,
    /// Start / finish marks.
    pub marks: bool,
    /// What the rest of the map does while racing (D66).
    pub focus: RaceFocusCfg,
    /// How the race lines are drawn (D80): as a road of their own (casing + fill in the race
    /// colour; in 3D a road deck along the line's own heights) or as the thin line of before.
    pub route: RouteStyle,
}

/// How race lines are drawn (D80).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum RouteStyle {
    /// A road of its own along the race line: the road look (width rule, casing under fill,
    /// round ends), in the race colour, over the roads it runs on. In 3D a road deck at the line's
    /// own heights (`racesel::RaceRoad`, `gl3d`). *Why (the user, 2026-10-09):* first "I dont like
    /// the way that the racetrack is drawn on top of the road", then, after a cross-country race
    /// off the road network, "it should also just draw a 3d road to draw the race line" (so not the
    /// nav roads recoloured: a race does not have to follow them).
    #[default]
    Road,
    /// The thin race line drawn on top of the map (the look before D80).
    Line,
}

impl Default for RaceCfg {
    fn default() -> Self {
        Self {
            mode: RaceLineMode::Current,
            radius_m: 1500.0,
            width_px: 4.0,
            color: Rgb::hex(0xf97316), // orange: the old circuit blue (#38bdf8) was the road colour (D86, D88)
            alpha: 0.85,
            marks: true,
            focus: RaceFocusCfg::default(),
            route: RouteStyle::Road,
        }
    }
}

// ── race focus ───────────────────────────────────────────────────────────────────────────────

/// What happens to the roads that are not part of the detected race while racing.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum OtherRoads {
    /// Drawn as always.
    Normal,
    /// Drawn in one faint neutral colour without casing (see [`RaceFocusCfg`]).
    #[default]
    Muted,
    /// Not drawn (the roads along the race corridor still are).
    Hidden,
    /// No road of the road layer at all, along the race or not (no casings, caps or jump lines
    /// either): only the race road (D82). Needs `RouteStyle::Road`; with `RouteStyle::Line` it
    /// falls back to `Hidden` (a lone thin line on the image would be all that is left). *Why (the
    /// user, 2026-10-09):* "Here should be a setting, to not draw anything from the normal road
    /// mesh and only draw the circuit using the 3d renderer".
    #[serde(rename = "race_only")]
    RaceOnly,
}

impl OtherRoads {
    /// What applies with race lines drawn as `route`: `RaceOnly` needs the race road.
    pub fn effective(self, route: RouteStyle) -> OtherRoads {
        match (self, route) {
            (OtherRoads::RaceOnly, RouteStyle::Line) => OtherRoads::Hidden,
            (o, _) => o,
        }
    }
}

/// The in-race focus (D66): while the car is in a race **and** a race line is selected (the
/// `Current` mode's guess found one), roads away from that line are muted or hidden and the
/// points of interest can be hidden, so the race stands out. With no selected line nothing
/// changes: a wrong guess must never blank the map. Relevance rules: `maprender::racesel`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct RaceFocusCfg {
    /// Roads off the race line's corridor.
    pub other_roads: OtherRoads,
    /// `Muted` look: colour, alpha and a width factor on top of each type's own width.
    pub mute_color: Rgb,
    pub mute_alpha: f32,
    pub mute_width: f32,
    /// Hide POIs (icons, gates, the current chest) while the focus is on. The race's start /
    /// finish marks stay.
    pub hide_pois: bool,
}

impl Default for RaceFocusCfg {
    fn default() -> Self {
        Self { other_roads: OtherRoads::Muted, mute_color: Rgb::hex(0xffffff), mute_alpha: 0.25, mute_width: 0.8, hide_pois: true }
    }
}

// ── image + tilt ─────────────────────────────────────────────────────────────────────────────

/// The satellite image under the vectors.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct ImageCfg {
    pub on: bool,
    pub opacity: f32,
    pub brightness: f32,
    /// Approximate: egui cannot desaturate a texture, so a grey veil stands in (`paint2d`).
    pub saturation: f32,
}

impl Default for ImageCfg {
    fn default() -> Self {
        Self { on: true, opacity: 1.0, brightness: 1.0, saturation: 1.0 }
    }
}

/// The tilted view: a flat perspective of the 2D map (D65). `perspective_px` is the eye
/// distance in px for a view as tall as the HUD pill (136 px); a taller view scales it with its
/// height (`Camera::focal_for`), so the same settings give the same picture on both maps.
/// `car_y` is where the car sits on the view's height (0 = top, 1 = bottom). `taper`: line
/// widths shrink towards the far edge with the perspective (the demo's CSS tilt does it by
/// construction); off = constant widths. `relief` turns the tilted view into the 3D one (phase K).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct TiltCfg {
    pub on: bool,
    pub angle_deg: f32,
    pub perspective_px: f32,
    pub car_y: f32,
    pub taper: bool,
    pub relief: ReliefCfg,
}

impl Default for TiltCfg {
    fn default() -> Self {
        Self { on: false, angle_deg: 55.0, perspective_px: 200.0, car_y: 0.85, taper: true, relief: ReliefCfg::default() }
    }
}

/// How a map is seen: flat 2D, the tilted flat map (D65), or the 3D relief view (phase K). Not
/// stored: derived from `tilt.on` and `tilt.relief.on` ([`TiltCfg::view_mode`]).
#[allow(dead_code)] // phase K: the settings card (K5) and the call sites (K3, K4) read it
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Flat,
    Tilted,
    /// "3D": terrain relief, roads with heights and decks. Called `Relief` because a Rust
    /// identifier cannot start with a number.
    Relief,
}

/// Where 3D roads get their height (D51): the nav **nodes'** own heights (bridges and elevated
/// expressways float; the default, the user reversed the earlier "always drape" decision) or the
/// terrain under them (`Terrain`, "drape"). Cross-country is always draped and jump lines are
/// always a taut string, whatever this says (see `mesh3d`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoadHeight {
    #[default]
    Nodes,
    Terrain,
}

/// Settings of the 3D view that the flat tilt does not have (phase K, design §8). Nested in
/// [`TiltCfg`] so old configs load (`serde(default)`), the Copy-to-category machinery
/// ([`LayerCategory::Tilt`]) copies it with the rest of the view settings, and the angle /
/// perspective / car position of the tilted view *are* the 3D camera's pitch / FOV / car
/// position (they are not duplicated here): that is what makes the three modes interchangeable.
///
/// Defaults (design §9.3): off on every map until the user has seen it; true relief
/// (`exaggeration` 1.0); the sea is flat at the real sea level (not configurable); a mild hill
/// shading of 0.35 over the satellite image; a 3 m deck under the roads.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct ReliefCfg {
    /// 3D instead of the flat tilt (implies `tilt.on`; ignored when that is off).
    pub on: bool,
    pub road_height: RoadHeight,
    /// Thickness of the road decks in metres (0 = paper-thin ribbons), [`ReliefCfg::DECK_RANGE`].
    pub deck_m: f32,
    /// Vertical scale of the world about y = 0 (the eye follows), [`ReliefCfg::EXAG_RANGE`].
    pub exaggeration: f32,
    /// Strength of the hill shading over the imagery, [`ReliefCfg::SHADING_RANGE`].
    pub shading: f32,
    /// The own-car marker in the 3D scene (D78): a 3D arrow (default, today's look) or a
    /// low-poly sedan. Flat / Tilted always draw the flat arrow.
    pub marker: MarkerStyle,
}

/// What the own car looks like in the 3D scene (D78). *Why a choice:* the user asked for both
/// ("a really simple 3d sedan model ... plus a normal arrow 3d model, and let the user decide");
/// the arrow is the default because it keeps today's look until the user picks the car.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum MarkerStyle {
    #[default]
    Arrow,
    Sedan,
}

impl ReliefCfg {
    pub const DECK_RANGE: (f32, f32) = (0.0, 20.0);
    pub const EXAG_RANGE: (f32, f32) = (0.5, 3.0);
    pub const SHADING_RANGE: (f32, f32) = (0.0, 1.0);

    /// The values clamped to their ranges (a hand-edited config can hold anything; NaN becomes
    /// the default).
    pub fn sane(self) -> ReliefCfg {
        let d = ReliefCfg::default();
        let fix = |v: f32, r: (f32, f32), d: f32| if v.is_finite() { v.clamp(r.0, r.1) } else { d };
        ReliefCfg {
            deck_m: fix(self.deck_m, Self::DECK_RANGE, d.deck_m),
            exaggeration: fix(self.exaggeration, Self::EXAG_RANGE, d.exaggeration),
            shading: fix(self.shading, Self::SHADING_RANGE, d.shading),
            ..self
        }
    }
}

impl Default for ReliefCfg {
    fn default() -> Self {
        Self { on: false, road_height: RoadHeight::Nodes, deck_m: 3.0, exaggeration: 1.0, shading: 0.35, marker: MarkerStyle::Arrow }
    }
}

#[allow(dead_code)] // see ViewMode
impl TiltCfg {
    pub fn view_mode(&self) -> ViewMode {
        if !self.on {
            ViewMode::Flat
        } else if self.relief.on {
            ViewMode::Relief
        } else {
            ViewMode::Tilted
        }
    }

    /// Switch to `mode`: writes `on` / `relief.on` and nothing else (the angle etc. stay, so
    /// going back and forth keeps the user's look).
    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.on = mode != ViewMode::Flat;
        self.relief.on = mode == ViewMode::Relief;
    }
}

// ── navigation route ─────────────────────────────────────────────────────────────────────────

/// The navigation route's look on a map (phase L, D84 / D92). The route itself comes from
/// `nav::view()`; this is only how a map shows it. It is a road of its own, like the race road
/// (D80): casing under a fill in `color`, round ends, in 3D a deck along the route's heights.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct NavRouteCfg {
    /// Draw the route and the destination pin on this map.
    pub on: bool,
    /// Fill colour of the route and of a local destination's pin. Default fuchsia `#d946ef`: used
    /// by no road type that is drawn (turnaround has it but is never drawn, D52) and clear of the
    /// race colours (orange, pink).
    pub color: Rgb,
    /// Factor on the route's width (`1.0` = [`super::style::NAV_ROUTE_WIDTH`] on the roads' width
    /// rule, i.e. a little wider than a highway).
    pub width: f32,
}

impl Default for NavRouteCfg {
    fn default() -> Self {
        Self { on: true, color: Rgb::hex(0xd946ef), width: 1.0 }
    }
}

impl NavRouteCfg {
    /// Narrowest and widest `width` factor a map draws.
    pub const WIDTH_RANGE: (f32, f32) = (0.5, 3.0);

    /// `width` clamped to [`Self::WIDTH_RANGE`] (NaN = 1): what the renderers use, so a hand-edited
    /// config cannot make the route invisible or fill the map.
    pub fn width_factor(&self) -> f32 {
        if self.width.is_finite() {
            self.width.clamp(Self::WIDTH_RANGE.0, Self::WIDTH_RANGE.1)
        } else {
            1.0
        }
    }
}

// ── the whole thing ──────────────────────────────────────────────────────────────────────────

/// The categories of a [`MapLayerConfig`] (= the settings cards of the Map tab). Race lines
/// include their in-race focus.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LayerCategory {
    Image,
    Tilt,
    RaceLines,
    Roads,
    Pois,
    /// The navigation route's look (L3); its card comes with the Navigation tab work (L5).
    #[allow(dead_code)] // constructed by the card's "Copy to ..." row (L5); tested here
    NavRoute,
}

impl LayerCategory {
    /// The categories that have a card on the Map tab today. `NavRoute` joins when L5 adds its
    /// card (the `ui.rs` copy test's fixture `two_maps` then needs differing `nav_route` values);
    /// until then `cfg::tests` covers its copy explicitly.
    #[cfg(test)]
    pub const ALL: [LayerCategory; 5] =
        [LayerCategory::Image, LayerCategory::Tilt, LayerCategory::RaceLines, LayerCategory::Roads, LayerCategory::Pois];
}

/// Everything the shared renderer draws besides the markers (own arrow, co-op, compass).
/// `Default` = the Dashboard map's defaults.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct MapLayerConfig {
    pub image: ImageCfg,
    pub roads: RoadsCfg,
    pub pois: PoisCfg,
    pub race_lines: RaceCfg,
    pub tilt: TiltCfg,
    /// The navigation route and the destination pin (phase L, D84). Per map, like the rest.
    pub nav_route: NavRouteCfg,
}

impl MapLayerConfig {
    /// Dashboard map: satellite at full strength, flat.
    pub fn dashboard() -> Self {
        Self::default()
    }

    /// HUD minimap (D62, D71): satellite at full opacity but dimmed to 50 % brightness and
    /// saturation so the vectors carry the picture, tilted 40 deg, POIs only near the car
    /// (1 km) and only the [`POI_HUD_DEFAULT_ON`] kinds, hidden above a 3 km view.
    pub fn hud() -> Self {
        let mut c = Self::default();
        c.image = ImageCfg { on: true, opacity: 1.0, brightness: 0.5, saturation: 0.5 };
        c.tilt.on = true;
        c.tilt.angle_deg = 40.0;
        c.pois.max_zoom_m = 3000.0;
        c.pois.near_only = true;
        c.pois.radius_m = 1000.0;
        c.pois.categories = POI_HUD_DEFAULT_ON.iter().map(|s| s.to_string()).collect();
        c
    }

    /// Overwrite **one category** of `self` with `from`'s; every other category stays as it is
    /// ("Copy to …" in the Map tab, D68). The one place that knows which field belongs to which
    /// category, so the UI never copies fields by hand.
    pub fn copy_category(&mut self, from: &MapLayerConfig, cat: LayerCategory) {
        match cat {
            LayerCategory::Image => self.image = from.image,
            LayerCategory::Tilt => self.tilt = from.tilt,
            LayerCategory::RaceLines => self.race_lines = from.race_lines,
            LayerCategory::Roads => self.roads = from.roads,
            LayerCategory::Pois => self.pois = from.pois.clone(),
            LayerCategory::NavRoute => self.nav_route = from.nav_route,
        }
    }

    /// Does any layer besides the image need the shared layer data (`store`)?
    pub fn wants_layers(&self) -> bool {
        self.roads.on || self.pois.on || self.race_lines.mode != RaceLineMode::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_round_trips_and_bad_values_do_not_break_the_load() {
        let c: Rgb = serde_json::from_str("\"#38bdf8\"").unwrap();
        assert_eq!(c, Rgb([0x38, 0xbd, 0xf8]));
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"#38bdf8\"");
        assert_eq!(serde_json::from_str::<Rgb>("\"nope\"").unwrap(), Rgb([255, 0, 255]));
        assert_eq!(serde_json::from_str::<Rgb>("12").unwrap(), Rgb([255, 0, 255]));
    }

    #[test]
    fn config_round_trips_and_partial_json_keeps_defaults() {
        let d = MapLayerConfig::default();
        let back: MapLayerConfig = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        let p: MapLayerConfig = serde_json::from_str(r##"{"roads":{"min_px":2.0,"styles":{"road":{"color":"#ffffff"}}},"tilt":{"on":true}}"##).unwrap();
        assert_eq!(p.roads.min_px, 2.0);
        assert_eq!(p.roads.max_px, 10.0);
        assert_eq!(p.roads.styles.road.color, Rgb([255, 255, 255]));
        assert_eq!(p.roads.styles.road.casing_color, Rgb::hex(0x06222f)); // untouched field of a touched style
        assert!(p.tilt.on && p.tilt.angle_deg == 55.0);
        assert_eq!(p.pois, d.pois);
        // A config saved before the race focus existed gets its defaults; partial values merge.
        assert_eq!(p.race_lines.focus, RaceFocusCfg::default());
        let f: MapLayerConfig = serde_json::from_str(r#"{"race_lines":{"focus":{"other_roads":"hidden"}}}"#).unwrap();
        assert_eq!((f.race_lines.focus.other_roads, f.race_lines.focus.mute_alpha, f.race_lines.focus.hide_pois), (OtherRoads::Hidden, 0.25, true));
        // D80: a config saved before the route style existed draws the route as roads.
        assert_eq!(p.race_lines.route, RouteStyle::Road);
        let r: MapLayerConfig = serde_json::from_str(r#"{"race_lines":{"route":"line"}}"#).unwrap();
        assert_eq!(r.race_lines.route, RouteStyle::Line);
    }

    #[test]
    fn defaults_are_the_demo_export() {
        let d = MapLayerConfig::default();
        assert_eq!(d.race_lines.mode, RaceLineMode::Current);
        assert_eq!((d.race_lines.width_px, d.race_lines.alpha), (4.0, 0.85));
        assert_eq!((d.pois.size_px, d.pois.max_zoom_m), (32.0, 10000.0));
        assert_eq!((d.roads.metres, d.roads.min_px, d.roads.max_px, d.roads.base_px), (10.0, 1.0, 10.0, 3.0));
        assert!(!d.tilt.on && MapLayerConfig::hud().tilt.on);
        assert_eq!(MapLayerConfig::hud().image.opacity, 1.0);
        assert_eq!(d.roads.styles.highway.width, 1.44);
        assert_eq!(d.roads.styles.tunnel.alpha, 0.85);
        for c in [&d, &MapLayerConfig::hud()] {
            assert_eq!(c.race_lines.focus.other_roads, OtherRoads::Muted);
            assert_eq!((c.race_lines.focus.mute_alpha, c.race_lines.focus.mute_width, c.race_lines.focus.mute_color), (0.25, 0.8, Rgb::hex(0xffffff)));
            assert!(c.race_lines.focus.hide_pois);
        }
        assert!(d.roads.styles.get(RoadType::Turnaround).is_none());
        assert!(RoadType::ALL.iter().filter(|t| **t != RoadType::Turnaround).all(|t| d.roads.styles.get(*t).is_some()));
    }

    /// D71: the user's own map settings of 2026-10-08 are the code defaults.
    #[test]
    fn defaults_are_the_users_settings_of_d71() {
        let cats = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let d = MapLayerConfig::dashboard();
        assert_eq!(d.pois.categories, cats(&["barn_find", "car_meet", "festival_site", "house", "aftermarket_spot", "aftermarket_board", "speed_trap", "speed_zone", "trailblazer", "drift_zone", "danger_sign", "treasure_chest_current"]));
        assert_eq!((d.pois.max_zoom_m, d.pois.near_only, d.pois.radius_m), (10000.0, false, 2000.0));
        assert_eq!((d.image.opacity, d.image.brightness, d.image.saturation), (1.0, 1.0, 1.0));
        assert!(!d.tilt.on);
        assert_eq!((d.roads.styles.road.color, d.roads.styles.other.width, d.roads.casing_px), (Rgb::hex(0x38bdf8), 0.81, 1.4));
        let h = MapLayerConfig::hud();
        assert_eq!(h.pois.categories, cats(&["festival_site", "house", "speed_trap", "speed_zone", "trailblazer", "drift_zone", "danger_sign"]));
        assert_eq!((h.pois.max_zoom_m, h.pois.near_only, h.pois.radius_m), (3000.0, true, 1000.0));
        assert_eq!((h.image.opacity, h.image.brightness, h.image.saturation), (1.0, 0.5, 0.5));
        assert!(h.tilt.on && h.tilt.angle_deg == 40.0);
        assert_eq!(h.roads, d.roads);
        assert_eq!(h.race_lines, d.race_lines);
    }

    /// A config whose every category differs from `MapLayerConfig::default()`.
    fn all_different() -> MapLayerConfig {
        let mut b = MapLayerConfig::hud();
        b.image.opacity = 0.2;
        b.tilt.angle_deg = 33.0;
        b.tilt.relief = ReliefCfg { on: true, road_height: RoadHeight::Terrain, deck_m: 7.0, exaggeration: 2.0, shading: 0.8, marker: MarkerStyle::Sedan }; // the 3D settings travel with the view
        b.race_lines.width_px = 9.0;
        b.race_lines.focus.other_roads = OtherRoads::Hidden; // the focus travels with the race lines
        b.race_lines.focus.mute_alpha = 0.6;
        b.roads.styles.road.color = Rgb::hex(0x123456);
        b.roads.casing_px = 3.0;
        b.pois.size_px = 20.0;
        b.pois.categories = vec!["barn_find".into()];
        b.nav_route = NavRouteCfg { on: false, color: Rgb::hex(0x00ff88), width: 2.0 };
        b
    }

    fn section(c: &MapLayerConfig, cat: LayerCategory) -> String {
        match cat {
            LayerCategory::Image => format!("{:?}", c.image),
            LayerCategory::Tilt => format!("{:?}", c.tilt),
            LayerCategory::RaceLines => format!("{:?}", c.race_lines),
            LayerCategory::Roads => format!("{:?}", c.roads),
            LayerCategory::Pois => format!("{:?}", c.pois),
            LayerCategory::NavRoute => format!("{:?}", c.nav_route),
        }
    }

    /// Every category, `NavRoute` included (it has no card yet, so it is not in `LayerCategory::ALL`).
    fn all_categories() -> impl Iterator<Item = LayerCategory> {
        LayerCategory::ALL.into_iter().chain([LayerCategory::NavRoute])
    }

    #[test]
    fn nav_route_defaults_serde_and_old_configs() {
        let d = NavRouteCfg::default();
        assert_eq!((d.on, d.color, d.width), (true, Rgb([0xd9, 0x46, 0xef]), 1.0));
        let json = serde_json::to_string(&MapLayerConfig::default()).unwrap();
        assert!(json.contains(r##""nav_route":{"on":true,"color":"#d946ef","width":1.0}"##), "{json}");
        assert_eq!(serde_json::from_str::<MapLayerConfig>(&json).unwrap(), MapLayerConfig::default());
        // A config from before the navigation has no `nav_route`: the defaults (on, fuchsia).
        let old: MapLayerConfig = serde_json::from_str(r#"{"tilt":{"on":true},"roads":{"min_px":2.0}}"#).unwrap();
        assert_eq!(old.nav_route, d);
        assert!(old.tilt.on && old.roads.min_px == 2.0);
        // A partial one keeps the other defaults; a bad colour does not break the load.
        let p: MapLayerConfig = serde_json::from_str(r##"{"nav_route":{"color":"#00ff00"}}"##).unwrap();
        assert_eq!(p.nav_route, NavRouteCfg { color: Rgb([0, 255, 0]), ..d });
        let b: MapLayerConfig = serde_json::from_str(r#"{"nav_route":{"on":false,"color":"nope"}}"#).unwrap();
        assert!(!b.nav_route.on);
        // The same on both maps' defaults, and the HUD variant.
        assert_eq!(MapLayerConfig::hud().nav_route, d);
        // The width the renderers use is clamped; NaN is 1.
        let w = |width: f32| NavRouteCfg { width, ..d }.width_factor();
        assert_eq!((w(0.0), w(99.0), w(1.7), w(f32::NAN)), (0.5, 3.0, 1.7, 1.0));
    }

    /// Copying a category changes that category on the target and nothing else; the source is
    /// untouched; after the copy the category is equal.
    #[test]
    fn copy_category_touches_only_its_category() {
        let a = MapLayerConfig::default();
        let b = all_different();
        for cat in all_categories() {
            assert_ne!(section(&a, cat), section(&b, cat), "{cat:?}: the fixture must differ");
            let mut t = a.clone();
            t.copy_category(&b, cat);
            for other in all_categories() {
                let want = if other == cat { section(&b, other) } else { section(&a, other) };
                assert_eq!(section(&t, other), want, "copying {cat:?} gave {other:?} the wrong values");
            }
            assert_eq!(b, all_different(), "the source must not change");
            // Idempotent, and copying onto an equal category is a no-op.
            let again = t.clone();
            t.copy_category(&b, cat);
            assert_eq!(t, again);
        }
    }

    /// Copying all five categories makes the target equal to the source.
    #[test]
    fn copying_every_category_clones_the_config() {
        let b = all_different();
        let mut t = MapLayerConfig::dashboard();
        for cat in all_categories() {
            t.copy_category(&b, cat);
        }
        assert_eq!(t, b);
    }

    #[test]
    fn relief_defaults_and_the_view_mode_derivation() {
        let r = ReliefCfg::default();
        assert_eq!((r.on, r.road_height, r.deck_m, r.exaggeration, r.shading, r.marker), (false, RoadHeight::Nodes, 3.0, 1.0, 0.35, MarkerStyle::Arrow));
        // Today's view modes stay: HUD tilted, Dashboard (and the Viewer) flat; 3D on neither.
        assert_eq!(MapLayerConfig::hud().tilt.view_mode(), ViewMode::Tilted);
        assert_eq!(MapLayerConfig::dashboard().tilt.view_mode(), ViewMode::Flat);
        let mut t = TiltCfg::default();
        assert_eq!(t.view_mode(), ViewMode::Flat);
        // `relief.on` alone does nothing without the tilt it implies.
        t.relief.on = true;
        assert_eq!(t.view_mode(), ViewMode::Flat);
        t.on = true;
        assert_eq!(t.view_mode(), ViewMode::Relief);
        t.relief.on = false;
        assert_eq!(t.view_mode(), ViewMode::Tilted);
        // set_view_mode round trips and keeps the look.
        t.angle_deg = 42.0;
        t.relief.deck_m = 9.0;
        for m in [ViewMode::Relief, ViewMode::Flat, ViewMode::Tilted, ViewMode::Relief] {
            t.set_view_mode(m);
            assert_eq!(t.view_mode(), m);
            assert_eq!((t.angle_deg, t.relief.deck_m), (42.0, 9.0));
        }
    }

    #[test]
    fn relief_serde_round_trips_and_old_configs_get_the_defaults() {
        let mut c = MapLayerConfig::hud();
        c.tilt.relief = ReliefCfg { on: true, road_height: RoadHeight::Terrain, deck_m: 5.0, exaggeration: 1.5, shading: 0.6, marker: MarkerStyle::Sedan };
        let json_check = serde_json::to_string(&c.tilt.relief).unwrap();
        assert!(json_check.contains(r#""marker":"sedan""#), "{json_check}");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""road_height":"terrain""#), "{json}");
        assert_eq!(serde_json::from_str::<MapLayerConfig>(&json).unwrap(), c);
        // A config from before phase K has no `relief`: defaults; the tilt values survive.
        let old: MapLayerConfig = serde_json::from_str(r#"{"tilt":{"on":true,"angle_deg":40.0,"perspective_px":200.0,"car_y":0.85,"taper":true}}"#).unwrap();
        assert_eq!(old.tilt.relief, ReliefCfg::default());
        assert_eq!(old.tilt.view_mode(), ViewMode::Tilted);
        // Partial relief merges with the defaults.
        let p: MapLayerConfig = serde_json::from_str(r#"{"tilt":{"relief":{"on":true,"deck_m":8.0}}}"#).unwrap();
        assert_eq!((p.tilt.relief.on, p.tilt.relief.deck_m, p.tilt.relief.exaggeration, p.tilt.relief.road_height), (true, 8.0, 1.0, RoadHeight::Nodes));
    }

    #[test]
    fn relief_sane_clamps_to_the_ranges() {
        let wild = ReliefCfg { on: true, road_height: RoadHeight::Terrain, deck_m: -4.0, exaggeration: 99.0, shading: f32::NAN, marker: MarkerStyle::Sedan };
        let s = wild.sane();
        assert_eq!((s.deck_m, s.exaggeration, s.shading), (0.0, 3.0, 0.35));
        assert!(s.on && s.road_height == RoadHeight::Terrain);
        assert_eq!(ReliefCfg::default().sane(), ReliefCfg::default());
    }
}
