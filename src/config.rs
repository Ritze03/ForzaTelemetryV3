use std::{collections::HashMap, path::PathBuf};

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
    pub dashboard_block_width: f32,
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
    // Dashboard layout
    pub dynamic_width: bool,
    // Shift indicator (global, % of engine_max_rpm)
    pub shift_low_pct: f32,
    pub shift_high_pct: f32,
    // Power curve
    pub power_curve_forced_induction: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            listen_port: 1337,
            fps_limit: 60.0,
            use_mph: false,
            use_fahrenheit: false,
            use_bar: false,
            theme: Theme::Dark,
            always_on_top: false,
            surface_rumble_max: 3.8,
            power_curve_step: 100.0,
            game_mode: GameMode::ForzaHorizon6,
            dashboard_block_width: 360.0,
            speed_align: TextAlign::Right,
            gear_align: TextAlign::Right,
            show_speed_delta: false,
            speed_delta_mode: SpeedDeltaMode::Track,
            sprint_type: SprintType::Incremental,
            sprint_show_other: true,
            tire_display_style: TireDisplayStyle::Combined,
            tire_slip_style: TireSlipStyle::Values,
            dynamic_width: true,
            shift_low_pct: 85.0,
            shift_high_pct: 95.0,
            power_curve_forced_induction: true,
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
        serde_json::from_value(val).unwrap_or(default)
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
        exe_dir().join("config.json")
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct CarSettings {
    pub name: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct AllCarSettings {
    pub cars: HashMap<i32, CarSettings>,
}

impl AllCarSettings {
    pub fn load() -> Self {
        if let Ok(data) = std::fs::read_to_string(Self::path()) {
            if let Ok(s) = serde_json::from_str(&data) {
                return s;
            }
        }
        Self::default()
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

    pub fn get_or_default(&mut self, ordinal: i32) -> &mut CarSettings {
        self.cars.entry(ordinal).or_default()
    }

    fn path() -> PathBuf {
        exe_dir().join("car_settings.json")
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}
