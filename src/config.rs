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
        }
    }
}

impl AppConfig {
    pub fn load() -> Self {
        if let Ok(data) = std::fs::read_to_string(Self::path()) {
            if let Ok(cfg) = serde_json::from_str(&data) {
                return cfg;
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

    fn path() -> PathBuf {
        exe_dir().join("config.json")
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CarSettings {
    pub name: String,
    pub shift_low_pct: f32,
    pub shift_high_pct: f32,
    pub max_rpm_measured: f32,
}

impl Default for CarSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            shift_low_pct: 91.0,
            shift_high_pct: 99.0,
            max_rpm_measured: 0.0,
        }
    }
}

impl CarSettings {
    pub fn shift_low_rpm(&self) -> f32 {
        self.max_rpm_measured * self.shift_low_pct / 100.0
    }

    pub fn shift_high_rpm(&self) -> f32 {
        self.max_rpm_measured * self.shift_high_pct / 100.0
    }
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
