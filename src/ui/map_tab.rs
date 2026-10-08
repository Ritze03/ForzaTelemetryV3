//! The Map tab (D67): a full-size map viewer, and a full-size settings mode for every map.
//!
//! *Viewer* (the tab's default view): the map fills the whole tab, drawn by the shared scene
//! (`map_scene`, the same code as the Dashboard's Map widget) with the viewer's own layer settings
//! (`AppConfig::viewer_layers`). Drag pans, the wheel zooms, "Follow car" keeps the car centred.
//!
//! *Settings* (the cog on the viewer; "Back to map" returns): a module selector, the Overlay
//! tab's control, over the pages Minimap · Dashboard map · Viewer · Map data. The first two moved
//! here from the Overlay tab, the last from Setup. Which mode and page were last open is
//! remembered (`map_tab_settings`, `map_tab_page`; never exported).
//!
//! *Why a tab of its own:* the user wanted the maps' settings out of the Overlay tab and the map
//! editor out of the bloated Setup tab, with the map itself full size next to them. Plan D67.

use std::cell::RefCell;
use std::sync::Arc;

use egui::{pos2, vec2, Align, Layout, Rect, Sense, Ui, UiBuilder};

use crate::app::ForzaApp;
use crate::config::{AppConfig, MapPage};
use crate::i18n::tr;
use crate::icons;
use crate::maprender::cfg::MapLayerConfig;
use crate::maprender::paint2d::IconAtlas;
use crate::maprender::store::Layers;
use crate::maprender::ui::{
    apply_copy, copy_row, layers_ui, status_ui, view_rows, CopyRequest, CopyWhat, LayerAux, MapId, ViewCfg,
};
use crate::maprender::RaceSel;
use crate::ui::map_scene::{self, ManualView, Scene, ViewIn};
use crate::ui::overlay_tab::{module_card, page_selector_with, status_line};
use crate::theme;

// ── viewer ───────────────────────────────────────────────────────────────────────────────────

/// The viewer's state (not saved: where you left the view is not worth a config key; the tab
/// opens on the car).
pub struct MapTabState {
    /// The user's pan and zoom over the base view (the car, `viewer_zoom_m`). Temporary: it
    /// resets once the player drives off again (`map_scene::DriveGate`, D72).
    pub manual: ManualView,
    /// The viewer's own race-line selection state (see `racesel`).
    pub race_sel: RefCell<RaceSel>,
}

impl Default for MapTabState {
    fn default() -> Self {
        Self { manual: ManualView::default(), race_sel: Default::default() }
    }
}

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    if app.config.map_tab_settings {
        settings(ui, app);
    } else {
        viewer(ui, app);
    }
}

fn viewer(ui: &mut Ui, app: &mut ForzaApp) {
    app.ensure_map_image();
    let rect = ui.available_rect_before_wrap();
    let allow = app.config.viewer_allow_pan_zoom;
    let resp = ui.allocate_rect(rect, if allow { Sense::click_and_drag() } else { Sense::click() });
    let car = (app.minimap_cached_car_x, app.minimap_cached_car_z);
    let yaw = if app.config.viewer_north_up { 0.0 } else { app.minimap_smoothed_yaw };
    let speed = app.telemetry.latest.as_ref().map(|p| p.speed);
    let base_zoom_m = app.config.viewer_zoom_m;

    // Input first; the scene is then drawn from the updated view, so a drag moves the map in
    // the same frame.
    if allow {
        let v = ViewIn { layers: &app.config.viewer_layers, yaw, rect, car, base_zoom_m, speed, now: ui.input(|i| i.time) };
        app.map_tab.manual.interact(ui, &resp, &v);
    } else {
        app.map_tab.manual.reset();
    }
    let (centre, zoom_m) = app.map_tab.manual.view(car, base_zoom_m);

    if let Some(texture) = map_scene::texture_or_status(ui, app, rect) {
        let cfg = &app.config;
        let scene = Scene {
            layers: &cfg.viewer_layers,
            centre,
            yaw,
            zoom_m,
            mirror: cfg.viewer_mirror_edges,
            compass: cfg.viewer_show_compass,
            race_sel: &app.map_tab.race_sel,
        };
        let cam = map_scene::draw(ui, app, rect, texture, &scene);
        // Same as the Dashboard map: a click drops a shared waypoint, a right-click clears it.
        if app.coop.role() != crate::coop::Role::Off {
            if resp.clicked() {
                if let Some([wx, wz]) = resp.interact_pointer_pos().and_then(|m| cam.unproject(m)) {
                    app.coop.set_waypoint(Some((wx, wz)), cfg.coop_hue);
                }
            }
            if resp.secondary_clicked() {
                app.coop.set_waypoint(None, 0.0);
            }
        }
    }

    // Controls on top of the map (drawn after it, so they take the clicks).
    let mv = &mut app.map_tab.manual;
    let following = mv.centre.is_none();
    let (follow_clicked, open_settings) = controls(ui, rect, following, zoom_m);
    if follow_clicked {
        // Following: freeze the view where it is. Panned (or only zoomed): back to the car and
        // the configured zoom, at once.
        if following && !mv.is_manual() {
            mv.pin_centre(centre, speed);
        } else {
            mv.reset();
        }
    }
    if open_settings {
        app.config.map_tab_settings = true;
    }
}

/// The viewer's buttons and zoom readout over the map: "Follow car" top left (lit while the view
/// follows the car), "Settings" top right, the radius bottom left. Returns (Follow car pressed,
/// Settings pressed).
fn controls(ui: &mut Ui, rect: Rect, following: bool, zoom_m: f32) -> (bool, bool) {
    const M: f32 = 10.0;
    let (mut follow_clicked, mut open) = (false, false);
    let left = Rect::from_min_size(rect.left_top() + vec2(M, M), vec2((rect.width() * 0.5 - M).max(0.0), 30.0));
    ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Min)), |ui| {
        let label = format!("{}  {}", icons::CROSSHAIRS, tr("Follow car"));
        let btn = if following { theme::primary_button(label) } else { theme::secondary_button(label) };
        follow_clicked = ui.add(btn).clicked();
    });
    let right = Rect::from_min_max(pos2(rect.center().x, rect.top() + M), pos2(rect.right() - M, rect.top() + M + 30.0));
    ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Min)), |ui| {
        open = ui.add(theme::secondary_button(format!("{}  {}", icons::COG, tr("Settings")))).clicked();
    });
    let text = format!("{:.0} m", zoom_m);
    let font = egui::FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text, font, theme::TEXT_DIM);
    let at = pos2(rect.left() + M, rect.bottom() - M - galley.size().y);
    ui.painter().rect_filled(Rect::from_min_size(at, galley.size()).expand2(vec2(5.0, 2.0)), 4.0, egui::Color32::from_black_alpha(150));
    ui.painter().galley(at, galley, theme::TEXT_DIM);
    (follow_clicked, open)
}

/// Tooltip of the "Allow pan and zoom" option (both maps): what it does and when it resets.
pub(crate) const PAN_ZOOM_TIP: &str = "Drag the map to pan and scroll to zoom. The view goes back to the car once you start driving again.";

// ── settings ─────────────────────────────────────────────────────────────────────────────────

fn settings(ui: &mut Ui, app: &mut ForzaApp) {
    if ui.add(theme::secondary_button(format!("{}  {}", icons::ARROW_LEFT, tr("Back to map")))).clicked() {
        app.config.map_tab_settings = false;
    }
    ui.add_space(8.0);
    page_selector(ui, &mut app.config.map_tab_page);
    ui.add_space(8.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match app.config.map_tab_page {
        MapPage::Minimap => {
            let (l, atlas) = layers_and_icons(ui, app);
            minimap_page(ui, &mut app.config, &l, atlas.as_deref());
        }
        MapPage::DashboardMap => {
            let (l, atlas) = layers_and_icons(ui, app);
            dashboard_page(ui, &mut app.config, &l, atlas.as_deref());
        }
        MapPage::Viewer => {
            let (l, atlas) = layers_and_icons(ui, app);
            viewer_page(ui, &mut app.config, &l, atlas.as_deref());
        }
        MapPage::MapData => crate::ui::map_data::page(ui, app),
    });
}

/// The module selector of the settings mode (the Overlay tab's control).
fn page_selector(ui: &mut Ui, page: &mut MapPage) -> bool {
    let opts = [
        (MapPage::Minimap, tr("Minimap")),
        (MapPage::DashboardMap, tr("Dashboard map")),
        (MapPage::Viewer, tr("Viewer")),
        (MapPage::MapData, tr("Map data")),
    ];
    page_selector_with(ui, page, &opts)
}

/// The layer store's state and this context's uploaded POI icons. Asking the store starts its
/// load (once per process), which is wanted: the map pages show what it found.
fn layers_and_icons(ui: &Ui, app: &ForzaApp) -> (Layers, Option<Arc<IconAtlas>>) {
    let l = crate::maprender::layers();
    let atlas = l.data.as_ref().and_then(|d| app.minimap_icons.borrow_mut().ensure(ui.ctx(), d.icons.as_ref()));
    (l, atlas)
}

/// HUD minimap page: the Minimap module card plus all layer settings of `overlay.map_layers`.
/// With "Use Dashboard map settings" on, the Dashboard's values are shown greyed instead (what
/// `OverlayConfig::effective` will really use). The plate opacity is the exception: it is not
/// copied from the Dashboard, so it stays editable. (Moved here from the Overlay tab, D67.)
fn minimap_page(ui: &mut Ui, cfg: &mut AppConfig, l: &Layers, atlas: Option<&IconAtlas>) {
    status_ui(ui, l);
    let follow = cfg.overlay.map_use_dashboard;
    if follow {
        status_line(ui, theme::FAINT, tr("The map follows the Dashboard map's settings."));
    }
    ui.add_space(4.0);
    let mut layers = if follow { cfg.minimap_layers.clone() } else { cfg.overlay.map_layers.clone() };
    let mut plate = cfg.overlay.map_plate_opacity;
    let on = cfg.overlay.enabled && cfg.overlay.minimap_on;
    let dash_view = ViewCfg::of_app(cfg);
    let mut reset = false;
    let mut view_req = None;
    let layer_req = {
        let o = &mut cfg.overlay;
        let mut lead = |ui: &mut Ui| minimap_card(ui, o, &dash_view, &mut reset, &mut view_req);
        let aux = LayerAux {
            icons: atlas,
            plate: Some((&mut plate, on)),
            enabled: on && !follow,
            which: MapId::Minimap,
            minimap_follows: follow,
        };
        layers_ui(ui, &mut layers, aux, &mut lead)
    };
    if reset {
        layers = MapLayerConfig::hud();
    }
    if !follow {
        cfg.overlay.map_layers = layers;
    }
    cfg.overlay.map_plate_opacity = plate;
    apply_requests(cfg, [layer_req, view_req]);
}

/// Apply the "Copy to …" the page's buttons asked for, after the page has written its own edits
/// back (so the source includes this frame's changes and the target is not overwritten by a
/// stale clone).
fn apply_requests(cfg: &mut AppConfig, reqs: [Option<CopyRequest>; 2]) {
    for r in reqs.into_iter().flatten() {
        apply_copy(cfg, &r);
    }
}

/// Dashboard map page: the view options and all layer settings of `minimap_layers`. (Mini-Settings
/// → Dashboard → Map keeps its quick options; both edit the same keys.)
fn dashboard_page(ui: &mut Ui, cfg: &mut AppConfig, l: &Layers, atlas: Option<&IconAtlas>) {
    status_ui(ui, l);
    ui.add_space(4.0);
    let mut layers = cfg.minimap_layers.clone();
    let mut view = ViewCfg::of_app(cfg);
    let mut allow = cfg.minimap_allow_pan_zoom;
    let mut reset = false;
    let mut view_req = None;
    let follows = cfg.overlay.map_use_dashboard;
    let mut lead = |ui: &mut Ui| {
        theme::card(ui, tr("Dashboard map"), |ui| {
            view_rows(ui, &mut view);
            theme::checkbox_row(ui, &mut allow, tr("Allow pan and zoom")).on_hover_text(tr(PAN_ZOOM_TIP));
            if ui.add(theme::secondary_button(tr("Reset map layers"))).clicked() {
                reset = true;
            }
            view_req = copy_row(ui, MapId::Dashboard, follows, CopyWhat::View);
        });
    };
    let aux = LayerAux { icons: atlas, plate: None, enabled: true, which: MapId::Dashboard, minimap_follows: follows };
    let layer_req = layers_ui(ui, &mut layers, aux, &mut lead);
    if reset {
        layers = MapLayerConfig::dashboard();
    }
    cfg.minimap_layers = layers;
    view.apply_app(cfg);
    cfg.minimap_allow_pan_zoom = allow;
    apply_requests(cfg, [layer_req, view_req]);
}

/// Viewer page: the viewer's own view options and all layer settings of `viewer_layers`. Its
/// co-op trails and player list follow the Dashboard map's co-op settings (Mini-Settings).
fn viewer_page(ui: &mut Ui, cfg: &mut AppConfig, l: &Layers, atlas: Option<&IconAtlas>) {
    status_ui(ui, l);
    ui.add_space(4.0);
    let mut layers = cfg.viewer_layers.clone();
    let (mut north_up, mut mirror, mut compass) = (cfg.viewer_north_up, cfg.viewer_mirror_edges, cfg.viewer_show_compass);
    let (mut allow, mut zoom) = (cfg.viewer_allow_pan_zoom, cfg.viewer_zoom_m);
    let mut reset = false;
    let mut view_req = None;
    let follows = cfg.overlay.map_use_dashboard;
    let mut lead = |ui: &mut Ui| {
        theme::card(ui, tr("Viewer"), |ui| {
            theme::checkbox_row(ui, &mut north_up, tr("Lock map north-up"))
                .on_hover_text(tr("Off: the map turns with the car's heading."));
            theme::checkbox_row(ui, &mut mirror, tr("Mirror map at edges"));
            theme::checkbox_row(ui, &mut compass, tr("Show compass"));
            theme::checkbox_row(ui, &mut allow, tr("Allow pan and zoom")).on_hover_text(tr(PAN_ZOOM_TIP));
            theme::slider_row(ui, tr("Zoom"), &mut zoom, 50.0..=8000.0, 50.0, 0, " m")
                .on_hover_text(tr("Metres from the centre of the map to its edge."));
            if ui.add(theme::secondary_button(tr("Reset map layers"))).clicked() {
                reset = true;
            }
            view_req = copy_row(ui, MapId::Viewer, follows, CopyWhat::View);
        });
    };
    let aux = LayerAux { icons: atlas, plate: None, enabled: true, which: MapId::Viewer, minimap_follows: follows };
    let layer_req = layers_ui(ui, &mut layers, aux, &mut lead);
    if reset {
        layers = crate::config::viewer_layers_default();
    }
    cfg.viewer_layers = layers;
    (cfg.viewer_north_up, cfg.viewer_mirror_edges, cfg.viewer_show_compass) = (north_up, mirror, compass);
    (cfg.viewer_allow_pan_zoom, cfg.viewer_zoom_m) = (allow, zoom);
    apply_requests(cfg, [layer_req, view_req]);
}

/// The Minimap module card of the HUD map page: module switch, "Use Dashboard map settings", the
/// view options (the Dashboard's values, greyed, while it is on) and the co-op switch. The
/// layer cards are `maprender::ui::layers_ui`'s. `reset` is set by the "Reset map layers" button.
fn minimap_card(
    ui: &mut Ui,
    o: &mut crate::config::OverlayConfig,
    dash_view: &ViewCfg,
    reset: &mut bool,
    view_req: &mut Option<CopyRequest>,
) {
    module_card(ui, o, tr("Minimap"), |o| &mut o.minimap_on, None, |ui, o| {
        theme::checkbox_row(ui, &mut o.map_use_dashboard, tr("Use Dashboard map settings")).on_hover_text(tr(
            "The view options and all layer settings follow the Dashboard map. Only the map plate opacity stays its own.",
        ));
        let follow = o.map_use_dashboard;
        let mut v = if follow { dash_view.clone() } else { ViewCfg::of_overlay(o) };
        ui.add_enabled_ui(!follow, |ui| view_rows(ui, &mut v));
        if !follow {
            v.apply_overlay(o);
        }
        ui.add_enabled_ui(!o.coop_use_dashboard, |ui| {
            theme::checkbox_row(ui, &mut o.coop_teammates, tr("Show co-op teammates"));
        });
        ui.add_enabled_ui(!follow, |ui| {
            if ui.add(theme::secondary_button(tr("Reset map layers"))).clicked() {
                *reset = true;
            }
        });
        let follow = o.map_use_dashboard;
        *view_req = copy_row(ui, MapId::Minimap, follow, CopyWhat::View);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::overlay_tab::tests::{check_panes, layers_ready, render, WIDTHS};

    /// The viewer's buttons and readout stay inside the tab at the window minimum and wider, in
    /// both languages, and the two buttons never overlap.
    #[test]
    fn viewer_controls_stay_inside_the_tab() {
        use crate::i18n::{with_language, Language};
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in [600.0, 700.0, 1000.0, 1235.0] {
                    let out = render("map_viewer", w, 500.0, |ui, _| {
                        let rect = ui.available_rect_before_wrap();
                        controls(ui, rect, true, 1500.0);
                    });
                    check_panes(&out, w, "viewer controls");
                    let mut labels = Vec::new();
                    for c in &out.shapes {
                        if let egui::Shape::Text(t) = &c.shape {
                            labels.push(t.visual_bounding_rect());
                        }
                    }
                    assert_eq!(labels.len(), 3, "{lang:?} at {w} px: follow, settings, radius");
                    for (i, a) in labels.iter().enumerate() {
                        for b in &labels[i + 1..] {
                            assert!(!a.intersects(*b), "{lang:?} at {w} px: labels touch: {a:?} / {b:?}");
                        }
                    }
                }
            });
        }
    }

    /// Both maps' pages (the shared `layers_ui`), the viewer's, plus the HUD page following the
    /// Dashboard and with its module off: six cards each, nothing leaves its pane. In German too
    /// (the long labels).
    #[test]
    fn map_pages_stay_inside_their_panes() {
        use crate::i18n::{with_language, Language};
        let l = layers_ready();
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in WIDTHS {
                    for (name, follow, module_on) in [
                        ("minimap", false, true),
                        ("minimap_follow", true, true),
                        ("minimap_off", false, false),
                        ("dashboard", false, true),
                        ("viewer", false, true),
                    ] {
                        let mut cfg = AppConfig::default();
                        cfg.overlay.map_use_dashboard = follow;
                        cfg.overlay.minimap_on = module_on;
                        let out = render(&format!("map_{name}"), w, 3600.0, |ui, atlas| match name {
                            n if n.starts_with("minimap") => minimap_page(ui, &mut cfg, &l, Some(atlas)),
                            "dashboard" => dashboard_page(ui, &mut cfg, &l, Some(atlas)),
                            _ => viewer_page(ui, &mut cfg, &l, Some(atlas)),
                        });
                        assert_eq!(check_panes(&out, w, name), 6, "{lang:?} {name} at {w} px: expected 6 card frames");
                    }
                }
            });
        }
    }

    /// The same pages without an install / while loading / after an error: the status line is
    /// there and the layout holds.
    #[test]
    fn map_pages_render_for_every_layer_status() {
        use crate::maprender::LayerStatus::{Error, Loading, NoInstall};
        for status in [NoInstall, Loading, Error("the nav could not be read: unexpected end of file in a long message".into())] {
            let l = Layers { status, data: None };
            let mut cfg = AppConfig::default();
            let out = render("map_status", 700.0, 3600.0, |ui, _| minimap_page(ui, &mut cfg, &l, None));
            assert_eq!(check_panes(&out, 700.0, "status"), 6);
            let out = render("map_status", 700.0, 3600.0, |ui, _| viewer_page(ui, &mut cfg, &l, None));
            assert_eq!(check_panes(&out, 700.0, "status"), 6);
        }
    }

    /// The module selector: one row where the labels fit, two where they don't (the long German
    /// labels at the window minimum), and its labels never run into each other. (The Map data
    /// page itself needs the app's state; its pane tests live in `map_data`.)
    #[test]
    fn module_selector_never_overlaps_its_labels() {
        let en = [
            (MapPage::Minimap, "Minimap"),
            (MapPage::DashboardMap, "Dashboard map"),
            (MapPage::Viewer, "Viewer"),
            (MapPage::MapData, "Map data"),
        ];
        let de = [
            (MapPage::Minimap, "Minikarte"),
            (MapPage::DashboardMap, "Dashboard-Karte"),
            (MapPage::Viewer, "Kartenansicht"),
            (MapPage::MapData, "Kartendaten"),
        ];
        for (lang, opts) in [("en", &en), ("de", &de)] {
            for w in [400.0, 600.0, 700.0, 800.0, 1000.0, 1280.0] {
                let mut page = MapPage::Minimap;
                let out = render(&format!("map_selector_{lang}"), w, 120.0, |ui, _| {
                    page_selector_with(ui, &mut page, opts);
                });
                let labels: Vec<Rect> = out
                    .shapes
                    .iter()
                    .filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.visual_bounding_rect()) } else { None })
                    .collect();
                assert_eq!(labels.len(), 4, "{lang} at {w} px");
                for (i, a) in labels.iter().enumerate() {
                    assert!(a.left() >= 0.0 && a.right() <= w, "{lang} at {w} px: label leaves the window: {a:?}");
                    for b in &labels[i + 1..] {
                        assert!(!a.expand(2.0).intersects(*b), "{lang} at {w} px: labels touch: {a:?} / {b:?}");
                    }
                }
            }
        }
    }

    /// The remembered mode and page round-trip through the config JSON and are never exported;
    /// the viewer's own settings are exported.
    #[test]
    fn remembered_state_is_saved_but_not_exported() {
        let mut c = AppConfig::default();
        c.map_tab_settings = true;
        c.map_tab_page = MapPage::MapData;
        c.viewer_north_up = false;
        let back: AppConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(back.map_tab_settings);
        assert_eq!(back.map_tab_page, MapPage::MapData);
        let all = vec![true; crate::config::KEY_GROUPS.len()];
        let json = crate::config::export_selected(&c, &all);
        assert!(!json.contains("map_tab_settings") && !json.contains("map_tab_page"));
        assert!(json.contains("viewer_layers") && json.contains("viewer_north_up"));
    }

    /// Defaults: the viewer opens north-up on a Dashboard-like look, with POIs kept visible at
    /// its wide zoom levels.
    #[test]
    fn viewer_defaults() {
        let c = AppConfig::default();
        assert!(c.viewer_north_up && c.viewer_mirror_edges && !c.viewer_show_compass);
        assert!(c.viewer_allow_pan_zoom && c.minimap_allow_pan_zoom);
        assert_eq!(c.viewer_zoom_m, 1500.0);
        assert!(!c.map_tab_settings && c.map_tab_page == MapPage::Minimap);
        let d = MapLayerConfig::dashboard();
        assert_eq!(c.viewer_layers.roads, d.roads);
        assert_eq!(c.viewer_layers.image, d.image);
        assert_eq!(c.viewer_layers.pois.max_zoom_m, 8000.0);
        assert!(c.viewer_layers.pois.max_zoom_m >= map_scene::ZOOM_MAX_M, "POIs show at every viewer zoom");
    }

    /// Run frames of `page` with `events` in the first one; returns the last frame's output.
    fn frame(ctx: &egui::Context, w: f32, events: Vec<egui::Event>, page: &mut dyn FnMut(&mut Ui)) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(w, 3600.0))),
            events,
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| page(ui));
        })
    }

    /// Centre of the topmost text shape reading `text`.
    fn find_text(out: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        let mut best: Option<Rect> = None;
        for c in &out.shapes {
            if let egui::Shape::Text(t) = &c.shape {
                let r = t.visual_bounding_rect();
                if t.galley.text() == text && best.map_or(true, |b| r.top() < b.top()) {
                    best = Some(r);
                }
            }
        }
        best.map(|r| r.center())
    }

    fn click(ctx: &egui::Context, w: f32, at: egui::Pos2, page: &mut dyn FnMut(&mut Ui)) {
        use egui::{Event, Modifiers, PointerButton};
        let btn = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        frame(ctx, w, vec![Event::PointerMoved(at)], page);
        frame(ctx, w, vec![btn(true)], page);
        frame(ctx, w, vec![btn(false)], page);
    }

    /// The whole flow with real pointer events: press the "Copy to…" of the Dashboard map page's
    /// view card, pick "Viewer" in the menu, and the viewer's view options now match the
    /// Dashboard map's; the button then reads "Copied". Also the Roads card's button: copies
    /// the roads to the Minimap through "Both"... only the greyed state is checked there.
    #[test]
    fn clicking_copy_to_applies_the_request() {
        let ctx = crate::ui::test_render::context();
        let l = layers_ready();
        let mut cfg = AppConfig::default();
        cfg.minimap_north_up = false;
        cfg.minimap_show_compass = true;
        cfg.viewer_north_up = true;
        cfg.viewer_show_compass = false;
        cfg.overlay.map_use_dashboard = false;
        let w = 1235.0;
        for _ in 0..3 {
            frame(&ctx, w, vec![], &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        }
        let out = frame(&ctx, w, vec![], &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        let at = find_text(&out, "Copy to…").expect("a Copy to… button");
        click(&ctx, w, at, &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        let mut out = frame(&ctx, w, vec![], &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        for _ in 0..2 {
            out = frame(&ctx, w, vec![], &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        }
        let item = find_text(&out, "Viewer").expect("the menu lists the Viewer");
        assert!(find_text(&out, "Minimap").is_some() && find_text(&out, "Both").is_some());
        click(&ctx, w, item, &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        assert!(!cfg.viewer_north_up && cfg.viewer_show_compass, "the viewer took the Dashboard map's view options");
        assert!(!cfg.minimap_north_up, "the source is unchanged");
        let out = frame(&ctx, w, vec![], &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        let copied = |c: &egui::epaint::ClippedShape| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text().ends_with("Copied"));
        assert!(out.shapes.iter().any(copied), "the button confirms");
    }

    /// The menu with the Minimap following the Dashboard map: the Minimap entry and "Both" are
    /// disabled (clicking them does nothing: the menu stays open, no "Copied"), the Viewer entry
    /// works.
    #[test]
    fn copy_menu_blocks_the_following_minimap() {
        let ctx = crate::ui::test_render::context();
        let mut cfg = AppConfig::default();
        cfg.overlay.map_use_dashboard = true;
        let own = cfg.overlay.map_layers.clone();
        cfg.minimap_layers.roads.casing_px = 5.5;
        let l = layers_ready();
        let w = 1235.0;
        let mut page = |ui: &mut Ui| dashboard_page(ui, &mut cfg, &l, None);
        for _ in 0..3 {
            frame(&ctx, w, vec![], &mut page);
        }
        let out = frame(&ctx, w, vec![], &mut page);
        // The Roads card's button is the fifth "Copy to…" from the top in the middle column at
        // this width; any card does, take the topmost.
        let at = find_text(&out, "Copy to…").unwrap();
        click(&ctx, w, at, &mut page);
        let mut out = frame(&ctx, w, vec![], &mut page);
        for _ in 0..2 {
            out = frame(&ctx, w, vec![], &mut page);
        }
        let copied = |out: &egui::FullOutput| {
            out.shapes.iter().any(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text().ends_with("Copied")))
        };
        for entry in ["Minimap", "Both"] {
            let p = find_text(&out, entry).expect("entry");
            click(&ctx, w, p, &mut page);
            out = frame(&ctx, w, vec![], &mut page);
            assert!(find_text(&out, "Viewer").is_some(), "{entry} is disabled: the menu stays open");
            assert!(!copied(&out), "{entry} must not copy");
        }
        let p = find_text(&out, "Viewer").unwrap();
        click(&ctx, w, p, &mut page);
        let out = frame(&ctx, w, vec![], &mut page);
        assert!(copied(&out), "the Viewer entry copies");
        assert_eq!(cfg.overlay.map_layers, own, "the following Minimap's own config is never written");
    }
}
