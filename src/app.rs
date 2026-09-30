use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Context, Pos2, Vec2};

use crate::config::{AppConfig, SpeedDeltaMode};
use crate::engines::{load_engines, EngineRecord};
use crate::focus::{FocusDetector, FocusParams, MonitorParams};
use crate::overlay::DisabledReason;
use crate::hotkeys::HotkeyListener;
use crate::input::InputSender;
use crate::listeners::backfire::BackfireView;
use crate::listeners::dsg::DsgView;
use crate::listeners::perf_test::PerfTest;
use crate::listeners::power_capture::{PowerCapture, PowerCurveSnapshot};
use crate::listeners::sprint_timer::SprintTimer;
use crate::listeners::worker::{Command, ListenerHandle};
use crate::network::{start_receiver, NetworkHandle};
use crate::minimap::{
    current_season, decode_and_cache_season, load_map_color_image, map_cache_path,
    season_display_name, Season,
};
use crate::packet::ForzaPacket;
use crate::telemetry::TelemetryState;

/// Whether a global hotkey should fire. Fires when our app is focused (unless a
/// text field is capturing keys), or when our app is not focused but the game is.
/// A third app focused → ignore. See the global-hotkeys spec §7.
pub(crate) fn global_hotkey_allowed(our_focused: bool, wants_text: bool, game_focused: bool) -> bool {
    if our_focused { !wants_text } else { game_focused }
}

/// The global-scope bindings the capture backend should match against.
pub(crate) fn global_bindings(
    cfg: &crate::config::AppConfig,
) -> Vec<(crate::keymap::HotkeyBinding, crate::config::HotkeyAction)> {
    use crate::config::HotkeyScope;
    cfg.hotkeys
        .bindings
        .iter()
        .filter(|(a, _)| a.scope() == HotkeyScope::Global)
        .map(|(a, b)| (*b, *a))
        .collect()
}

// ── Minimap map loading ───────────────────────────────────────────

/// Message type sent from the map-loading background thread to the main thread.
pub enum MapLoadMessage {
    /// Sent immediately with the display names of every season that needs caching.
    CacheBuildStarted { names: Vec<String> },
    /// One season's cache was just written; carry its name so the UI can remove it.
    CacheBuilt { name: String },
    /// All necessary caches are built and the current season's image is ready.
    Done(Option<(egui::ColorImage, [u32; 2])>),
}

/// Background thread entry point. Builds any missing caches in parallel across all 4 seasons,
/// sending a `CacheBuilt` message for each completion, then loads the current season's image
/// from cache and sends `Done`.
fn map_load_thread(current_season: Season, quality: f32, tx: mpsc::Sender<MapLoadMessage>) {
    let all_seasons = crate::minimap::ALL_SEASONS;
    let quality_pct = quality.round() as u32;

    let to_build: Vec<Season> = all_seasons
        .iter()
        .copied()
        .filter(|&s| !map_cache_path(s, quality_pct).exists())
        .collect();

    if !to_build.is_empty() {
        // Send immediately — file-exist checks are done, decodes haven't started yet.
        // This lets the widget switch to the progress screen before any slow work begins.
        let _ = tx.send(MapLoadMessage::CacheBuildStarted {
            names: to_build
                .iter()
                .map(|&s| season_display_name(s).to_string())
                .collect(),
        });

        let handles: Vec<_> = to_build
            .iter()
            .copied()
            .map(|season| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    decode_and_cache_season(season, quality);
                    let _ = tx.send(MapLoadMessage::CacheBuilt {
                        name: season_display_name(season).to_string(),
                    });
                })
            })
            .collect();

        for h in handles {
            let _ = h.join();
        }
    }

    let result = load_map_color_image(current_season, quality);
    let _ = tx.send(MapLoadMessage::Done(result));
}

// ── Session stats ──────────────────────────────────────────────────

pub struct SuspensionStats {
    history: VecDeque<(Instant, [f32; 4])>,
    pub min: [f32; 4],
    pub max: [f32; 4],
    pub initialized: bool,
}

impl Default for SuspensionStats {
    fn default() -> Self {
        Self {
            history: VecDeque::new(),
            min: [1.0; 4],
            max: [0.0; 4],
            initialized: false,
        }
    }
}

impl SuspensionStats {
    pub fn update(&mut self, vals: [f32; 4]) {
        let now = Instant::now();
        self.history.push_back((now, vals));
        while let Some(&(t, _)) = self.history.front() {
            if now.duration_since(t) > Duration::from_secs(5) {
                self.history.pop_front();
            } else {
                break;
            }
        }
        self.min = [1.0; 4];
        self.max = [0.0; 4];
        for &(_, v) in &self.history {
            for i in 0..4 {
                self.min[i] = self.min[i].min(v[i]);
                self.max[i] = self.max[i].max(v[i]);
            }
        }
        self.initialized = !self.history.is_empty();
    }
}

#[derive(Default)]
pub struct GForceStats {
    pub max_lateral: f32,
    pub max_longitudinal: f32,
    pub max_vertical: f32,
    pub peak_lateral: f32,
    pub peak_longitudinal: f32,
    pub peak_reset_timer: Option<Instant>,
    /// Recent (lat, lon) samples for the traction-circle trail (~1.5 s window).
    pub g_history: VecDeque<(Instant, f32, f32)>,
}

impl GForceStats {
    pub fn update(&mut self, lat: f32, lon: f32, vert: f32) {
        let now = Instant::now();
        self.g_history.push_back((now, lat, lon));
        while let Some(&(t, _, _)) = self.g_history.front() {
            if now.duration_since(t) > Duration::from_millis(1500) {
                self.g_history.pop_front();
            } else {
                break;
            }
        }

        let cur_mag = (lat * lat + lon * lon).sqrt();
        let peak_mag = (self.peak_lateral.powi(2) + self.peak_longitudinal.powi(2)).sqrt();

        if cur_mag > peak_mag {
            self.peak_lateral = lat;
            self.peak_longitudinal = lon;
            self.peak_reset_timer = None;
        } else if peak_mag > 0.01 {
            if self.peak_reset_timer.is_none() {
                self.peak_reset_timer = Some(Instant::now());
            }
            if let Some(t) = self.peak_reset_timer {
                if t.elapsed() >= Duration::from_secs(5) {
                    *self = GForceStats::default();
                    return;
                }
            }
        }

        if lat.abs() >= 0.5 {
            self.max_lateral = self.max_lateral.max(lat.abs());
        }
        if lon.abs() >= 0.1 {
            self.max_longitudinal = self.max_longitudinal.max(lon.abs());
        }
        if vert.abs() >= 0.2 {
            self.max_vertical = self.max_vertical.max(vert.abs());
        }
    }
}

// ── Dashboard drag / resize state ─────────────────────────────────

pub struct DashboardDragState {
    pub widget_idx: usize,
    pub pointer_offset: Vec2,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
}

pub struct DashboardResizeState {
    pub widget_idx: usize,
    pub edge: ResizeEdge,
    pub origin_col: usize,
    pub origin_row: usize,
    pub origin_span: (usize, usize),
    pub origin_ptr: Pos2,
}

// ── Tabs ───────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
pub enum Tab {
    Dashboard,
    Overlay,
    Backfire,
    Gearbox,
    PowerCurve,
    EngineSwaps,
    Coop,
    Settings,
    Changelog,
    Debug,
}

/// Which page the mini-settings popup shows. `General` is a global page not tied
/// to any app tab; everything else mirrors a real `Tab`.
#[derive(PartialEq, Clone, Copy)]
pub enum PageSettingsTab {
    General,
    Tab(Tab),
}

/// A tab-bar button, 30px tall, fill-only (no outline). `label = None` renders an
/// icon-only compact tab: 30px wide with the glyph ink-centred via the cache (plain
/// `Align2::CENTER_CENTER` centres the layout box, not the ink, so icons look
/// ragged). `label = Some` renders icon + text, width-fitted. Reuses egui's
/// `interact_selectable` visuals so selected/hover colours track the theme.
fn tab_button(
    ui: &mut egui::Ui,
    current: &mut Tab,
    cache: &mut crate::iconcache::IconCenterCache,
    tab: Tab,
    icon: &str,
    label: Option<&str>,
    high_contrast: bool,
) {
    let selected = *current == tab;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let full = label.map(|text| format!("{icon}  {text}"));
    let width = match &full {
        None => 30.0,
        Some(s) => {
            ui.painter()
                .layout_no_wrap(s.clone(), font.clone(), egui::Color32::WHITE)
                .size()
                .x
                + 14.0 // 7px padding each side
        }
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let vis = ui.style().interact_selectable(&resp, selected);
    let hc = high_contrast && full.is_none();
    let normal_icon = vis.text_color(); // colour the icon uses without high contrast
    if selected || resp.hovered() {
        // In high contrast the active tab takes the normal icon colour as its
        // background, so the white icon reads against a solid block.
        let bg = if hc && selected { normal_icon } else { vis.bg_fill };
        ui.painter().rect_filled(rect, 4.0, bg); // fill only — no outline
    }
    // Icon-only (compact) buttons can be forced white for high contrast.
    let color = if hc { egui::Color32::WHITE } else { normal_icon };
    match full {
        None => {
            let pos = cache.centered_pos(ui, icon, font.clone(), rect.center());
            ui.painter().text(pos, egui::Align2::LEFT_TOP, icon, font, color);
        }
        Some(s) => {
            ui.painter().text(
                egui::pos2(rect.left() + 7.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                s,
                font,
                color,
            );
        }
    }
    if resp.clicked() {
        *current = tab;
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand);
}

/// English title for a tab, used by the Modern top bar's current-page pill.
fn tab_title(tab: Tab) -> &'static str {
    match tab {
        Tab::Dashboard => "Dashboard",
        Tab::Overlay => "Overlay",
        Tab::PowerCurve => "Power Curve",
        Tab::Coop => "Co-Op",
        Tab::Backfire => "Backfire",
        Tab::Gearbox => "Automatic Gearbox",
        Tab::EngineSwaps => "Engine Swaps",
        Tab::Settings => "Setup",
        Tab::Changelog => "What's New",
        Tab::Debug => "Debug",
    }
}

/// The pill font — used by both the chip and the reserved-width measurement.
const PILL_FONT: f32 = 12.5;

/// Width the current-page pill reserves: the widest tab title's chip width. Reserving
/// the max (rather than the current tab's) keeps the icon tabs after the pill from
/// jumping sideways when you switch to a longer/shorter tab name — the slot is fixed,
/// so the tabs only shift once, uniformly, when the bar itself gets narrow.
fn max_pill_width(ui: &egui::Ui) -> f32 {
    const TABS: [Tab; 10] = [
        Tab::Dashboard, Tab::Overlay, Tab::Backfire, Tab::Gearbox, Tab::PowerCurve,
        Tab::EngineSwaps, Tab::Coop, Tab::Settings, Tab::Changelog, Tab::Debug,
    ];
    TABS.iter()
        .map(|&t| {
            ui.painter()
                .layout_no_wrap(
                    crate::i18n::tr(tab_title(t)).to_owned(),
                    egui::FontId::proportional(PILL_FONT),
                    crate::theme::TEXT,
                )
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        + 24.0
}

/// A rounded "pill" chip showing the current page (Modern top bar), styled like
/// the Graphite selection chip: full-radius, selection fill + border. The chip is
/// drawn at its natural width but reserves `reserve_w` (see `max_pill_width`), left-
/// aligned, so following widgets keep a stable position as the current tab changes.
fn page_pill(ui: &mut egui::Ui, text: &str, reserve_w: f32) {
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(PILL_FONT),
        crate::theme::TEXT,
    );
    let chip = galley.size() + egui::vec2(24.0, 8.0);
    let (slot, _) =
        ui.allocate_exact_size(egui::vec2(reserve_w.max(chip.x), chip.y), egui::Sense::hover());
    let rect = egui::Rect::from_min_size(slot.min, chip);
    ui.painter().rect(
        rect,
        egui::CornerRadius::same((rect.height() * 0.5) as u8),
        crate::theme::SEL,
        egui::Stroke::new(1.0, crate::theme::SELBD),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, crate::theme::TEXT);
}

#[derive(PartialEq, Clone, Copy, Default)]
pub enum DashboardSubTab {
    #[default]
    General,
    Modules,
    Kmh,
    Gear,
    Rpm,
    SprintTimes,
    Tires,
    Suspension,
    Shift,
    Engine,
    GForce,
    Inputs,
    Boost,
    Graphs,
    MiniMap,
}

/// Modal dialog state for the Profile Manager (Settings → PROFILES).
#[derive(PartialEq, Clone, Copy, Default)]
pub enum ProfileDialog {
    #[default]
    None,
    New,
    Duplicate,
    Rename,
    ConfirmDelete,
    Export, // large two-pane export dialog
    Import, // large two-pane import dialog
}

/// Nested sub-tabs inside the mini-settings "Map" tab.
#[derive(PartialEq, Clone, Copy, Default)]
pub enum MiniMapTab {
    #[default]
    General,
    Coop,
}

/// Last-known non-paused position of a co-op player, so a paused player can still
/// be drawn at their last spot. (Class/PI travel in the packet — see coop::outgoing.)
#[derive(Clone, Copy)]
pub struct CoopSeen {
    pub x: f32,
    pub z: f32,
    pub yaw: f32,
    /// Last non-empty car class/PI (a paused game transmits zeros — keep the
    /// previous real values so the player list doesn't fall back to "D 0").
    pub car_class: i32,
    pub pi: i32,
}

// ── App ────────────────────────────────────────────────────────────

pub struct ForzaApp {
    pub config: AppConfig,
    pub engines: Vec<EngineRecord>,
    pub labels: crate::labels::Labels,
    pub telemetry: TelemetryState,
    pub current_tab: Tab,

    pub sprint_timer: SprintTimer,
    /// UI-local copies of the listener thread's Backfire / gearbox state, refreshed from its
    /// mailbox once a frame (`listeners/worker.rs`). Read-only here — the live listeners run
    /// on that thread so they keep working while the window is hidden.
    pub backfire: BackfireView,
    pub dsg: DsgView,
    input: InputSender,
    pub hotkeys: HotkeyListener,
    pub focus: Arc<FocusDetector>,
    /// Hotkey rebind state (Setup → Hotkey, Overlay → Hide HUD): the action capturing a new
    /// key. The key itself is taken by [`ForzaApp::capture_rebind`].
    pub rebinding: Option<crate::config::HotkeyAction>,
    /// The armed rebind button's id and rect, refreshed by [`ForzaApp::track_rebind_button`]
    /// each frame it's drawn: a press anywhere else cancels the capture (the tab mockup).
    rebind_button: Option<(egui::Id, egui::Rect)>,
    /// The tab shown last frame; a tab switch drops the rebind capture and the Overlay
    /// layout selection so neither acts on keys pressed elsewhere.
    last_tab: Tab,
    /// Hide HUD hotkey state, copied from `ListenerView::hud_hidden` (Overlay tab hint).
    pub hud_hidden: bool,
    /// Detect-button countdown deadline (active-window auto-fill).
    pub detect_until: Option<std::time::Instant>,
    /// Last Custom-preview / Detect result for the settings page.
    pub focus_preview: String,
    pub power_capture: PowerCapture,
    pub saved_power_curve: Option<PowerCurveSnapshot>,
    pub perf_test: PerfTest,

    // Engine swaps search filter
    pub engine_search: String,

    // Settings: pending port change
    pub pending_port: u16,

    pub last_car_ordinal: i32,
    pub last_packet_time: Option<Instant>,

    // Session maxima (reset on car change)
    pub max_power_ps: f32,
    pub max_torque_nm: f32,
    pub max_boost_psi: f32,
    pub max_speed_kmh: f32,
    pub cached_engine_max_rpm: f64,
    pub fi_detected: bool,
    /// Highest RPM seen while making power (>0 W) — the dynamically detected redline. Per-car.
    /// Measured on the listener thread (the gearbox needs it there); this is the UI's copy.
    pub dynamic_max_rpm: f32,
    /// Estimated tire radius per wheel [FL, FR, RL, RR], meters. The packet has no radius,
    /// so it's derived from speed / wheel rotation while gripping (EMA-smoothed). Per-car.
    pub wheel_radius_est: [f32; 4],

    // Reusable icon-centering cache for icon-in-a-box rendering (compact tabs, …).
    pub icon_center_cache: crate::iconcache::IconCenterCache,

    // Session stats
    pub suspension_stats: SuspensionStats,
    pub gforce_stats: GForceStats,

    // Cached car identity — persists when is_race_on == 0 (paused)
    pub cached_car_class_str: String,
    pub cached_car_class: i32,
    pub cached_car_pi: i32,
    pub cached_drivetrain_str: String,
    pub cached_drivetrain: i32,
    pub cached_num_cylinders: i32,

    // Speed delta tracking
    pub speed_delta_kmh: f32,
    last_tracked_speed: f32,
    last_track_instant: Option<Instant>,
    speed_history: VecDeque<(Instant, f32)>,

    // Power curve plot: request auto-fit on next frame (set by clear, save, or middle-click)
    pub power_plot_auto_bounds: bool,

    // Page-specific settings popup
    pub page_settings_open: bool,
    pub page_settings_opacity: f32,
    pub page_settings_tab: PageSettingsTab,
    pub page_dashboard_sub_tab: DashboardSubTab,
    pub page_map_sub_tab: MiniMapTab,
    // Profile Manager UI state (Settings → PROFILES). `*_sel` vecs align to
    // crate::config::KEY_GROUPS by index.
    pub profile_dialog: ProfileDialog,       // modal New / Duplicate / Rename / Delete / Export / Import
    pub profile_dialog_focus: bool,          // request focus on the dialog's text field next frame
    pub profile_name_buf: String,            // name field for New / Duplicate / Rename
    pub profile_io_status: String,
    pub profile_export_sel: Vec<bool>,
    pub profile_import_buf: String,
    pub profile_import_builtin: Option<usize>, // import source: Some(i) = bundled preset i, None = paste buffer
    pub profile_import_sel: Vec<bool>,
    pub profile_import_present: Vec<bool>,   // which groups the source JSON actually contains
    pub profile_import_new: bool,            // import target: true = new profile, false = overwrite existing
    pub profile_import_new_name: String,
    pub profile_import_overwrite: String,    // selected existing profile to overwrite

    // "What's New" changelog viewer: per-category filter toggles (transient UI state)
    pub changelog_show_added: bool,
    pub changelog_show_fixed: bool,
    pub changelog_show_removed: bool,
    pub changelog_show_info: bool,

    // Dashboard widget drag / resize state
    pub dashboard_drag: Option<DashboardDragState>,
    pub dashboard_resize: Option<DashboardResizeState>,

    // Mini map
    pub minimap_texture: Option<egui::TextureHandle>,
    pub minimap_orig_size: [u32; 2], // original image dims for world-space coverage maths
    pub minimap_current_zoom: f32,
    pub minimap_loaded_season: Season, // season currently in the texture
    minimap_stopped_at: Option<Instant>, // when speed dropped below threshold
    minimap_last_render_time: f64,     // egui time of last cached-position refresh
    pub minimap_cached_car_x: f32,     // throttled position cache for minimap rendering
    pub minimap_cached_car_z: f32,
    pub minimap_cached_yaw: f32,
    pub minimap_cached_raw_yaw: f32, // always raw pkt.yaw, for arrow orientation
    pub minimap_smoothed_yaw: f32,   // lerped yaw used for actual rendering
    minimap_img_receiver: Option<Receiver<MapLoadMessage>>,
    pub minimap_cache_progress: Option<Vec<String>>, // display names of seasons still being built
    /// Recent world-space path per player (key "local" or a co-op UUID), for map trails.
    /// Only maintained/drawn while in a co-op session.
    pub minimap_trails: HashMap<String, VecDeque<(f32, f32, Instant)>>,
    /// Last non-paused telemetry per player (key "local" or a co-op UUID), so a
    /// paused player still shows at their last spot with their real class/PI.
    pub coop_last_pos: HashMap<String, CoopSeen>,
    /// Rolling ~30 s trace of (t_active_secs, speed km/h, rpm) for the Speed
    /// Trace widget. The x-axis is accumulated *active* time — it only advances
    /// when a sample is accepted — so game pauses neither gap nor slide the plot.
    pub trace_history: VecDeque<(f32, f32, f32)>,
    /// Active-time seconds of the last accepted trace sample.
    trace_active_secs: f32,
    /// Wall-clock instant of the last accepted trace sample.
    trace_last_sample: Option<Instant>,

    // Preset loader selected index (None = nothing selected)

    // Co-Op
    pub coop: crate::coop::CoopState,
    pub coop_join_input: String,
    pub coop_copied_at: Option<Instant>,

    /// The UDP threads' end of the raw packet channel; kept so a port change can start a
    /// fresh UDP thread on the same channel, leaving the listener thread untouched.
    packet_tx: Sender<ForzaPacket>,
    listener: ListenerHandle,
    /// Last `ListenerView::toggle_gen` this UI has adopted — see `worker.rs`.
    last_toggle_gen: u64,
    _network: NetworkHandle,
    overlay: OverlayRuntime,
}

/// HUD overlay state for the Overlay tab (I9). Linux-only; elsewhere always `Off`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum OverlayStatus {
    /// `overlay.enabled` is off (or the `FORZA_OVERLAY_TEST` dev pattern owns the overlay).
    #[default]
    Off,
    /// Waiting for the overlay thread to connect and set up EGL (≤ 5 s).
    Starting,
    Running,
    /// Couldn't start; the reason is user-facing. Retried when the overlay is re-enabled.
    Disabled(DisabledReason),
    /// Was running, then its thread exited (connection lost, repeated EGL failures). Also
    /// retried by re-enabling.
    Stopped,
}

/// The app's end of the overlay (see [`ForzaApp::sync_overlay`]).
#[derive(Default)]
struct OverlayRuntime {
    status: OverlayStatus,
    /// Focus-detector params last pushed; the overlay settings are part of them.
    focus_params: Option<FocusParams>,
    /// `overlay.enabled` as last acted on (false while the dev pattern runs).
    #[cfg(target_os = "linux")]
    wanted: bool,
    #[cfg(target_os = "linux")]
    handle: Option<crate::overlay::OverlayHandle>,
    /// Result of an in-flight `OverlayHandle::spawn` on its helper thread.
    #[cfg(target_os = "linux")]
    pending: Option<Receiver<Result<crate::overlay::OverlayHandle, DisabledReason>>>,
}

/// Drop (= shut down + join) the overlay off the UI thread: the join waits for its current
/// frame, which must not stall ours. If the thread can't spawn, the handle drops here.
#[cfg(target_os = "linux")]
fn drop_overlay_async(h: crate::overlay::OverlayHandle) {
    let _ = std::thread::Builder::new().name("overlay-drop".into()).spawn(move || drop(h));
}

/// The focus detector's params. It runs for the hotkey/input gates, and whenever the
/// overlay is on: its `focus_only` (D5) and its monitor detection (D18) need a real
/// answer, and an idle detector fails open (always "focused").
fn focus_params(cfg: &AppConfig) -> FocusParams {
    let o = &cfg.overlay;
    FocusParams {
        method: cfg.hotkeys.focus_method,
        custom_cmd: cfg.hotkeys.custom_cmd.clone(),
        game_match: cfg.hotkeys.game_match.clone(),
        poll_hz: cfg.hotkeys.focus_poll_hz,
        enabled: cfg.hotkeys.input_focus_gate
            || cfg.hotkeys.gate_mode == crate::config::GateMode::WindowFocus
            || o.enabled,
        monitor: (cfg!(target_os = "linux") && o.enabled).then(|| MonitorParams {
            method: o.monitor_method,
            cmd: o.monitor_cmd.clone(),
            fixed: o.monitor_fixed.clone(),
        }),
    }
}

/// Speed Trace window length, in accepted-sample ("active") seconds.
pub const TRACE_WINDOW_SECS: f32 = 30.0;

/// Decide whether the Speed Trace accepts a new sample and, if so, where the
/// active-time axis moves to. `dt_wall` is the wall-clock seconds since the
/// last accepted sample (`None` = first sample ever). Samples are throttled to
/// ~25 Hz, and a long gap — e.g. a game pause, during which no samples are
/// accepted — is clamped to a single frame so the axis never jumps.
fn trace_step(dt_wall: Option<f32>, t_active: f32) -> Option<f32> {
    match dt_wall {
        None => Some(t_active),
        Some(dt) if dt < 0.04 => None,
        Some(dt) => Some(t_active + dt.min(0.1)),
    }
}

impl ForzaApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        // Geist family, copied from the ritz launcher (licences in assets/fonts/):
        // Geist Mono (TTF) for crisp UI text, Geist Mono Nerd Font (OTF) for icon
        // glyphs only — text stays a clean TTF, only \uf… icons fall back to the OTF.
        fonts.font_data.insert(
            "geist_mono".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/GeistMono-Regular.ttf"))
                .into(),
        );
        fonts.font_data.insert(
            "geist_icons".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/GeistMonoNerdFont-Regular.otf"))
                .into(),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let fam = fonts.families.entry(family).or_default();
            fam.insert(0, "geist_mono".to_owned()); // primary UI text
            fam.push("geist_icons".to_owned());     // icon-glyph fallback
        }
        _cc.egui_ctx.set_fonts(fonts);
        crate::theme::apply(&_cc.egui_ctx);

        let config = AppConfig::load();
        let engines = load_engines();

        // Two hops: UDP thread → listener thread (Backfire + gearbox) → UI. The second hop
        // is the listener's capped packet mailbox, not a channel — see `worker.rs`.
        let (packet_tx, packet_rx) = mpsc::channel();
        let network = start_receiver(config.listen_port, packet_tx.clone());
        let pending_port = config.listen_port;

        // Spawn background thread to load the seasonal map image (skip if Map module disabled)
        let season = current_season();
        let map_rx = if !config
            .disabled_modules
            .contains(&crate::config::WidgetKind::MiniMap)
        {
            let (map_tx, map_rx) = mpsc::channel::<MapLoadMessage>();
            let map_quality = config.minimap_quality;
            std::thread::spawn(move || {
                map_load_thread(season, map_quality, map_tx);
            });
            Some(map_rx)
        } else {
            None
        };

        let initial_zoom = config.minimap_zoom_stopped_m;
        let config_coop_last_code = config.coop_last_code.clone();

        // Hotkeys: shared "input allowed" flag, focus detector, capture backend.
        let input_allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let focus = Arc::new(FocusDetector::new(focus_params(&config)));
        let (hotkeys, hotkey_rx) = HotkeyListener::new(global_bindings(&config));
        let mut input = InputSender::new();
        input.set_focus_gate(input_allowed.clone());

        // The listener thread owns Backfire, the gearbox, the per-car calibrations and the
        // global hotkeys, so they all keep running when the window stops being drawn.
        let mut coop = crate::coop::CoopState::new(
            &config.coop_name,
            config.coop_hue,
            config.coop_buffer_ms,
        );
        // Rejoin the last Trystero room (opt-in; Cloudflare codes are per-session, so
        // there is nothing to rejoin there).
        if config.coop_autoconnect
            && config.coop_transport == crate::config::CoopTransport::Trystero
            && !config.coop_room.trim().is_empty()
        {
            coop.start_trystero(
                config.coop_room.trim(),
                &config.coop_name,
                config.coop_hue,
                config.coop_buffer_ms,
            );
        }
        let listener = crate::listeners::worker::spawn(
            packet_rx,
            hotkey_rx,
            input.clone(),
            input_allowed,
            focus.clone(),
            config.clone(),
            coop.reader(),
        );

        Self {
            config,
            engines,
            labels: crate::labels::Labels::load(&_cc.egui_ctx),
            telemetry: TelemetryState::new(),
            current_tab: Tab::Dashboard,
            sprint_timer: SprintTimer::new(),
            backfire: BackfireView::default(),
            dsg: DsgView::default(),
            input,
            hotkeys,
            focus,
            rebinding: None,
            rebind_button: None,
            last_tab: Tab::Dashboard,
            hud_hidden: false,
            detect_until: None,
            focus_preview: String::new(),
            power_capture: PowerCapture::new(),
            saved_power_curve: None,
            perf_test: PerfTest::new(),
            engine_search: String::new(),
            pending_port,
            last_car_ordinal: 0,
            last_packet_time: None,
            max_power_ps: 0.0,
            max_torque_nm: 0.0,
            max_boost_psi: 0.0,
            max_speed_kmh: 0.0,
            cached_engine_max_rpm: 0.0,
            fi_detected: false,
            dynamic_max_rpm: 0.0,
            wheel_radius_est: [0.33; 4],
            icon_center_cache: crate::iconcache::IconCenterCache::new(),
            suspension_stats: SuspensionStats::default(),
            gforce_stats: GForceStats::default(),
            cached_car_class_str: String::new(),
            cached_car_class: -1,
            cached_car_pi: 0,
            cached_drivetrain: -1,
            cached_drivetrain_str: "XWD".to_string(),
            cached_num_cylinders: 0,
            speed_delta_kmh: 0.0,
            last_tracked_speed: 0.0,
            last_track_instant: None,
            speed_history: VecDeque::new(),
            power_plot_auto_bounds: false,
            page_settings_open: false,
            page_settings_opacity: 0.5,
            page_settings_tab: PageSettingsTab::Tab(Tab::Dashboard),
            page_dashboard_sub_tab: DashboardSubTab::default(),
            page_map_sub_tab: MiniMapTab::default(),
            profile_dialog: ProfileDialog::None,
            profile_dialog_focus: false,
            profile_name_buf: String::new(),
            profile_io_status: String::new(),
            profile_export_sel: vec![true; crate::config::KEY_GROUPS.len()],
            profile_import_buf: String::new(),
            profile_import_builtin: None,
            profile_import_sel: vec![true; crate::config::KEY_GROUPS.len()],
            profile_import_present: vec![false; crate::config::KEY_GROUPS.len()],
            profile_import_new: true,
            profile_import_new_name: String::new(),
            profile_import_overwrite: String::new(),
            changelog_show_added: true,
            changelog_show_fixed: true,
            changelog_show_removed: true,
            changelog_show_info: true,
            dashboard_drag: None,
            dashboard_resize: None,
            minimap_texture: None,
            minimap_orig_size: [1, 1],
            minimap_current_zoom: initial_zoom,
            minimap_loaded_season: season,
            minimap_stopped_at: None,
            minimap_last_render_time: 0.0,
            minimap_cached_car_x: 0.0,
            minimap_cached_car_z: 0.0,
            minimap_cached_yaw: 0.0,
            minimap_cached_raw_yaw: 0.0,
            minimap_smoothed_yaw: 0.0,
            minimap_img_receiver: map_rx,
            minimap_cache_progress: None,
            minimap_trails: HashMap::new(),
            coop_last_pos: HashMap::new(),
            trace_history: VecDeque::new(),
            trace_active_secs: 0.0,
            trace_last_sample: None,
            coop,
            coop_join_input: config_coop_last_code,
            coop_copied_at: None,
            packet_tx,
            listener,
            last_toggle_gen: 0,
            _network: network,
            overlay: OverlayRuntime::default(),
        }
    }

    /// Port change: start a fresh UDP thread on the same packet channel and drop the old
    /// handle (its thread exits within its 200 ms read timeout). The listener thread keeps
    /// its receiver — and therefore its calibration and shift state — right through.
    pub fn restart_receiver(&mut self, port: u16) {
        self._network = start_receiver(port, self.packet_tx.clone());
        self.config.listen_port = port;
    }

    /// True while Backfire's simulated keypress may still be echoing back in
    /// telemetry as a fake accel value. The window is anchored by the input
    /// worker at actual key emission (input::EchoWindow); an active render-FPS
    /// limit is added as grace, since a slow frame cadence delays packet
    /// processing past the raw window.
    pub fn backfire_echo_active(&self) -> bool {
        self.input
            .synthetic_active(crate::listeners::backfire::echo_grace(&self.config))
    }

    /// Perform an app-focused hotkey action (handled on egui input).
    fn run_app_hotkey(&mut self, action: crate::config::HotkeyAction) {
        use crate::config::HotkeyAction::*;
        match action {
            MiniSettings => {
                self.page_settings_open = !self.page_settings_open;
                self.page_settings_tab = PageSettingsTab::Tab(self.current_tab);
                if !self.page_settings_open { self.config.save(); }
            }
            DashboardEdit => {
                if self.current_tab == Tab::Dashboard {
                    self.config.dashboard_edit_mode = !self.config.dashboard_edit_mode;
                }
            }
            _ => {}
        }
    }

    /// Clear the RPM (redline) calibration + engagement (tab button; the Reset-RPM hotkey
    /// does the same on the listener thread): forgets the detected redline and sets the box
    /// hands-off until the driver's next manual upshift re-locks it. The per-gear speed map
    /// is left intact. The listener thread owns the calibration, so this is a command — the
    /// UI's copy catches up with the next snapshot.
    pub fn clear_rpm_calibration(&self) {
        self.listener.send(Command::ClearRpmCalibration);
    }

    /// Clear the per-gear speed map (tab button only). The detected redline is left intact.
    pub fn clear_gear_map(&self) {
        self.listener.send(Command::ClearGearMap);
    }

    /// Push current hotkey config to the live backend + focus detector. Call
    /// after any hotkey/detection setting changes.
    pub fn sync_hotkeys(&mut self) {
        self.hotkeys.set_bindings(global_bindings(&self.config));
        let p = focus_params(&self.config);
        self.focus.set_params(p.clone());
        self.overlay.focus_params = Some(p);
    }

    pub fn overlay_status(&self) -> &OverlayStatus {
        &self.overlay.status
    }

    /// Hotkey capture while `rebinding` is armed, for every rebind button (Setup → Hotkey and
    /// the Overlay tab's Hide HUD row): Esc cancels, Backspace unbinds ("Not set", D28), any
    /// other bindable key binds with the modifiers held. Runs before the tabs are drawn, so a
    /// tab's own capture code never sees the key. Returns true while a capture is armed, so
    /// the key doesn't also fire an in-app hotkey.
    ///
    /// The capture disarms (without binding) on a primary press anywhere but the armed button,
    /// and whenever another widget holds keyboard focus (a text field) — so a key meant for
    /// something else never rebinds silently. Tab switches disarm it in `update`. A key that
    /// ends the capture (bind / Backspace / Esc) is consumed, so it doesn't also act elsewhere.
    fn capture_rebind(&mut self, ctx: &Context) -> bool {
        use crate::keymap::{HotKey, HotkeyBinding, Mods};
        let Some(action) = self.rebinding else { return false };
        let button = self.rebind_button;
        let pressed_elsewhere = ctx.input(|i| {
            i.pointer.primary_pressed()
                && !matches!((button, i.pointer.interact_pos()), (Some((_, r)), Some(p)) if r.contains(p))
        });
        // `wants_keyboard_input`, except the armed button itself (it may hold keyboard focus
        // when armed via Tab + Enter).
        let other_focus = ctx.memory(|m| m.focused()).is_some_and(|f| Some(f) != button.map(|(id, _)| id));
        if pressed_elsewhere || other_focus {
            self.rebinding = None;
            self.rebind_button = None;
            return false;
        }
        let pressed = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, m)) = pressed else { return true };
        match key {
            egui::Key::Escape => {}
            egui::Key::Backspace => {
                self.config.hotkeys.unbind(action);
                self.sync_hotkeys();
            }
            k => {
                // Not a bindable key (e.g. a lone modifier arrives as none): keep waiting.
                let Some(hk) = HotKey::from_egui(k) else { return true };
                let mods = Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, sup: false };
                self.config.hotkeys.bind(action, HotkeyBinding { mods, key: hk });
                self.sync_hotkeys();
            }
        }
        ctx.input_mut(|i| {
            i.events.retain(|e| !matches!(e, egui::Event::Key { key: k, pressed: true, .. } if *k == key))
        });
        self.rebinding = None;
        self.rebind_button = None;
        true
    }

    /// Called by a rebind button right after it's drawn (and its click handled): while its
    /// action is the armed one, remember where it is for [`Self::capture_rebind`].
    pub fn track_rebind_button(&mut self, action: crate::config::HotkeyAction, resp: &egui::Response) {
        if self.rebinding == Some(action) {
            self.rebind_button = Some((resp.id, resp.rect));
        }
    }

    /// Once a frame: keep the focus detector's overlay inputs current (the Overlay tab and
    /// profile loads edit them without calling `sync_hotkeys`), then start/stop the overlay
    /// thread to follow `overlay.enabled`.
    fn sync_overlay(&mut self) {
        let p = focus_params(&self.config);
        if self.overlay.focus_params.as_ref() != Some(&p) {
            self.focus.set_params(p.clone());
            self.overlay.focus_params = Some(p);
        }
        #[cfg(target_os = "linux")]
        self.sync_overlay_thread();
    }

    /// Start on enable, stop on disable, collect the spawn result, notice a dead thread.
    /// Failures aren't retried until the next off → on, so a missing layer-shell costs one
    /// probe, not one per frame.
    #[cfg(target_os = "linux")]
    fn sync_overlay_thread(&mut self) {
        use std::sync::mpsc::TryRecvError;
        // The dev test pattern (main.rs) owns the overlay while it's requested.
        let want = self.config.overlay.enabled && !crate::overlay::dev_test_requested();
        if want != self.overlay.wanted {
            self.overlay.wanted = want;
            if !want {
                self.detach_overlay();
                self.overlay.status = OverlayStatus::Off;
            } else if self.overlay.pending.is_some() {
                self.overlay.status = OverlayStatus::Starting; // re-enabled mid-start: reuse it
            } else if self.overlay.handle.is_none() {
                self.start_overlay();
            }
        }
        if let Some(rx) = &self.overlay.pending {
            let result = match rx.try_recv() {
                Ok(r) => r,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    Err(DisabledReason::Wayland("overlay thread didn't start".into()))
                }
            };
            self.overlay.pending = None;
            match (want, result) {
                (true, Ok(h)) => self.attach_overlay(h),
                (true, Err(reason)) => self.overlay.status = OverlayStatus::Disabled(reason),
                (false, Ok(h)) => drop_overlay_async(h), // disabled while starting
                (false, Err(_)) => {}
            }
        }
        if self.overlay.handle.as_ref().is_some_and(|h| h.is_dead()) {
            self.detach_overlay();
            self.overlay.status = OverlayStatus::Stopped;
        }
    }

    /// `OverlayHandle::spawn` blocks for up to 5 s waiting for the compositor, so it runs on
    /// a helper thread and [`Self::sync_overlay_thread`] polls the result.
    #[cfg(target_os = "linux")]
    fn start_overlay(&mut self) {
        let opts = crate::overlay::OverlayOptions {
            output: self.focus.monitor_output(),
            test_pattern: false,
            coop: Some(self.coop.reader()),
        };
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("overlay-start".into()).spawn(move || {
            // A dropped receiver (app closing) drops the handle here, which shuts it down.
            let _ = tx.send(crate::overlay::OverlayHandle::spawn(opts));
        });
        self.overlay.status = match spawned {
            Ok(_) => {
                self.overlay.pending = Some(rx);
                OverlayStatus::Starting
            }
            Err(e) => OverlayStatus::Disabled(DisabledReason::Wayland(e.to_string())),
        };
    }

    /// Feed the running overlay: snapshots + wakes from the listener thread, output changes
    /// from the focus thread's monitor detection (which re-sends the current output to a
    /// new sink on its next tick).
    #[cfg(target_os = "linux")]
    fn attach_overlay(&mut self, h: crate::overlay::OverlayHandle) {
        let waker = h.waker();
        self.listener.set_hud_sink(Some(crate::overlay::snapshot::HudSink::new(h.slot(), move || waker.wake())));
        let sender = h.sender();
        self.focus.set_output_sink(Some(Box::new(move |name| {
            sender.send(crate::overlay::OverlayCmd::SetOutput(name))
        })));
        self.overlay.handle = Some(h);
        self.overlay.status = OverlayStatus::Running;
    }

    /// Unhook the feeds first, so nothing targets the overlay as it shuts down, then drop it.
    #[cfg(target_os = "linux")]
    fn detach_overlay(&mut self) {
        if let Some(h) = self.overlay.handle.take() {
            self.listener.set_hud_sink(None);
            self.focus.set_output_sink(None);
            drop_overlay_async(h);
        }
    }

    /// Copy the listener thread's published state into our local copy, if it's free right
    /// now — never wait, a missed frame just redraws the previous values. A global hotkey
    /// can have toggled Backfire / the gearbox while we weren't being drawn at all, so adopt
    /// the enable flags whenever the toggle generation moves; `last_toggle_gen` is echoed
    /// back on the next push so our (then stale) config can't undo the toggle.
    fn sync_listener_view(&mut self) {
        if let Some(view) = self.listener.try_view() {
            self.adopt_listener_view(view);
        }
    }

    fn adopt_listener_view(&mut self, view: crate::listeners::worker::ListenerView) {
        self.dsg = view.dsg;
        self.backfire = view.backfire;
        self.dynamic_max_rpm = view.dynamic_max_rpm;
        self.hud_hidden = view.hud_hidden;
        if view.toggle_gen != self.last_toggle_gen {
            self.last_toggle_gen = view.toggle_gen;
            self.config.dsg_enabled = view.dsg_enabled;
            self.config.backfire_enabled = view.backfire_enabled;
        }
    }

    pub fn drain_packets(&mut self) {
        let step = self.config.power_curve_step;
        let accel_s = self.config.accel_start_kmh;
        let accel_e = self.config.accel_end_kmh;
        let decel_s = self.config.decel_start_kmh;
        let decel_e = self.config.decel_end_kmh;
        self.perf_test.decel.dynamic_mode = self.config.decel_dynamic_mode;

        // Take the whole mailbox in one lock and process it unlocked. It is already capped
        // at `worker::UI_BACKLOG_CAP` with the oldest dropped, so a window that was hidden
        // for minutes hands back at most a few seconds of telemetry instead of replaying
        // everything through the sprint timer, the trace buffer and the Co-Op relay.
        for pkt in self.listener.take_packets() {
            self.last_packet_time = Some(Instant::now());

            // Car change: reset per-car state
            if pkt.car_ordinal != 0 && pkt.car_ordinal != self.last_car_ordinal {
                self.last_car_ordinal = pkt.car_ordinal;
                self.sprint_timer.reset();
                self.power_capture.on_car_changed();
                self.perf_test.reset();
                self.max_power_ps = 0.0;
                self.max_torque_nm = 0.0;
                self.max_boost_psi = 0.0;
                self.max_speed_kmh = 0.0;
                self.cached_engine_max_rpm = 0.0;
                self.fi_detected = false;
                self.wheel_radius_est = [0.33; 4];
                // The gearbox's own per-car reset (calibration, shift state, saved profile)
                // happens on the listener thread — it saw this packet first.
            }

            // Session maxima + cache car identity
            if pkt.is_race_on != 0 {
                self.cached_car_class_str = pkt.car_class_str().to_string();
                self.cached_car_class = pkt.car_class;
                self.cached_car_pi = pkt.car_performance_index;
                self.cached_drivetrain_str = pkt.drivetrain_str().to_string();
                self.cached_drivetrain = pkt.drivetrain_type;
                self.cached_num_cylinders = pkt.num_cylinders;
                if pkt.engine_max_rpm > 0.0 {
                    self.cached_engine_max_rpm = pkt.engine_max_rpm as f64;
                }
                if pkt.boost > 0.05 {
                    self.fi_detected = true;
                }
                if pkt.speed >= 0.1 {
                    self.max_power_ps = self.max_power_ps.max(pkt.power_ps());
                    self.max_torque_nm = self.max_torque_nm.max(pkt.torque_nm());
                    self.max_boost_psi = self.max_boost_psi.max(pkt.boost);
                    self.max_speed_kmh = self.max_speed_kmh.max(pkt.speed * 3.6);
                }

                let lat = pkt.acceleration_x / 9.81;
                let lon = pkt.acceleration_z / 9.81;
                let vert = pkt.acceleration_y / 9.81;
                self.gforce_stats.update(lat, lon, vert);

                self.suspension_stats.update([
                    pkt.normalized_suspension_travel_fl,
                    pkt.normalized_suspension_travel_fr,
                    pkt.normalized_suspension_travel_rl,
                    pkt.normalized_suspension_travel_rr,
                ]);

                // Tire radius estimate: while a wheel is gripping at speed,
                // radius ≈ vehicle speed / wheel rotation speed. EMA-smoothed.
                let rotations = [
                    pkt.wheel_rotation_speed_fl,
                    pkt.wheel_rotation_speed_fr,
                    pkt.wheel_rotation_speed_rl,
                    pkt.wheel_rotation_speed_rr,
                ];
                let combined_slips = [
                    pkt.tire_combined_slip_fl,
                    pkt.tire_combined_slip_fr,
                    pkt.tire_combined_slip_rl,
                    pkt.tire_combined_slip_rr,
                ];
                for i in 0..4 {
                    if combined_slips[i].abs() < 0.1 && pkt.speed > 5.0 && rotations[i] > 1.0 {
                        let radius = pkt.speed / rotations[i];
                        self.wheel_radius_est[i] += 0.02 * (radius - self.wheel_radius_est[i]);
                    }
                }

                // Speed delta tracking — maintain 1-second rolling history
                let cur_kmh = pkt.speed * 3.6;
                let now = Instant::now();
                self.speed_history.push_back((now, cur_kmh));
                while let Some(&(t, _)) = self.speed_history.front() {
                    if now.duration_since(t) > Duration::from_secs(1) {
                        self.speed_history.pop_front();
                    } else {
                        break;
                    }
                }

                // Speed/RPM trace history (~30 s window, ~25 Hz) on an active-time
                // axis: paused packets are skipped and the clock only advances when
                // a sample is accepted (a long pause counts as one frame), so pauses
                // neither gap nor slide the plot and resume appends seamlessly.
                if !pkt.is_paused() {
                    let dt_wall = self
                        .trace_last_sample
                        .map(|t| now.duration_since(t).as_secs_f32());
                    if let Some(t) = trace_step(dt_wall, self.trace_active_secs) {
                        self.trace_active_secs = t;
                        self.trace_last_sample = Some(now);
                        self.trace_history
                            .push_back((t, cur_kmh, pkt.current_engine_rpm));
                        while let Some(&(t0, ..)) = self.trace_history.front() {
                            if t - t0 > TRACE_WINDOW_SECS {
                                self.trace_history.pop_front();
                            } else {
                                break;
                            }
                        }
                    }
                }
                match self.config.speed_delta_mode {
                    SpeedDeltaMode::Calculate => {
                        if let Some(&(_, oldest)) = self.speed_history.front() {
                            self.speed_delta_kmh = cur_kmh - oldest;
                        }
                    }
                    SpeedDeltaMode::Track => {
                        if self
                            .last_track_instant
                            .map(|t| t.elapsed() >= Duration::from_secs(1))
                            .unwrap_or(true)
                        {
                            self.speed_delta_kmh = cur_kmh - self.last_tracked_speed;
                            self.last_tracked_speed = cur_kmh;
                            self.last_track_instant = Some(now);
                        }
                    }
                }
            }

            // Brake + HandBrake both at 100% → clear power curve only
            if pkt.brake == 255 && pkt.hand_brake == 255 {
                self.power_capture.clear();
            }

            self.sprint_timer.update(&pkt);
            // Backfire's synthetic W press echoes back as accel ≥ 245 while
            // coasting — without this gate every pop seeds the power curve with
            // bogus low-power points in RPM buckets no real pull has covered yet.
            if !self.backfire_echo_active() {
                self.power_capture.update(&pkt, step);
            }
            self.perf_test
                .update(&pkt, accel_s, accel_e, decel_s, decel_e);
            // Backfire + the gearbox already ran on this packet, on the listener thread.

            // Co-Op's outgoing relay runs on the listener thread too (see worker.rs).

            self.telemetry.update(pkt);
        }

        // Mark disconnected after 2 s without a packet
        if let Some(t) = self.last_packet_time {
            if t.elapsed() > Duration::from_secs(2) {
                self.telemetry.is_connected = false;
            }
        }
    }

    /// Append current positions to each player's map trail. Only active during a
    /// co-op session (local + remotes); cleared otherwise so solo behaviour is unchanged.
    fn update_minimap_trails(&mut self) {
        use std::collections::HashSet;
        const MIN_MOVE: f32 = 4.0; // metres between recorded points
        const MAX_PTS: usize = 400;
        const TELEPORT: f32 = 300.0; // jump this far in one packet ⇒ clear the trail

        if self.coop.role() == crate::coop::Role::Off {
            if !self.minimap_trails.is_empty() {
                self.minimap_trails.clear();
            }
            if !self.coop_last_pos.is_empty() {
                self.coop_last_pos.clear();
            }
            return;
        }

        // Drop points older than the fade window so trails stay bounded by time too.
        let max_age = Duration::from_secs_f32(self.config.coop_trail_fade_secs.max(0.5));
        let now = Instant::now();
        fn push(
            trails: &mut HashMap<String, VecDeque<(f32, f32, Instant)>>,
            key: String,
            x: f32,
            z: f32,
            now: Instant,
            max_age: Duration,
        ) {
            let dq = trails.entry(key).or_default();
            match dq.back() {
                Some(&(px, pz, _)) => {
                    let moved = (px - x).hypot(pz - z);
                    if moved >= TELEPORT {
                        dq.clear(); // teleport (fast-travel / reset) — drop the stale line
                        dq.push_back((x, z, now));
                    } else if moved >= MIN_MOVE {
                        dq.push_back((x, z, now));
                    }
                }
                None => dq.push_back((x, z, now)),
            }
            while dq.front().is_some_and(|&(_, _, t)| now.duration_since(t) > max_age) {
                dq.pop_front();
            }
            if dq.len() > MAX_PTS {
                dq.pop_front();
            }
        }

        // Remember each player's last useful telemetry: position only from
        // non-paused packets, class/PI only from packets that carry one (PI 0 =
        // empty, e.g. paused) — so a pause never blanks either.
        fn remember(seen: &mut HashMap<String, CoopSeen>, key: &str, p: &crate::packet::ForzaPacket) {
            let e = seen.entry(key.to_string()).or_insert(CoopSeen {
                x: p.position_x,
                z: p.position_z,
                yaw: p.yaw,
                car_class: p.car_class,
                pi: p.car_performance_index,
            });
            if !p.is_paused() {
                e.x = p.position_x;
                e.z = p.position_z;
                e.yaw = p.yaw;
            }
            if p.car_performance_index != 0 {
                e.car_class = p.car_class;
                e.pi = p.car_performance_index;
            }
        }

        let mut present: HashSet<String> = HashSet::new();
        present.insert("local".to_string());
        if let Some(pkt) = &self.telemetry.latest {
            remember(&mut self.coop_last_pos, "local", pkt);
            // Skip paused games (car at origin) so we don't draw a line to (0,0).
            if pkt.is_race_on != 0 && !pkt.is_paused() {
                push(
                    &mut self.minimap_trails,
                    "local".to_string(),
                    pkt.position_x,
                    pkt.position_z,
                    now,
                    max_age,
                );
            }
        }
        for (info, rp) in self.coop.remote_players() {
            remember(&mut self.coop_last_pos, &info.id, &rp);
            if !rp.is_paused() {
                push(
                    &mut self.minimap_trails,
                    info.id.clone(),
                    rp.position_x,
                    rp.position_z,
                    now,
                    max_age,
                );
            }
            present.insert(info.id);
        }
        self.minimap_trails.retain(|k, _| present.contains(k));
        self.coop_last_pos.retain(|k, _| present.contains(k));
    }
}

impl eframe::App for ForzaApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        crate::i18n::set_language(self.config.language);
        self.sync_listener_view();
        self.sync_overlay();
        self.drain_packets();
        // Advance co-op jitter buffers so remote player positions are ready to draw.
        self.coop.tick();
        self.update_minimap_trails();

        // Poll minimap image receiver — drain all pending messages this frame
        if self.minimap_img_receiver.is_some() {
            loop {
                let msg = self.minimap_img_receiver.as_ref().unwrap().try_recv();
                match msg {
                    Ok(MapLoadMessage::CacheBuildStarted { names }) => {
                        self.minimap_cache_progress = Some(names);
                    }
                    Ok(MapLoadMessage::CacheBuilt { name }) => {
                        if let Some(ref mut list) = self.minimap_cache_progress {
                            list.retain(|n| n != &name);
                        }
                    }
                    Ok(MapLoadMessage::Done(result)) => {
                        self.minimap_cache_progress = None;
                        if let Some((img, orig_size)) = result {
                            self.minimap_orig_size = orig_size;
                            self.minimap_texture = Some(ctx.load_texture(
                                "minimap",
                                img,
                                egui::TextureOptions {
                                    magnification: egui::TextureFilter::Linear,
                                    minification: egui::TextureFilter::Linear,
                                    wrap_mode: egui::TextureWrapMode::MirroredRepeat,
                                    mipmap_mode: None,
                                },
                            ));
                        }
                        self.minimap_img_receiver = None;
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.minimap_cache_progress = None;
                        self.minimap_img_receiver = None;
                        break;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
        }

        // Auto-reload when the season changes (skip if Map module disabled)
        let season_now = current_season();
        if season_now != self.minimap_loaded_season
            && self.minimap_img_receiver.is_none()
            && !self
                .config
                .disabled_modules
                .contains(&crate::config::WidgetKind::MiniMap)
        {
            let (map_tx, map_rx) = mpsc::channel::<MapLoadMessage>();
            let q = self.config.minimap_quality;
            std::thread::spawn(move || {
                map_load_thread(season_now, q, map_tx);
            });
            self.minimap_texture = None;
            self.minimap_img_receiver = Some(map_rx);
            self.minimap_loaded_season = season_now;
        }

        // Throttle minimap position/yaw cache refresh to minimap_fps_limit
        {
            let now = ctx.input(|i| i.time);
            let should_update = if self.config.minimap_fps_limit_enabled {
                let interval = 1.0 / self.config.minimap_fps_limit.max(1.0) as f64;
                if now - self.minimap_last_render_time >= interval {
                    self.minimap_last_render_time = now;
                    true
                } else {
                    false
                }
            } else {
                true
            };
            if should_update {
                if let Some(ref pkt) = self.telemetry.latest {
                    if pkt.is_race_on != 0 {
                        self.minimap_cached_car_x = pkt.position_x;
                        self.minimap_cached_car_z = pkt.position_z;
                        self.minimap_cached_yaw =
                            crate::minimap::target_yaw(pkt, self.config.minimap_use_movement_dir);
                        self.minimap_cached_raw_yaw = pkt.yaw;
                    }
                }
            }
        }

        // Shared "stopped" state: under 5 km/h for at least 1.5 s. Both the
        // ease-to-north and the zoom-out read it, so they kick in together.
        let speed_kmh = self
            .telemetry
            .latest
            .as_ref()
            .map(|p| p.speed * 3.6)
            .unwrap_or(0.0);
        let minimap_stopped = if speed_kmh >= crate::minimap::STOPPED_KMH {
            self.minimap_stopped_at = None;
            false
        } else {
            let stopped_at = self.minimap_stopped_at.get_or_insert_with(Instant::now);
            stopped_at.elapsed().as_secs_f32() >= crate::minimap::STOPPED_SECS
        };

        // Smooth rotation: lerp minimap_smoothed_yaw toward the latest target every frame
        {
            // In heading-up mode, ease the map to north once the car has been stopped.
            let stopped_north = self.config.minimap_north_up_when_stopped && minimap_stopped;

            let target = if let Some(ref pkt) = self.telemetry.latest {
                if pkt.is_race_on != 0 {
                    if stopped_north {
                        0.0
                    } else {
                        crate::minimap::target_yaw(pkt, self.config.minimap_use_movement_dir)
                    }
                } else {
                    self.minimap_smoothed_yaw
                }
            } else {
                self.minimap_smoothed_yaw
            };

            // Ease-to-north always animates (that's the whole point); otherwise honour the setting.
            if self.config.minimap_smooth_rotation || stopped_north {
                let dt = ctx.input(|i| i.unstable_dt);
                self.minimap_smoothed_yaw = crate::minimap::ease_yaw(self.minimap_smoothed_yaw, target, dt);
            } else {
                self.minimap_smoothed_yaw = self.minimap_cached_yaw;
            }
        }

        // Smooth minimap zoom: immediate zoom-in when driving, 1.5 s delay before zooming out
        {
            let dt = ctx.input(|i| i.unstable_dt);
            if minimap_stopped {
                self.minimap_current_zoom = crate::minimap::ease_zoom(
                    self.minimap_current_zoom, self.config.minimap_zoom_stopped_m, dt);
            } else if speed_kmh >= crate::minimap::STOPPED_KMH {
                self.minimap_current_zoom = crate::minimap::ease_zoom(
                    self.minimap_current_zoom, self.config.minimap_zoom_driving_m, dt);
            }
            // else: under 5 km/h but not yet 1.5 s — hold the current zoom.
        }

        // F11 fullscreen toggle (Windows only)
        #[cfg(target_os = "windows")]
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
        }

        // ── Hotkeys ────────────────────────────────────────────────
        // App-focused actions: matched from config against egui input (only
        // delivered while our window is focused → inherently UI-only).
        if self.current_tab != self.last_tab {
            self.last_tab = self.current_tab;
            self.rebinding = None;
            crate::ui::overlay_tab::clear_layout_selection(ctx);
        }
        let capturing = self.capture_rebind(ctx);
        if !capturing {
            use crate::config::{HotkeyAction, HotkeyScope};
            let m = ctx.input(|i| i.modifiers);
            for action in HotkeyAction::ALL.iter().copied() {
                if action.scope() != HotkeyScope::AppFocused { continue; }
                let Some(b) = self.config.hotkeys.bindings.get(&action).copied() else { continue; };
                let pressed = ctx.input(|i| i.key_pressed(b.key.to_egui()))
                    && m.ctrl == b.mods.ctrl && m.alt == b.mods.alt
                    && m.shift == b.mods.shift;
                if pressed { self.run_app_hotkey(action); }
            }
        }
        // Global actions (G / B / Reset-RPM) and the synthetic-input focus gate are handled
        // on the listener thread — they have to keep working while this loop isn't running.
        // Detect button: when the 3s countdown elapses, capture the active window.
        if let Some(t) = self.detect_until {
            if std::time::Instant::now() >= t {
                self.detect_until = None;
                if let Ok(name) = self.focus.query_now() {
                    if let Some(first) = name.split_whitespace().next() {
                        self.config.hotkeys.game_match = first.to_string();
                        self.sync_hotkeys();
                    }
                }
            } else {
                ctx.request_repaint(); // keep the countdown ticking
            }
        }

        // Tab bar. 4px top + 30px button row + 5px bottom, then the panel's divider.
        let mut tab_frame = egui::Frame::side_top_panel(&ctx.style()).fill(crate::theme::HEAD);
        tab_frame.inner_margin.left = 4;
        tab_frame.inner_margin.right = 4;
        tab_frame.inner_margin.top = 4;
        tab_frame.inner_margin.bottom = 5; // +1 over the top to visually centre against the divider
        egui::TopBottomPanel::top("tab_bar")
            .frame(tab_frame)
            .show(ctx, |ui| {
                use crate::config::TopBarStyle;
                use crate::i18n::tr;
                use crate::icons;
                let style = self.config.top_bar_style;
                let left = [
                    (Tab::Dashboard,   icons::DASHBOARD,  "Dashboard"),
                    (Tab::Overlay,     icons::OVERLAY,    "Overlay"),
                    (Tab::PowerCurve,  icons::LINE_CHART, "Power Curve"),
                    (Tab::Coop,        icons::USERS,      "Co-Op"),
                    (Tab::Backfire,    icons::BOLT,       "Backfire"),
                    (Tab::Gearbox,     icons::GEARBOX,    "Automatic Gearbox"),
                    (Tab::EngineSwaps, icons::ENGINE,     "Engine Swaps"),
                ];
                let right = [
                    (Tab::Settings,  icons::COG,      "Setup"),
                    (Tab::Debug,     icons::BUG,      "Debug"),
                    (Tab::Changelog, icons::BULLHORN, "What's New"),
                ];
                // right_to_left adds items right→left, so Setup stays rightmost (where
                // users expect it), Debug sits just left of it, then What's New.
                ui.horizontal(|ui| {
                    ui.set_min_height(30.0);
                    match style {
                        TopBarStyle::Legacy => {
                            for (tab, icon, text) in left {
                                ui.selectable_value(&mut self.current_tab, tab, format!("{}  {}", icon, tr(text)));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                for (tab, icon, text) in right {
                                    ui.selectable_value(&mut self.current_tab, tab, format!("{}  {}", icon, tr(text)));
                                }
                            });
                        }
                        TopBarStyle::Simple => {
                            for (tab, icon, _) in left {
                                tab_button(ui, &mut self.current_tab, &mut self.icon_center_cache, tab, icon, None, self.config.high_contrast_icons);
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                for (tab, icon, _) in right {
                                    tab_button(ui, &mut self.current_tab, &mut self.icon_center_cache, tab, icon, None, self.config.high_contrast_icons);
                                }
                            });
                        }
                        TopBarStyle::Modern => {
                            let bar = ui.max_rect();
                            let spacing = ui.spacing().item_spacing.x;
                            // LEFT: wordmark, and optionally a divider + current-page pill.
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("Forza Telemetry V3")
                                    .color(crate::theme::ACCENT)
                                    .size(16.0)
                                    .strong(),
                            );
                            if self.config.modern_show_pill {
                                ui.add_space(8.0);
                                let (div, _) = ui.allocate_exact_size(egui::vec2(1.0, 18.0), egui::Sense::hover());
                                ui.painter().rect_filled(div, 0.0, crate::theme::BORDER);
                                ui.add_space(8.0);
                                let reserve = max_pill_width(ui);
                                page_pill(ui, tr(tab_title(self.current_tab)), reserve);
                            }
                            // CENTER: icon tabs, centred across the full bar width.
                            let used = ui.cursor().min.x - bar.left();
                            let n = left.len() as f32;
                            let center_w = n * 30.0 + (n - 1.0) * spacing;
                            let center_start = (bar.width() - center_w) / 2.0;
                            ui.add_space((center_start - used).max(spacing));
                            for (tab, icon, _) in left {
                                tab_button(ui, &mut self.current_tab, &mut self.icon_center_cache, tab, icon, None, self.config.high_contrast_icons);
                            }
                            // RIGHT: icon tabs, right-aligned in the remaining space.
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                for (tab, icon, _) in right {
                                    tab_button(ui, &mut self.current_tab, &mut self.icon_center_cache, tab, icon, None, self.config.high_contrast_icons);
                                }
                            });
                        }
                    }
                });
            });

        // ── Bottom status bar ──────────────────────────────────────
        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(crate::theme::PANEL2))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    use crate::i18n::tr;
                    use crate::icons;
                    // Instant, centred tooltips for the Backfire / Gearbox indicators.
                    // egui's built-in `on_hover_text` uses a hover grace-period +
                    // pointer-anchored placement that a ui-local `tooltip_delay = 0.0`
                    // override does NOT reliably cancel (the delay is read from the
                    // context style, not this ui). Drawing the tooltip ourselves in a
                    // Tooltip-order Area renders it the same frame the pointer is over
                    // the indicator (no delay) and lets us centre it just above the
                    // indicator's rect instead of the default bottom-right offset.
                    let show_center_tooltip = |ui: &egui::Ui, rect: egui::Rect, text: String| {
                        egui::Area::new(egui::Id::new(("status_bar_tt", &text)))
                            .order(egui::Order::Tooltip)
                            .interactable(false)
                            .fixed_pos(rect.center_top() - egui::vec2(0.0, 6.0))
                            .pivot(egui::Align2::CENTER_BOTTOM)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::popup(ui.style()).show(ui, |ui| {
                                    ui.label(text);
                                });
                            });
                    };
                    // LEFT: connection status + pps, then the Co-Op indicator.
                    // With text on: icon + "Connected"/"Disconnected" word; icon-only
                    // otherwise (PLUG green / NO_SIGNAL red, ink-centred in a fixed box,
                    // the word moves to a hover tooltip). The pps stays — it's a number.
                    // Co-Op icon-only: USERS + player count, state carried by colour
                    // (WARN while connecting, GOOD once everyone is connected).
                    // Why: colour carries the state so icon-only mode stays readable.
                    // NO_SIGNAL renders wider than its glyph advance, so the text variant
                    // needs an extra space to match PLUG's visual gap.
                    let show_text = self.config.status_bar_show_text;
                    let (color, icon, word) = if self.telemetry.is_connected {
                        (crate::theme::GOOD, icons::PLUG, tr("Connected"))
                    } else {
                        (crate::theme::DANGER, icons::NO_SIGNAL, tr("Disconnected"))
                    };
                    // Connection status + pps are informational, not content to
                    // select/copy — disable text selection on just these two labels.
                    if show_text {
                        let sep = if self.telemetry.is_connected { " " } else { "  " };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{icon}{sep}{word}")).color(color),
                            )
                            .selectable(false),
                        );
                    } else {
                        let font = egui::FontId::proportional(14.0);
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(22.0, 18.0), egui::Sense::hover());
                        let pos = self
                            .icon_center_cache
                            .centered_pos(ui, icon, font.clone(), rect.center());
                        ui.painter()
                            .text(pos, egui::Align2::LEFT_TOP, icon, font, color);
                        if resp.hovered() {
                            show_center_tooltip(ui, rect, word.to_string());
                        }
                    }
                    if self.telemetry.is_connected {
                        // Right-align in a 3-wide field so the label doesn't shift
                        // as the packet rate gains or loses a digit.
                        ui.add(
                            egui::Label::new(format!(
                                "  {:>3.0} pps",
                                self.telemetry.packets_per_sec
                            ))
                            .selectable(false),
                        );
                    }

                    // Co-Op indicator (visible from any tab)
                    let coop_role = self.coop.role();
                    if coop_role != crate::coop::Role::Off {
                        if self.config.status_bar_show_text {
                            ui.separator();
                        }
                        let connecting = self.coop.is_connecting();
                        let verb = match coop_role {
                            crate::coop::Role::Host => tr("Hosting"),
                            _ => tr("Joined"),
                        };
                        let n = self.coop.roster().len();
                        let state = if connecting { tr("Connecting…") } else { tr("Connected") };
                        let full = format!("{} · {} {} · {}", verb, n, tr("players"), state);
                        if show_text {
                            // Hosting keeps its accent colour once up; WARN while connecting.
                            let c = if connecting {
                                crate::theme::WARN
                            } else if coop_role == crate::coop::Role::Host {
                                crate::theme::ACCENT
                            } else {
                                crate::theme::GOOD
                            };
                            let resp = ui.colored_label(
                                c,
                                format!("{}  {} · {} {}", icons::USERS, verb, n, tr("players")),
                            );
                            if resp.hovered() {
                                show_center_tooltip(ui, resp.rect, full);
                            }
                        } else {
                            let c = if connecting { crate::theme::WARN } else { crate::theme::GOOD };
                            let resp = ui.colored_label(c, format!("{}  {}", icons::USERS, n));
                            if resp.hovered() {
                                show_center_tooltip(ui, resp.rect, full);
                            }
                        }
                    }

                    // CENTER: Backfire + Automatic Gearbox indicators, centered on the
                    // whole bar. Backfire: green (active) / red (off). Gearbox: green
                    // (active), pastel-amber "Uncalibrated" (enabled but not yet engaged),
                    // or red (off). With text on, each shows its tab icon + an
                    // Active/Deactivated word, split by a divider; icon-only otherwise, the
                    // glyphs ink-centred in fixed boxes exactly like the tab bar.
                    // Both features live on the listener thread; if it ever dies (a panic),
                    // they are silently gone, so say so here rather than leave a frozen
                    // "Active" on screen. The tooltips pick the same word up.
                    let dead = self.listener.is_dead();
                    let (bf_color, bf_word) = if dead {
                        (crate::theme::DANGER, tr("Stopped (error)"))
                    } else if self.config.backfire_enabled {
                        (crate::theme::GOOD, tr("Active"))
                    } else {
                        (crate::theme::DANGER, tr("Deactivated"))
                    };
                    let (gb_color, gb_word) = if dead {
                        (crate::theme::DANGER, tr("Stopped (error)"))
                    } else if !self.config.dsg_enabled {
                        (crate::theme::DANGER, tr("Deactivated"))
                    } else if self.dsg.engaged {
                        (crate::theme::GOOD, tr("Active"))
                    } else {
                        (crate::theme::WARN, tr("Uncalibrated"))
                    };
                    let gap = ui.spacing().item_spacing.x;
                    if self.config.status_bar_show_text {
                        let bf_text = format!("{}  {}", icons::BOLT, bf_word);
                        let gb_text = format!("{}  {}", icons::GEARBOX, gb_word);
                        // Measure the pair (+ divider) so we can pad up to the bar's centre.
                        let body = egui::TextStyle::Body.resolve(ui.style());
                        let text_w = |s: &str| {
                            ui.painter()
                                .layout_no_wrap(s.to_owned(), body.clone(), egui::Color32::WHITE)
                                .rect
                                .width()
                        };
                        // A vertical separator is `spacing` wide (default 6px) + a gap each side.
                        let group_w = text_w(&bf_text) + (6.0 + 2.0 * gap) + text_w(&gb_text);
                        let pad = (ui.max_rect().center().x - group_w / 2.0) - ui.cursor().min.x;
                        if pad > gap {
                            ui.add_space(pad);
                        }
                        let bf_resp = ui.colored_label(bf_color, bf_text);
                        if bf_resp.hovered() {
                            show_center_tooltip(
                                ui,
                                bf_resp.rect,
                                format!("{} — {}", tr("Backfire"), bf_word),
                            );
                        }
                        ui.separator();
                        let gb_resp = ui.colored_label(gb_color, gb_text);
                        if gb_resp.hovered() {
                            show_center_tooltip(
                                ui,
                                gb_resp.rect,
                                format!("{} — {}", tr("Automatic Gearbox"), gb_word),
                            );
                        }
                    } else {
                        // Icon-only: two fixed 22px boxes, no divider, ink-centred glyphs.
                        let box_w = 22.0;
                        let group_w = box_w * 2.0 + gap;
                        let pad = (ui.max_rect().center().x - group_w / 2.0) - ui.cursor().min.x;
                        if pad > gap {
                            ui.add_space(pad);
                        }
                        let font = egui::FontId::proportional(14.0);
                        for (icon, color, hover) in [
                            (icons::BOLT, bf_color, format!("{} — {}", tr("Backfire"), bf_word)),
                            (icons::GEARBOX, gb_color, format!("{} — {}", tr("Automatic Gearbox"), gb_word)),
                        ] {
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(box_w, 18.0), egui::Sense::hover());
                            let pos = self
                                .icon_center_cache
                                .centered_pos(ui, icon, font.clone(), rect.center());
                            ui.painter()
                                .text(pos, egui::Align2::LEFT_TOP, icon, font.clone(), color);
                            if resp.hovered() {
                                show_center_tooltip(ui, rect, hover);
                            }
                        }
                    }

                    // RIGHT: cog toggle
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let cog_color = if self.page_settings_open {
                            egui::Color32::from_rgb(255, 200, 60)
                        } else {
                            egui::Color32::GRAY
                        };
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(22.0, 18.0), egui::Sense::click());
                        // Ink-centre the glyph (fa-cog's layout box isn't symmetric),
                        // matching how the tab-bar icons are centred.
                        let font = egui::FontId::proportional(16.0);
                        let pos = self
                            .icon_center_cache
                            .centered_pos(ui, icons::COG, font.clone(), rect.center());
                        ui.painter()
                            .text(pos, egui::Align2::LEFT_TOP, icons::COG, font, cog_color);
                        if resp.clicked() {
                            self.page_settings_open = !self.page_settings_open;
                            self.page_settings_tab = PageSettingsTab::Tab(self.current_tab);
                            if !self.page_settings_open {
                                self.config.save();
                            }
                        }
                        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    });
                });
            });

        // ── Page settings floating window ──────────────────────────
        if self.page_settings_open {
            let opacity = self.page_settings_opacity;
            let win_resp = egui::Window::new("page_settings_win")
                .title_bar(false)
                .resizable(false)
                .fixed_size([650.0, 650.0])
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -36.0))
                .frame(egui::Frame::window(&ctx.style()).multiply_with_opacity(opacity))
                .show(ctx, |ui| {
                    ui.set_opacity(opacity);
                    use crate::config::{SpeedDeltaMode, SprintType, TextAlign};
                    use crate::icons;
                    use crate::i18n::tr;

                    // Main tab row (no Settings tab here). General is a global page.
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.page_settings_tab, PageSettingsTab::General, tr("General"));
                        for (tab, lbl) in [
                            (Tab::Dashboard,    "Dashboard"),
                            (Tab::Backfire,     "Backfire"),
                            (Tab::Gearbox,      "Gearbox"),
                            (Tab::PowerCurve,   "Power Graph"),
                            (Tab::EngineSwaps,  "Engines"),
                        ] {
                            ui.selectable_value(&mut self.page_settings_tab, PageSettingsTab::Tab(tab), tr(lbl));
                        }
                    });
                    ui.separator();

                    ui.set_min_height(590.0);

                    match self.page_settings_tab {
                        PageSettingsTab::General => {
                            use crate::config::TopBarStyle;
                            ui.horizontal(|ui| {
                                ui.label(tr("Top Bar Style"));
                                egui::ComboBox::from_id_salt("top_bar_style")
                                    .selected_text(match self.config.top_bar_style {
                                        TopBarStyle::Modern => tr("Modern"),
                                        TopBarStyle::Simple => tr("Simple"),
                                        TopBarStyle::Legacy => tr("Legacy"),
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.config.top_bar_style, TopBarStyle::Modern, tr("Modern"));
                                        ui.selectable_value(&mut self.config.top_bar_style, TopBarStyle::Simple, tr("Simple"));
                                        ui.selectable_value(&mut self.config.top_bar_style, TopBarStyle::Legacy, tr("Legacy"));
                                    });
                            });
                            if self.config.top_bar_style == TopBarStyle::Modern {
                                crate::theme::styled_checkbox(ui, &mut self.config.modern_show_pill, tr("Show current tab pill"));
                            }
                            if matches!(self.config.top_bar_style, TopBarStyle::Modern | TopBarStyle::Simple) {
                                crate::theme::styled_checkbox(ui, &mut self.config.high_contrast_icons, tr("High contrast icons"));
                            }
                            crate::theme::styled_checkbox(ui, &mut self.config.status_bar_show_text, tr("Status bar: show text labels"));
                            crate::theme::styled_checkbox(ui, &mut self.config.minisettings_transparent, tr("Mini-settings fade when not hovered"));
                        }
                        PageSettingsTab::Tab(Tab::Dashboard) => {
                            // Sub-tab row (wraps onto extra lines when space runs out)
                            ui.horizontal_wrapped(|ui| {
                                for (sub, lbl) in [
                                    (DashboardSubTab::General,     "General"),
                                    (DashboardSubTab::Modules,     "Modules"),
                                    (DashboardSubTab::Kmh,         "Km/h"),
                                    (DashboardSubTab::Gear,        "Gear"),
                                    (DashboardSubTab::Rpm,         "RPM"),
                                    (DashboardSubTab::SprintTimes, "Sprint"),
                                    (DashboardSubTab::Tires,       "Tires"),
                                    (DashboardSubTab::Suspension,  "Suspension"),
                                    (DashboardSubTab::Shift,       "Shift"),
                                    (DashboardSubTab::Engine,      "Engine"),
                                    (DashboardSubTab::GForce,      "G-Force"),
                                    (DashboardSubTab::Inputs,      "Inputs"),
                                    (DashboardSubTab::Boost,       "Boost"),
                                    (DashboardSubTab::Graphs,      "Power Graph"),
                                    (DashboardSubTab::MiniMap,     "Map"),
                                ] {
                                    ui.selectable_value(&mut self.page_dashboard_sub_tab, sub, tr(lbl));
                                }
                            });
                            ui.separator();
                            ui.add_space(8.0);

                            match self.page_dashboard_sub_tab {
                                DashboardSubTab::General => {
                                    // Edit Mode toggle
                                    let active = self.config.dashboard_edit_mode;
                                    let btn = ui.add(
                                        egui::Button::new(
                                            egui::RichText::new(format!("{}  {}", crate::icons::PENCIL, tr("Edit Mode")))
                                                .color(if active {
                                                    egui::Color32::from_rgb(80, 150, 255)
                                                } else {
                                                    egui::Color32::from_gray(190)
                                                }),
                                        )
                                        .fill(if active {
                                            egui::Color32::from_rgba_premultiplied(30, 70, 180, 70)
                                        } else {
                                            egui::Color32::from_rgb(60, 60, 60)
                                        }),
                                    );
                                    if btn.clicked() {
                                        self.config.dashboard_edit_mode = !self.config.dashboard_edit_mode;
                                    }
                                    ui.add_space(8.0);

                                    ui.label(tr("Grid columns"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.grid_cols, 1..=40_usize),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(tr("Grid rows"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.grid_rows, 1..=40_usize),
                                    );
                                    ui.add_space(4.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config.dashboard_show_grid, tr("Show grid"));
                                    crate::theme::styled_checkbox(ui, &mut self.config.dashboard_show_outlines, tr("Show widget outlines"));
                                    crate::theme::styled_checkbox(ui, &mut self.config.hide_widget_titles, tr("Hide widget titles"));
                                    ui.label(
                                        egui::RichText::new(tr("Hide every widget's title row so the content gets the space."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                    ui.add_space(8.0);
                                    if ui.button(tr("Reset Layout")).clicked() {
                                        self.config.dashboard_widgets =
                                            crate::config::default_widget_layout();
                                        self.config.save();
                                    }
                                }
                                DashboardSubTab::Modules => {
                                    use crate::config::WidgetKind;
                                    ui.label(
                                        egui::RichText::new(tr("Right-click a module to reset its position (auto-placed)."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                    ui.add_space(4.0);
                                    for kind in [
                                        WidgetKind::Speed, WidgetKind::Gear, WidgetKind::Rpm,
                                        WidgetKind::Inputs, WidgetKind::Car, WidgetKind::Engine,
                                        WidgetKind::Position, WidgetKind::Race,
                                        WidgetKind::Tires, WidgetKind::GForce, WidgetKind::Suspension,
                                        WidgetKind::MiniMap, WidgetKind::CoopPlayers, WidgetKind::Trace,
                                        WidgetKind::Boost, WidgetKind::SessionStats,
                                        WidgetKind::PowerGraph, WidgetKind::BoostGraph,
                                    ] {
                                        let mut enabled = !self.config.disabled_modules.contains(&kind);
                                        let resp = crate::theme::styled_checkbox(ui, &mut enabled, kind.label());
                                        if resp.secondary_clicked() {
                                            crate::config::park_widget(&mut self.config.dashboard_widgets, &kind);
                                            self.config.save();
                                        }
                                        if resp.changed() {
                                            if enabled {
                                                self.config.disabled_modules.retain(|k| k != &kind);
                                                if kind == WidgetKind::MiniMap
                                                    && self.minimap_texture.is_none()
                                                    && self.minimap_img_receiver.is_none()
                                                {
                                                    let (tx, rx) = mpsc::channel::<MapLoadMessage>();
                                                    let s = current_season();
                                                    let q = self.config.minimap_quality;
                                                    std::thread::spawn(move || { map_load_thread(s, q, tx); });
                                                    self.minimap_img_receiver = Some(rx);
                                                    self.minimap_loaded_season = s;
                                                }
                                            } else {
                                                if !self.config.disabled_modules.contains(&kind) {
                                                    self.config.disabled_modules.push(kind.clone());
                                                }
                                                if kind == WidgetKind::MiniMap {
                                                    self.minimap_texture = None;
                                                    self.minimap_img_receiver = None;
                                                }
                                            }
                                        }
                                    }
                                }
                                DashboardSubTab::Kmh => {
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Alignment"));
                                        egui::ComboBox::from_id_salt("speed_align")
                                            .selected_text(match self.config.speed_align {
                                                TextAlign::Right            => tr("Right"),
                                                TextAlign::Center           => tr("Center"),
                                                TextAlign::RightPlaceholder => tr("Right w/ Placeholder"),
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.speed_align, TextAlign::Right,            tr("Right"));
                                                ui.selectable_value(&mut self.config.speed_align, TextAlign::Center,           tr("Center"));
                                                ui.selectable_value(&mut self.config.speed_align, TextAlign::RightPlaceholder, tr("Right w/ Placeholder"));
                                            });
                                    });
                                    ui.add_space(8.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config.show_speed_delta, tr("Show Accel/Decel Tracker"));
                                    if self.config.show_speed_delta {
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.label(tr("Mode"));
                                            egui::ComboBox::from_id_salt("speed_delta_mode")
                                                .selected_text(match self.config.speed_delta_mode {
                                                    SpeedDeltaMode::Track     => tr("Track (1s comparison)"),
                                                    SpeedDeltaMode::Calculate => tr("Calculate (frame-to-frame)"),
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut self.config.speed_delta_mode, SpeedDeltaMode::Track,     tr("Track (1s comparison)"));
                                                    ui.selectable_value(&mut self.config.speed_delta_mode, SpeedDeltaMode::Calculate, tr("Calculate (frame-to-frame)"));
                                                });
                                        });
                                    }
                                }
                                DashboardSubTab::Gear => {
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Alignment"));
                                        egui::ComboBox::from_id_salt("gear_align")
                                            .selected_text(match self.config.gear_align {
                                                TextAlign::Right | TextAlign::RightPlaceholder => tr("Right"),
                                                TextAlign::Center => tr("Center"),
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.gear_align, TextAlign::Right,  tr("Right"));
                                                ui.selectable_value(&mut self.config.gear_align, TextAlign::Center, tr("Center"));
                                            });
                                    });
                                }
                                DashboardSubTab::SprintTimes => {
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Type"));
                                        egui::ComboBox::from_id_salt("sprint_type")
                                            .selected_text(match self.config.sprint_type {
                                                SprintType::Incremental => tr("Incremental (segment times)"),
                                                SprintType::Absolute    => tr("Absolute (0 to X times)"),
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.sprint_type, SprintType::Incremental, tr("Incremental (segment times)"));
                                                ui.selectable_value(&mut self.config.sprint_type, SprintType::Absolute,    tr("Absolute (0 to X times)"));
                                            });
                                    });
                                    ui.add_space(8.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config.sprint_show_other,
                                        tr("Show other type in parentheses"));
                                }
                                DashboardSubTab::Tires => {
                                    use crate::config::TireDisplayStyle;
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Style"));
                                        egui::ComboBox::from_id_salt("tire_display_style")
                                            .selected_text(match self.config.tire_display_style {
                                                TireDisplayStyle::Tires => tr("Tires"),
                                                TireDisplayStyle::Bars  => tr("Bars"),
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.tire_display_style, TireDisplayStyle::Tires, tr("Tires"));
                                                ui.selectable_value(&mut self.config.tire_display_style, TireDisplayStyle::Bars,  tr("Bars"));
                                            });
                                    });
                                    if self.config.tire_display_style == TireDisplayStyle::Bars {
                                        use crate::config::TireBarValue;
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label(tr("Bar Display Value"));
                                            egui::ComboBox::from_id_salt("tire_bar_value")
                                                .selected_text(match self.config.tire_bar_value {
                                                    TireBarValue::Temperature => tr("Temperature"),
                                                    TireBarValue::Slip        => tr("Slip"),
                                                    TireBarValue::Combined    => tr("Combined"),
                                                    TireBarValue::Stacked     => tr("Stacked"),
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut self.config.tire_bar_value, TireBarValue::Temperature, tr("Temperature"));
                                                    ui.selectable_value(&mut self.config.tire_bar_value, TireBarValue::Slip,        tr("Slip"));
                                                    ui.selectable_value(&mut self.config.tire_bar_value, TireBarValue::Combined,    tr("Combined"));
                                                    ui.selectable_value(&mut self.config.tire_bar_value, TireBarValue::Stacked,     tr("Stacked"));
                                                });
                                        });
                                        if matches!(self.config.tire_bar_value, TireBarValue::Combined | TireBarValue::Stacked) {
                                            crate::theme::styled_checkbox(ui, &mut self.config.tire_bar_swap, tr("Switch Values"));
                                            ui.label(
                                                egui::RichText::new(tr("Swaps temp and slip in the bars only; the text rows stay put."))
                                                    .size(11.0)
                                                    .color(egui::Color32::GRAY),
                                            );
                                        }
                                    }
                                }
                                DashboardSubTab::Suspension => {
                                    crate::theme::styled_checkbox(ui, &mut self.config.suspension_invert, tr("Invert values"));
                                    ui.label(
                                        egui::RichText::new(tr("Show suspension height (extension up) instead of raw compression."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                                DashboardSubTab::Rpm => {
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Max RPM"));
                                        egui::ComboBox::from_id_salt("page_max_rpm_mode_combo")
                                            .selected_text(self.config.max_rpm_mode.label())
                                            .show_ui(ui, |ui| {
                                                for mode in [
                                                    crate::config::MaxRpmSource::GameProvided,
                                                    crate::config::MaxRpmSource::DetectDynamically,
                                                ] {
                                                    ui.selectable_value(
                                                        &mut self.config.max_rpm_mode,
                                                        mode,
                                                        mode.label(),
                                                    );
                                                }
                                            });
                                    });
                                    ui.label(
                                        egui::RichText::new(tr(
                                            "Max RPM used for the RPM widget and shift indicator.",
                                        ))
                                        .size(11.0)
                                        .color(egui::Color32::GRAY),
                                    );
                                }
                                DashboardSubTab::Shift => {
                                    ui.label(tr("Shift indicator thresholds (% of engine max RPM)"));
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Low (warn)"));
                                        ui.add(
                                            egui::Slider::new(&mut self.config.shift_low_pct, 50.0..=99.0)
                                                .suffix("%"),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(tr("High (shift)"));
                                        ui.add(
                                            egui::Slider::new(&mut self.config.shift_high_pct, 51.0..=100.0)
                                                .suffix("%"),
                                        );
                                    });
                                }
                                DashboardSubTab::Engine => {
                                    use crate::config::EngineDisplayMode as EDM;
                                    ui.label(tr("Show per line"));
                                    ui.add_space(4.0);
                                    for (mode, lbl) in [
                                        (EDM::Current, "Current values"),
                                        (EDM::Max,     "Max values"),
                                        (EDM::Both,    "Both"),
                                    ] {
                                        crate::theme::styled_radio(ui, &mut self.config.engine_display_mode, mode, tr(lbl));
                                    }
                                    ui.add_space(8.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config.engine_show_type, tr("Show engine type"));
                                    ui.label(
                                        egui::RichText::new(tr("Adds an \"Electric\" or cylinder-count caption under the values."))
                                            .size(11.0).color(egui::Color32::GRAY),
                                    );
                                }
                                DashboardSubTab::GForce => {
                                    crate::theme::styled_checkbox(ui, &mut self.config.gforce_show_text, tr("Show text"));
                                    ui.label(
                                        egui::RichText::new(tr("Current/Peak G-force readout beside the plot. Off = the plot fills the whole widget."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                    crate::theme::styled_checkbox(ui, &mut self.config.gforce_show_labels, tr("Show labels"));
                                    ui.label(
                                        egui::RichText::new(tr("Show the \"Current:\"/\"Peak:\" header rows. Off = only the value rows."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                                DashboardSubTab::Inputs => {
                                    crate::theme::styled_checkbox(ui,
                                        &mut self.config.input_bars_full_width,
                                        tr("Full-width bars"),
                                    );
                                    ui.label(
                                        egui::RichText::new(tr("Bars span the full width with the label and value drawn inside."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                    ui.add_space(6.0);
                                    crate::theme::styled_checkbox(ui,
                                        &mut self.config.input_steer_compact,
                                        tr("Compact steering"),
                                    );
                                    ui.add_space(6.0);
                                    crate::theme::styled_checkbox(ui,
                                        &mut self.config.inputs_filter_backfire_accel,
                                        tr("Filter Accel while Backfire fires"),
                                    );
                                    ui.label(
                                        egui::RichText::new(tr("Hides the fake throttle blip Backfire injects, so the Accel bar reflects only your real pedal."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                                DashboardSubTab::Boost => {
                                    crate::theme::styled_checkbox(ui,
                                        &mut self.config.boost_in_bar,
                                        tr("Value inside the bar"),
                                    );
                                    ui.label(
                                        egui::RichText::new(tr("Compact: draws the current value inside a full-width bar, with the peak in parentheses below."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                }
                                DashboardSubTab::Graphs => {
                                    crate::theme::styled_checkbox(ui, &mut self.config.power_graph_show_boost, tr("Show Boost"));
                                    ui.add_space(6.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config.power_graph_compact, tr("Compact"));
                                    ui.label(egui::RichText::new(tr("Compact style for small cells: hides title, legend and axes; peaks labelled inside the plot.")).size(11.0).color(egui::Color32::GRAY));
                                    ui.add_space(6.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config.power_graph_show_grid, tr("Show grid"));
                                    // Same capture options as the full Power Graph tab (shared config).
                                    ui.add_space(8.0);
                                    crate::ui::power_curve::options_ui(ui, &mut self.config);
                                }
                                DashboardSubTab::MiniMap => {
                                    ui.horizontal(|ui| {
                                        for (sub, lbl) in [
                                            (MiniMapTab::General, "General"),
                                            (MiniMapTab::Coop,    "Co-Op"),
                                        ] {
                                            ui.selectable_value(&mut self.page_map_sub_tab, sub, tr(lbl));
                                        }
                                    });
                                    ui.separator();
                                    ui.add_space(6.0);
                                    match self.page_map_sub_tab {
                                    MiniMapTab::General => {
                                    ui.horizontal(|ui| {
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_fps_limit_enabled, tr("Render FPS limit"));
                                        if self.config.minimap_fps_limit_enabled {
                                            ui.add(
                                                egui::Slider::new(&mut self.config.minimap_fps_limit, 5.0..=120.0)
                                                    .step_by(1.0)
                                                    .suffix(" fps"),
                                            );
                                        }
                                    });
                                    crate::theme::styled_checkbox(ui, &mut self.config.minimap_north_up, tr("Lock map north-up")).on_hover_text("F10");
                                    if !self.config.minimap_north_up {
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_north_up_when_stopped, tr("North up when stopped"));
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_smooth_rotation, tr("Smooth rotation"));
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_use_movement_dir, tr("Use movement direction as rotation"));
                                    }
                                    crate::theme::styled_checkbox(ui, &mut self.config.minimap_mirror_edges, tr("Mirror map at edges"));
                                    crate::theme::styled_checkbox(ui, &mut self.config.minimap_show_compass, tr("Show compass"));
                                    ui.add_space(4.0);
                                    ui.label(tr("Zoom when driving (radius, metres)"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.minimap_zoom_driving_m, 50.0..=3000.0)
                                            .suffix(" m"),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(tr("Zoom when stopped (radius, metres)"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.minimap_zoom_stopped_m, 500.0..=6000.0)
                                            .suffix(" m"),
                                    );
                                    ui.add_space(8.0);
                                    ui.label(tr("Image quality"));
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Slider::new(&mut self.config.minimap_quality, 20.0..=100.0)
                                                .step_by(5.0)
                                                .suffix("%"),
                                        );
                                        if ui.button(tr("Reload Map")).clicked() {
                                            let (map_tx, map_rx) = mpsc::channel::<MapLoadMessage>();
                                            let s = current_season();
                                            let q = self.config.minimap_quality;
                                            std::thread::spawn(move || { map_load_thread(s, q, map_tx); });
                                            self.minimap_texture = None;
                                            self.minimap_img_receiver = Some(map_rx);
                                            self.minimap_loaded_season = s;
                                        }
                                        if ui.button(tr("Rebuild Map Cache")).clicked() {
                                            let cache_dir = crate::config::app_data_dir().join("map_cache");
                                            let _ = std::fs::remove_dir_all(&cache_dir);
                                            let (map_tx, map_rx) = mpsc::channel::<MapLoadMessage>();
                                            let s = current_season();
                                            let q = self.config.minimap_quality;
                                            std::thread::spawn(move || { map_load_thread(s, q, map_tx); });
                                            self.minimap_texture = None;
                                            self.minimap_cache_progress = None;
                                            self.minimap_img_receiver = Some(map_rx);
                                            self.minimap_loaded_season = s;
                                        }
                                    });
                                    ui.label(
                                        egui::RichText::new(tr("100% = full resolution; lower = faster load. Cache makes repeat loads near-instant."))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                    );
                                    ui.add_space(8.0);
                                    ui.collapsing(tr("Advanced calibration"), |ui| {
                                        ui.add_space(4.0);
                                        ui.label(
                                            egui::RichText::new(tr(
                                                "Tune if the car dot is misaligned with the map.\n\
                                                 Default values are derived from in-game reference points."
                                            ))
                                            .size(11.0)
                                            .color(egui::Color32::GRAY),
                                        );
                                        ui.add_space(6.0);
                                        ui.horizontal(|ui| {
                                            ui.label(tr("Pixels per metre"));
                                            ui.add(
                                                egui::DragValue::new(&mut self.config.minimap_px_per_m)
                                                    .speed(0.001)
                                                    .range(0.01..=10.0),
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label(tr("World origin X (m at pixel 0)"));
                                            ui.add(
                                                egui::DragValue::new(&mut self.config.minimap_world_origin_x)
                                                    .speed(10.0),
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label(tr("World origin Z (m at pixel 0)"));
                                            ui.add(
                                                egui::DragValue::new(&mut self.config.minimap_world_origin_z)
                                                    .speed(10.0),
                                            );
                                        });
                                        ui.add_space(4.0);
                                        if ui.button(tr("Reset to defaults")).clicked() {
                                            let d = crate::minimap::MapCalibration::DEFAULT;
                                            self.config.minimap_px_per_m = d.px_per_m;
                                            self.config.minimap_world_origin_x = d.origin_x;
                                            self.config.minimap_world_origin_z = d.origin_z;
                                        }
                                    });
                                    }
                                    MiniMapTab::Coop => {
                                        ui.label(crate::theme::section_label(tr("Tracer fade")));
                                        ui.add_space(4.0);
                                        ui.label(tr("Fade after (time)"));
                                        ui.add(egui::Slider::new(&mut self.config.coop_trail_fade_secs, 1.0..=60.0).suffix(" s"));
                                        ui.add_space(4.0);
                                        ui.label(tr("Fade after (distance)"));
                                        ui.add(egui::Slider::new(&mut self.config.coop_trail_fade_m, 50.0..=3000.0).suffix(" m"));
                                        ui.label(
                                            egui::RichText::new(tr("Tracers fade out with whichever comes first — age or distance behind the player."))
                                                .size(11.0).color(egui::Color32::GRAY),
                                        );
                                        ui.add_space(10.0);
                                        ui.separator();
                                        ui.add_space(6.0);
                                        crate::theme::styled_checkbox(ui, &mut self.config.coop_map_playerlist, tr("Show player list on map"));
                                        ui.add_enabled_ui(self.config.coop_map_playerlist, |ui| {
                                            ui.add_space(2.0);
                                            ui.label(egui::RichText::new(tr("Columns")).size(11.0).color(egui::Color32::GRAY));
                                            crate::theme::styled_checkbox(ui, &mut self.config.coop_list_distance, tr("Distance"));
                                            crate::theme::styled_checkbox(ui, &mut self.config.coop_list_speed, tr("Speed"));
                                            crate::theme::styled_checkbox(ui, &mut self.config.coop_list_gear, tr("Gear"));
                                            crate::theme::styled_checkbox(ui, &mut self.config.coop_list_class, tr("Car class"));
                                        });
                                    }
                                    }
                                }
                            }
                        }
                        PageSettingsTab::Tab(Tab::PowerCurve) => {
                            crate::ui::power_curve::options_ui(ui, &mut self.config);
                        }
                        PageSettingsTab::Tab(Tab::Gearbox) => {
                            crate::theme::styled_checkbox(ui,
                                &mut self.config.dsg_show_debug_panel,
                                tr("Show debug panel"),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(tr(
                                    "Shows the gearbox Debug box (live decision state + shift log) \
                                     in the controls column."
                                ))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                            );
                        }
                        _ => {
                            ui.centered_and_justified(|ui| {
                                ui.label(
                                    egui::RichText::new(tr("No options for this page"))
                                        .color(egui::Color32::GRAY),
                                );
                            });
                        }
                    }
                    // Always fill the full window height
                    let rem = ui.available_height();
                    if rem > 0.0 { ui.add_space(rem); }
                    let _ = icons::COG;
                });
            let hovered = win_resp
                .map(|r| {
                    let mut rect = r.response.rect;
                    rect.set_bottom(ctx.screen_rect().bottom());
                    ctx.input(|i| {
                        i.pointer
                            .hover_pos()
                            .map(|p| rect.contains(p))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false);

            // Fade over 0.25 s: range is 0.5 units, rate = 0.5 / 0.25 s = 2.0 /s
            let target = if hovered || !self.config.minisettings_transparent { 1.0_f32 } else { 0.5_f32 };
            let dt = ctx.input(|i| i.unstable_dt).min(0.1);
            let diff = target - self.page_settings_opacity;
            let step = 2.0_f32 * dt;
            self.page_settings_opacity = if diff.abs() <= step {
                target
            } else {
                self.page_settings_opacity + diff.signum() * step
            };
            if (self.page_settings_opacity - target).abs() > 0.001 {
                ctx.request_repaint();
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| match self.current_tab {
            Tab::Dashboard => crate::ui::dashboard::show(ui, self),
            Tab::Overlay => crate::ui::overlay_tab::show(ui, self),
            Tab::Backfire => crate::ui::backfire::show_backfire(ui, self),
            Tab::Gearbox => crate::ui::gearbox::show_gearbox(ui, self),
            Tab::PowerCurve => crate::ui::power_curve::show(ui, self),
            Tab::EngineSwaps => crate::ui::engine_swaps::show(ui, self),
            Tab::Coop => crate::ui::coop::show(ui, self),
            Tab::Settings => crate::ui::settings::show(ui, self),
            Tab::Changelog => crate::ui::changelog::show(ui, self),
            Tab::Debug => crate::ui::debug_tab::show(ui, self),
        });

        // Hand the listener thread this frame's config plus the two focus facts only egui
        // knows (the global-hotkey gate needs them). Pushed unconditionally — it's one
        // AppConfig clone per frame, exactly what `drain_packets` used to do for `fun_cfg`,
        // and repeating it means a push the thread was busy for simply lands next frame.
        self.listener.push(
            &self.config,
            self.last_toggle_gen,
            ctx.input(|i| i.focused),
            // A rebind capture counts as text input: the key being bound mustn't also fire
            // its current global action (G toggling the gearbox, H hiding the HUD).
            ctx.wants_keyboard_input() || self.rebinding.is_some(),
        );

        // FPS limiter
        if self.config.fps_limit_enabled {
            ctx.request_repaint_after(Duration::from_secs_f32(1.0 / self.config.fps_limit));
        } else {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Pick up a hotkey toggle we may never have been drawn to see (quitting from a
        // minimized window), so the saved config matches what the user last pressed. Unlike
        // the per-frame sync this one blocks: there is no next frame to retry on, and the
        // listener holds that lock only for a memcpy.
        if let Some(view) = self.listener.view_now() {
            self.adopt_listener_view(view);
        }
        self.config.save();
        // The listener thread owns the per-car calibrations — let it flush them and stop.
        self.listener.shutdown();
        // Blocking this time: the join tears the layer surface down cleanly before exit.
        #[cfg(target_os = "linux")]
        if let Some(h) = self.overlay.handle.take() {
            self.focus.set_output_sink(None);
            drop(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::trace_step;

    #[test]
    fn trace_step_first_sample_starts_at_current_axis_time() {
        assert_eq!(trace_step(None, 0.0), Some(0.0));
        assert_eq!(trace_step(None, 12.5), Some(12.5));
    }

    #[test]
    fn trace_step_throttles_to_25_hz() {
        assert_eq!(trace_step(Some(0.01), 5.0), None);
        assert_eq!(trace_step(Some(0.039), 5.0), None);
        assert_eq!(trace_step(Some(0.05), 5.0), Some(5.05));
    }

    #[test]
    fn trace_step_clamps_pause_gap_to_one_frame() {
        // A 2-minute pause advances the active axis by at most 0.1 s, so the
        // resumed line appends seamlessly instead of jumping.
        assert_eq!(trace_step(Some(120.0), 30.0), Some(30.1));
    }
}

#[cfg(test)]
mod hotkey_tests {
    use super::global_hotkey_allowed;

    #[test]
    fn fires_when_app_focused_and_not_typing() {
        assert!(global_hotkey_allowed(true, false, false));
    }
    #[test]
    fn blocked_when_app_focused_but_typing() {
        assert!(!global_hotkey_allowed(true, true, false));
    }
    #[test]
    fn fires_when_not_ours_but_game_focused() {
        assert!(global_hotkey_allowed(false, false, true));
    }
    #[test]
    fn blocked_when_third_app_focused() {
        assert!(!global_hotkey_allowed(false, false, false));
    }
}
