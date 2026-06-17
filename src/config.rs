use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum GameMode {
    ForzaHorizon6,
    ForzaMotorsport7,
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
        WidgetLayout { kind: WidgetKind::Car,        col: 12, row: 1, col_span:  4, row_span: 3 },
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
    // Car widget
    pub car_widget_show_position: bool,
    pub car_widget_show_rotation: bool,
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
    // Global FPS limiter toggle
    pub fps_limit_enabled: bool,
    // Disabled widget modules (empty = all enabled)
    pub disabled_modules: Vec<WidgetKind>,
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
            car_widget_show_position: true,
            car_widget_show_rotation: true,
            minimap_px_per_m: 0.3722,
            minimap_world_origin_x: -12540.0,
            minimap_world_origin_z: 10738.0,
            minimap_zoom_driving_m: 1500.0,
            minimap_zoom_stopped_m: 3000.0,
            minimap_quality: 100.0,
            minimap_fps_limit: 30.0,
            minimap_fps_limit_enabled: true,
            minimap_smooth_rotation: true,
            minimap_use_movement_dir: true,
            fps_limit_enabled: true,
            disabled_modules: vec![],
        }
    }
}

fn inject_missing_widget_kinds(widgets: &mut Vec<WidgetLayout>) {
    // Widget kinds that should always exist in the layout (skip Empty).
    // If a kind is absent from the saved list, append it off to the side so
    // the user can drag it into place from edit mode.
    let all_kinds = [
        WidgetKind::Speed, WidgetKind::Gear, WidgetKind::Rpm,
        WidgetKind::Inputs, WidgetKind::Car, WidgetKind::Race,
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

impl AppConfig {
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

