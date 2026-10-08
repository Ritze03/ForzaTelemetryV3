//! The settings UI of the shared map renderer (D63): everything `MapLayerConfig` holds, as
//! cards, written once and used by both map tabs of the Overlay tab (Dashboard map and HUD
//! minimap).
//!
//! *Why one function for both maps:* the two maps are one renderer with two parameter sets
//! (D61). A control that exists for one must exist for the other, so [`layers_ui`] takes the
//! config to edit and is the only place that lays the layer cards out. What differs per map
//! (the view options' storage, the HUD's plate opacity, the module switch) is passed in or put
//! in the page's `lead` card.
//!
//! No explanatory text under options (STYLING-GUIDE): explanations are tooltips.

use egui::{pos2, vec2, Color32, Rect, RichText, Sense, Stroke, TextureId, Ui};

use super::cfg::{
    DashStyle, ImageCfg, MapLayerConfig, PoisCfg, RaceCfg, RaceLineMode, RoadStyles, RoadTypeStyle, RoadsCfg, TiltCfg,
};
use super::paint2d::IconAtlas;
use super::store::{LayerStatus, Layers};
use super::style::{self, Shape};
use crate::config::{AppConfig, OverlayConfig};
use crate::i18n::tr;
use crate::theme;
use crate::ui::overlay_tab::{control_row, control_row_tip, pct_row, status_line};

/// From this page width up the layer cards sit in three columns, below it in two (the same
/// switch as the rest of the Overlay tab, see `overlay_tab::THREE_COLS_MIN_W`).
const THREE_COLS_MIN_W: f32 = 1100.0;

// ── status ───────────────────────────────────────────────────────────────────────────────────

/// The layer store's state and its note (e.g. "override ignored"), shown above the cards of a
/// map tab so a map without roads explains itself.
pub fn status_ui(ui: &mut Ui, l: &Layers) {
    match &l.status {
        LayerStatus::NoInstall => status_line(
            ui,
            theme::FAINT,
            tr("No Forza Horizon 6 install found. Roads, points of interest and race lines need it."),
        ),
        LayerStatus::Loading => {
            status_line(ui, theme::FAINT, tr("Loading map layers…"));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        LayerStatus::Ready => status_line(ui, theme::GOOD, tr("Map layers loaded")),
        LayerStatus::Error(e) => status_line(ui, theme::DANGER, &format!("{} {e}", tr("Map layers failed:"))),
    }
    if let Some(note) = l.data.as_ref().and_then(|d| d.note.as_deref()) {
        status_line(ui, theme::WARN, note);
    }
}

// ── view options (zoom, orientation) ─────────────────────────────────────────────────────────

/// The view options both maps have, copied out of whichever config holds them (`AppConfig`'s
/// `minimap_*` keys for the Dashboard, `OverlayConfig`'s `map_*` keys for the HUD) so one
/// function edits both.
#[derive(Clone, PartialEq, Debug)]
pub struct ViewCfg {
    pub north_up: bool,
    pub north_up_when_stopped: bool,
    pub smooth_rotation: bool,
    pub use_movement_dir: bool,
    pub look_stick: bool,
    pub mirror_edges: bool,
    pub compass: bool,
    pub zoom_driving_m: f32,
    pub zoom_stopped_m: f32,
}

impl ViewCfg {
    pub fn of_app(c: &AppConfig) -> Self {
        Self {
            north_up: c.minimap_north_up,
            north_up_when_stopped: c.minimap_north_up_when_stopped,
            smooth_rotation: c.minimap_smooth_rotation,
            use_movement_dir: c.minimap_use_movement_dir,
            look_stick: c.minimap_look_stick,
            mirror_edges: c.minimap_mirror_edges,
            compass: c.minimap_show_compass,
            zoom_driving_m: c.minimap_zoom_driving_m,
            zoom_stopped_m: c.minimap_zoom_stopped_m,
        }
    }

    pub fn apply_app(&self, c: &mut AppConfig) {
        c.minimap_north_up = self.north_up;
        c.minimap_north_up_when_stopped = self.north_up_when_stopped;
        c.minimap_smooth_rotation = self.smooth_rotation;
        c.minimap_use_movement_dir = self.use_movement_dir;
        c.minimap_look_stick = self.look_stick;
        c.minimap_mirror_edges = self.mirror_edges;
        c.minimap_show_compass = self.compass;
        c.minimap_zoom_driving_m = self.zoom_driving_m;
        c.minimap_zoom_stopped_m = self.zoom_stopped_m;
    }

    pub fn of_overlay(o: &OverlayConfig) -> Self {
        Self {
            north_up: o.map_north_up,
            north_up_when_stopped: o.map_north_up_when_stopped,
            smooth_rotation: o.map_smooth_rotation,
            use_movement_dir: o.map_use_movement_dir,
            look_stick: o.map_look_stick,
            mirror_edges: o.map_mirror_edges,
            compass: o.compass,
            zoom_driving_m: o.zoom_driving_m,
            zoom_stopped_m: o.zoom_stopped_m,
        }
    }

    pub fn apply_overlay(&self, o: &mut OverlayConfig) {
        o.map_north_up = self.north_up;
        o.map_north_up_when_stopped = self.north_up_when_stopped;
        o.map_smooth_rotation = self.smooth_rotation;
        o.map_use_movement_dir = self.use_movement_dir;
        o.map_look_stick = self.look_stick;
        o.map_mirror_edges = self.mirror_edges;
        o.compass = self.compass;
        o.zoom_driving_m = self.zoom_driving_m;
        o.zoom_stopped_m = self.zoom_stopped_m;
    }
}

/// The rows of a map's view options (the body of its first card).
pub fn view_rows(ui: &mut Ui, v: &mut ViewCfg) {
    theme::checkbox_row(ui, &mut v.north_up, tr("Lock map north-up"));
    ui.add_enabled_ui(!v.north_up, |ui| {
        theme::checkbox_row(ui, &mut v.north_up_when_stopped, tr("North up when stopped")).on_hover_text(tr(
            "Heading-up only: the map eases back to north after the car has stopped, and returns to heading-up when it moves.",
        ));
        theme::checkbox_row(ui, &mut v.smooth_rotation, tr("Smooth rotation"));
        theme::checkbox_row(ui, &mut v.use_movement_dir, tr("Use movement direction as rotation")).on_hover_text(tr(
            "Rotate the map to the direction the car is travelling instead of the way it points (differs while drifting).",
        ));
    });
    theme::checkbox_row(ui, &mut v.mirror_edges, tr("Mirror map at edges"));
    theme::checkbox_row(ui, &mut v.look_stick, tr("Rotate with right stick"));
    theme::checkbox_row(ui, &mut v.compass, tr("Show compass"));
    let zoom_tip = tr("Metres from the centre of the map to its edge.");
    theme::slider_row(ui, tr("Zoom when driving"), &mut v.zoom_driving_m, 50.0..=6000.0, 50.0, 0, " m").on_hover_text(zoom_tip);
    theme::slider_row(ui, tr("Zoom when stopped"), &mut v.zoom_stopped_m, 50.0..=6000.0, 50.0, 0, " m").on_hover_text(zoom_tip);
}

// ── the layer cards ──────────────────────────────────────────────────────────────────────────

/// What [`layers_ui`] needs besides the config it edits.
pub struct LayerAux<'a> {
    /// The POI icons of this egui context (shown next to the category names); `None` = coloured
    /// shapes instead.
    pub icons: Option<&'a IconAtlas>,
    /// HUD only: the minimap's own plate opacity (an `OverlayConfig` field, not part of the
    /// layer config) and whether its row is enabled.
    pub plate: Option<(&'a mut f32, bool)>,
    /// False greys the layer cards (module off, or following the Dashboard map).
    pub enabled: bool,
}

/// All layer settings of one map as cards in two or three columns, with `lead` (the page's own
/// first card: view options, module switch) on top of the first column. Both map tabs call
/// this.
pub fn layers_ui(ui: &mut Ui, cfg: &mut MapLayerConfig, ax: LayerAux, lead: &mut dyn FnMut(&mut Ui)) {
    let three = ui.available_width() >= THREE_COLS_MIN_W;
    let LayerAux { icons, plate, enabled } = ax;
    let MapLayerConfig { image, roads, pois, race_lines, tilt } = cfg;
    let mut plate = plate;
    let mut image_card_ = |ui: &mut Ui| image_card(ui, image, plate.as_mut().map(|(v, e)| (&mut **v, *e)), enabled);
    let mut tilt_card_ = |ui: &mut Ui| gated(ui, enabled, |ui| tilt_card(ui, tilt));
    let mut race_card_ = |ui: &mut Ui| gated(ui, enabled, |ui| race_lines_card(ui, race_lines));
    let mut roads_card_ = |ui: &mut Ui| gated(ui, enabled, |ui| roads_card(ui, roads));
    let mut pois_card_ = |ui: &mut Ui| gated(ui, enabled, |ui| pois_card(ui, pois, icons));
    ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
    let n = if three { 3 } else { 2 };
    theme::columns(ui, n, |uis| {
        for u in uis.iter_mut() {
            u.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
        }
        if three {
            lead(&mut uis[0]);
            image_card_(&mut uis[0]);
            tilt_card_(&mut uis[0]);
            race_card_(&mut uis[0]);
            roads_card_(&mut uis[1]);
            pois_card_(&mut uis[2]);
        } else {
            lead(&mut uis[0]);
            roads_card_(&mut uis[0]);
            image_card_(&mut uis[1]);
            tilt_card_(&mut uis[1]);
            race_card_(&mut uis[1]);
            pois_card_(&mut uis[1]);
        }
    });
}

fn gated(ui: &mut Ui, enabled: bool, body: impl FnOnce(&mut Ui)) {
    ui.add_enabled_ui(enabled, body);
}

fn image_card(ui: &mut Ui, c: &mut ImageCfg, plate: Option<(&mut f32, bool)>, enabled: bool) {
    theme::card(ui, tr("Image"), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            theme::checkbox_row(ui, &mut c.on, tr("Satellite image"));
            ui.add_enabled_ui(c.on, |ui| {
                pct_row(ui, tr("Opacity"), &mut c.opacity, 0.0, 100.0, 1.0, None);
                pct_row(ui, tr("Brightness"), &mut c.brightness, 0.0, 100.0, 1.0, None);
                pct_row(
                    ui,
                    tr("Saturation"),
                    &mut c.saturation,
                    0.0,
                    100.0,
                    1.0,
                    Some(tr("Approximate: the app can't desaturate the image, so a grey veil stands in for it.")),
                );
            });
        });
        if let Some((v, on)) = plate {
            ui.add_enabled_ui(on, |ui| {
                pct_row(
                    ui,
                    tr("Map plate opacity"),
                    v,
                    0.0,
                    100.0,
                    1.0,
                    Some(tr("A plate behind the minimap's image; the far edge of a tilted map fades into it. Not the same as the General tab's Plate opacity.")),
                );
            });
        }
    });
}

fn tilt_card(ui: &mut Ui, c: &mut TiltCfg) {
    theme::card(ui, tr("Tilted view"), |ui| {
        theme::checkbox_row(ui, &mut c.on, tr("Tilt the map"));
        ui.add_enabled_ui(c.on, |ui| {
            theme::slider_row(ui, tr("Angle"), &mut c.angle_deg, 5.0..=80.0, 1.0, 0, "°");
            theme::slider_row(ui, tr("Perspective"), &mut c.perspective_px, 50.0..=600.0, 10.0, 0, " px").on_hover_text(tr(
                "The eye distance for a view as tall as the HUD minimap (136 px). A taller map scales it, so both maps look alike. Smaller = stronger perspective.",
            ));
            pct_row(
                ui,
                tr("Car position"),
                &mut c.car_y,
                10.0,
                95.0,
                1.0,
                Some(tr("Where the car sits on the map's height: 0 % = top, 100 % = bottom.")),
            );
            theme::checkbox_row(ui, &mut c.taper, tr("Thinner lines in the distance"));
        });
    });
}

// ── race lines ───────────────────────────────────────────────────────────────────────────────

fn mode_label(m: RaceLineMode) -> &'static str {
    tr(match m {
        RaceLineMode::Off => "Off",
        RaceLineMode::Current => "Current race",
        RaceLineMode::Nearest => "Nearest line",
        RaceLineMode::Near => "Near the car",
        RaceLineMode::All => "All lines",
    })
}

fn race_lines_card(ui: &mut Ui, c: &mut RaceCfg) {
    theme::card(ui, tr("Race lines"), |ui| {
        let mode_tip = tr("Current race is a best guess from where the car is and which way it drives: the game doesn't say which race it is. Nearest and Near use the search radius.");
        control_row_tip(ui, tr("Show"), mode_tip, |ui| {
            egui::ComboBox::from_id_salt("map_race_mode")
                .selected_text(mode_label(c.mode))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for m in [RaceLineMode::Off, RaceLineMode::Current, RaceLineMode::Nearest, RaceLineMode::Near, RaceLineMode::All] {
                        ui.selectable_value(&mut c.mode, m, mode_label(m));
                    }
                });
        });
        ui.add_enabled_ui(c.mode != RaceLineMode::Off, |ui| {
            ui.add_enabled_ui(matches!(c.mode, RaceLineMode::Nearest | RaceLineMode::Near), |ui| {
                theme::slider_row(ui, tr("Search radius"), &mut c.radius_m, 100.0..=5000.0, 50.0, 0, " m");
            });
            theme::slider_row(ui, tr("Line width"), &mut c.width_px, 1.0..=12.0, 0.5, 1, " px");
            control_row(ui, tr("Circuit colour"), |ui| {
                egui::color_picker::color_edit_button_srgb(ui, &mut c.circuit_color.0);
            });
            control_row(ui, tr("Sprint colour"), |ui| {
                egui::color_picker::color_edit_button_srgb(ui, &mut c.sprint_color.0);
            });
            pct_row(ui, tr("Opacity"), &mut c.alpha, 0.0, 100.0, 1.0, None);
            theme::checkbox_row(ui, &mut c.marks, tr("Start / finish marks"));
        });
    });
}

// ── roads ────────────────────────────────────────────────────────────────────────────────────

fn dash_label(d: DashStyle) -> &'static str {
    tr(match d {
        DashStyle::None => "Solid",
        DashStyle::Dashed => "Dashed",
        DashStyle::Short => "Short dashes",
        DashStyle::Dotted => "Dotted",
    })
}

/// The editable road types, in the order shown. Turnarounds are never drawn (D52), so they
/// have no row.
fn road_type_rows(s: &mut RoadStyles) -> [(&mut RoadTypeStyle, &'static str); 8] {
    [
        (&mut s.road, tr("Road")),
        (&mut s.highway, tr("Highway")),
        (&mut s.offroad, tr("Off-road")),
        (&mut s.other, tr("Other")),
        (&mut s.trail, tr("Trail")),
        (&mut s.crosscountry, tr("Cross-country")),
        (&mut s.tunnel, tr("Tunnel")),
        (&mut s.jump, tr("Jump line")),
    ]
}

fn roads_card(ui: &mut Ui, c: &mut RoadsCfg) {
    theme::card(ui, tr("Roads"), |ui| {
        theme::checkbox_row(ui, &mut c.on, tr("Show roads"));
        ui.add_enabled_ui(c.on, |ui| {
            theme::checkbox_row(ui, &mut c.scale_with_zoom, tr("Scale width with zoom")).on_hover_text(tr(
                "On: the line width follows the zoom, kept between the minimum and maximum. Off: one fixed width.",
            ));
            if c.scale_with_zoom {
                theme::slider_row(ui, tr("Road width"), &mut c.metres, 1.0..=40.0, 0.5, 1, " m")
                    .on_hover_text(tr("How wide a road is drawn in metres at the map's scale, before the minimum and maximum."));
                theme::slider_row(ui, tr("Minimum width"), &mut c.min_px, 0.5..=10.0, 0.1, 1, " px");
                theme::slider_row(ui, tr("Maximum width"), &mut c.max_px, 1.0..=30.0, 0.5, 1, " px");
            } else {
                theme::slider_row(ui, tr("Road width"), &mut c.base_px, 0.5..=20.0, 0.5, 1, " px");
            }
            theme::slider_row(ui, tr("Outline width"), &mut c.casing_px, 0.0..=6.0, 0.1, 1, " px")
                .on_hover_text(tr("Extra width of the dark outline under each line."));
            pct_row(ui, tr("Outline opacity"), &mut c.casing_alpha, 0.0, 100.0, 1.0, None);
            ui.add_space(2.0);
            ui.label(theme::section_label(tr("By type")));
            for (i, (style, name)) in road_type_rows(&mut c.styles).into_iter().enumerate() {
                road_type_block(ui, i, name, style);
            }
            if ui.add(theme::secondary_button(tr("Reset road styles"))).clicked() {
                c.styles = RoadStyles::default();
            }
        });
    });
}

/// One road type: on/off + the two colours on top, then width factor, dash, opacity and the
/// outline switch (wrapping, so a narrow card stacks them).
fn road_type_block(ui: &mut Ui, i: usize, name: &str, s: &mut RoadTypeStyle) {
    let frame = egui::Frame::new()
        .fill(theme::WELL)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(6));
    let inner_w = (ui.available_width() - frame.total_margin().sum().x).max(0.0);
    frame.show(ui, |ui| {
        ui.set_width(inner_w);
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            theme::styled_checkbox(ui, &mut s.on, name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::color_picker::color_edit_button_srgb(ui, &mut s.casing_color.0).on_hover_text(tr("Outline colour"));
                egui::color_picker::color_edit_button_srgb(ui, &mut s.color.0).on_hover_text(tr("Line colour"));
            });
        });
        ui.add_enabled_ui(s.on, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::DragValue::new(&mut s.width).range(0.1..=4.0).speed(0.01).fixed_decimals(2).prefix("× "))
                    .on_hover_text(tr("Width relative to the base road width"));
                egui::ComboBox::from_id_salt(("map_road_dash", i))
                    .selected_text(dash_label(s.dash))
                    .width(86.0)
                    .show_ui(ui, |ui| {
                        for d in [DashStyle::None, DashStyle::Dashed, DashStyle::Short, DashStyle::Dotted] {
                            ui.selectable_value(&mut s.dash, d, dash_label(d));
                        }
                    })
                    .response
                    .on_hover_text(tr("Line pattern"));
                let mut pct = s.alpha * 100.0;
                if ui
                    .add(egui::DragValue::new(&mut pct).range(0.0..=100.0).speed(1.0).fixed_decimals(0).suffix(" %"))
                    .on_hover_text(tr("Opacity"))
                    .changed()
                {
                    s.alpha = pct / 100.0;
                }
                theme::styled_checkbox(ui, &mut s.casing, tr("Outline"));
            });
        });
    });
    ui.add_space(4.0);
}

// ── points of interest ───────────────────────────────────────────────────────────────────────

/// The category checkboxes' groups. Every `style::POI_CATS` id appears in exactly one (tested),
/// so a later category can't be left without a checkbox.
const POI_GROUPS: &[(&str, &[&str])] = &[
    (
        "Events",
        &[
            "race_pin",
            "touge_event",
            "special_event",
            "rush_event",
            "showcase",
            "drag_meet",
            "drag_meet_finish",
            "horizon_job",
            "job_activation",
            "horizon_story",
            "story_activation",
            "eliminator_spawn",
            "flag_rush_flag",
        ],
    ),
    ("Zones and gates", &["speed_trap", "speed_zone", "trailblazer", "drift_zone", "danger_sign"]),
    (
        "Places",
        &[
            "car_meet",
            "festival_site",
            "fast_travel",
            "house",
            "estate",
            "estate_entrance",
            "landmark",
            "parking_area",
            "aftermarket_spot",
            "aftermarket_board",
        ],
    ),
    (
        "Collectibles",
        &[
            "barn_find",
            "barn_find_hint",
            "treasure_car",
            "treasure_chest_current",
            "treasure_chest",
            "treasure_chest_board",
            "xp_board",
            "mascot",
            "pinata",
            "creature_zone",
            "upsell",
        ],
    ),
];

fn group_label(g: &str) -> &'static str {
    tr(match g {
        "Events" => "Events",
        "Zones and gates" => "Zones and gates",
        "Places" => "Places",
        _ => "Collectibles",
    })
}

/// The name of a POI category (its config id in `style::POI_CATS`).
pub fn cat_name(id: &str) -> &str {
    match id {
        "race_pin" => tr("Race"),
        "touge_event" => tr("Touge event"),
        "landmark" => tr("Landmark"),
        "barn_find" => tr("Barn find"),
        "barn_find_hint" => tr("Barn find hint"),
        "car_meet" => tr("Car meet"),
        "drag_meet" => tr("Drag meet"),
        "drag_meet_finish" => tr("Drag meet finish"),
        "estate" => tr("Estate"),
        "estate_entrance" => tr("Estate entrance"),
        "fast_travel" => tr("Fast travel"),
        "festival_site" => tr("Festival site"),
        "house" => tr("House"),
        "aftermarket_spot" => tr("Aftermarket spot"),
        "aftermarket_board" => tr("Aftermarket board"),
        "horizon_job" => tr("Horizon job"),
        "horizon_story" => tr("Horizon story"),
        "job_activation" => tr("Job start"),
        "story_activation" => tr("Story start"),
        "special_event" => tr("Special event"),
        "rush_event" => tr("Rush event"),
        "showcase" => tr("Showcase"),
        "treasure_car" => tr("Treasure car"),
        "upsell" => tr("Upsell"),
        "pinata" => tr("Piñata"),
        "eliminator_spawn" => tr("Eliminator"),
        "parking_area" => tr("Parking area"),
        "creature_zone" => tr("Creature zone"),
        "flag_rush_flag" => tr("Flag rush flag"),
        "treasure_chest_board" => tr("Treasure chest board"),
        "treasure_chest" => tr("Treasure chest"),
        "treasure_chest_current" => tr("Current treasure chest"),
        "xp_board" => tr("XP board"),
        "mascot" => tr("Mascot"),
        "speed_trap" => tr("Speed trap"),
        "speed_zone" => tr("Speed zone"),
        "trailblazer" => tr("Trailblazer"),
        "drift_zone" => tr("Drift zone"),
        "danger_sign" => tr("Danger sign"),
        _ => id,
    }
}

const ICON_PX: f32 = 18.0;

/// The category's game icon, or its coloured fallback marker when there is none.
fn cat_icon(ui: &mut Ui, cat: &style::PoiCat, icon: Option<(TextureId, Rect)>) {
    let (r, _) = ui.allocate_exact_size(vec2(ICON_PX, ICON_PX), Sense::hover());
    if !ui.is_rect_visible(r) {
        return;
    }
    let p = ui.painter();
    if let Some((tex, uv)) = icon {
        p.image(tex, r, uv, Color32::WHITE);
        return;
    }
    let col = cat.color.color(1.0);
    let c = r.center();
    match cat.shape {
        Shape::Circle => {
            p.circle_filled(c, 6.0, col);
        }
        Shape::Ring => {
            p.circle_stroke(c, 5.0, Stroke::new(2.0, col));
        }
        Shape::Square => {
            p.rect_filled(Rect::from_center_size(c, vec2(10.0, 10.0)), 2.0, col);
        }
        Shape::Diamond => {
            let d = 7.0;
            p.add(egui::Shape::convex_polygon(
                vec![pos2(c.x, c.y - d), pos2(c.x + d, c.y), pos2(c.x, c.y + d), pos2(c.x - d, c.y)],
                col,
                Stroke::NONE,
            ));
        }
    }
}

fn cat_checkbox(ui: &mut Ui, c: &mut PoisCfg, id: &str, icons: Option<&IconAtlas>) {
    let Some(idx) = style::cat_index(id) else { return };
    let icon = icons.and_then(|a| a.rects.get(idx).copied().flatten().map(|uv| (a.texture, uv)));
    let mut on = c.categories.iter().any(|x| x == id);
    ui.horizontal(|ui| {
        cat_icon(ui, &style::POI_CATS[idx], icon);
        if theme::styled_checkbox(ui, &mut on, cat_name(id)).changed() {
            if on {
                c.categories.push(id.to_string());
            } else {
                c.categories.retain(|x| x != id);
            }
        }
    });
}

fn pois_card(ui: &mut Ui, c: &mut PoisCfg, icons: Option<&IconAtlas>) {
    theme::card(ui, tr("Points of interest"), |ui| {
        theme::checkbox_row(ui, &mut c.on, tr("Show points of interest"));
        ui.add_enabled_ui(c.on, |ui| {
            theme::slider_row(ui, tr("Icon size"), &mut c.size_px, 12.0..=64.0, 1.0, 0, " px");
            theme::slider_row(ui, tr("Max zoom radius"), &mut c.max_zoom_m, 500.0..=10000.0, 100.0, 0, " m").on_hover_text(tr(
                "Points of interest are hidden while the map shows a radius larger than this. The Dashboard map's default radius (5 km) is above the default 3 km, so zoom in to see them.",
            ));
            theme::checkbox_row(ui, &mut c.near_only, tr("Only near the car"));
            ui.add_enabled_ui(c.near_only, |ui| {
                theme::slider_row(ui, tr("Radius around the car"), &mut c.radius_m, 100.0..=5000.0, 50.0, 0, " m");
            });
            theme::checkbox_row(ui, &mut c.gates, tr("Gate lines")).on_hover_text(tr(
                "Draw the line across the road for speed zones, trailblazers, drift zones and speed traps.",
            ));
            for (group, ids) in POI_GROUPS {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(group_label(group)).size(11.0).color(theme::TEXT_DIM));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(tr("None")).clicked() {
                            c.categories.retain(|x| !ids.contains(&x.as_str()));
                        }
                        if ui.small_button(tr("All")).clicked() {
                            for id in *ids {
                                if !c.categories.iter().any(|x| x == id) {
                                    c.categories.push(id.to_string());
                                }
                            }
                        }
                    });
                });
                theme::columns(ui, 2, |cols| {
                    for (n, id) in ids.iter().enumerate() {
                        cat_checkbox(&mut cols[n % 2], c, id, icons);
                    }
                });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::roadtypes::RoadType;

    #[test]
    fn every_poi_category_has_one_checkbox_and_a_name() {
        let mut seen = std::collections::HashSet::new();
        for (_, ids) in POI_GROUPS {
            for id in *ids {
                assert!(style::cat_index(id).is_some(), "group lists unknown category {id}");
                assert!(seen.insert(*id), "category {id} in two groups");
            }
        }
        for c in style::POI_CATS {
            assert!(seen.contains(c.id), "category {} has no checkbox", c.id);
            assert_ne!(cat_name(c.id), c.id, "category {} has no name", c.id);
        }
    }

    #[test]
    fn every_drawn_road_type_has_a_row() {
        let drawn = RoadType::ALL.iter().filter(|t| **t != RoadType::Turnaround).count();
        let mut s = RoadStyles::default();
        assert_eq!(road_type_rows(&mut s).len(), drawn);
    }

    #[test]
    fn view_cfg_round_trips_through_both_configs() {
        let mut app = AppConfig::default();
        let mut v = ViewCfg::of_app(&app);
        v.north_up = !v.north_up;
        v.zoom_stopped_m = 1234.0;
        v.apply_app(&mut app);
        assert_eq!(ViewCfg::of_app(&app), v);
        let mut o = OverlayConfig::default();
        v.apply_overlay(&mut o);
        assert_eq!(ViewCfg::of_overlay(&o), v);
        assert_eq!((o.map_north_up, o.zoom_stopped_m), (v.north_up, 1234.0));
    }
}
