//! Config of the shared map renderer: what the Dashboard map (`AppConfig::minimap_layers`) and
//! the HUD minimap (`OverlayConfig::map_layers`) draw on top of / instead of the satellite
//! image. Plain serde types, no egui state; the defaults are the user's demo export (D62).
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

/// Category names switched on by default (D62): the demo's category ids, see `style::POI_CATS`.
pub const POI_DEFAULT_ON: &[&str] = &[
    "barn_find",
    "car_meet",
    "fast_travel",
    "festival_site",
    "house",
    "aftermarket_spot",
    "aftermarket_board",
    "horizon_job",
    "horizon_story",
    "xp_board",
    "speed_trap",
    "speed_zone",
    "trailblazer",
    "drift_zone",
    "danger_sign",
    "treasure_chest_current",
];

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
            max_zoom_m: 3000.0,
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
    /// Circuits and point-to-point sprints get their own colour.
    pub circuit_color: Rgb,
    pub sprint_color: Rgb,
    pub alpha: f32,
    /// Start / finish marks.
    pub marks: bool,
    /// What the rest of the map does while racing (D66).
    pub focus: RaceFocusCfg,
}

impl Default for RaceCfg {
    fn default() -> Self {
        Self {
            mode: RaceLineMode::Current,
            radius_m: 1500.0,
            width_px: 4.0,
            circuit_color: Rgb::hex(0x38bdf8),
            sprint_color: Rgb::hex(0xfb7185),
            alpha: 0.85,
            marks: true,
            focus: RaceFocusCfg::default(),
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
    /// Not drawn.
    Hidden,
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
/// construction); off = constant widths.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct TiltCfg {
    pub on: bool,
    pub angle_deg: f32,
    pub perspective_px: f32,
    pub car_y: f32,
    pub taper: bool,
}

impl Default for TiltCfg {
    fn default() -> Self {
        Self { on: false, angle_deg: 55.0, perspective_px: 200.0, car_y: 0.85, taper: true }
    }
}

// ── the whole thing ──────────────────────────────────────────────────────────────────────────

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
}

impl MapLayerConfig {
    /// Dashboard map: satellite at full strength, flat.
    pub fn dashboard() -> Self {
        Self::default()
    }

    /// HUD minimap (D62): satellite dimmed to 50 % in opacity, brightness and saturation so the
    /// vectors carry the picture, and tilted.
    pub fn hud() -> Self {
        let mut c = Self::default();
        c.image = ImageCfg { on: true, opacity: 0.5, brightness: 0.5, saturation: 0.5 };
        c.tilt.on = true;
        c
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
    }

    #[test]
    fn defaults_are_the_demo_export() {
        let d = MapLayerConfig::default();
        assert_eq!(d.race_lines.mode, RaceLineMode::Current);
        assert_eq!((d.race_lines.width_px, d.race_lines.alpha), (4.0, 0.85));
        assert_eq!((d.pois.size_px, d.pois.max_zoom_m), (32.0, 3000.0));
        assert_eq!((d.roads.metres, d.roads.min_px, d.roads.max_px, d.roads.base_px), (10.0, 1.0, 10.0, 3.0));
        assert!(!d.tilt.on && MapLayerConfig::hud().tilt.on);
        assert_eq!(MapLayerConfig::hud().image.opacity, 0.5);
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
}
