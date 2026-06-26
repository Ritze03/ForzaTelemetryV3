use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Theme {
    Dark,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum GameMode {
    ForzaHorizon6,
    ForzaMotorsport7,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Default)]
pub enum MaxRpmSource {
    GameProvided,
    #[default]
    DetectDynamically,
}

impl MaxRpmSource {
    pub fn label(&self) -> &'static str {
        match self {
            MaxRpmSource::GameProvided => "Game Data",
            MaxRpmSource::DetectDynamically => "Auto Detect",
        }
    }
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
        match self {
            GearboxMode::Street => "Street",
            GearboxMode::Sport  => "Sport",
            GearboxMode::Race   => "Race",
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
pub enum TireSlipStyle {
    #[default]
    Values,
    Graph,
    Both,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub enum TireDisplayStyle {
    #[default]
    Separate,
    Combined,
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
}

impl WidgetKind {
    pub fn label(&self) -> &'static str {
        match self {
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
        }
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
    pub game_mode: GameMode,
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
    pub tire_slip_style: TireSlipStyle,
    // Shift indicator (global, % of engine_max_rpm)
    pub shift_low_pct: f32,
    pub shift_high_pct: f32,
    // Power curve
    pub power_curve_forced_induction: bool,
    pub power_curve_save_fi_state: bool,
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
    pub backfire_test_mode: bool,
    pub backfire_disable_standstill: bool,
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
    pub dsg_downshift_powerband_buffer_pct: f32, // extra headroom (% of the inter-gear RPM jump) required below redline to downshift
    pub dsg_kickdown_powerband_buffer_pct: f32, // same, but for full-throttle kickdowns (usually smaller = drops deeper)
    pub dsg_debug: bool,
    pub dsg_log_shifts: bool, // append each shift (pre/post RPM + speed, inputs) to a CSV for analysis
    // Max-RPM source for the dashboard RPM widget
    pub max_rpm_mode: MaxRpmSource,
    // Acceleration / deceleration test parameters
    pub accel_start_kmh: f32,
    pub accel_end_kmh: f32,
    pub decel_start_kmh: f32,
    pub decel_end_kmh: f32,
    pub decel_dynamic_mode: bool,
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
            game_mode: GameMode::ForzaHorizon6,
            speed_align: TextAlign::RightPlaceholder,
            gear_align: TextAlign::Center,
            show_speed_delta: true,
            speed_delta_mode: SpeedDeltaMode::Calculate,
            sprint_type: SprintType::Absolute,
            sprint_show_other: true,
            tire_display_style: TireDisplayStyle::Combined,
            tire_slip_style: TireSlipStyle::Both,
            shift_low_pct: 85.0,
            shift_high_pct: 95.0,
            power_curve_forced_induction: true,
            power_curve_save_fi_state: true,
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
            backfire_test_mode: false,
            backfire_disable_standstill: true,
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
            dsg_downshift_powerband_buffer_pct: 20.0,
            dsg_kickdown_powerband_buffer_pct: 0.0,
            dsg_debug: false,
            dsg_log_shifts: false,
            max_rpm_mode: MaxRpmSource::GameProvided,
            accel_start_kmh: 0.0,
            accel_end_kmh: 100.0,
            decel_start_kmh: 100.0,
            decel_end_kmh: 0.0,
            decel_dynamic_mode: false,
        }
    }
}

pub fn inject_missing_widget_kinds(widgets: &mut Vec<WidgetLayout>) {
    // Widget kinds that should always exist in the layout (skip Empty).
    // If a kind is absent from the saved list, append it off to the side so
    // the user can drag it into place from edit mode.
    let all_kinds = [
        WidgetKind::Speed, WidgetKind::Gear, WidgetKind::Rpm,
        WidgetKind::Inputs, WidgetKind::Car, WidgetKind::Engine,
        WidgetKind::Position, WidgetKind::Race,
        WidgetKind::Tires, WidgetKind::GForce, WidgetKind::Suspension,
        WidgetKind::MiniMap,
    ];
    // Find highest row used so we can park new widgets below everything.
    let max_row = widgets.iter().map(|w| w.row + w.row_span).max().unwrap_or(0);
    let mut col_cursor = 0usize;
    for kind in all_kinds {
        if !widgets.iter().any(|w| w.kind == kind) {
            widgets.push(WidgetLayout {
                kind,
                col: col_cursor,
                row: max_row,
                col_span: 2,
                row_span: 2,
            });
            col_cursor += 2;
        }
    }
}

// ── Dashboard preset (dashboard fields only) ───────────────────────

#[derive(Deserialize, Default)]
pub struct DashboardPreset {
    pub grid_cols:                 Option<usize>,
    pub grid_rows:                 Option<usize>,
    pub dashboard_widgets:         Option<Vec<WidgetLayout>>,
    pub dashboard_edit_mode:       Option<bool>,
    pub dashboard_show_grid:       Option<bool>,
    pub dashboard_show_outlines:   Option<bool>,
    pub minimap_fps_limit:         Option<f32>,
    pub minimap_fps_limit_enabled: Option<bool>,
    pub disabled_modules:          Option<Vec<WidgetKind>>,
}

impl DashboardPreset {
    pub fn apply_to(&self, cfg: &mut AppConfig) {
        if let Some(v) = self.grid_cols               { cfg.grid_cols = v; }
        if let Some(v) = self.grid_rows               { cfg.grid_rows = v; }
        if let Some(ref v) = self.dashboard_widgets {
            cfg.dashboard_widgets = v.clone();
            inject_missing_widget_kinds(&mut cfg.dashboard_widgets);
        }
        if let Some(v) = self.dashboard_edit_mode     { cfg.dashboard_edit_mode = v; }
        if let Some(v) = self.dashboard_show_grid     { cfg.dashboard_show_grid = v; }
        if let Some(v) = self.dashboard_show_outlines { cfg.dashboard_show_outlines = v; }
        if let Some(v) = self.minimap_fps_limit       { cfg.minimap_fps_limit = v; }
        if let Some(v) = self.minimap_fps_limit_enabled { cfg.minimap_fps_limit_enabled = v; }
        if let Some(ref v) = self.disabled_modules    { cfg.disabled_modules = v.clone(); }
    }
}

// ──────────────────────────────────────────────────────────────────

impl AppConfig {
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
        let Ok(data) = std::fs::read_to_string(Self::path()) else { return default; };
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
        }
        let mut cfg: AppConfig = serde_json::from_value(val).unwrap_or(default);
        // Ensure every widget kind has at least one entry in the layout.
        // New kinds added to WidgetKind won't appear in old saved configs otherwise.
        inject_missing_widget_kinds(&mut cfg.dashboard_widgets);
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
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ForzaTelemetryV3")
}
