//! The settings UI of the shared map renderer (D63): everything `MapLayerConfig` holds, as
//! cards, written once and used by the three map pages of the Map tab (Minimap, Dashboard map
//! and Viewer, which share one set of settings).
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
    DashStyle, ImageCfg, LayerCategory, MapLayerConfig, OtherRoads, PoisCfg, RaceCfg, RaceLineMode, RoadStyles, RoadTypeStyle, RoadHeight, RoadsCfg, ReliefCfg, TiltCfg, ViewMode,
};
use super::paint2d::IconAtlas;
use super::store::{LayerStatus, Layers};
use super::style::{self, Shape};
use crate::config::{AppConfig, OverlayConfig};
use crate::i18n::tr;
use crate::theme;
use crate::ui::overlay_tab::{control_row, control_row_tip, pct_row, status_line};

/// From this page width up the layer cards sit in three columns, below it in two (the same
/// switch as the Overlay tab, see `overlay_tab::THREE_COLS_MIN_W`).
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

// ── 3D status ────────────────────────────────────────────────────────────────────────────────

/// What the View mode card says while 3D is selected (K4); nothing when all is well.
#[derive(Clone, Debug, PartialEq)]
pub enum Status3d {
    /// The terrain is being read / built.
    Loading,
    /// The map draws as Tilted instead; the reason (a GL message is English, as `gl3d` words it).
    Unavailable(String),
}

/// The line for the 3D state, from the terrain store and the most recent GL failure
/// ([`gl3d::last_failure`](super::gl3d::last_failure)). *Why the GL failure comes first:* the
/// terrain is of no use when the context cannot draw it, and the reason is the one the user can
/// act on (update the driver, ...).
pub fn status_3d(failure: Option<&str>, terrain: &super::store::TerrainStatus) -> Option<Status3d> {
    use super::store::TerrainStatus as T;
    if let Some(why) = failure {
        return Some(Status3d::Unavailable(why.to_string()));
    }
    match terrain {
        T::Ready(_) => None,
        T::Loading => Some(Status3d::Loading),
        T::NoInstall => Some(Status3d::Unavailable(tr("no Forza Horizon 6 install found (the terrain comes from the game files)").to_string())),
        T::Error(e) => Some(Status3d::Unavailable(e.clone())),
    }
}

/// [`status_3d`] of the live process. Polling the terrain store starts its lazy load, which is
/// wanted: the card shows this only while 3D is selected.
#[cfg(not(test))]
fn live_status_3d() -> Option<Status3d> {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        status_3d(super::gl3d::last_failure().as_deref(), &super::store::terrain())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Some(Status3d::Unavailable(tr("this system has no 3D map renderer").to_string()))
    }
}

/// Tests must not start the terrain loader (it reads the real install); they set the state.
#[cfg(test)]
thread_local! {
    static TEST_STATUS_3D: std::cell::RefCell<Option<Status3d>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn live_status_3d() -> Option<Status3d> {
    TEST_STATUS_3D.with(|s| s.borrow().clone())
}

fn status_3d_row(ui: &mut Ui, s: &Option<Status3d>) {
    match s {
        None => {}
        Some(Status3d::Loading) => {
            status_line(ui, theme::FAINT, tr("Loading terrain…"));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
        Some(Status3d::Unavailable(why)) => status_line(ui, theme::WARN, &format!("{} {why}", tr("3D not available:"))),
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

// ── "Copy to …" (D68) ────────────────────────────────────────────────────────────────────────

/// The maps that have settings: two, since the Map tab's viewer shares the Dashboard map's (D73).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MapId {
    /// The HUD minimap (`overlay.map_layers`, `overlay.map_*`).
    Minimap,
    /// The Dashboard's Map widget and the Map tab's viewer (`minimap_layers`, `minimap_*`).
    Dashboard,
}

impl MapId {
    pub const ALL: [MapId; 2] = [MapId::Minimap, MapId::Dashboard];

    pub fn name(self) -> &'static str {
        tr(match self {
            MapId::Minimap => "Minimap",
            MapId::Dashboard => "Dashboard map & Viewer",
        })
    }
}

/// What a "Copy to …" button copies: one layer card, or the page's view options.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CopyWhat {
    Layer(LayerCategory),
    View,
}

/// A pressed "Copy to …": overwrite `what` of every map in `to` with `from`'s. Returned by
/// [`layers_ui`] (and [`copy_menu`]) and applied by the page with [`apply_copy`], so the UI
/// code never touches another map's config.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CopyRequest {
    pub from: MapId,
    pub what: CopyWhat,
    pub to: Vec<MapId>,
}

/// The layers a map *draws with*: the HUD minimap draws the Dashboard map's while it follows it.
fn layers_of(cfg: &AppConfig, id: MapId) -> &MapLayerConfig {
    match id {
        MapId::Minimap if cfg.overlay.map_use_dashboard => &cfg.minimap_layers,
        MapId::Minimap => &cfg.overlay.map_layers,
        MapId::Dashboard => &cfg.minimap_layers,
    }
}

/// The view options a map draws with.
fn view_of(cfg: &AppConfig, id: MapId) -> ViewCfg {
    match id {
        MapId::Minimap if !cfg.overlay.map_use_dashboard => ViewCfg::of_overlay(&cfg.overlay),
        MapId::Minimap | MapId::Dashboard => ViewCfg::of_app(cfg),
    }
}

/// Apply a [`CopyRequest`]. Skipped targets: the source itself, and the Minimap while it follows
/// the Dashboard map (it draws the Dashboard's values anyway, so writing its own would
/// silently change what it shows after the user turns the follow off). Returns the maps that
/// were written.
pub fn apply_copy(cfg: &mut AppConfig, req: &CopyRequest) -> Vec<MapId> {
    let mut done = Vec::new();
    for &to in &req.to {
        if to == req.from || (to == MapId::Minimap && cfg.overlay.map_use_dashboard) {
            continue;
        }
        match req.what {
            CopyWhat::Layer(cat) => {
                let src = layers_of(cfg, req.from).clone();
                match to {
                    MapId::Minimap => cfg.overlay.map_layers.copy_category(&src, cat),
                    MapId::Dashboard => cfg.minimap_layers.copy_category(&src, cat),
                }
            }
            CopyWhat::View => copy_view(cfg, req.from, to),
        }
        done.push(to);
    }
    done
}

/// Copy the view options from one map to another (all of [`ViewCfg`]).
fn copy_view(cfg: &mut AppConfig, from: MapId, to: MapId) {
    let src = view_of(cfg, from);
    match to {
        MapId::Minimap => src.apply_overlay(&mut cfg.overlay),
        MapId::Dashboard => src.apply_app(cfg),
    }
}

/// How long the button says "Copied".
const COPIED_SECS: f64 = 1.5;

/// Why the Minimap cannot be a copy target while it follows the Dashboard map.
const FOLLOWS_TIP: &str = "The Minimap follows the Dashboard map's settings. Turn that off on the Minimap page to copy into it.";

/// The "Copy to …" menu button of one card: a small button that opens a list of the other maps
/// plus "Both". Returns the request when one is picked. `minimap_follows`: the Minimap follows
/// the Dashboard map, so it is greyed as a target (with a tooltip saying why). The button reads
/// "Copied" for a moment afterwards.
pub fn copy_menu(ui: &mut Ui, which: MapId, minimap_follows: bool, what: CopyWhat) -> Option<CopyRequest> {
    let id = egui::Id::new(("map_copy_done", which, what));
    let now = ui.input(|i| i.time);
    let done_at: Option<f64> = ui.data(|d| d.get_temp(id));
    let recent = done_at.filter(|t| now - t < COPIED_SECS);
    if let Some(t) = recent {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((COPIED_SECS - (now - t)).max(0.0) + 0.05));
    }
    let label = if recent.is_some() { format!("{}  {}", crate::icons::CHECK, tr("Copied")) } else { tr("Copy to…").to_string() };
    let others: Vec<MapId> = MapId::ALL.into_iter().filter(|m| *m != which).collect();
    let blocked = |m: MapId| m == MapId::Minimap && minimap_follows;
    let mut req = None;
    // Closes on a pick (`ui.close()` below) or a click outside, not on a click on a disabled entry.
    let cfg = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let (btn, _) = egui::containers::menu::MenuButton::from_button(theme::secondary_button(label).small()).config(cfg).ui(ui, |ui| {
        for &m in &others {
            let b = ui.add_enabled(!blocked(m), egui::Button::new(m.name()));
            let b = if blocked(m) { b.on_disabled_hover_text(tr(FOLLOWS_TIP)) } else { b };
            if b.clicked() {
                req = Some(CopyRequest { from: which, what, to: vec![m] });
                ui.close();
            }
        }
        ui.separator();
        let any_blocked = others.iter().any(|m| blocked(*m));
        let b = ui.add_enabled(!any_blocked, egui::Button::new(tr("Both")));
        let b = if any_blocked { b.on_disabled_hover_text(tr(FOLLOWS_TIP)) } else { b };
        if b.clicked() {
            req = Some(CopyRequest { from: which, what, to: others.clone() });
            ui.close();
        }
    });
    btn.on_hover_text(tr("Overwrite this card's settings on the other map(s) with the ones shown here."));
    if req.is_some() {
        ui.data_mut(|d| d.insert_temp(id, now));
        ui.ctx().request_repaint();
    }
    req
}

/// The row at the end of a card holding its "Copy to …" button, right-aligned.
pub fn copy_row(ui: &mut Ui, which: MapId, minimap_follows: bool, what: CopyWhat) -> Option<CopyRequest> {
    ui.add_space(4.0);
    // In a `horizontal`: a bare right-to-left layout would claim all the height left in the page.
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| copy_menu(ui, which, minimap_follows, what)).inner
    })
    .inner
}

/// [`copy_row`] of a layer card of [`layers_ui`].
fn card_copy_row(ui: &mut Ui, cp: &CopyCtx, cat: LayerCategory) {
    if let Some(r) = copy_row(ui, cp.which, cp.minimap_follows, CopyWhat::Layer(cat)) {
        *cp.out.borrow_mut() = Some(r);
    }
}

/// What the cards of [`layers_ui`] need for their "Copy to …" row.
struct CopyCtx {
    which: MapId,
    minimap_follows: bool,
    out: std::cell::RefCell<Option<CopyRequest>>,
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
    /// False greys the layer cards (module off, or following the Dashboard map). The "Copy to …"
    /// rows stay usable: copying *from* a greyed page copies what the map really draws.
    pub enabled: bool,
    /// Which map these cards edit (the source of their "Copy to …" menus).
    pub which: MapId,
    /// The Minimap follows the Dashboard map, so it cannot be copied into.
    pub minimap_follows: bool,
    /// The "allow 3D on Windows" flag (`OverlayConfig::map_3d_windows`), shown on the View mode
    /// card on Windows only; a copy the page writes back, like `plate`.
    pub windows_3d: Option<&'a mut bool>,
}

/// All layer settings of one map as cards in two or three columns, with `lead` (the page's own
/// first card: view options, module switch) on top of the first column. All three map pages
/// call this. Returns the "Copy to …" a card's button asked for, for the page to apply with
/// [`apply_copy`] (this function only edits `cfg`, it never sees the other maps).
pub fn layers_ui(ui: &mut Ui, cfg: &mut MapLayerConfig, ax: LayerAux, lead: &mut dyn FnMut(&mut Ui)) -> Option<CopyRequest> {
    let three = ui.available_width() >= THREE_COLS_MIN_W;
    let LayerAux { icons, plate, enabled, which, minimap_follows, windows_3d } = ax;
    let mut win3d = windows_3d;
    let cp = CopyCtx { which, minimap_follows, out: Default::default() };
    let cp = &cp;
    let MapLayerConfig { image, roads, pois, race_lines, tilt } = cfg;
    let mut plate = plate;
    let mut image_card_ = |ui: &mut Ui| image_card(ui, image, plate.as_mut().map(|(v, e)| (&mut **v, *e)), enabled, cp);
    let mut tilt_card_ = |ui: &mut Ui| tilt_card(ui, tilt, enabled, cp, win3d.as_deref_mut());
    let mut race_card_ = |ui: &mut Ui| race_lines_card(ui, race_lines, enabled, cp);
    let mut roads_card_ = |ui: &mut Ui| roads_card(ui, roads, enabled, cp);
    let mut pois_card_ = |ui: &mut Ui| pois_card(ui, pois, icons, enabled, cp);
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
    cp.out.take()
}

fn image_card(ui: &mut Ui, c: &mut ImageCfg, plate: Option<(&mut f32, bool)>, enabled: bool, cp: &CopyCtx) {
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
        card_copy_row(ui, cp, LayerCategory::Image);
    });
}

/// The "View mode" card: Flat / Tilted / 3D (D67), then the tilt rows (Tilted and 3D share the
/// camera, so they are one set of rows) and, for 3D only, the relief options. The 3D group is
/// *hidden* in the other modes (not greyed): it is a whole group of rows that mean nothing
/// there. `ViewMode` is derived (`TiltCfg::view_mode`), so the control only writes `on` /
/// `relief.on` and the rest of the user's look stays when switching back and forth.
///
/// `win3d` is the one "allow 3D on Windows" flag (`OverlayConfig::map_3d_windows`): on Windows the
/// 3D segment reads "3D (experimental)" and, while 3D is picked, a checkbox turns it on (until
/// then the maps stay Tilted and the status line says so). Elsewhere it is not shown.
fn tilt_card(ui: &mut Ui, c: &mut TiltCfg, enabled: bool, cp: &CopyCtx, win3d: Option<&mut bool>) {
    theme::card(ui, tr("View mode"), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            view_mode_picker(ui, c);
            let mode = c.view_mode();
            if mode == ViewMode::Relief {
                let mut allowed = true;
                if cfg!(windows) {
                    ui.add_space(4.0);
                    let mut local = false;
                    let flag = win3d.unwrap_or(&mut local);
                    theme::checkbox_row(ui, flag, tr("Allow 3D on Windows")).on_hover_text(tr(
                        "The 3D map has not been tested on Windows yet. Tick this to try it; if the map misbehaves, untick it to get the Tilted view back.",
                    ));
                    allowed = *flag;
                }
                ui.add_space(4.0);
                if allowed {
                    status_3d_row(ui, &live_status_3d());
                } else {
                    status_3d_row(ui, &Some(Status3d::Unavailable(tr("switched off on Windows until you allow it above").to_string())));
                }
            }
            ui.add_space(4.0);
            ui.add_enabled_ui(mode != ViewMode::Flat, |ui| {
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
                // In 3D the lines are meshes whose widths follow the perspective by themselves.
                ui.add_enabled_ui(mode != ViewMode::Relief, |ui| {
                    theme::checkbox_row(ui, &mut c.taper, tr("Thinner lines in the distance"))
                        .on_disabled_hover_text(tr("Only for the Tilted view: the 3D view sets the line widths itself."));
                });
            });
            if mode == ViewMode::Relief {
                relief_rows(ui, &mut c.relief);
            }
        });
        card_copy_row(ui, cp, LayerCategory::Tilt);
    });
}

/// The Flat / Tilted / 3D control. The tooltip sits on the 3D segment only (`theme::segmented`
/// has no per-segment response): a hover-only widget over its third.
fn view_mode_picker(ui: &mut Ui, c: &mut TiltCfg) {
    let mut mode = c.view_mode();
    let top = ui.cursor().min.y;
    let (left, width) = (ui.cursor().min.x, ui.available_width());
    let opts = [(ViewMode::Flat, tr("Flat")), (ViewMode::Tilted, tr("Tilted")), (ViewMode::Relief, if cfg!(windows) { tr("3D (experimental)") } else { tr("3D") })];
    if theme::segmented(ui, &mut mode, &opts) {
        c.set_view_mode(mode);
    }
    let third = Rect::from_min_max(pos2(left + width * 2.0 / 3.0, top), pos2(left + width, ui.min_rect().bottom()));
    ui.interact(third, ui.id().with("view_mode_3d_tip"), Sense::hover())
        .on_hover_text(tr("Uses your graphics card; falls back to Tilted if it isn't supported."));
}

fn road_height_label(h: RoadHeight) -> &'static str {
    tr(match h {
        RoadHeight::Nodes => "Node heights",
        RoadHeight::Terrain => "On the terrain",
    })
}

/// The 3D-only options (phase K, D51). Values are clamped to `ReliefCfg`'s ranges (the sliders
/// do it for typing too, `sane()` also catches a hand-edited config).
fn relief_rows(ui: &mut Ui, r: &mut ReliefCfg) {
    *r = r.sane();
    ui.add_space(2.0);
    ui.label(theme::section_label(tr("3D")));
    let tip = tr("Where the roads get their height. Node heights: bridges and ramps float at the height the game's road network has. On the terrain: every road is laid on the ground. Cross-country is always laid on the ground, jumps are a taut string between take-off and landing.");
    control_row_tip(ui, tr("Road height"), tip, |ui| {
        egui::ComboBox::from_id_salt("map_relief_road_height")
            .selected_text(road_height_label(r.road_height))
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for h in [RoadHeight::Nodes, RoadHeight::Terrain] {
                    ui.selectable_value(&mut r.road_height, h, road_height_label(h));
                }
            });
    });
    let (d, e, s) = (ReliefCfg::DECK_RANGE, ReliefCfg::EXAG_RANGE, ReliefCfg::SHADING_RANGE);
    theme::slider_row(ui, tr("Deck thickness"), &mut r.deck_m, d.0..=d.1, 0.5, 1, " m")
        .on_hover_text(tr("How thick the road bodies are. 0 = paper-thin ribbons."));
    theme::slider_row(ui, tr("Height exaggeration"), &mut r.exaggeration, e.0..=e.1, 0.1, 1, "×")
        .on_hover_text(tr("Stretches the hills and valleys; 1 = true to scale."));
    pct_row(ui, tr("Hill shading"), &mut r.shading, s.0 * 100.0, s.1 * 100.0, 1.0, Some(tr("Light and shadow on the slopes over the satellite image.")));
    *r = r.sane();
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

fn other_roads_label(o: OtherRoads) -> &'static str {
    tr(match o {
        OtherRoads::Normal => "Normal",
        OtherRoads::Muted => "Muted",
        OtherRoads::Hidden => "Hidden",
    })
}

fn race_lines_card(ui: &mut Ui, c: &mut RaceCfg, enabled: bool, cp: &CopyCtx) {
    theme::card(ui, tr("Race lines"), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
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
            race_focus_rows(ui, c);
        });
        });
        card_copy_row(ui, cp, LayerCategory::RaceLines);
    });
}

/// The in-race focus (D66): what the rest of the map does while a race line is detected. Only
/// the "Current race" mode detects one, so the rows are greyed in the other modes.
fn race_focus_rows(ui: &mut Ui, c: &mut RaceCfg) {
    let tip = tr("Applies only in the Current race mode, while the car is in a race and a race line was detected. Otherwise the map stays as it is.");
    ui.add_enabled_ui(c.mode == RaceLineMode::Current, |ui| {
        ui.add_space(2.0);
        ui.label(theme::section_label(tr("In a race"))).on_hover_text(tip);
        let f = &mut c.focus;
        control_row_tip(ui, tr("Other roads"), tip, |ui| {
            egui::ComboBox::from_id_salt("map_race_other_roads")
                .selected_text(other_roads_label(f.other_roads))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for o in [OtherRoads::Normal, OtherRoads::Muted, OtherRoads::Hidden] {
                        ui.selectable_value(&mut f.other_roads, o, other_roads_label(o));
                    }
                });
        });
        ui.add_enabled_ui(f.other_roads == OtherRoads::Muted, |ui| {
            control_row(ui, tr("Muted colour"), |ui| {
                egui::color_picker::color_edit_button_srgb(ui, &mut f.mute_color.0);
            });
            pct_row(ui, tr("Muted opacity"), &mut f.mute_alpha, 0.0, 100.0, 1.0, None);
            theme::slider_row(ui, tr("Muted width"), &mut f.mute_width, 0.2..=1.5, 0.05, 2, "×")
                .on_hover_text(tr("Width of the muted roads relative to their normal width."));
        });
        theme::checkbox_row(ui, &mut f.hide_pois, tr("Hide points of interest in a race")).on_hover_text(tip);
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

/// Upper end of the road width sliders (minimum, maximum, fixed width), px. *Why 100 (D74):* the
/// zoom rule clamps the width to `max_px`, and the old 30 px limit stopped roads growing when
/// zoomed far in; the user wants to tune it themselves ("raise the limit to, like, 50"), so the
/// range goes past that. Nothing else in the renderer caps the width (`style::road_base_px`
/// is the only clamp).
pub const ROAD_PX_MAX: f32 = 100.0;

fn roads_card(ui: &mut Ui, c: &mut RoadsCfg, enabled: bool, cp: &CopyCtx) {
    theme::card(ui, tr("Roads"), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
        theme::checkbox_row(ui, &mut c.on, tr("Show roads"));
        ui.add_enabled_ui(c.on, |ui| {
            theme::checkbox_row(ui, &mut c.scale_with_zoom, tr("Scale width with zoom")).on_hover_text(tr(
                "On: the line width follows the zoom, kept between the minimum and maximum. Off: one fixed width.",
            ));
            if c.scale_with_zoom {
                theme::slider_row(ui, tr("Road width"), &mut c.metres, 1.0..=40.0, 0.5, 1, " m")
                    .on_hover_text(tr("How wide a road is drawn in metres at the map's scale, before the minimum and maximum."));
                theme::slider_row(ui, tr("Minimum width"), &mut c.min_px, 0.5..=ROAD_PX_MAX, 0.1, 1, " px")
                    .on_hover_text(tr("The thinnest a road gets when zoomed far out."));
                theme::slider_row(ui, tr("Maximum width"), &mut c.max_px, 1.0..=ROAD_PX_MAX, 0.5, 1, " px")
                    .on_hover_text(tr("The widest a road gets when zoomed far in. Raise it to let roads keep growing with the zoom."));
            } else {
                theme::slider_row(ui, tr("Road width"), &mut c.base_px, 0.5..=ROAD_PX_MAX, 0.5, 1, " px");
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
        card_copy_row(ui, cp, LayerCategory::Roads);
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

fn pois_card(ui: &mut Ui, c: &mut PoisCfg, icons: Option<&IconAtlas>, enabled: bool, cp: &CopyCtx) {
    theme::card(ui, tr("Points of interest"), |ui| {
        ui.add_enabled_ui(enabled, |ui| {
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
        card_copy_row(ui, cp, LayerCategory::Pois);
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

    // ── "Copy to …" ──────────────────────────────────────────────────────────────────────────

    use super::super::cfg::Rgb;

    /// A config where each map has its own look in every category.
    fn two_maps() -> AppConfig {
        let mut c = AppConfig::default();
        c.overlay.map_use_dashboard = false;
        c.overlay.map_layers = MapLayerConfig::hud();
        c.overlay.map_layers.roads.styles.road.color = Rgb::hex(0x111111);
        c.overlay.map_layers.pois.size_px = 11.0;
        c.overlay.map_layers.image.opacity = 0.11;
        c.overlay.map_layers.tilt.angle_deg = 11.0;
        c.overlay.map_layers.race_lines.focus.mute_alpha = 0.11;
        c.minimap_layers.roads.styles.road.color = Rgb::hex(0x222222);
        c.minimap_layers.pois.size_px = 22.0;
        c.minimap_layers.image.opacity = 0.22;
        c.minimap_layers.tilt.angle_deg = 22.0;
        c.minimap_layers.race_lines.focus.mute_alpha = 0.22;
        c
    }

    fn layers_mut_of(c: &mut AppConfig, m: MapId) -> &mut MapLayerConfig {
        match m {
            MapId::Minimap => &mut c.overlay.map_layers,
            MapId::Dashboard => &mut c.minimap_layers,
        }
    }

    fn req(from: MapId, what: CopyWhat, to: &[MapId]) -> CopyRequest {
        CopyRequest { from, what, to: to.to_vec() }
    }

    /// Every category, from every map to every other map: the target gets that category only,
    /// the third map and the source are untouched.
    #[test]
    fn copy_changes_one_category_of_the_target_only() {
        for from in MapId::ALL {
            for to in MapId::ALL.into_iter().filter(|m| *m != from) {
                for cat in LayerCategory::ALL {
                    let before = two_maps();
                    let mut c = before.clone();
                    assert_eq!(apply_copy(&mut c, &req(from, CopyWhat::Layer(cat), &[to])), vec![to]);
                    for m in MapId::ALL {
                        let (mut got, mut want) = (c.clone(), before.clone());
                        let (got, want) = (layers_mut_of(&mut got, m).clone(), layers_mut_of(&mut want, m).clone());
                        if m == to {
                            let mut expect = want.clone();
                            expect.copy_category(layers_mut_of(&mut before.clone(), from), cat);
                            assert_eq!(got, expect, "{from:?} -> {to:?} {cat:?}");
                            assert_ne!(got, want, "{from:?} -> {to:?} {cat:?} changed nothing");
                        } else {
                            assert_eq!(got, want, "{from:?} -> {to:?} {cat:?} touched {m:?}");
                        }
                    }
                }
            }
        }
    }

    /// "Both" with two maps is just the other one; a request that names the source is ignored for it.
    #[test]
    fn copy_to_both_writes_the_other_map_and_skips_the_source() {
        let mut c = two_maps();
        let both = MapId::ALL;
        let done = apply_copy(&mut c, &req(MapId::Minimap, CopyWhat::Layer(LayerCategory::Roads), &both));
        assert_eq!(done, vec![MapId::Dashboard]);
        assert_eq!(c.minimap_layers.roads, c.overlay.map_layers.roads);
        assert_eq!(c.minimap_layers.pois.size_px, 22.0, "other categories stay");
        let mut c = two_maps();
        assert!(apply_copy(&mut c, &req(MapId::Dashboard, CopyWhat::Layer(LayerCategory::Pois), &[MapId::Dashboard])).is_empty());
        assert_eq!(c.minimap_layers, two_maps().minimap_layers);
    }

    /// The race focus travels with the Race lines card.
    #[test]
    fn race_lines_copy_includes_the_focus() {
        let mut c = two_maps();
        apply_copy(&mut c, &req(MapId::Minimap, CopyWhat::Layer(LayerCategory::RaceLines), &[MapId::Dashboard]));
        assert_eq!(c.minimap_layers.race_lines.focus.mute_alpha, 0.11);
    }

    /// While the Minimap follows the Dashboard map, copying into it is refused (its own config is
    /// not written, and nothing is reported as done); the Dashboard map is no target of itself.
    #[test]
    fn following_minimap_is_no_target() {
        let mut c = two_maps();
        c.overlay.map_use_dashboard = true;
        let own = c.overlay.map_layers.clone();
        let done = apply_copy(&mut c, &req(MapId::Dashboard, CopyWhat::Layer(LayerCategory::Roads), &MapId::ALL));
        assert!(done.is_empty());
        assert_eq!(c.overlay.map_layers, own);
        assert!(apply_copy(&mut c, &req(MapId::Dashboard, CopyWhat::View, &[MapId::Minimap])).is_empty());
    }

    /// View options: all of them, in both directions, each map's own keys.
    #[test]
    fn view_copy_maps_each_maps_own_keys() {
        let mut c = two_maps();
        c.overlay.map_north_up = true;
        c.overlay.zoom_driving_m = 777.0;
        c.overlay.compass = true;
        c.minimap_north_up = false;
        c.minimap_zoom_driving_m = 1234.0;
        c.minimap_show_compass = false;
        apply_copy(&mut c, &req(MapId::Minimap, CopyWhat::View, &[MapId::Dashboard]));
        assert!(c.minimap_north_up && c.minimap_show_compass);
        assert_eq!(c.minimap_zoom_driving_m, 777.0);
        assert_eq!(ViewCfg::of_app(&c), ViewCfg::of_overlay(&c.overlay));

        c.minimap_mirror_edges = !c.overlay.map_mirror_edges;
        c.minimap_zoom_stopped_m = 321.0;
        apply_copy(&mut c, &req(MapId::Dashboard, CopyWhat::View, &[MapId::Minimap]));
        assert_eq!(c.overlay.map_mirror_edges, c.minimap_mirror_edges);
        assert_eq!(c.overlay.zoom_stopped_m, 321.0);
        assert_eq!(ViewCfg::of_app(&c), ViewCfg::of_overlay(&c.overlay));
    }

    // ── the "View mode" card ─────────────────────────────────────────────────────────────────

    /// One frame of the View mode card alone, with `events` fed in. Returns the frame's output.
    fn card_frame(ctx: &egui::Context, t: &mut TiltCfg, events: Vec<egui::Event>, time: f64) -> egui::FullOutput {
        let cp = CopyCtx { which: MapId::Dashboard, minimap_follows: false, out: Default::default() };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 900.0))),
            events,
            time: Some(time),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| tilt_card(ui, t, true, &cp, None));
        })
    }

    fn texts(out: &egui::FullOutput) -> Vec<(String, Rect)> {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.visual_bounding_rect())),
                _ => None,
            })
            .collect()
    }

    fn has_text(out: &egui::FullOutput, s: &str) -> bool {
        texts(out).iter().any(|(t, _)| t == s)
    }

    /// Frames until the layout has settled; returns the last output.
    fn settle(ctx: &egui::Context, t: &mut TiltCfg, time: &mut f64) -> egui::FullOutput {
        let mut out = card_frame(ctx, t, vec![], *time);
        for _ in 0..2 {
            *time += 0.1;
            out = card_frame(ctx, t, vec![], *time);
        }
        out
    }

    /// Click the centre of the (first) text `label` in the card.
    fn click_label(ctx: &egui::Context, t: &mut TiltCfg, time: &mut f64, label: &str) {
        let out = settle(ctx, t, time);
        let pos = texts(&out).iter().find(|(s, _)| s == label).unwrap_or_else(|| panic!("no {label:?} in the card")).1.center();
        let btn = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        for evs in [vec![egui::Event::PointerMoved(pos)], vec![btn(true)], vec![btn(false)]] {
            *time += 0.1;
            card_frame(ctx, t, evs, *time);
        }
        *time += 0.1;
        card_frame(ctx, t, vec![], *time);
    }

    fn odd_tilt() -> TiltCfg {
        TiltCfg {
            on: false,
            angle_deg: 47.0,
            perspective_px: 310.0,
            car_y: 0.7,
            taper: false,
            relief: ReliefCfg { on: false, road_height: RoadHeight::Terrain, deck_m: 6.5, exaggeration: 2.0, shading: 0.8 },
        }
    }

    /// The control writes `on` / `relief.on` and nothing else, so going back and forth keeps the
    /// user's angle, perspective and 3D options.
    #[test]
    fn view_mode_control_writes_the_mode_fields_only() {
        crate::i18n::with_language(crate::i18n::Language::English, || {
            let ctx = crate::ui::test_render::context();
            let mut time = 0.0;
            let mut t = odd_tilt();
            assert_eq!(t.view_mode(), ViewMode::Flat);
            let look = TiltCfg { on: false, relief: ReliefCfg { on: false, ..t.relief }, ..t };
            for (label, mode, on, relief_on) in [
                ("Tilted", ViewMode::Tilted, true, false),
                ("3D", ViewMode::Relief, true, true),
                ("Flat", ViewMode::Flat, false, false),
                ("3D", ViewMode::Relief, true, true),
                ("Tilted", ViewMode::Tilted, true, false),
            ] {
                click_label(&ctx, &mut t, &mut time, label);
                assert_eq!(t.view_mode(), mode, "after clicking {label}");
                assert_eq!((t.on, t.relief.on), (on, relief_on), "after clicking {label}");
                assert_eq!((t.angle_deg, t.perspective_px, t.car_y, t.taper), (look.angle_deg, look.perspective_px, look.car_y, look.taper));
                assert_eq!(ReliefCfg { on: false, ..t.relief }, look.relief, "the 3D options survive a switch");
            }
        });
    }

    /// Road height, deck thickness, exaggeration and hill shading exist in 3D only; the tilt
    /// rows are there for Tilted and 3D (greyed in Flat).
    #[test]
    fn relief_rows_show_in_3d_only() {
        use crate::i18n::{with_language, Language};
        let ctx = crate::ui::test_render::context();
        for (lang, relief_labels) in [
            (Language::English, ["Road height", "Deck thickness", "Height exaggeration", "Hill shading"]),
            (Language::German, ["Straßenhöhe", "Fahrbahndicke", "Höhenüberhöhung", "Hangschattierung"]),
        ] {
            with_language(lang, || {
                let tilt_labels = [tr("Angle"), tr("Perspective"), tr("Car position"), tr("Thinner lines in the distance")];
                let mut time = 0.0;
                for mode in [ViewMode::Flat, ViewMode::Tilted, ViewMode::Relief] {
                    let mut t = odd_tilt();
                    t.set_view_mode(mode);
                    let out = settle(&ctx, &mut t, &mut time);
                    for l in tilt_labels {
                        assert!(has_text(&out, l), "{lang:?} {mode:?}: {l:?} missing");
                    }
                    for l in relief_labels {
                        assert_eq!(has_text(&out, l), mode == ViewMode::Relief, "{lang:?} {mode:?}: {l:?}");
                    }
                    assert_eq!(has_text(&out, tr("3D")), true, "the segment is always there");
                    assert_eq!(t.view_mode(), mode, "showing the card changes nothing");
                }
            });
        }
    }

    /// A hand-edited config with values outside the ranges is clamped when the 3D rows show
    /// (NaN falls back to the default); in the other modes the card leaves it alone.
    #[test]
    fn relief_values_are_clamped_to_the_cfg_ranges() {
        let ctx = crate::ui::test_render::context();
        let mut time = 0.0;
        let wild = ReliefCfg { on: true, road_height: RoadHeight::Terrain, deck_m: -4.0, exaggeration: 99.0, shading: f32::NAN };
        let mut t = TiltCfg { on: true, relief: wild, ..TiltCfg::default() };
        settle(&ctx, &mut t, &mut time);
        let r = t.relief;
        assert_eq!(r, wild.sane());
        assert_eq!((r.deck_m, r.exaggeration, r.shading), (ReliefCfg::DECK_RANGE.0, ReliefCfg::EXAG_RANGE.1, ReliefCfg::default().shading));
        assert_eq!(r.road_height, RoadHeight::Terrain);

        let mut t = TiltCfg { on: true, relief: ReliefCfg { on: false, ..wild }, ..TiltCfg::default() };
        settle(&ctx, &mut t, &mut time);
        assert_eq!(t.relief.exaggeration, 99.0, "Tilted does not touch the 3D options");
    }

    /// The tooltip of the 3D segment: only over that segment, not over Flat.
    #[test]
    fn the_3d_segment_has_the_graphics_card_tooltip() {
        crate::i18n::with_language(crate::i18n::Language::English, || {
            let ctx = crate::ui::test_render::context();
            ctx.style_mut(|s| s.interaction.tooltip_delay = 0.0);
            let mut time = 0.0;
            let mut t = TiltCfg::default();
            let out = settle(&ctx, &mut t, &mut time);
            let find = |l: &str| texts(&out).iter().find(|(s, _)| s == l).unwrap().1.center();
            let tip = tr("Uses your graphics card; falls back to Tilted if it isn't supported.");
            for (label, shown) in [("Flat", false), ("3D", true)] {
                let pos = find(label);
                let mut last = None;
                for i in 0..6 {
                    time += 0.2;
                    let evs = if i == 0 { vec![egui::Event::PointerMoved(pos)] } else { vec![] };
                    last = Some(card_frame(&ctx, &mut t, evs, time));
                }
                assert_eq!(has_text(&last.unwrap(), tip), shown, "tooltip over {label}");
                time += 0.2;
                card_frame(&ctx, &mut t, vec![egui::Event::PointerGone], time);
            }
        });
    }

    /// "Copy to …" on the View mode card is `LayerCategory::Tilt`: it takes the 3D options and
    /// the mode with it, leaves every other category of the target alone.
    #[test]
    fn copying_view_mode_copies_the_relief_fields() {
        let mut c = two_maps();
        let relief = ReliefCfg { on: true, road_height: RoadHeight::Terrain, deck_m: 9.0, exaggeration: 2.5, shading: 0.9 };
        c.overlay.map_layers.tilt.on = true;
        c.overlay.map_layers.tilt.relief = relief;
        assert_eq!(c.minimap_layers.tilt.relief, ReliefCfg::default());
        let before = c.clone();
        let done = apply_copy(&mut c, &req(MapId::Minimap, CopyWhat::Layer(LayerCategory::Tilt), &[MapId::Dashboard]));
        assert_eq!(done, vec![MapId::Dashboard]);
        let tilt = &c.minimap_layers.tilt;
        assert_eq!(tilt.relief, relief);
        assert_eq!(tilt.view_mode(), ViewMode::Relief);
        assert_eq!(tilt.angle_deg, 11.0, "the tilt rows travel with it");
        assert_eq!(c.minimap_layers.roads, before.minimap_layers.roads);
        assert_eq!(c.minimap_layers.image, before.minimap_layers.image);
        assert_eq!(c.overlay.map_layers, before.overlay.map_layers);
        // The other categories do not carry the 3D options.
        let mut d = before.clone();
        apply_copy(&mut d, &req(MapId::Minimap, CopyWhat::Layer(LayerCategory::Roads), &[MapId::Dashboard]));
        assert_eq!(d.minimap_layers.tilt, before.minimap_layers.tilt);
    }

    // ── the 3D status line (K4) ──────────────────────────────────────────────────────────────

    /// The state -> line mapping: a GL failure outranks the terrain's state, a loaded terrain
    /// and no failure say nothing.
    #[test]
    fn the_3d_status_follows_the_gl_failure_then_the_terrain() {
        use crate::maprender::store::TerrainStatus as T;
        use crate::maprender::terrain::Terrain;
        let ready = T::Ready(std::sync::Arc::new(Terrain::flat(1.0)));
        assert_eq!(status_3d(None, &ready), None);
        assert_eq!(status_3d(None, &T::Loading), Some(Status3d::Loading));
        assert_eq!(status_3d(Some("OpenGL 3.1 is too old"), &T::Loading), Some(Status3d::Unavailable("OpenGL 3.1 is too old".into())));
        assert_eq!(status_3d(Some("3D is too slow on this GPU"), &ready), Some(Status3d::Unavailable("3D is too slow on this GPU".into())));
        assert_eq!(status_3d(None, &T::Error("raster unreadable".into())), Some(Status3d::Unavailable("raster unreadable".into())));
        let Some(Status3d::Unavailable(no_install)) = status_3d(None, &T::NoInstall) else { panic!("NoInstall must be reported") };
        assert!(no_install.contains("Forza Horizon 6"), "{no_install}");
    }

    /// The View mode card shows the line in 3D only, in both languages, and nothing when fine.
    #[test]
    fn the_card_shows_the_3d_status_in_3d_mode_only() {
        use crate::i18n::{with_language, Language};
        let ctx = crate::ui::test_render::context();
        let set = |s: Option<Status3d>| TEST_STATUS_3D.with(|t| *t.borrow_mut() = s);
        for (lang, loading, unavailable) in [
            (Language::English, "Loading terrain…", "3D not available: OpenGL 3.1 is too old"),
            (Language::German, "Gelände wird geladen…", "3D nicht verfügbar: OpenGL 3.1 is too old"),
        ] {
            with_language(lang, || {
                let mut time = 0.0;
                for (mode, state, shown) in [
                    (ViewMode::Relief, Some(Status3d::Loading), Some(loading)),
                    (ViewMode::Relief, Some(Status3d::Unavailable("OpenGL 3.1 is too old".into())), Some(unavailable)),
                    (ViewMode::Relief, None, None),
                    (ViewMode::Tilted, Some(Status3d::Loading), None),
                    (ViewMode::Flat, Some(Status3d::Unavailable("x".into())), None),
                ] {
                    set(state);
                    let mut t = odd_tilt();
                    t.set_view_mode(mode);
                    let out = settle(&ctx, &mut t, &mut time);
                    let all: Vec<String> = texts(&out).into_iter().map(|(s, _)| s).collect();
                    for l in [loading, unavailable] {
                        assert_eq!(all.iter().any(|s| s == l), shown == Some(l), "{lang:?} {mode:?}: {l:?} in {all:?}");
                    }
                }
            });
        }
        set(None);
    }
}
