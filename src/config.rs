use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::Language;

/// The "getting started" config used on a fresh install (no `config.json` yet):
/// a snapshot of a well-rounded real-world setup. Personal/session Co-Op fields
/// (`coop_name`, `coop_hue`, `coop_last_code`) were reset to neutral defaults in
/// the snapshot. Loaded via the same merge path as an on-disk config, so any
/// field added after the snapshot simply fills from the code `Default`.
const DEFAULT_CONFIG_JSON: &str = include_str!("../assets/default-config.json");

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Theme {
    Dark,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum MaxRpmSource {
    GameProvided,
    #[default]
    DetectDynamically,
}

impl MaxRpmSource {
    pub fn label(&self) -> &'static str {
        crate::i18n::tr(match self {
            MaxRpmSource::GameProvided => "Game Data",
            MaxRpmSource::DetectDynamically => "Auto Detect",
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum TopBarStyle {
    #[default]
    Modern, // wordmark + current-page pill on the left, icon tabs centered
    Simple, // icon-only tabs
    Legacy, // full labelled buttons
}


#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum BackfireDynamicMode {
    TimeBased,   // hold length estimated from packets/sec
    #[default]
    PacketBased, // hold until the next packet arrives (exact one frame)
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum GearboxMode {
    Street,
    #[default]
    Sport,
    Race,
}

impl GearboxMode {
    pub fn label(&self) -> &'static str {
        crate::i18n::tr(match self {
            GearboxMode::Street => "Street",
            GearboxMode::Sport  => "Sport",
            GearboxMode::Race   => "Race",
        })
    }
}

// ── Hotkeys ──────────────────────────────────────────────────────────────────

/// A bindable hotkey action. Scope is fixed per action (see `scope`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HotkeyAction {
    ToggleGearbox,
    ToggleBackfire,
    ResetCalibration,
    MiniSettings,
    DashboardEdit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyScope {
    Global,
    AppFocused,
}

impl HotkeyAction {
    /// Every action, in display order (globals first).
    pub const ALL: &'static [HotkeyAction] = &[
        HotkeyAction::ToggleGearbox,
        HotkeyAction::ResetCalibration,
        HotkeyAction::ToggleBackfire,
        HotkeyAction::MiniSettings,
        HotkeyAction::DashboardEdit,
    ];
    pub fn scope(self) -> HotkeyScope {
        match self {
            HotkeyAction::ToggleGearbox
            | HotkeyAction::ToggleBackfire
            | HotkeyAction::ResetCalibration => HotkeyScope::Global,
            HotkeyAction::MiniSettings | HotkeyAction::DashboardEdit => HotkeyScope::AppFocused,
        }
    }
    /// English label for the settings row.
    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::ToggleGearbox => "Toggle Automatic Gearbox",
            HotkeyAction::ToggleBackfire => "Toggle Backfire",
            HotkeyAction::ResetCalibration => "Reset Gearbox Calibration",
            HotkeyAction::MiniSettings => "Open mini-settings",
            HotkeyAction::DashboardEdit => "Toggle dashboard edit",
        }
    }
}

/// How global hotkeys decide the game is focused.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum GateMode {
    #[default]
    TelemetryLive,
    WindowFocus,
}

/// Which active-window query the WindowFocus gate uses (Linux). Windows always
/// uses GetForegroundWindow and ignores this.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum FocusMethod {
    #[default]
    Hyprland,
    X11,
    Custom,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct HotkeyConfig {
    #[serde(default = "default_bindings")]
    pub bindings: HashMap<HotkeyAction, crate::keymap::HotkeyBinding>,
    #[serde(default)]
    pub gate_mode: GateMode,
    #[serde(default)]
    pub focus_method: FocusMethod,
    #[serde(default)]
    pub custom_cmd: String,
    #[serde(default = "default_game_match")]
    pub game_match: String,
    #[serde(default)]
    pub input_focus_gate: bool,
    #[serde(default = "default_poll_hz")]
    pub focus_poll_hz: f32,
}

fn default_game_match() -> String { "Forza".to_string() }
fn default_poll_hz() -> f32 { 4.0 }

/// Ensure every `HotkeyAction` has a binding, filling any missing from defaults — so an
/// older config (or a newly-added action) stays usable without a manual rebind.
pub fn inject_missing_hotkeys(hk: &mut HotkeyConfig) {
    let defaults = default_bindings();
    for action in HotkeyAction::ALL.iter().copied() {
        hk.bindings.entry(action).or_insert_with(|| defaults[&action]);
    }
}

fn default_bindings() -> HashMap<HotkeyAction, crate::keymap::HotkeyBinding> {
    use crate::keymap::{HotKey, HotkeyBinding, Mods};
    let mut m = HashMap::new();
    m.insert(HotkeyAction::ToggleGearbox, HotkeyBinding { mods: Mods::default(), key: HotKey::G });
    m.insert(HotkeyAction::ToggleBackfire, HotkeyBinding { mods: Mods::default(), key: HotKey::B });
    m.insert(HotkeyAction::ResetCalibration, HotkeyBinding { mods: Mods::default(), key: HotKey::F });
    m.insert(HotkeyAction::MiniSettings, HotkeyBinding { mods: Mods { ctrl: true, ..Default::default() }, key: HotKey::S });
    m.insert(HotkeyAction::DashboardEdit, HotkeyBinding { mods: Mods { ctrl: true, ..Default::default() }, key: HotKey::E });
    m
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
            gate_mode: GateMode::default(),
            focus_method: FocusMethod::default(),
            custom_cmd: String::new(),
            game_match: default_game_match(),
            input_focus_gate: false,
            focus_poll_hz: default_poll_hz(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
pub struct GearboxTuning {
    pub cruise_rpm_pct: f32,      // low-throttle target, % of Shift RPM
    pub accel_gamma: f32,         // accelerator response curve: effective = raw^gamma (1.0 = linear)
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum TextAlign {
    #[default]
    Right,
    Center,
    RightPlaceholder,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum SpeedDeltaMode {
    #[default]
    Track,
    Calculate,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum SprintType {
    #[default]
    Incremental,
    Absolute,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum TireDisplayStyle {
    #[default]
    Tires,
    Bars,
}

/// Which values the Engine widget shows per line.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum EngineDisplayMode {
    Current,
    Max,
    #[default]
    Both,
}

/// What the "Bars" tire style visualizes in each corner's bar.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum TireBarValue {
    #[default]
    Temperature,
    Slip,
    /// Two bars side by side (temp + slip).
    Combined,
    /// One bar split at the vertical middle: one value fills up, the other down.
    Stacked,
}

// ── Widget grid system ─────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum WidgetKind {
    #[default]
    Empty,
    Speed,
    Gear,
    Rpm,
    Inputs,
    Car,
    Engine,
    Position,
    Race,
    Tires,
    GForce,
    Suspension,
    MiniMap,
    CoopPlayers,
    Trace,
    Boost,
    SessionStats,
    PowerGraph,
    BoostGraph,
}

impl WidgetKind {
    pub fn label(&self) -> &'static str {
        crate::i18n::tr(match self {
            WidgetKind::Empty      => "Empty",
            WidgetKind::Speed      => "Speed",
            WidgetKind::Gear       => "Gear",
            WidgetKind::Rpm        => "RPM",
            WidgetKind::Inputs     => "Inputs",
            WidgetKind::Car        => "Car",
            WidgetKind::Engine     => "Engine",
            WidgetKind::Position   => "Position",
            WidgetKind::Race       => "Race / Sprint",
            WidgetKind::Tires      => "Tires",
            WidgetKind::GForce     => "G-Forces",
            WidgetKind::Suspension => "Suspension",
            WidgetKind::MiniMap    => "Map",
            WidgetKind::CoopPlayers => "Co-Op Players",
            WidgetKind::Trace       => "Speed Trace",
            WidgetKind::Boost       => "Boost",
            WidgetKind::SessionStats => "Session Stats",
            WidgetKind::PowerGraph => "Power Graph",
            WidgetKind::BoostGraph => "Boost Graph",
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WidgetLayout {
    pub kind: WidgetKind,
    pub col: usize,
    pub row: usize,
    pub col_span: usize,
    pub row_span: usize,
}

pub fn default_widget_layout() -> Vec<WidgetLayout> {
    vec![
        WidgetLayout { kind: WidgetKind::Speed,      col:  0, row: 0, col_span:  1, row_span: 1 },
        WidgetLayout { kind: WidgetKind::Gear,       col:  1, row: 0, col_span:  1, row_span: 1 },
        WidgetLayout { kind: WidgetKind::Rpm,        col:  2, row: 0, col_span: 14, row_span: 1 },
        WidgetLayout { kind: WidgetKind::Suspension, col:  0, row: 1, col_span:  3, row_span: 3 },
        WidgetLayout { kind: WidgetKind::Tires,      col:  3, row: 1, col_span:  9, row_span: 3 },
        WidgetLayout { kind: WidgetKind::Car,        col: 12, row: 1, col_span:  4, row_span: 2 },
        WidgetLayout { kind: WidgetKind::Engine,     col: 12, row: 3, col_span:  4, row_span: 1 },
        WidgetLayout { kind: WidgetKind::Inputs,     col:  0, row: 4, col_span:  3, row_span: 2 },
        WidgetLayout { kind: WidgetKind::MiniMap,    col:  3, row: 4, col_span: 13, row_span: 6 },
        WidgetLayout { kind: WidgetKind::Race,       col:  0, row: 6, col_span:  3, row_span: 2 },
        WidgetLayout { kind: WidgetKind::GForce,     col:  0, row: 8, col_span:  3, row_span: 2 },
    ]
}

// ──────────────────────────────────────────────────────────────────

/// Saved DSG calibration for one car. The full CarOrdinal → CarCalibration map lives in its
/// own file (`automatic-gearbox-saved-calibrations.json`), separate from config.json.
#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct CarCalibration {
    pub gear_redline_speeds: [f32; 11],
    pub max_rpm: f32,
}

fn car_calibrations_path() -> PathBuf {
    app_data_dir().join("automatic-gearbox-saved-calibrations.json")
}

pub fn load_car_calibrations() -> HashMap<i32, CarCalibration> {
    std::fs::read_to_string(car_calibrations_path())
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

pub fn save_car_calibrations(map: &HashMap<i32, CarCalibration>) {
    let path = car_calibrations_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if let Ok(data) = serde_json::to_string_pretty(map) {
        std::fs::write(&path, data).ok();
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub listen_port: u16,
    pub fps_limit: f32,
    pub use_mph: bool,
    pub use_fahrenheit: bool,
    pub use_bar: bool,
    pub theme: Theme,
    pub always_on_top: bool,
    pub surface_rumble_max: f32,
    pub power_curve_step: f32,
    // Alignment
    pub speed_align: TextAlign,
    pub gear_align: TextAlign,
    // Speed delta
    pub show_speed_delta: bool,
    pub speed_delta_mode: SpeedDeltaMode,
    // Sprint times
    pub sprint_type: SprintType,
    pub sprint_show_other: bool,
    // Tires
    pub tire_display_style: TireDisplayStyle,
    pub tire_bar_value: TireBarValue, // what the Bars style shows in the bars
    pub tire_bar_swap: bool,          // Combined/Stacked: swap temp and slip in the bars
    pub suspension_invert: bool,      // show suspension height (extension up) instead of raw compression
    // Shift indicator (global, % of engine_max_rpm)
    pub shift_low_pct: f32,
    pub shift_high_pct: f32,
    // Power curve
    pub power_curve_forced_induction: bool,
    pub power_curve_save_fi_state: bool,
    // Dashboard graph widgets
    pub power_graph_show_boost: bool, // extra boost line in the Power Graph widget
    pub power_graph_compact: bool, // compact style: no title/legend/axes, peaks annotated inside
    pub power_graph_show_grid: bool, // show x/y grid lines in the Power Graph widget
    // Dashboard widget grid
    pub grid_cols: usize,
    pub grid_rows: usize,
    pub dashboard_widgets: Vec<WidgetLayout>,
    pub dashboard_edit_mode: bool,
    pub dashboard_show_grid: bool,
    pub dashboard_show_outlines: bool,
    // Mini map calibration (world coords → image pixel transform)
    // pixel_x = (world_x - minimap_world_origin_x) * minimap_px_per_m
    // pixel_y = (minimap_world_origin_z - world_z) * minimap_px_per_m
    pub minimap_px_per_m: f32,
    pub minimap_world_origin_x: f32,
    pub minimap_world_origin_z: f32,
    // Mini map zoom (metres visible from centre to edge)
    pub minimap_zoom_driving_m: f32,
    pub minimap_zoom_stopped_m: f32,
    // Mini map image quality (20–100 %; 100 = raw image, lower = resized on load)
    pub minimap_quality: f32,
    // Mini map render FPS limit (independent of global FPS)
    pub minimap_fps_limit: f32,
    pub minimap_fps_limit_enabled: bool,
    // Mini map rotation options
    pub minimap_smooth_rotation: bool,
    pub minimap_use_movement_dir: bool,
    pub minimap_mirror_edges: bool,
    pub minimap_north_up: bool, // lock map north-up instead of heading-up
    pub minimap_north_up_when_stopped: bool, // in heading-up mode, ease to north when stopped
    pub minimap_show_compass: bool, // show the north compass on the map
    // G-Force widget
    pub gforce_show_text: bool,   // show the Current/Peak text column; off = plot fills the widget
    pub gforce_show_labels: bool, // show the "Current:"/"Peak:" header rows above the value rows
    // Global
    pub hide_widget_titles: bool, // hide every dashboard widget's title row
    pub top_bar_style: TopBarStyle, // Modern (title+pill), Simple (icon-only), or Legacy (labelled buttons)
    pub modern_show_pill: bool, // Modern bar: show the current-tab pill next to the title
    pub high_contrast_icons: bool, // draw compact tab icons white instead of the accent tone
    // Engine widget
    pub engine_display_mode: EngineDisplayMode, // Current / Max / Both values per line
    pub engine_show_type: bool,   // show an "Electric"/"N cyl" caption under the values
    // Inputs widget
    pub input_bars_full_width: bool, // full-width bars with the label + value drawn inside
    pub input_steer_compact: bool,   // compact steering: drop the "Steer" heading
    // Boost widget
    pub boost_in_bar: bool,          // compact: draw the value inside a full-width bar, peak below
    // Global FPS limiter toggle
    pub fps_limit_enabled: bool,
    // Disabled widget modules (empty = all enabled)
    pub disabled_modules: Vec<WidgetKind>,
    // Backfire
    pub backfire_enabled: bool,
    pub backfire_dynamic_rpm: bool,
    pub backfire_dynamic_min_pct: f32,  // % of engine_max_rpm for dynamic min
    pub backfire_dynamic_max_pct: f32,  // % of engine_max_rpm for dynamic max
    pub backfire_max_rpm: f32,
    pub backfire_min_rpm: f32,
    pub backfire_interval_rpm: f32,
    pub backfire_accel_time_ms: u64,
    pub backfire_dynamic_duration: bool, // key-press length = one frame (from packets/sec) instead of the fixed ms
    pub backfire_dynamic_mode: BackfireDynamicMode, // when dynamic: estimate hold from packets/sec, or hold until next packet
    pub backfire_test_mode: bool,
    pub backfire_disable_standstill: bool,
    pub inputs_filter_backfire_accel: bool, // Inputs widget shows Accel as 0 while Backfire is actively firing
    // Hotkeys (global + app-focused rebindable shortcuts + focus detection)
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
    // DSG automatic gearbox
    pub dsg_enabled: bool,
    pub dsg_shift_rpm_pct: f32,       // Max RPM ceiling: % of max_rpm (calibration + full-throttle shift point)
    pub dsg_upshift_speed_pct: f32,  // upshift only once speed reaches this % of the gear's redline speed
    pub dsg_gearbox_mode: GearboxMode,
    pub dsg_auto_race_mode: bool, // force Race mode whenever in an actual race (race position > P0)
    pub dsg_tuning_street: GearboxTuning,
    pub dsg_tuning_sport: GearboxTuning,
    pub dsg_tuning_race: GearboxTuning,
    pub dsg_kickdown_cooldown_secs: f32,
    pub dsg_downshift_deadzone_pct: f32, // hold gear while cruising until revs drop below this % of shift RPM
    pub dsg_full_throttle_pct: f32, // throttle % at/above which a non-race mode uses the full powerband (economical below)
    pub dsg_race_gear_overlap_pct: f32, // Race only: extend each gear's range downward by this many %-points of max RPM (downshift hysteresis)
    pub dsg_downshift_powerband_buffer_pct: f32, // extra headroom (% of the inter-gear RPM jump) required below redline to downshift
    pub dsg_kickdown_powerband_buffer_pct: f32, // same, but for full-throttle kickdowns (usually smaller = drops deeper)
    pub dsg_debug: bool,
    pub dsg_show_debug_panel: bool, // show the gearbox Debug box at all (toggled from the status-bar cog)
    pub dsg_log_shifts: bool, // append each shift (pre/post RPM + speed, inputs) to a CSV for analysis
    pub dsg_save_calibration: bool, // opt-in: persist per-car calibration and auto-load it (skips the manual 1st-gear pull)
    pub dsg_ignore_backfire_accel: bool, // shift logic + live throttle-bar viz ignore accel while Backfire is actively firing
    // Max-RPM source for the dashboard RPM widget
    pub max_rpm_mode: MaxRpmSource,
    // Acceleration / deceleration test parameters
    pub accel_start_kmh: f32,
    pub accel_end_kmh: f32,
    pub decel_start_kmh: f32,
    pub decel_end_kmh: f32,
    pub decel_dynamic_mode: bool,
    // UI language
    pub language: Language,
    // Co-Op
    pub coop_name: String,
    pub coop_hue: f32,        // 0..360, player marker colour
    pub coop_buffer_ms: u32,  // jitter buffer for remote players (pacing)
    pub coop_port: u16,       // local host port cloudflared points at
    pub coop_last_code: String, // last join code, prefilled next launch
    // Co-Op map: tracer fade + on-map player list
    pub coop_trail_fade_secs: f32, // trail fades out over this many seconds
    pub coop_trail_fade_m: f32,    // …and over this distance behind the player
    pub coop_map_playerlist: bool, // overlay a player list on the minimap
    pub coop_list_distance: bool,
    pub coop_list_speed: bool,
    pub coop_list_gear: bool,
    pub coop_list_class: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            listen_port: 1337,
            fps_limit: 60.0,
            use_mph: false,
            use_fahrenheit: false,
            use_bar: true,
            theme: Theme::Dark,
            always_on_top: false,
            surface_rumble_max: 3.8,
            power_curve_step: 100.0,
            speed_align: TextAlign::RightPlaceholder,
            gear_align: TextAlign::Center,
            show_speed_delta: true,
            speed_delta_mode: SpeedDeltaMode::Calculate,
            sprint_type: SprintType::Absolute,
            sprint_show_other: true,
            tire_display_style: TireDisplayStyle::Tires,
            tire_bar_value: TireBarValue::default(),
            tire_bar_swap: false,
            suspension_invert: true,
            shift_low_pct: 85.0,
            shift_high_pct: 95.0,
            power_curve_forced_induction: true,
            power_curve_save_fi_state: true,
            power_graph_show_boost: false,
            power_graph_compact: false,
            power_graph_show_grid: true,
            grid_cols: 16,
            grid_rows: 10,
            dashboard_widgets: default_widget_layout(),
            dashboard_edit_mode: false,
            dashboard_show_grid: true,
            dashboard_show_outlines: true,
            minimap_px_per_m: 0.3722,
            minimap_world_origin_x: -12540.0,
            minimap_world_origin_z: 10738.0,
            minimap_zoom_driving_m: 1500.0,
            minimap_zoom_stopped_m: 3000.0,
            minimap_quality: 100.0,
            minimap_fps_limit: 60.0,
            minimap_fps_limit_enabled: true,
            minimap_smooth_rotation: true,
            minimap_use_movement_dir: true,
            minimap_mirror_edges: true,
            minimap_north_up: false,
            minimap_north_up_when_stopped: false,
            minimap_show_compass: true,
            gforce_show_text: true,
            gforce_show_labels: true,
            hide_widget_titles: false,
            top_bar_style: TopBarStyle::Modern,
            modern_show_pill: true,
            high_contrast_icons: false,
            engine_display_mode: EngineDisplayMode::Both,
            engine_show_type: false,
            input_bars_full_width: false,
            input_steer_compact: false,
            boost_in_bar: false,
            fps_limit_enabled: false,
            disabled_modules: vec![WidgetKind::Position],
            backfire_enabled: false,
            backfire_dynamic_rpm: true,
            backfire_dynamic_min_pct: 60.0,
            backfire_dynamic_max_pct: 95.0,
            backfire_max_rpm: 8000.0,
            backfire_min_rpm: 4000.0,
            backfire_interval_rpm: 100.0,
            backfire_accel_time_ms: 8,
            backfire_dynamic_duration: true,
            backfire_dynamic_mode: BackfireDynamicMode::PacketBased,
            backfire_test_mode: false,
            backfire_disable_standstill: true,
            inputs_filter_backfire_accel: true,
            hotkeys: HotkeyConfig::default(),
            dsg_enabled: false,
            dsg_shift_rpm_pct: 98.0,
            dsg_upshift_speed_pct: 80.0,
            dsg_gearbox_mode: GearboxMode::Sport,
            dsg_auto_race_mode: true,
            dsg_tuning_street: GearboxTuning { cruise_rpm_pct: 35.0, accel_gamma: 1.0 },
            dsg_tuning_sport:  GearboxTuning { cruise_rpm_pct: 50.0, accel_gamma: 1.0 },
            dsg_tuning_race:   GearboxTuning { cruise_rpm_pct: 85.0, accel_gamma: 1.0 },
            dsg_kickdown_cooldown_secs: 5.0,
            dsg_downshift_deadzone_pct: 60.0,
            dsg_full_throttle_pct: 95.0,
            dsg_race_gear_overlap_pct: 10.0,
            dsg_downshift_powerband_buffer_pct: 20.0,
            dsg_kickdown_powerband_buffer_pct: 0.0,
            dsg_debug: false,
            dsg_show_debug_panel: false,
            dsg_log_shifts: false,
            dsg_save_calibration: false,
            dsg_ignore_backfire_accel: true,
            max_rpm_mode: MaxRpmSource::GameProvided,
            accel_start_kmh: 0.0,
            accel_end_kmh: 100.0,
            decel_start_kmh: 100.0,
            decel_end_kmh: 0.0,
            decel_dynamic_mode: false,
            language: Language::English,
            coop_name: "Player".to_string(),
            coop_hue: 205.0,
            coop_buffer_ms: 0,
            coop_port: crate::coop::DEFAULT_COOP_PORT,
            coop_last_code: String::new(),
            coop_trail_fade_secs: 10.0,
            coop_trail_fade_m: 500.0,
            coop_map_playerlist: false,
            coop_list_distance: true,
            coop_list_speed: true,
            coop_list_gear: true,
            coop_list_class: false,
        }
    }
}

/// Remove `kind`'s layout entry (if any) and re-insert it parked below
/// everything else, so the user can drag it into place from edit mode.
pub fn park_widget(widgets: &mut Vec<WidgetLayout>, kind: &WidgetKind) {
    widgets.retain(|w| w.kind != *kind);
    // Find highest row used so we can park the widget below everything.
    let max_row = widgets.iter().map(|w| w.row + w.row_span).max().unwrap_or(0);
    widgets.push(WidgetLayout {
        kind: kind.clone(),
        col: 0,
        row: max_row,
        col_span: 2,
        row_span: 2,
    });
}

pub fn inject_missing_widget_kinds(widgets: &mut Vec<WidgetLayout>) {
    // Widget kinds that should always exist in the layout (skip Empty).
    // If a kind is absent from the saved list, park it off to the side so
    // the user can drag it into place from edit mode.
    let all_kinds = [
        WidgetKind::Speed, WidgetKind::Gear, WidgetKind::Rpm,
        WidgetKind::Inputs, WidgetKind::Car, WidgetKind::Engine,
        WidgetKind::Position, WidgetKind::Race,
        WidgetKind::Tires, WidgetKind::GForce, WidgetKind::Suspension,
        WidgetKind::MiniMap, WidgetKind::CoopPlayers, WidgetKind::Trace, WidgetKind::Boost,
        WidgetKind::SessionStats,
        WidgetKind::PowerGraph, WidgetKind::BoostGraph,
    ];
    for kind in all_kinds {
        if !widgets.iter().any(|w| w.kind == kind) {
            park_widget(widgets, &kind);
        }
    }
}

// ── Dashboard preset ───────────────────────────────────────────────
// A preset is just a partial config: any AppConfig key present in the
// preset JSON overwrites the current value, everything else is left as-is.
// New minisettings fields travel automatically — no struct to maintain.
/// Bundled presets, selectable by index. Shared by the Settings page and the
/// mini-settings Config sub-tab.
pub const PRESET_NAMES: &[&str] = &["Ale (halb)", "Ritze (ganz)"];
pub const PRESET_DATA: &[&str] = &[
    include_str!("../assets/configs/ale.json"),
    include_str!("../assets/configs/ritze.json"),
];

/// Rewrite the removed `tire_display_style` values ("Separate", "Combined")
/// onto the surviving "Tires" variant so older saved configs and presets keep
/// a valid value instead of failing to deserialize.
fn migrate_tire_display_style(map: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some("Separate" | "Combined") = map.get("tire_display_style").and_then(|v| v.as_str()) {
        map.insert("tire_display_style".to_string(), serde_json::json!("Tires"));
    }
}

fn apply_preset_overlay(cfg: &mut AppConfig, mut overlay: serde_json::Value) {
    if let serde_json::Value::Object(ref mut m) = overlay {
        migrate_tire_display_style(m);
    }
    let Ok(mut base) = serde_json::to_value(&*cfg) else { return; };
    if let (
        serde_json::Value::Object(base_map),
        serde_json::Value::Object(over),
    ) = (&mut base, overlay)
    {
        for (k, v) in over { base_map.insert(k, v); }
    }
    if let Ok(new_cfg) = serde_json::from_value::<AppConfig>(base) {
        *cfg = new_cfg;
        inject_missing_widget_kinds(&mut cfg.dashboard_widgets);
    }
}

pub fn apply_preset(cfg: &mut AppConfig, preset_json: &str) {
    if let Ok(overlay) = serde_json::from_str::<serde_json::Value>(preset_json) {
        apply_preset_overlay(cfg, overlay);
    }
}

/// Dashboard-layout keys: widget placement + grid. Always part of a preset.
pub const LAYOUT_KEYS: &[&str] = &[
    "grid_cols", "grid_rows", "dashboard_widgets", "dashboard_edit_mode",
    "dashboard_show_grid", "dashboard_show_outlines", "disabled_modules",
];

/// Mini-settings (cog wheel) keys: optional part of a preset, toggled on export/import.
/// Hand-maintained — a new mini-setting that should travel gets added here.
pub const MINISETTINGS_KEYS: &[&str] = &[
    "boost_in_bar", "coop_list_class", "coop_list_distance", "coop_list_gear",
    "coop_list_speed", "coop_map_playerlist", "coop_trail_fade_m", "coop_trail_fade_secs",
    "dsg_show_debug_panel", "engine_display_mode", "engine_show_type", "gear_align",
    "gforce_show_labels", "gforce_show_text", "hide_widget_titles", "high_contrast_icons",
    "input_bars_full_width", "input_steer_compact",
    "inputs_filter_backfire_accel",
    "max_rpm_mode", "minimap_fps_limit", "minimap_fps_limit_enabled", "minimap_mirror_edges", "modern_show_pill",
    "minimap_north_up", "minimap_north_up_when_stopped", "minimap_px_per_m", "minimap_quality", "minimap_show_compass",
    "minimap_smooth_rotation", "minimap_use_movement_dir", "minimap_world_origin_x",
    "minimap_world_origin_z", "minimap_zoom_driving_m", "minimap_zoom_stopped_m",
    "power_curve_forced_induction", "power_curve_save_fi_state", "power_curve_step",
    "power_graph_compact", "power_graph_show_boost", "power_graph_show_grid", "shift_high_pct", "shift_low_pct", "show_speed_delta",
    "speed_align", "speed_delta_mode", "sprint_show_other", "sprint_type", "suspension_invert",
    "tire_bar_swap", "tire_bar_value", "tire_display_style",
];

/// Serialize the dashboard layout as pretty JSON, optionally including mini-settings.
pub fn export_preset(cfg: &AppConfig, include_minisettings: bool) -> String {
    let Ok(serde_json::Value::Object(map)) = serde_json::to_value(cfg) else { return String::new(); };
    let mut out = serde_json::Map::new();
    let keys = LAYOUT_KEYS.iter()
        .chain(if include_minisettings { MINISETTINGS_KEYS } else { &[] });
    for &k in keys {
        if let Some(v) = map.get(k) { out.insert(k.to_string(), v.clone()); }
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap_or_default()
}

/// Apply an exported JSON, optionally dropping the mini-settings so only the layout
/// is imported. Returns false if the JSON didn't parse (nothing applied).
pub fn import_preset(cfg: &mut AppConfig, json: &str, include_minisettings: bool) -> bool {
    let Ok(mut overlay) = serde_json::from_str::<serde_json::Value>(json) else { return false; };
    if !include_minisettings {
        if let serde_json::Value::Object(ref mut m) = overlay {
            for k in MINISETTINGS_KEYS { m.remove(*k); }
        }
    }
    apply_preset_overlay(cfg, overlay);
    true
}

// ──────────────────────────────────────────────────────────────────

impl AppConfig {
    /// Reset the gearbox **slider / numeric** values to the tuned baseline (Ritze's
    /// current settings). Deliberately leaves the mode dropdown, the toggles, and the
    /// per-car calibrations untouched — only sliders and numeric fields change.
    pub fn reset_gearbox_numeric(&mut self) {
        self.dsg_shift_rpm_pct = 98.0;
        self.dsg_upshift_speed_pct = 80.0;
        self.dsg_race_gear_overlap_pct = 10.0;
        self.dsg_kickdown_cooldown_secs = 5.0;
        self.dsg_downshift_deadzone_pct = 90.0;
        self.dsg_full_throttle_pct = 95.0;
        self.dsg_downshift_powerband_buffer_pct = 20.0;
        self.dsg_kickdown_powerband_buffer_pct = 30.0;
        self.dsg_tuning_street = GearboxTuning { cruise_rpm_pct: 30.0, accel_gamma: 3.0 };
        self.dsg_tuning_sport  = GearboxTuning { cruise_rpm_pct: 50.0, accel_gamma: 3.0 };
        self.dsg_tuning_race   = GearboxTuning { cruise_rpm_pct: 85.0, accel_gamma: 1.0 };
    }

    /// Tuning parameters for the currently selected gearbox mode.
    pub fn dsg_active_tuning(&self) -> GearboxTuning {
        match self.dsg_gearbox_mode {
            GearboxMode::Street => self.dsg_tuning_street,
            GearboxMode::Sport  => self.dsg_tuning_sport,
            GearboxMode::Race   => self.dsg_tuning_race,
        }
    }

    /// Mutable tuning for the currently selected mode (for the advanced sliders).
    pub fn dsg_active_tuning_mut(&mut self) -> &mut GearboxTuning {
        match self.dsg_gearbox_mode {
            GearboxMode::Street => &mut self.dsg_tuning_street,
            GearboxMode::Sport  => &mut self.dsg_tuning_sport,
            GearboxMode::Race   => &mut self.dsg_tuning_race,
        }
    }

    /// The mode the gearbox actually drives with: Race whenever we're in an actual race and the
    /// auto-switch is enabled, otherwise the manually selected mode. Race is detected from the race
    /// position (P0 = free roam, P1+ = an actual race) — `IsRaceOn` stays 1 while free-roaming too.
    pub fn dsg_effective_mode(&self, in_race: bool) -> GearboxMode {
        if self.dsg_auto_race_mode && in_race {
            GearboxMode::Race
        } else {
            self.dsg_gearbox_mode
        }
    }

    /// Tuning for the mode actually in effect (Race when auto-switched in a race).
    pub fn dsg_effective_tuning(&self, in_race: bool) -> GearboxTuning {
        match self.dsg_effective_mode(in_race) {
            GearboxMode::Street => self.dsg_tuning_street,
            GearboxMode::Sport  => self.dsg_tuning_sport,
            GearboxMode::Race   => self.dsg_tuning_race,
        }
    }

    pub fn load() -> Self {
        let default = Self::default();
        // No on-disk config yet (fresh install) → start from the embedded
        // "getting started" defaults (a real-world well-rounded config), then run
        // it through the same merge path so any newer field still fills from the
        // code default. Personal Co-Op fields (name/colour/last code) were reset
        // in the snapshot, so they use their neutral defaults. See DEFAULT_CONFIG_JSON.
        let data = std::fs::read_to_string(Self::path())
            .unwrap_or_else(|_| DEFAULT_CONFIG_JSON.to_string());
        let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&data) else { return default; };
        // Merge: fill any missing keys (e.g. newly added fields) with their default values
        // so that adding a new config field never silently resets the entire config.
        if let Ok(def_val) = serde_json::to_value(&default) {
            if let (
                serde_json::Value::Object(ref mut saved),
                serde_json::Value::Object(defaults),
            ) = (&mut val, def_val)
            {
                for (k, v) in defaults {
                    saved.entry(k).or_insert(v);
                }
            }
        }
        // If theme is the now-removed "Light" value, fall back to Dark gracefully
        if let serde_json::Value::Object(ref mut map) = val {
            if map.get("theme").and_then(|v| v.as_str()) == Some("Light") {
                map.insert("theme".to_string(), serde_json::json!("Dark"));
            }
            // Migrate the old `compact_tabs` bool to the `top_bar_style` enum so an
            // existing config keeps its look (checked → Simple, unchecked → Legacy).
            if let Some(compact) = map.get("compact_tabs").and_then(|v| v.as_bool()) {
                map.insert(
                    "top_bar_style".to_string(),
                    serde_json::json!(if compact { "Simple" } else { "Legacy" }),
                );
            }
            migrate_tire_display_style(map);
        }
        let mut cfg: AppConfig = serde_json::from_value(val).unwrap_or(default);
        // Ensure every widget kind has at least one entry in the layout.
        // New kinds added to WidgetKind won't appear in old saved configs otherwise.
        inject_missing_widget_kinds(&mut cfg.dashboard_widgets);
        inject_missing_hotkeys(&mut cfg.hotkeys);
        cfg
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Ok(data) = serde_json::to_string_pretty(self) {
            std::fs::write(&path, data).ok();
        }
    }

    fn path() -> PathBuf {
        app_data_dir().join("config.json")
    }
}


pub fn app_data_dir() -> PathBuf {
    // Test/dev override: point the whole app (config, calibrations, map cache, cloudflared)
    // at an alternate dir so a throwaway config can't clobber the real one.
    if let Some(dir) = std::env::var_os("FORZA_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ForzaTelemetryV3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_serde_roundtrips() {
        let c = AppConfig::default();
        let s = serde_json::to_string(&c).expect("serialize");
        let back: AppConfig = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back.coop_port, c.coop_port);
        assert_eq!(back.minimap_north_up, c.minimap_north_up);
        assert_eq!(back.dashboard_widgets.len(), c.dashboard_widgets.len());
    }

    #[test]
    fn old_config_missing_new_keys_deserializes_via_defaults() {
        // Mirrors the merge in AppConfig::load(): an older config lacking new fields
        // fills them from defaults rather than failing to parse.
        let saved = serde_json::json!({ "listen_port": 4321 });
        let def = serde_json::to_value(AppConfig::default()).unwrap();
        let (serde_json::Value::Object(mut m), serde_json::Value::Object(d)) =
            (saved, def) else { panic!() };
        for (k, v) in d { m.entry(k).or_insert(v); }
        let cfg: AppConfig = serde_json::from_value(serde_json::Value::Object(m)).expect("merge parse");
        assert_eq!(cfg.listen_port, 4321);       // kept from the old config
        assert_eq!(cfg.coop_port, DEFAULT_COOP_PORT_TEST); // filled from default
    }

    #[test]
    fn embedded_default_config_parses_and_resets_personal_coop() {
        // The fresh-install default must always parse through the load() merge path.
        let mut val: serde_json::Value = serde_json::from_str(DEFAULT_CONFIG_JSON).expect("embedded default is valid JSON");
        let def = serde_json::to_value(AppConfig::default()).unwrap();
        if let (serde_json::Value::Object(m), serde_json::Value::Object(d)) = (&mut val, def) {
            for (k, v) in d { m.entry(k).or_insert(v); }
        }
        let cfg: AppConfig = serde_json::from_value(val).expect("embedded default merges into AppConfig");
        // Personal Co-Op fields were reset in the snapshot.
        assert_eq!(cfg.coop_name, "Player");
        assert_eq!(cfg.coop_hue, 205.0);
        assert!(cfg.coop_last_code.is_empty());
    }

    #[test]
    fn hotkey_config_deserializes_from_empty_via_defaults() {
        let hk: HotkeyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(hk, HotkeyConfig::default());
    }

    #[test]
    fn default_hotkey_bindings_are_g_b_ctrl_s_ctrl_e() {
        use crate::keymap::{HotKey, Mods};
        let hk = HotkeyConfig::default();
        assert_eq!(hk.bindings[&HotkeyAction::ToggleGearbox].key, HotKey::G);
        assert_eq!(hk.bindings[&HotkeyAction::ToggleBackfire].key, HotKey::B);
        let mini = &hk.bindings[&HotkeyAction::MiniSettings];
        assert_eq!(mini.key, HotKey::S);
        assert_eq!(mini.mods, Mods { ctrl: true, ..Default::default() });
        assert_eq!(hk.bindings[&HotkeyAction::DashboardEdit].key, HotKey::E);
    }

    #[test]
    fn action_scopes_split_global_and_app() {
        assert_eq!(HotkeyAction::ToggleGearbox.scope(), HotkeyScope::Global);
        assert_eq!(HotkeyAction::MiniSettings.scope(), HotkeyScope::AppFocused);
    }

    // Keep the expected default in sync without importing the coop module into the test.
    const DEFAULT_COOP_PORT_TEST: u16 = 7071;

    #[test]
    fn preset_overlay_applies_and_leaves_other_keys_untouched() {
        let mut cfg = AppConfig::default();
        let listen = cfg.listen_port; // not in the preset → must survive
        let preset = r#"{ "grid_cols": 37, "minimap_north_up": true }"#;
        apply_preset(&mut cfg, preset);
        assert_eq!(cfg.grid_cols, 37);          // overwritten from preset
        assert!(cfg.minimap_north_up);          // overwritten from preset
        assert_eq!(cfg.listen_port, listen);    // untouched
        // Bad JSON is a no-op, not a reset.
        apply_preset(&mut cfg, "not json");
        assert_eq!(cfg.grid_cols, 37);
    }

    #[test]
    fn export_import_respects_minisettings_toggle() {
        let mut src = AppConfig::default();
        src.grid_cols = 33;             // layout
        src.gforce_show_text = false;   // mini-setting

        // Export without mini-settings: only layout keys, no local keys.
        let layout_only = export_preset(&src, false);
        let serde_json::Value::Object(m) = serde_json::from_str(&layout_only).unwrap() else { panic!() };
        assert!(m.contains_key("grid_cols"));
        assert!(!m.contains_key("gforce_show_text"), "mini-setting leaked into layout-only export");
        assert!(!m.contains_key("listen_port"), "local key must never export");

        // Full export, but import ignoring mini-settings: layout applies, mini-setting does not.
        let full = export_preset(&src, true);
        let mut dst = AppConfig::default();
        assert!(import_preset(&mut dst, &full, false));
        assert_eq!(dst.grid_cols, 33);          // layout imported
        assert!(dst.gforce_show_text);          // mini-setting kept at default (ignored)

        // Import including mini-settings: both travel.
        let mut dst2 = AppConfig::default();
        assert!(import_preset(&mut dst2, &full, true));
        assert!(!dst2.gforce_show_text);

        assert!(!import_preset(&mut dst2, "not json", true)); // bad JSON → false
    }

    #[test]
    fn bundled_presets_apply_cleanly() {
        // include_str! presets are never type-checked; guard that each one both
        // parses and applies (a rejected key set would silently no-op).
        for (name, data) in PRESET_NAMES.iter().zip(PRESET_DATA) {
            let mut cfg = AppConfig::default();
            assert!(import_preset(&mut cfg, data, true), "{name} failed to parse");
            // A layout key from the preset must have taken effect.
            let want: serde_json::Value = serde_json::from_str(data).unwrap();
            let want_cols = want["grid_cols"].as_u64().unwrap() as usize;
            assert_eq!(cfg.grid_cols, want_cols, "{name} did not apply (invalid value rejected?)");
        }
    }

    #[test]
    fn injected_widgets_appended_not_duplicated() {
        let mut ws = default_widget_layout();
        let before = ws.len();
        inject_missing_widget_kinds(&mut ws);
        // Every optional kind now present exactly once.
        for k in [WidgetKind::CoopPlayers, WidgetKind::Trace, WidgetKind::Boost, WidgetKind::SessionStats] {
            assert_eq!(ws.iter().filter(|w| w.kind == k).count(), 1, "optional widget present exactly once");
        }
        // Running it again is idempotent.
        let after_once = ws.len();
        inject_missing_widget_kinds(&mut ws);
        assert_eq!(ws.len(), after_once, "idempotent");
        assert!(ws.len() > before);
    }
}
