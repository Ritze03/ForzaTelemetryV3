//! The Map tab (D67): a full-size map viewer, and a full-size settings mode for every map.
//!
//! *Viewer* (the tab's default view): the map fills the whole tab, drawn by the shared scene
//! (`map_scene`, the same code as the Dashboard's Map widget) with the **Dashboard map's settings**
//! (`minimap_layers`, `minimap_*`; D73: the viewer has none of its own). Drag pans, the wheel
//! zooms, "Follow car" keeps the car centred.
//!
//! *Settings* (the cog on the viewer; "Back to map" returns): a module selector, the Overlay
//! tab's control, over the pages Minimap · Dashboard map & Viewer · Map data. The first two moved
//! here from the Overlay tab, the last from Setup. Every map setting is here and nowhere else
//! (D79): Mini-Settings has none, so the Map data page also holds the map image (quality, reload,
//! cache, calibration) and the pages the co-op options and the Dashboard map's FPS limit. Which mode and page were last open is
//! remembered (`map_tab_settings`, `map_tab_page`; never exported).
//!
//! *Why a tab of its own:* the user wanted the maps' settings out of the Overlay tab and the map
//! editor out of the bloated Setup tab, with the map itself full size next to them. Plan D67.

use std::cell::RefCell;
use std::sync::Arc;

use egui::{pos2, vec2, Align, Layout, Rect, RichText, Sense, Ui, UiBuilder};

use crate::app::ForzaApp;
use crate::config::{AppConfig, MapPage, WidgetKind};
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
use crate::ui::overlay_tab::{control_row, module_card, page_selector_with, status_line};
use crate::theme;

// ── viewer ───────────────────────────────────────────────────────────────────────────────────

/// The viewer's state (not saved: where you left the view is not worth a config key; the tab
/// opens on the car).
#[derive(Default)]
pub struct MapTabState {
    /// The user's pan and zoom over the base view (the car, at the Dashboard map's eased zoom,
    /// `ForzaApp::minimap_current_zoom`). Temporary: it
    /// resets once the player drives off again (`map_scene::DriveGate`, D72).
    pub manual: ManualView,
    /// The viewer's own race-line selection state (see `racesel`).
    pub race_sel: RefCell<RaceSel>,
    /// The viewer's "Set destination" button is armed: the next left click sets the destination
    /// (the click itself, a right click or Esc disarms). Not used by the Navigation tab, whose
    /// every click sets it.
    pub dest_armed: bool,
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
    map_pane(ui, app, rect, MapPane::Viewer);
}

/// Which tab a [`map_pane`] belongs to: they draw the same scene with the same settings (D73) but
/// keep their own view state and read a click differently ([`click_action`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MapPane {
    /// The Map tab's viewer: left-click = co-op waypoint, right-click = clear it; the destination
    /// is set with Shift+click or the "Set destination" button.
    Viewer,
    /// The Navigation tab's map: every left click sets the destination.
    Navigation,
}

fn state(app: &mut ForzaApp, which: MapPane) -> &mut MapTabState {
    match which {
        MapPane::Viewer => &mut app.map_tab,
        MapPane::Navigation => &mut app.nav_ui.map,
    }
}

fn state_ref(app: &ForzaApp, which: MapPane) -> &MapTabState {
    match which {
        MapPane::Viewer => &app.map_tab,
        MapPane::Navigation => &app.nav_ui.map,
    }
}

/// A click on a map pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Click {
    Primary,
    Secondary,
}

/// What a click on a map pane does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ClickAction {
    SetDestination,
    SetWaypoint,
    ClearWaypoint,
    /// Leave the armed "Set destination" mode without doing anything else.
    Disarm,
    Nothing,
}

/// The one rule for clicks on the two maps, kept free of egui so it can be tested.
///
/// * Navigation tab: a left click sets the destination, nothing else (D84).
/// * Viewer: a plain left click is the co-op waypoint and a right click clears it, as before the
///   navigation existed; **Shift+left** or an **armed** "Set destination" button makes the left
///   click set the destination instead; while armed a right click only disarms (it does not clear
///   the waypoint). Waypoints exist only in a co-op session.
///
/// *Why not a right-click menu:* the right click already clears the waypoint, and a menu would
/// change that; the toggle button is discoverable and works without co-op.
pub(crate) fn click_action(pane: MapPane, click: Click, shift: bool, armed: bool, in_session: bool) -> ClickAction {
    match (pane, click) {
        (MapPane::Navigation, Click::Primary) => ClickAction::SetDestination,
        (MapPane::Navigation, Click::Secondary) => ClickAction::Nothing,
        (MapPane::Viewer, Click::Primary) if shift || armed => ClickAction::SetDestination,
        (MapPane::Viewer, Click::Primary) if in_session => ClickAction::SetWaypoint,
        (MapPane::Viewer, Click::Secondary) if armed => ClickAction::Disarm,
        (MapPane::Viewer, Click::Secondary) if in_session => ClickAction::ClearWaypoint,
        (MapPane::Viewer, _) => ClickAction::Nothing,
    }
}

/// One map pane filling `rect`: pan / zoom input, the scene, clicks, and the buttons on top.
/// Shared by the Map tab's viewer and the Navigation tab's map.
pub(crate) fn map_pane(ui: &mut Ui, app: &mut ForzaApp, rect: Rect, which: MapPane) {
    // The viewer is the Dashboard map's twin (D73): same layers, same view options, same yaw
    // (incl. the right-stick look), same base zoom.
    let allow = app.config.minimap_allow_pan_zoom;
    let resp = ui.allocate_rect(rect, if allow { Sense::click_and_drag() } else { Sense::click() });
    let car = (app.minimap_cached_car_x, app.minimap_cached_car_z);
    let yaw = app.minimap_look.view_yaw(app.minimap_base_yaw());
    let speed = app.telemetry.latest.as_ref().map(|p| p.speed);
    let base_zoom_m = app.minimap_current_zoom;

    // Input first; the scene is then drawn from the updated view, so a drag moves the map in
    // the same frame.
    if allow {
        let v = ViewIn { layers: &app.config.minimap_layers, yaw, rect, car, base_zoom_m, speed, now: ui.input(|i| i.time) };
        // (Not `state()`: `v` borrows the config, a disjoint field.)
        let st = match which {
            MapPane::Viewer => &mut app.map_tab,
            MapPane::Navigation => &mut app.nav_ui.map,
        };
        st.manual.interact(ui, &resp, &v);
    } else {
        state(app, which).manual.reset();
    }
    let (centre, zoom_m) = state_ref(app, which).manual.view(car, base_zoom_m);

    let cam = map_scene::texture_or_status(ui, app, rect).map(|texture| {
        let cfg = &app.config;
        let scene = Scene {
            layers: &cfg.minimap_layers,
            centre,
            yaw,
            zoom_m,
            mirror: cfg.minimap_mirror_edges,
            compass: cfg.minimap_show_compass,
            race_sel: &state_ref(app, which).race_sel,
        };
        map_scene::draw(ui, app, rect, texture, &scene)
    });

    // Clicks. Waypoints exist only in a co-op session.
    let in_session = app.coop.role() != crate::coop::Role::Off;
    let (shift, esc) = ui.input(|i| (i.modifiers.shift, i.key_pressed(egui::Key::Escape)));
    if esc {
        state(app, which).dest_armed = false;
    }
    let armed = state_ref(app, which).dest_armed;
    if (armed || which == MapPane::Navigation) && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if let Some(cam) = &cam {
        let click = if resp.clicked() {
            Some(Click::Primary)
        } else if resp.secondary_clicked() {
            Some(Click::Secondary)
        } else {
            None
        };
        if let Some(click) = click {
            let at = || resp.interact_pointer_pos().and_then(|m| map_scene::pick(cam, m));
            match click_action(which, click, shift, armed, in_session) {
                ClickAction::SetDestination => {
                    if let Some(w) = at() {
                        crate::ui::nav_tab::set_destination(app, w);
                        state(app, which).dest_armed = false;
                    }
                }
                ClickAction::SetWaypoint => {
                    if let Some([wx, wz]) = at() {
                        app.coop.set_waypoint(Some((wx, wz)), app.config.coop_hue);
                    }
                }
                ClickAction::ClearWaypoint => app.coop.set_waypoint(None, 0.0),
                ClickAction::Disarm => state(app, which).dest_armed = false,
                ClickAction::Nothing => {}
            }
        }
    }

    // Controls on top of the map (drawn after it, so they take the clicks).
    let dest_now = crate::nav::view().dest;
    let has_dest = dest_now.is_some();
    let shared = dest_now.is_some_and(|d| matches!(d.source, crate::nav::DestSource::Shared { .. }));
    if which == MapPane::Navigation && !has_dest {
        crate::ui::nav_tab::hint_pill(ui, rect);
    }
    let dest_btn = (which == MapPane::Viewer).then(|| DestButton { armed: state_ref(app, which).dest_armed, has_dest, shared });
    let manual = state_ref(app, which).manual.is_manual();
    let out = controls(ui, rect, manual, zoom_m, app.config.minimap_show_compass, dest_btn);
    if out.follow {
        // Panned (or only zoomed): back to the car and the configured zoom, at once.
        state(app, which).manual.reset();
    }
    if out.settings {
        app.config.map_tab_settings = true;
        app.config.map_tab_page = MapPage::DashboardMap;
        app.current_tab = crate::app::Tab::Map;
    }
    if out.arm {
        let st = state(app, which);
        st.dest_armed = !st.dest_armed;
    }
    if out.clear {
        crate::ui::nav_tab::clear_destination(app);
    }
}

/// The viewer's "Set destination" toggle and, while a route exists, "Clear route".
#[derive(Clone, Copy, Debug)]
pub(crate) struct DestButton {
    pub armed: bool,
    pub has_dest: bool,
    /// The destination is the co-op room's: Clear clears it for everyone (as on the Navigation tab).
    pub shared: bool,
}

/// What the buttons over a map pane asked for this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ControlsOut {
    pub follow: bool,
    pub settings: bool,
    pub arm: bool,
    pub clear: bool,
}

/// The pane's buttons and zoom readout over the map: "Follow car" top left (only while the view
/// is manual, like the Dashboard map's; right of the compass when that is on), the radius bottom
/// left, bottom right "Settings" and, in the viewer, "Set destination" (a toggle) with "Clear
/// route" beside it while a route exists. *Why bottom right:* the top right belongs to the co-op
/// player list (D73). *Why only while manual:* a "Follow car" that is already following does
/// nothing and only covers the map (user, D79).
fn controls(ui: &mut Ui, rect: Rect, manual: bool, zoom_m: f32, compass: bool, dest: Option<DestButton>) -> ControlsOut {
    const M: f32 = 10.0;
    let mut out = ControlsOut::default();
    if manual {
        let dx = if compass { (map_scene::compass_rect(rect).right() - rect.left() + 6.0 - M).max(0.0) } else { 0.0 };
        let left = Rect::from_min_size(rect.left_top() + vec2(M + dx, M), vec2((rect.width() * 0.5 - M - dx).max(0.0), 30.0));
        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Min)), |ui| {
            let label = format!("{}  {}", icons::CROSSHAIRS, tr("Follow car"));
            out.follow = ui.add(theme::secondary_button(label)).clicked();
        });
    }
    // From the radius readout (about 70 px) to the right edge: the buttons grow leftwards.
    let right = Rect::from_min_max(pos2(rect.left() + 100.0, rect.bottom() - M - 30.0), pos2(rect.right() - M, rect.bottom() - M));
    ui.scope_builder(UiBuilder::new().max_rect(right).layout(Layout::right_to_left(Align::Max)), |ui| {
        out.settings = ui
            .add(theme::secondary_button(format!("{}  {}", icons::COG, tr("Settings"))))
            .on_hover_text(tr("The map's settings (the Dashboard map & Viewer page of the Map tab)."))
            .clicked();
        if let Some(d) = dest {
            let label = format!("{}  {}", icons::NAVIGATION, tr("Set destination"));
            let button = if d.armed { theme::primary_button(label) } else { theme::secondary_button(label) };
            out.arm = ui
                .add(button)
                .on_hover_text(tr("Click, then click the map to navigate there. Shift+click does the same without this button; right-click or Esc cancels."))
                .clicked();
            if d.has_dest {
                let label = if d.shared { tr("Clear for everyone") } else { tr("Clear route") };
                let b = ui.add(theme::secondary_button(format!("{}  {}", icons::TIMES, label)));
                let tip = if d.shared { tr("Clears the destination for the whole co-op room.") } else { tr("Clears the destination and its route.") };
                out.clear = b.on_hover_text(tip).clicked();
            }
        }
    });
    let text = format!("{:.0} m", zoom_m);
    let font = egui::FontId::proportional(11.0);
    let galley = ui.painter().layout_no_wrap(text, font, theme::TEXT_DIM);
    let at = pos2(rect.left() + M, rect.bottom() - M - galley.size().y);
    ui.painter().rect_filled(Rect::from_min_size(at, galley.size()).expand2(vec2(5.0, 2.0)), 4.0, egui::Color32::from_black_alpha(150));
    ui.painter().galley(at, galley, theme::TEXT_DIM);
    out
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
            let was_shown = dashboard_map_shown(&app.config);
            dashboard_page(ui, &mut app.config, &l, atlas.as_deref());
            if dashboard_map_shown(&app.config) != was_shown {
                app.dashboard_map_toggled();
            }
        }
        MapPage::MapData => crate::ui::map_data::page(ui, app),
    });
}

/// The module selector of the settings mode (the Overlay tab's control).
fn page_selector(ui: &mut Ui, page: &mut MapPage) -> bool {
    let opts = [
        (MapPage::Minimap, tr("Minimap")),
        (MapPage::DashboardMap, tr("Dashboard map & Viewer")),
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
    let mut win3d = cfg.overlay.map_3d_windows;
    let on = cfg.overlay.enabled && cfg.overlay.minimap_on;
    let dash_view = ViewCfg::of_app(cfg);
    let dash_fade = (cfg.coop_trail_fade_secs, cfg.coop_trail_fade_m);
    let mut reset = false;
    let mut view_req = None;
    let layer_req = {
        let o = &mut cfg.overlay;
        let mut lead = |ui: &mut Ui| minimap_card(ui, o, &dash_view, dash_fade, &mut reset, &mut view_req);
        let aux = LayerAux {
            icons: atlas,
            plate: Some((&mut plate, on)),
            enabled: on && !follow,
            which: MapId::Minimap,
            minimap_follows: follow,
            windows_3d: Some(&mut win3d),
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
    cfg.overlay.map_3d_windows = win3d;
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

/// Dashboard map & Viewer page: the view options and all layer settings of `minimap_layers`,
/// which the Dashboard's Map widget and the Map tab viewer both draw with (D73). (Mini-Settings has
/// no map settings, D79.)
fn dashboard_page(ui: &mut Ui, cfg: &mut AppConfig, l: &Layers, atlas: Option<&IconAtlas>) {
    status_ui(ui, l);
    ui.add_space(4.0);
    let mut layers = cfg.minimap_layers.clone();
    let mut view = ViewCfg::of_app(cfg);
    let mut allow = cfg.minimap_allow_pan_zoom;
    let mut win3d = cfg.overlay.map_3d_windows;
    let mut reset = false;
    let mut view_req = None;
    let follows = cfg.overlay.map_use_dashboard;
    let (mut fps_on, mut fps) = (cfg.minimap_fps_limit_enabled, cfg.minimap_fps_limit);
    let mut coop = DashCoop::of(cfg);
    let mut shown = dashboard_map_shown(cfg);
    let mut lead = |ui: &mut Ui| {
        theme::card(ui, tr("Dashboard map & Viewer"), |ui| {
            theme::checkbox_row(ui, &mut shown, tr("Show Dashboard map")).on_hover_text(tr(
                "Whether the Dashboard has its Map module. The Viewer on this tab is always there.",
            ));
            view_rows(ui, &mut view);
            theme::checkbox_row(ui, &mut allow, tr("Allow pan and zoom")).on_hover_text(tr(PAN_ZOOM_TIP));
            theme::checkbox_row(ui, &mut fps_on, tr("Render FPS limit")).on_hover_text(tr(
                "Limits how often the Dashboard map takes the car's position and heading. Off = every frame.",
            ));
            ui.add_enabled_ui(fps_on, |ui| {
                theme::slider_row(ui, tr("FPS limit"), &mut fps, 5.0..=120.0, 1.0, 0, " fps");
            });
            if ui.add(theme::secondary_button(tr("Reset map layers"))).clicked() {
                reset = true;
            }
            view_req = copy_row(ui, MapId::Dashboard, follows, CopyWhat::View);
        });
        dashboard_coop_card(ui, &mut coop);
    };
    let aux = LayerAux { icons: atlas, plate: None, enabled: true, which: MapId::Dashboard, minimap_follows: follows, windows_3d: Some(&mut win3d) };
    let layer_req = layers_ui(ui, &mut layers, aux, &mut lead);
    if reset {
        layers = MapLayerConfig::dashboard();
    }
    cfg.minimap_layers = layers;
    cfg.overlay.map_3d_windows = win3d;
    view.apply_app(cfg);
    cfg.minimap_allow_pan_zoom = allow;
    cfg.minimap_fps_limit_enabled = fps_on;
    cfg.minimap_fps_limit = fps;
    set_dashboard_map_shown(cfg, shown);
    coop.apply(cfg);
    apply_requests(cfg, [layer_req, view_req]);
}

/// Whether the Dashboard has its Map module (`disabled_modules`; it was Mini-Settings → Dashboard →
/// Modules → Map until D90).
fn dashboard_map_shown(c: &AppConfig) -> bool {
    !c.disabled_modules.contains(&WidgetKind::MiniMap)
}

fn set_dashboard_map_shown(c: &mut AppConfig, shown: bool) {
    if shown {
        c.disabled_modules.retain(|k| k != &WidgetKind::MiniMap);
    } else if dashboard_map_shown(c) {
        c.disabled_modules.push(WidgetKind::MiniMap);
    }
}

/// The Dashboard map's co-op options (the keys the Dashboard map and the Viewer draw with, and
/// that the HUD minimap follows with "Use Dashboard co-op settings"): trail fade and the player
/// list. (Were in Mini-Settings → Dashboard → Map → Co-Op until D79.)
#[derive(Clone, PartialEq, Debug)]
struct DashCoop {
    fade_secs: f32,
    fade_m: f32,
    list: bool,
    distance: bool,
    speed: bool,
    gear: bool,
    class: bool,
}

impl DashCoop {
    fn of(c: &AppConfig) -> Self {
        Self {
            fade_secs: c.coop_trail_fade_secs,
            fade_m: c.coop_trail_fade_m,
            list: c.coop_map_playerlist,
            distance: c.coop_list_distance,
            speed: c.coop_list_speed,
            gear: c.coop_list_gear,
            class: c.coop_list_class,
        }
    }

    fn apply(&self, c: &mut AppConfig) {
        c.coop_trail_fade_secs = self.fade_secs;
        c.coop_trail_fade_m = self.fade_m;
        c.coop_map_playerlist = self.list;
        c.coop_list_distance = self.distance;
        c.coop_list_speed = self.speed;
        c.coop_list_gear = self.gear;
        c.coop_list_class = self.class;
    }
}

fn dashboard_coop_card(ui: &mut Ui, c: &mut DashCoop) {
    theme::card(ui, tr("Co-Op"), |ui| {
        ui.label(theme::section_label(tr("Tracer fade")));
        coop_fade_rows(ui, &mut c.fade_secs, &mut c.fade_m);
        ui.add_space(4.0);
        theme::checkbox_row(ui, &mut c.list, tr("Show player list on map"));
        ui.add_enabled_ui(c.list, |ui| {
            ui.label(RichText::new(tr("Columns")).size(11.0).color(theme::TEXT_DIM));
            theme::checkbox_row(ui, &mut c.distance, tr("Distance"));
            theme::checkbox_row(ui, &mut c.speed, tr("Speed"));
            theme::checkbox_row(ui, &mut c.gear, tr("Gear"));
            theme::checkbox_row(ui, &mut c.class, tr("Car class"));
        });
    });
}

/// The two trail-fade sliders (time and distance) both maps' co-op cards share.
fn coop_fade_rows(ui: &mut Ui, secs: &mut f32, metres: &mut f32) {
    let tip = tr("Tracers fade out with whichever comes first — age or distance behind the player.");
    theme::slider_row(ui, tr("Fade after (time)"), secs, 1.0..=60.0, 1.0, 0, " s").on_hover_text(tip);
    theme::slider_row(ui, tr("Fade after (distance)"), metres, 50.0..=3000.0, 50.0, 0, " m").on_hover_text(tip);
}

/// The Map data page's second card: the map image the maps are drawn on. Quality of the built
/// image, rebuilding it, and the calibration of the world-to-image mapping. (Were in Mini-Settings
/// → Dashboard → Map until D79.)
pub(crate) fn map_image_card(ui: &mut Ui, app: &mut ForzaApp) {
    let have_install = crate::gamedata::install::find_media(None).is_some();
    if let Some(rebuild) = map_image_ui(ui, &mut app.config, have_install) {
        app.reload_map_image(rebuild);
    }
}

/// The card's body. Returns `Some(rebuild)` when "Reload Map" (false) or "Rebuild Map Cache" (true)
/// was pressed. `have_install`: the cache can only be rebuilt from the game's files.
fn map_image_ui(ui: &mut Ui, c: &mut AppConfig, have_install: bool) -> Option<bool> {
    let mut reload = None;
    theme::card(ui, tr("Map image"), |ui| {
        theme::slider_row(ui, tr("Image quality"), &mut c.minimap_quality, 20.0..=100.0, 5.0, 0, "%").on_hover_text(tr(
            "100% = full resolution; lower = faster load. Cache makes repeat loads near-instant.",
        ));
        ui.horizontal_wrapped(|ui| {
            if ui.add(theme::secondary_button(tr("Reload Map"))).on_hover_text(tr("Builds the map image again, at the quality above.")).clicked() {
                reload = Some(false);
            }
            // Why: rebuilding deletes the cache, which without an install is the only copy of the map.
            if ui
                .add_enabled(have_install, theme::secondary_button(tr("Rebuild Map Cache")))
                .on_hover_text(tr("Deletes the cached map images and builds them again."))
                .on_disabled_hover_text(tr("Needs your Forza Horizon 6 install — the map is read from it"))
                .clicked()
            {
                reload = Some(true);
            }
        });
        ui.add_space(6.0);
        ui.collapsing(tr("Advanced calibration"), |ui| {
            ui.label(RichText::new(tr("Tune if the car dot is misaligned with the map.\nDefault values are derived from in-game reference points.")).size(11.0).color(theme::TEXT_DIM));
            ui.add_space(6.0);
            control_row(ui, tr("Pixels per metre"), |ui| {
                ui.add(egui::DragValue::new(&mut c.minimap_px_per_m).speed(0.001).range(0.01..=10.0));
            });
            control_row(ui, tr("World origin X (m at pixel 0)"), |ui| {
                ui.add(egui::DragValue::new(&mut c.minimap_world_origin_x).speed(10.0));
            });
            control_row(ui, tr("World origin Z (m at pixel 0)"), |ui| {
                ui.add(egui::DragValue::new(&mut c.minimap_world_origin_z).speed(10.0));
            });
            ui.add_space(4.0);
            if ui.add(theme::secondary_button(tr("Reset to defaults"))).clicked() {
                let d = crate::minimap::MapCalibration::DEFAULT;
                c.minimap_px_per_m = d.px_per_m;
                c.minimap_world_origin_x = d.origin_x;
                c.minimap_world_origin_z = d.origin_z;
            }
        });
    });
    reload
}

/// The Minimap module card of the HUD map page: module switch, "Use Dashboard map settings", the
/// view options (the Dashboard's values, greyed, while it is on) and the co-op options. The
/// layer cards are `maprender::ui::layers_ui`'s. `reset` is set by the "Reset map layers" button.
fn minimap_card(
    ui: &mut Ui,
    o: &mut crate::config::OverlayConfig,
    dash_view: &ViewCfg,
    dash_fade: (f32, f32),
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
        minimap_coop_rows(ui, o, dash_fade);
        ui.add_enabled_ui(!follow, |ui| {
            if ui.add(theme::secondary_button(tr("Reset map layers"))).clicked() {
                *reset = true;
            }
        });
        let follow = o.map_use_dashboard;
        *view_req = copy_row(ui, MapId::Minimap, follow, CopyWhat::View);
    });
}

/// The HUD minimap's co-op options (inside its module card): "Use Dashboard co-op settings" and,
/// when that is off, what the minimap shows of the team and how its trails fade. While it is on
/// the rows show what the HUD really uses (`OverlayConfig::effective`: everything on, the
/// Dashboard's fade), greyed. (Were in Mini-Settings → Overlay → Co-Op until D79.)
fn minimap_coop_rows(ui: &mut Ui, o: &mut crate::config::OverlayConfig, dash_fade: (f32, f32)) {
    ui.add_space(2.0);
    ui.label(theme::section_label(tr("Co-Op")));
    theme::checkbox_row(ui, &mut o.coop_use_dashboard, tr("Use Dashboard co-op settings"))
        .on_hover_text(tr("Show everything and fade the trails like the Dashboard map does."));
    let follow = o.coop_use_dashboard;
    let (mut mates, mut ways, mut trails, mut secs, mut m) =
        if follow { (true, true, true, dash_fade.0, dash_fade.1) } else { (o.coop_teammates, o.coop_waypoints, o.coop_trails, o.coop_trail_fade_secs, o.coop_trail_fade_m) };
    ui.add_enabled_ui(!follow, |ui| {
        theme::checkbox_row(ui, &mut mates, tr("Show co-op teammates"));
        theme::checkbox_row(ui, &mut ways, tr("Show shared waypoints"));
        theme::checkbox_row(ui, &mut trails, tr("Show trails"));
        ui.add_enabled_ui(trails, |ui| coop_fade_rows(ui, &mut secs, &mut m));
    });
    if !follow {
        (o.coop_teammates, o.coop_waypoints, o.coop_trails, o.coop_trail_fade_secs, o.coop_trail_fade_m) = (mates, ways, trails, secs, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::overlay_tab::tests::{check_panes, layers_ready, render, WIDTHS};

    /// The pane's buttons and readout stay inside the tab at the window minimum and wider, in
    /// both languages, and never overlap each other, the compass (top left) or the co-op player
    /// list (top right). The Settings button sits bottom right (D73); in the viewer "Set
    /// destination" (and "Clear route" while a route exists) sit to its left.
    #[test]
    fn viewer_controls_stay_inside_the_tab() {
        use crate::i18n::{with_language, Language};
        // (what the viewer adds, text shapes it adds)
        let dests = [
            (None, 0),
            (Some(DestButton { armed: false, has_dest: false, shared: false }), 1),
            (Some(DestButton { armed: true, has_dest: true, shared: true }), 2),
        ];
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in [600.0, 700.0, 1000.0, 1235.0] {
                    for compass in [false, true] {
                        for (dest, extra) in dests {
                            let h = 500.0;
                            // Following the car: no Follow car button. Panned or zoomed: it shows.
                            let out = render("map_viewer", w, h, |ui, _| {
                                let rect = ui.available_rect_before_wrap();
                                controls(ui, rect, false, 1500.0, compass, dest);
                            });
                            check_panes(&out, w, "viewer controls, following");
                            let n = out.shapes.iter().filter(|c| matches!(&c.shape, egui::Shape::Text(_))).count();
                            assert_eq!(n, 2 + extra, "{lang:?} at {w} px: while following only the buttons and the radius show");
                            let out = render("map_viewer", w, h, |ui, _| {
                                let rect = ui.available_rect_before_wrap();
                                controls(ui, rect, true, 1500.0, compass, dest);
                            });
                            check_panes(&out, w, "viewer controls");
                            // Text shapes in paint order: Follow car, Settings, [Set destination, [Clear route]], the radius.
                            let labels: Vec<Rect> = out
                                .shapes
                                .iter()
                                .filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.visual_bounding_rect()) } else { None })
                                .collect();
                            assert_eq!(labels.len(), 3 + extra, "{lang:?} at {w} px: follow, settings, destination buttons, radius");
                            let (follow, settings, radius) = (labels[0], labels[1], *labels.last().unwrap());
                            for (i, a) in labels.iter().enumerate() {
                                for b in &labels[i + 1..] {
                                    assert!(!a.intersects(*b), "{lang:?} at {w} px: labels touch: {a:?} / {b:?}");
                                }
                            }
                            // Corners: Follow top left, the buttons bottom right, the radius bottom left.
                            assert!(follow.center().x < w * 0.5 && follow.center().y < h * 0.25, "{lang:?} {w}: follow top left {follow:?}");
                            assert!(settings.center().x > w * 0.5 && settings.center().y > h * 0.75, "{lang:?} {w}: settings bottom right {settings:?}");
                            assert!(radius.center().x < w * 0.5 && radius.center().y > h * 0.75, "{lang:?} {w}: radius bottom left {radius:?}");
                            // The destination buttons sit left of Settings, on the same row, clear of the radius.
                            for b in &labels[2..labels.len() - 1] {
                                assert!(b.right() < settings.left() && b.center().y > h * 0.75, "{lang:?} {w}: destination button beside Settings {b:?}");
                                assert!(b.left() > radius.right() + 8.0, "{lang:?} {w}: destination button over the radius readout {b:?}");
                            }
                            // The co-op list: top right, up to ~320 px wide and 9 rows (17 px each) tall.
                            let list = Rect::from_min_size(pos2(w - 320.0 - 6.0, 6.0), vec2(320.0, 10.0 + 17.0 * 9.0));
                            assert!(!list.intersects(settings), "{lang:?} at {w} px: settings under the co-op list");
                            assert!(!list.intersects(follow) || w < 700.0, "{lang:?} at {w} px: follow under the co-op list");
                            // The compass (top left, scaled with the map) is clear of the button.
                            if compass {
                                let c = Rect::from_min_size(pos2(0.0, 0.0), vec2(map_scene::compass_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))).right(), 50.0));
                                assert!(!c.intersects(follow), "{lang:?} at {w} px: follow car over the compass");
                            }
                        }
                    }
                }
            });
        }
    }

    /// The click rule of the two maps (D84): the Navigation tab's left click always sets the
    /// destination; the viewer keeps left = waypoint and right = clear waypoint and adds Shift+click
    /// and the armed button for the destination; armed, a right click only disarms.
    #[test]
    fn clicks_on_the_maps_do_what_each_map_promises() {
        use Click::{Primary, Secondary};
        use ClickAction::*;
        for shift in [false, true] {
            for armed in [false, true] {
                for session in [false, true] {
                    assert_eq!(click_action(MapPane::Navigation, Primary, shift, armed, session), SetDestination, "nav tab, any left click");
                    assert_eq!(click_action(MapPane::Navigation, Secondary, shift, armed, session), Nothing, "nav tab: the right click does nothing");
                }
            }
        }
        let v = |c, shift, armed, session| click_action(MapPane::Viewer, c, shift, armed, session);
        // Plain clicks: the waypoint, only in a session.
        assert_eq!(v(Primary, false, false, true), SetWaypoint);
        assert_eq!(v(Primary, false, false, false), Nothing);
        assert_eq!(v(Secondary, false, false, true), ClearWaypoint);
        assert_eq!(v(Secondary, false, false, false), Nothing);
        // Shift+click or the armed button: the destination, with or without a session.
        for session in [false, true] {
            assert_eq!(v(Primary, true, false, session), SetDestination);
            assert_eq!(v(Primary, false, true, session), SetDestination);
            assert_eq!(v(Primary, true, true, session), SetDestination);
            // Armed: the right click cancels and leaves the waypoint alone.
            assert_eq!(v(Secondary, false, true, session), Disarm);
        }
        // Shift + right click is still the waypoint clear.
        assert_eq!(v(Secondary, true, false, true), ClearWaypoint);
    }

    /// Both maps' pages (the shared `layers_ui`), plus the HUD page following the Dashboard and
    /// with its module off: seven cards each (eight on the Dashboard page, with its Co-Op card),
    /// nothing leaves its pane. In German too (the long labels).
    #[test]
    fn map_pages_stay_inside_their_panes() {
        use crate::i18n::{with_language, Language};
        let l = layers_ready();
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in WIDTHS {
                    // `relief`: the View mode card in 3D mode, with all its extra rows (the
                    // tallest and widest the card gets).
                    for (name, follow, module_on, relief) in [
                        ("minimap", false, true, false),
                        ("minimap_follow", true, true, false),
                        ("minimap_off", false, false, false),
                        ("dashboard", false, true, false),
                        ("minimap_3d", false, true, true),
                        ("minimap_follow_3d", true, true, true),
                        ("dashboard_3d", false, true, true),
                    ] {
                        let mut cfg = AppConfig::default();
                        cfg.overlay.map_use_dashboard = follow;
                        cfg.overlay.minimap_on = module_on;
                        if relief {
                            for t in [&mut cfg.overlay.map_layers.tilt, &mut cfg.minimap_layers.tilt] {
                                t.set_view_mode(crate::maprender::cfg::ViewMode::Relief);
                            }
                        }
                        let out = render(&format!("map_{name}"), w, 3600.0, |ui, atlas| match name {
                            n if n.starts_with("minimap") => minimap_page(ui, &mut cfg, &l, Some(atlas)),
                            _ => dashboard_page(ui, &mut cfg, &l, Some(atlas)),
                        });
                        let cards = if name.starts_with("dashboard") { 8 } else { 7 };
                        assert_eq!(check_panes(&out, w, name), cards, "{lang:?} {name} at {w} px: expected {cards} card frames");
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
            assert_eq!(check_panes(&out, 700.0, "status"), 7);
            let out = render("map_status", 700.0, 3600.0, |ui, _| dashboard_page(ui, &mut cfg, &l, None));
            assert_eq!(check_panes(&out, 700.0, "status"), 8);
        }
    }

    /// The text is drawn (card titles are drawn in capitals).
    fn has_text(out: &egui::FullOutput, text: &str) -> bool {
        find_text(out, text).is_some() || find_text(out, &text.to_uppercase()).is_some()
    }

    /// The Race lines card's new controls (D82): the Race line style row with either style, and
    /// "Race road only" as the selected In a race / Other roads entry; nothing leaves its pane.
    #[test]
    fn race_lines_card_shows_the_route_style_and_race_road_only() {
        use crate::i18n::{with_language, tr, Language};
        use crate::maprender::cfg::{OtherRoads, RouteStyle};
        let l = layers_ready();
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in WIDTHS {
                    for (route, other) in [(RouteStyle::Road, OtherRoads::RaceOnly), (RouteStyle::Line, OtherRoads::Muted), (RouteStyle::Road, OtherRoads::Normal)] {
                        let mut cfg = AppConfig::default();
                        cfg.minimap_layers.race_lines.route = route;
                        cfg.minimap_layers.race_lines.focus.other_roads = other;
                        let out = render("map_race_card", w, 3600.0, |ui, atlas| dashboard_page(ui, &mut cfg, &l, Some(atlas)));
                        assert_eq!(check_panes(&out, w, "race card"), 8);
                        assert!(has_text(&out, tr("Race line style")), "{lang:?} {w}: the style row");
                        let style = if route == RouteStyle::Road { tr("Road") } else { tr("Line") };
                        assert!(has_text(&out, style), "{lang:?} {w}: the style row shows {style}");
                        if other == OtherRoads::RaceOnly {
                            assert!(has_text(&out, tr("Race road only")), "{lang:?} {w}: Other roads shows Race road only");
                        }
                    }
                }
            });
        }
    }

    /// The map settings that left Mini-Settings (D79) are all here: FPS limit and the Co-Op card on
    /// the Dashboard page, "Use Dashboard co-op settings" on the Minimap page, and the Map image
    /// card (quality, reload, rebuild, calibration) on the Map data page, in both languages and at
    /// every pane width.
    #[test]
    fn map_settings_from_mini_settings_are_on_the_map_tab() {
        use crate::i18n::{with_language, tr, Language};
        let l = layers_ready();
        for lang in [Language::English, Language::German] {
            with_language(lang, || {
                for w in WIDTHS {
                    let mut cfg = AppConfig::default();
                    let out = render("map_dash_extras", w, 3600.0, |ui, atlas| dashboard_page(ui, &mut cfg, &l, Some(atlas)));
                    for t in ["Show Dashboard map", "Render FPS limit", "Co-Op", "Tracer fade", "Show player list on map", "Allow pan and zoom", "Rotate with right stick"] {
                        assert!(has_text(&out, tr(t)), "{lang:?} {w}: Dashboard page lacks {t}");
                    }
                    cfg.overlay.map_use_dashboard = false;
                    let out = render("map_mini_extras", w, 3600.0, |ui, atlas| minimap_page(ui, &mut cfg, &l, Some(atlas)));
                    for t in ["Use Dashboard co-op settings", "Show co-op teammates", "Show shared waypoints", "Show trails"] {
                        assert!(has_text(&out, tr(t)), "{lang:?} {w}: Minimap page lacks {t}");
                    }
                    for have_install in [true, false] {
                        let out = render("map_image", w, 1200.0, |ui, _| {
                            map_image_ui(ui, &mut cfg, have_install);
                        });
                        assert_eq!(check_panes(&out, w, "map image"), 1);
                        for t in ["Map image", "Image quality", "Reload Map", "Rebuild Map Cache", "Advanced calibration"] {
                            assert!(has_text(&out, tr(t)), "{lang:?} {w}: Map image card lacks {t}");
                        }
                    }
                }
            });
        }
    }

    /// D90: the Dashboard's Map module switch is `disabled_modules` and nothing else; the helpers
    /// add / remove exactly `WidgetKind::MiniMap` and keep the other modules.
    #[test]
    fn show_dashboard_map_toggles_only_the_map_module() {
        let mut c = AppConfig::default();
        set_dashboard_map_shown(&mut c, true);
        assert!(dashboard_map_shown(&c));
        c.disabled_modules.push(WidgetKind::Trace);
        set_dashboard_map_shown(&mut c, false);
        set_dashboard_map_shown(&mut c, false);
        assert!(!dashboard_map_shown(&c));
        assert_eq!(c.disabled_modules.iter().filter(|k| **k == WidgetKind::MiniMap).count(), 1);
        assert!(c.disabled_modules.contains(&WidgetKind::Trace));
        set_dashboard_map_shown(&mut c, true);
        assert!(dashboard_map_shown(&c) && c.disabled_modules.contains(&WidgetKind::Trace));
    }

    /// The module selector: one row where the labels fit, two where they don't (the long German
    /// labels at the window minimum), and its labels never run into each other. (The Map data
    /// page itself needs the app's state; its pane tests live in `map_data`.)
    #[test]
    fn module_selector_never_overlaps_its_labels() {
        let en = [
            (MapPage::Minimap, "Minimap"),
            (MapPage::DashboardMap, "Dashboard map & Viewer"),
            (MapPage::MapData, "Map data"),
        ];
        let de = [
            (MapPage::Minimap, "Minikarte"),
            (MapPage::DashboardMap, "Dashboard-Karte & Viewer"),
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
                assert_eq!(labels.len(), 3, "{lang} at {w} px");
                for (i, a) in labels.iter().enumerate() {
                    assert!(a.left() >= 0.0 && a.right() <= w, "{lang} at {w} px: label leaves the window: {a:?}");
                    for b in &labels[i + 1..] {
                        assert!(!a.expand(2.0).intersects(*b), "{lang} at {w} px: labels touch: {a:?} / {b:?}");
                    }
                }
            }
        }
    }

    /// The remembered mode and page round-trip through the config JSON and are never exported.
    #[test]
    fn remembered_state_is_saved_but_not_exported() {
        let mut c = AppConfig::default();
        c.map_tab_settings = true;
        c.map_tab_page = MapPage::MapData;
        let back: AppConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert!(back.map_tab_settings);
        assert_eq!(back.map_tab_page, MapPage::MapData);
        let all = vec![true; crate::config::KEY_GROUPS.len()];
        let json = crate::config::export_selected(&c, &all);
        assert!(!json.contains("map_tab_settings") && !json.contains("map_tab_page"));
    }

    /// The viewer has no settings of its own (D73): a new config has no `viewer_*` keys, and
    /// the old default of its pan / zoom option is the Dashboard map's.
    #[test]
    fn the_viewer_has_no_config_keys_of_its_own() {
        let c = AppConfig::default();
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("viewer_"), "no viewer_* key is written any more");
        let all = vec![true; crate::config::KEY_GROUPS.len()];
        assert!(!crate::config::export_selected(&c, &all).contains("viewer_"));
        assert!(c.minimap_allow_pan_zoom);
        assert!(!c.map_tab_settings && c.map_tab_page == MapPage::Minimap);
    }

    /// An old config (D67/D72, before D73) with the Viewer's own keys and the removed Viewer page
    /// loads: every other key is kept, nothing counts as unreadable (so no `.bad-` backup), and
    /// the next save drops the old keys. `map_tab_page: "viewer"` becomes the shared page.
    #[test]
    fn an_old_config_with_viewer_keys_still_loads() {
        let mut v = serde_json::to_value(AppConfig::default()).unwrap();
        v["minimap_zoom_driving_m"] = 1234.0.into();
        v["map_tab_page"] = "viewer".into();
        v["viewer_layers"] = serde_json::to_value(MapLayerConfig::dashboard()).unwrap();
        v["viewer_north_up"] = false.into();
        v["viewer_mirror_edges"] = false.into();
        v["viewer_show_compass"] = true.into();
        v["viewer_allow_pan_zoom"] = false.into();
        v["viewer_zoom_m"] = 2500.0.into();
        let c: AppConfig = serde_json::from_value(v.clone()).expect("old keys are ignored, not an error");
        assert_eq!(c.map_tab_page, MapPage::DashboardMap);
        assert_eq!(c.minimap_zoom_driving_m, 1234.0, "other keys are kept");
        assert!(c.minimap_north_up && c.minimap_allow_pan_zoom, "the viewer's old values are not migrated");
        assert!(!serde_json::to_string(&c).unwrap().contains("viewer_"), "the next save drops them");
        // The page name written before D73 for the Dashboard page still loads too.
        v["map_tab_page"] = "dashboard_map".into();
        let c: AppConfig = serde_json::from_value(v).unwrap();
        assert_eq!(c.map_tab_page, MapPage::DashboardMap);
    }

    /// The Roads card's width sliders reach 100 px (D74), and the zoom rule really lets a road
    /// grow that far: nothing in `road_base_px` caps it below `max_px`.
    #[test]
    fn the_road_width_limit_goes_up_to_100_px() {
        use crate::maprender::ui::ROAD_PX_MAX;
        assert_eq!(ROAD_PX_MAX, 100.0);
        let mut c = MapLayerConfig::dashboard().roads;
        assert_eq!(c.max_px, 10.0, "the default stays; the user tunes it");
        c.max_px = ROAD_PX_MAX;
        c.min_px = 1.0;
        c.metres = 10.0;
        // 12 px per metre: far zoomed in. The rule would give 120 px, the limit holds it at 100.
        assert_eq!(crate::maprender::style::road_base_px(&c, 12.0), ROAD_PX_MAX);
        assert_eq!(crate::maprender::style::road_base_px(&c, 5.0), 50.0, "below the limit it follows the zoom exactly");
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
    /// view card, pick "Minimap" in the menu, and the HUD minimap's view options now match the
    /// Dashboard map's; the button then reads "Copied".
    #[test]
    fn clicking_copy_to_applies_the_request() {
        let ctx = crate::ui::test_render::context();
        let l = layers_ready();
        let mut cfg = AppConfig::default();
        cfg.minimap_north_up = false;
        cfg.minimap_show_compass = true;
        cfg.overlay.map_north_up = true;
        cfg.overlay.compass = false;
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
        let item = find_text(&out, "Minimap").expect("the menu lists the Minimap");
        assert!(find_text(&out, "Both").is_some());
        assert!(find_text(&out, "Viewer").is_none(), "no separate Viewer entry: it shares the Dashboard map's settings");
        click(&ctx, w, item, &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        assert!(!cfg.overlay.map_north_up && cfg.overlay.compass, "the HUD minimap took the Dashboard map's view options");
        assert!(!cfg.minimap_north_up, "the source is unchanged");
        let out = frame(&ctx, w, vec![], &mut |ui| dashboard_page(ui, &mut cfg, &l, None));
        let copied = |c: &egui::epaint::ClippedShape| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text().ends_with("Copied"));
        assert!(out.shapes.iter().any(copied), "the button confirms");
    }

    /// The menu with the Minimap following the Dashboard map: the Minimap entry and "Both" are
    /// disabled (clicking them does nothing: the menu stays open, no "Copied", nothing written).
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
            assert!(find_text(&out, "Both").is_some() && find_text(&out, "Minimap").is_some(), "{entry} is disabled: the menu stays open");
            assert!(!copied(&out), "{entry} must not copy");
        }
        assert_eq!(cfg.overlay.map_layers, own, "the following Minimap's own config is never written");
    }
}
