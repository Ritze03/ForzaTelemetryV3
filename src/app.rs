use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui::{Context, Pos2, Vec2};

use crate::config::{
    load_car_calibrations, save_car_calibrations, AppConfig, CarCalibration, SpeedDeltaMode,
};
use crate::engines::{load_engines, EngineRecord};
use crate::input::InputSender;
use crate::listeners::backfire::BackfireListener;
use crate::listeners::dsg::DsgListener;
use crate::listeners::perf_test::PerfTest;
use crate::listeners::power_capture::{PowerCapture, PowerCurveSnapshot};
use crate::listeners::sprint_timer::SprintTimer;
use crate::network::{start_receiver, NetworkHandle};
use crate::packet::ForzaPacket;
use crate::telemetry::TelemetryState;

// ── Season detection ──────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

pub fn current_season() -> Season {
    // Spring started 2026-06-12 14:30:00 UTC (7:30 AM PDT). Unix: 1749738600.
    // Cycle repeats weekly: Spring → Summer → Autumn → Winter.
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let secs = now - 1_749_738_600_i64;
    if secs < 0 {
        return Season::Spring;
    }
    match (secs / 604_800) % 4 {
        0 => Season::Spring,
        1 => Season::Summer,
        2 => Season::Autumn,
        _ => Season::Winter,
    }
}

fn season_map_bytes(season: Season) -> &'static [u8] {
    match season {
        Season::Spring => include_bytes!("../assets/maps/spring.jpg"),
        Season::Summer => include_bytes!("../assets/maps/summer.jpg"),
        Season::Autumn => include_bytes!("../assets/maps/autumn.jpg"),
        Season::Winter => include_bytes!("../assets/maps/winter.jpg"),
    }
}

/// Message type sent from the map-loading background thread to the main thread.
pub enum MapLoadMessage {
    /// Sent immediately with the display names of every season that needs caching.
    CacheBuildStarted { names: Vec<String> },
    /// One season's cache was just written; carry its name so the UI can remove it.
    CacheBuilt { name: String },
    /// All necessary caches are built and the current season's image is ready.
    Done(Option<(egui::ColorImage, [u32; 2])>),
}

fn season_display_name(season: Season) -> &'static str {
    match season {
        Season::Spring => "Spring",
        Season::Summer => "Summer",
        Season::Autumn => "Autumn",
        Season::Winter => "Winter",
    }
}

fn map_cache_path(season: Season, quality_pct: u32) -> std::path::PathBuf {
    crate::config::app_data_dir()
        .join("map_cache")
        .join(format!(
            "{}_q{}.bin",
            season_display_name(season).to_lowercase(),
            quality_pct
        ))
}

fn try_load_map_cache(path: &std::path::Path) -> Option<(egui::ColorImage, [u32; 2])> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 16 {
        return None;
    }
    let orig_w = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let orig_h = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let w = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
    let h = u32::from_le_bytes(data[12..16].try_into().ok()?) as usize;
    if data.len() != 16 + w * h * 4 {
        return None;
    }
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], &data[16..]);
    Some((color_image, [orig_w, orig_h]))
}

fn write_map_cache(path: &std::path::Path, orig: [u32; 2], scaled: [u32; 2], rgba: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut buf = Vec::with_capacity(16 + rgba.len());
    buf.extend_from_slice(&orig[0].to_le_bytes());
    buf.extend_from_slice(&orig[1].to_le_bytes());
    buf.extend_from_slice(&scaled[0].to_le_bytes());
    buf.extend_from_slice(&scaled[1].to_le_bytes());
    buf.extend_from_slice(rgba);
    let _ = std::fs::write(path, buf);
}

/// Decodes the JPEG for `season` and writes the binary cache file.
/// No-ops if the cache already exists. Does NOT return the image data.
fn decode_and_cache_season(season: Season, quality: f32) {
    let quality_pct = quality.round() as u32;
    let cache_file = map_cache_path(season, quality_pct);
    if cache_file.exists() {
        return;
    }
    let bytes = season_map_bytes(season);
    let Ok(img) = image::load_from_memory(bytes) else {
        return;
    };
    let orig_size = [img.width(), img.height()];
    let rgba = if quality >= 99.9 {
        img.into_rgba8()
    } else {
        let nw = ((orig_size[0] as f32 * quality / 100.0) as u32).max(1);
        let nh = ((orig_size[1] as f32 * quality / 100.0) as u32).max(1);
        img.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
            .into_rgba8()
    };
    let (w, h) = rgba.dimensions();
    write_map_cache(&cache_file, orig_size, [w, h], rgba.as_raw());
}

/// Loads the current season's map. Always reads from cache (building it first if needed).
fn load_map_color_image(season: Season, quality: f32) -> Option<(egui::ColorImage, [u32; 2])> {
    decode_and_cache_season(season, quality);
    try_load_map_cache(&map_cache_path(season, quality.round() as u32))
}

/// Background thread entry point. Builds any missing caches in parallel across all 4 seasons,
/// sending a `CacheBuilt` message for each completion, then loads the current season's image
/// from cache and sends `Done`.
fn map_load_thread(current_season: Season, quality: f32, tx: mpsc::Sender<MapLoadMessage>) {
    let all_seasons = [
        Season::Spring,
        Season::Summer,
        Season::Autumn,
        Season::Winter,
    ];
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

// ── Minimap helpers ───────────────────────────────────────────────

/// Returns the yaw angle the minimap should orient to.
/// If `use_movement_dir` and the car is moving, derives heading from velocity vector.
fn minimap_target_yaw(pkt: &crate::packet::ForzaPacket, use_movement_dir: bool) -> f32 {
    if use_movement_dir && pkt.speed > 1.0 {
        pkt.yaw + f32::atan2(pkt.velocity_x, pkt.velocity_z)
    } else {
        pkt.yaw
    }
}

/// Linearly interpolates between two angles, taking the shortest arc.
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut diff = (b - a).rem_euclid(TAU);
    if diff > PI {
        diff -= TAU;
    }
    a + diff * t
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
    Backfire,
    Gearbox,
    PowerCurve,
    EngineSwaps,
    Coop,
    Settings,
    Changelog,
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
    Config,
}

/// Nested sub-tabs inside the mini-settings "Map" tab.
#[derive(PartialEq, Clone, Copy, Default)]
pub enum MiniMapTab {
    #[default]
    General,
    Coop,
}

/// Last-known non-paused position of a co-op player, so a paused player can still
/// be drawn at their last spot. (Class/PI travel in the packet — see push_local.)
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
    pub backfire: BackfireListener,
    pub dsg: DsgListener,
    input: InputSender,
    pub power_capture: PowerCapture,
    pub saved_power_curve: Option<PowerCurveSnapshot>,
    pub perf_test: PerfTest,

    // Engine swaps search filter
    pub engine_search: String,

    // Settings: pending port change
    pub pending_port: u16,

    pub last_car_ordinal: i32,
    pub last_packet_time: Option<Instant>,

    /// Saved per-car DSG calibrations (own file, not config.json). Used only when
    /// `config.dsg_save_calibration` is on.
    pub car_calibrations: HashMap<i32, CarCalibration>,

    // Session maxima (reset on car change)
    pub max_power_ps: f32,
    pub max_torque_nm: f32,
    pub max_boost_psi: f32,
    pub max_speed_kmh: f32,
    pub cached_engine_max_rpm: f64,
    pub fi_detected: bool,
    /// Highest RPM seen while making power (>0 W) — the dynamically detected redline. Per-car.
    pub dynamic_max_rpm: f32,
    /// Estimated tire radius per wheel [FL, FR, RL, RR], meters. The packet has no radius,
    /// so it's derived from speed / wheel rotation while gripping (EMA-smoothed). Per-car.
    pub wheel_radius_est: [f32; 4],

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
    pub page_settings_tab: Tab,
    pub page_dashboard_sub_tab: DashboardSubTab,
    pub page_map_sub_tab: MiniMapTab,
    // Dashboard config export/import ("Config" sub-tab)
    pub config_import_buf: String,
    pub config_io_status: String,
    pub config_export_minisettings: bool,
    pub config_import_minisettings: bool,

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
    pub pending_preset: Option<usize>,

    // Co-Op
    pub coop: crate::coop::CoopState,
    pub coop_join_input: String,
    pub coop_copied_at: Option<Instant>,

    // Recording
    pub recorder: Option<crate::recorder::RecordState>,
    pub replay_selected: Option<usize>,
    pub csv_export_msg: Option<String>,

    receiver: Receiver<ForzaPacket>,
    _network: NetworkHandle,
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

        let (sender, receiver) = mpsc::channel();
        let network = start_receiver(config.listen_port, sender);
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
        let config_coop_name = config.coop_name.clone();
        let config_coop_hue = config.coop_hue;
        let config_coop_buffer_ms = config.coop_buffer_ms;
        let config_coop_last_code = config.coop_last_code.clone();
        Self {
            config,
            engines,
            labels: crate::labels::Labels::load(&_cc.egui_ctx),
            telemetry: TelemetryState::new(),
            current_tab: Tab::Dashboard,
            sprint_timer: SprintTimer::new(),
            backfire: BackfireListener::new(),
            dsg: DsgListener::new(),
            input: InputSender::new(),
            power_capture: PowerCapture::new(),
            saved_power_curve: None,
            perf_test: PerfTest::new(),
            engine_search: String::new(),
            pending_port,
            last_car_ordinal: 0,
            last_packet_time: None,
            car_calibrations: load_car_calibrations(),
            max_power_ps: 0.0,
            max_torque_nm: 0.0,
            max_boost_psi: 0.0,
            max_speed_kmh: 0.0,
            cached_engine_max_rpm: 0.0,
            fi_detected: false,
            dynamic_max_rpm: 0.0,
            wheel_radius_est: [0.33; 4],
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
            page_settings_tab: Tab::Dashboard,
            page_dashboard_sub_tab: DashboardSubTab::default(),
            page_map_sub_tab: MiniMapTab::default(),
            config_import_buf: String::new(),
            config_io_status: String::new(),
            config_export_minisettings: true,
            config_import_minisettings: true,
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
            pending_preset: None,
            coop: crate::coop::CoopState::new(
                &config_coop_name,
                config_coop_hue,
                config_coop_buffer_ms,
            ),
            coop_join_input: config_coop_last_code,
            coop_copied_at: None,
            recorder: None,
            replay_selected: None,
            csv_export_msg: None,
            receiver,
            _network: network,
        }
    }

    pub fn restart_receiver(&mut self, port: u16) {
        let (sender, receiver) = mpsc::channel();
        let network = start_receiver(port, sender);
        self.receiver = receiver;
        self._network = network;
        self.config.listen_port = port;
    }

    /// Start/stop recording received telemetry to a file.
    pub fn toggle_recording(&mut self) {
        if self.recorder.is_some() {
            self.recorder = None; // Drop flushes and closes the file.
        } else {
            match crate::recorder::RecordState::start() {
                Ok(r) => self.recorder = Some(r),
                Err(e) => eprintln!("recording failed to start: {e}"),
            }
        }
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

    pub fn drain_packets(&mut self) {
        let step = self.config.power_curve_step;
        let accel_s = self.config.accel_start_kmh;
        let accel_e = self.config.accel_end_kmh;
        let decel_s = self.config.decel_start_kmh;
        let decel_e = self.config.decel_end_kmh;
        self.perf_test.decel.dynamic_mode = self.config.decel_dynamic_mode;
        let fun_cfg = self.config.clone();

        let mut received = 0;
        while let Ok(pkt) = self.receiver.try_recv() {
            self.last_packet_time = Some(Instant::now());

            // Car change: reset per-car state
            if pkt.car_ordinal != 0 && pkt.car_ordinal != self.last_car_ordinal {
                self.last_car_ordinal = pkt.car_ordinal;
                self.sprint_timer.reset();
                self.power_capture.on_car_changed();
                self.perf_test.reset();
                self.dsg.reset_calibration();
                self.dsg.reset_state();
                self.max_power_ps = 0.0;
                self.max_torque_nm = 0.0;
                self.max_boost_psi = 0.0;
                self.max_speed_kmh = 0.0;
                self.cached_engine_max_rpm = 0.0;
                self.fi_detected = false;
                self.dynamic_max_rpm = 0.0;
                self.wheel_radius_est = [0.33; 4];

                // Opt-in: flush saved calibrations to disk (car change is a natural checkpoint),
                // then restore the new car's profile and skip the manual 1st→2nd pull.
                if self.config.dsg_save_calibration {
                    save_car_calibrations(&self.car_calibrations);
                    if let Some(cal) = self.car_calibrations.get(&pkt.car_ordinal) {
                        self.dsg.gear_redline_speeds = cal.gear_redline_speeds;
                        self.dsg.engaged = true;
                        self.dynamic_max_rpm = cal.max_rpm;
                    }
                }
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
                // Dynamic redline: highest RPM seen while the engine is making power, ignoring
                // moments where the handbrake is pulled or a tyre is slipping (>0.5) — those
                // inflate RPM without real road speed.
                if pkt.power > 0.0
                    && pkt.hand_brake == 0
                    && pkt.tire_slip_ratio_fl.abs() <= 0.5
                    && pkt.tire_slip_ratio_fr.abs() <= 0.5
                    && pkt.tire_slip_ratio_rl.abs() <= 0.5
                    && pkt.tire_slip_ratio_rr.abs() <= 0.5
                {
                    self.dynamic_max_rpm = self.dynamic_max_rpm.max(pkt.current_engine_rpm);
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
            if pkt.brake >= 255 && pkt.hand_brake >= 255 {
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
            self.backfire.update(&pkt, &fun_cfg, &self.input, self.telemetry.packets_per_sec);
            let suppress_gearbox_accel =
                self.config.dsg_ignore_backfire_accel && self.backfire_echo_active();
            self.dsg.update(
                &pkt,
                &fun_cfg,
                &self.input,
                self.dynamic_max_rpm,
                suppress_gearbox_accel,
            );

            // Opt-in: keep the in-memory per-car calibration current; it's written to its own
            // file on car change and on exit.
            if self.config.dsg_save_calibration
                && pkt.car_ordinal != 0
                && self.dsg.gear_redline_speeds[1] > 0.0
            {
                self.car_calibrations.insert(
                    pkt.car_ordinal,
                    CarCalibration {
                        gear_redline_speeds: self.dsg.gear_redline_speeds,
                        max_rpm: self.dynamic_max_rpm,
                    },
                );
            }

            // Co-Op: relay our locally-received telemetry to peers. A paused game
            // zeroes car class/PI, so carry over the cached values (same as the Car
            // widget) so peers keep showing our real class while we're paused.
            if pkt.is_paused() && self.cached_car_pi != 0 {
                let mut out = pkt.clone();
                out.car_class = self.cached_car_class;
                out.car_performance_index = self.cached_car_pi;
                self.coop.push_local(&out);
            } else {
                self.coop.push_local(&pkt);
            }

            // Recording: capture the packet with its timestamp.
            if let Some(rec) = self.recorder.as_mut() {
                rec.write_packet(&pkt.to_bytes());
            }

            self.telemetry.update(pkt);

            received += 1;
            if received >= 200 {
                break;
            }
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
                            minimap_target_yaw(pkt, self.config.minimap_use_movement_dir);
                        self.minimap_cached_raw_yaw = pkt.yaw;
                    }
                }
            }
        }

        // Smooth rotation: lerp minimap_smoothed_yaw toward the latest target every frame
        {
            let target = if let Some(ref pkt) = self.telemetry.latest {
                if pkt.is_race_on != 0 {
                    minimap_target_yaw(pkt, self.config.minimap_use_movement_dir)
                } else {
                    self.minimap_smoothed_yaw
                }
            } else {
                self.minimap_smoothed_yaw
            };

            if self.config.minimap_smooth_rotation {
                let dt = ctx.input(|i| i.unstable_dt).min(0.1);
                let lerp_t = (6.0 * dt).min(1.0);
                self.minimap_smoothed_yaw = lerp_angle(self.minimap_smoothed_yaw, target, lerp_t);
            } else {
                self.minimap_smoothed_yaw = self.minimap_cached_yaw;
            }
        }

        // Smooth minimap zoom: immediate zoom-in when driving, 1.5 s delay before zooming out
        {
            let dt = ctx.input(|i| i.unstable_dt).min(0.1);
            let speed_kmh = self
                .telemetry
                .latest
                .as_ref()
                .map(|p| p.speed * 3.6)
                .unwrap_or(0.0);
            let lerp_t = (3.0 * dt).min(1.0);
            if speed_kmh >= 5.0 {
                self.minimap_stopped_at = None;
                self.minimap_current_zoom = self.minimap_current_zoom * (1.0 - lerp_t)
                    + self.config.minimap_zoom_driving_m * lerp_t;
            } else {
                let stopped_at = self.minimap_stopped_at.get_or_insert_with(Instant::now);
                if stopped_at.elapsed().as_secs_f32() >= 1.5 {
                    self.minimap_current_zoom = self.minimap_current_zoom * (1.0 - lerp_t)
                        + self.config.minimap_zoom_stopped_m * lerp_t;
                }
            }
        }

        // F11 fullscreen toggle (Windows only)
        #[cfg(target_os = "windows")]
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            let fs = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
        }

        // Hotkey: F10 toggle map orientation.
        // (F-keys never conflict with text entry, so they're safe to handle unconditionally.)
        if ctx.input(|i| i.key_pressed(egui::Key::F10)) {
            self.config.minimap_north_up = !self.config.minimap_north_up;
        }

        // Tab bar
        egui::TopBottomPanel::top("tab_bar")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(crate::theme::HEAD))
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    use crate::i18n::tr;
                    use crate::icons;
                    ui.selectable_value(
                        &mut self.current_tab,
                        Tab::Dashboard,
                        format!("{}  {}", icons::DASHBOARD, tr("Dashboard")),
                    );
                    ui.selectable_value(
                        &mut self.current_tab,
                        Tab::Coop,
                        format!("{}  {}", icons::USERS, tr("Co-Op")),
                    );
                    // Intentional single space, otherwise spacing looks too large
                    ui.selectable_value(
                        &mut self.current_tab,
                        Tab::Backfire,
                        format!("{} {}", icons::BOLT, tr("Backfire")),
                    );
                    ui.selectable_value(
                        &mut self.current_tab,
                        Tab::Gearbox,
                        format!("{}  {}", icons::GAMEPAD, tr("Automatic Gearbox")),
                    );
                    ui.selectable_value(
                        &mut self.current_tab,
                        Tab::PowerCurve,
                        format!("{}  {}", icons::LINE_CHART, tr("Power Curve")),
                    );
                    ui.selectable_value(
                        &mut self.current_tab,
                        Tab::EngineSwaps,
                        format!("{}  {}", icons::WRENCH, tr("Engine Swaps")),
                    );
                    // RIGHT-bound tabs. A right_to_left layout places items
                    // right→left, so Setup is added first (rightmost) and
                    // What's New lands to its left.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.selectable_value(
                            &mut self.current_tab,
                            Tab::Settings,
                            format!("{}  {}", icons::COG, tr("Setup")),
                        );
                        ui.selectable_value(
                            &mut self.current_tab,
                            Tab::Changelog,
                            format!("{}  {}", icons::BULLHORN, tr("What's New")),
                        );
                    });
                });
                ui.add_space(2.0);
            });

        // ── Bottom status bar ──────────────────────────────────────
        egui::TopBottomPanel::bottom("status_bar")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(crate::theme::PANEL2))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    use crate::i18n::tr;
                    use crate::icons;
                    // LEFT: connection status + pps
                    // NO_SIGNAL renders wider than its glyph advance, so it needs an
                    // extra space to match PLUG's visual gap.
                    let (color, label) = if self.telemetry.is_connected {
                        (crate::theme::GOOD, format!("{} {}", icons::PLUG, tr("Connected")))
                    } else {
                        (crate::theme::DANGER, format!("{}  {}", icons::NO_SIGNAL, tr("Disconnected")))
                    };
                    ui.colored_label(color, label);
                    if self.telemetry.is_connected {
                        ui.label(format!("  {:.0} pps", self.telemetry.packets_per_sec));
                    }

                    // Co-Op indicator (visible from any tab)
                    let coop_role = self.coop.role();
                    if coop_role != crate::coop::Role::Off {
                        ui.separator();
                        let (c, verb) = match coop_role {
                            crate::coop::Role::Host => (crate::theme::ACCENT, tr("Hosting")),
                            _ => (crate::theme::GOOD, tr("Joined")),
                        };
                        let n = self.coop.roster().len();
                        ui.colored_label(
                            c,
                            format!("{}  {} · {} {}", icons::USERS, verb, n, tr("players")),
                        );
                    }

                    // Recording indicator (visible from any tab)
                    if let Some(rec) = self.recorder.as_ref() {
                        ui.separator();
                        let s = rec.elapsed().as_secs();
                        ui.colored_label(
                            crate::theme::DANGER,
                            format!(
                                "{}  REC {}:{:02} · {} pkts",
                                icons::CIRCLE,
                                s / 60,
                                s % 60,
                                rec.packets
                            ),
                        );
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
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            icons::COG,
                            egui::FontId::proportional(16.0),
                            cog_color,
                        );
                        if resp.clicked() {
                            self.page_settings_open = !self.page_settings_open;
                            self.page_settings_tab = self.current_tab;
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

                    // Main tab row (no Settings tab here)
                    ui.horizontal_wrapped(|ui| {
                        for (tab, lbl) in [
                            (Tab::Dashboard,    "Dashboard"),
                            (Tab::Backfire,     "Backfire"),
                            (Tab::Gearbox,      "Gearbox"),
                            (Tab::PowerCurve,   "Power"),
                            (Tab::EngineSwaps,  "Engines"),
                        ] {
                            ui.selectable_value(&mut self.page_settings_tab, tab, tr(lbl));
                        }
                    });
                    ui.separator();

                    ui.set_min_height(590.0);

                    match self.page_settings_tab {
                        Tab::Dashboard => {
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
                                    (DashboardSubTab::Config,      "Config"),
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

                                    ui.label(tr("Grid columns:"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.grid_cols, 1..=40_usize),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(tr("Grid rows:"));
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
                                        ui.label(tr("Alignment:"));
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
                                            ui.label(tr("Mode:"));
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
                                        ui.label(tr("Alignment:"));
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
                                        ui.label(tr("Type:"));
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
                                        ui.label(tr("Style:"));
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
                                            ui.label(tr("Bar Display Value:"));
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
                                        ui.label(tr("Max RPM:"));
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
                                    ui.label(tr("Shift indicator thresholds (% of engine max RPM):"));
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label(tr("Low (warn):"));
                                        ui.add(
                                            egui::Slider::new(&mut self.config.shift_low_pct, 50.0..=99.0)
                                                .suffix("%"),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(tr("High (shift):"));
                                        ui.add(
                                            egui::Slider::new(&mut self.config.shift_high_pct, 51.0..=100.0)
                                                .suffix("%"),
                                        );
                                    });
                                }
                                DashboardSubTab::Engine => {
                                    use crate::config::EngineDisplayMode as EDM;
                                    ui.label(tr("Show per line:"));
                                    ui.add_space(4.0);
                                    for (mode, lbl) in [
                                        (EDM::Current, "Current values"),
                                        (EDM::Max,     "Max values"),
                                        (EDM::Both,    "Both"),
                                    ] {
                                        ui.radio_value(&mut self.config.engine_display_mode, mode, tr(lbl));
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
                                }
                                DashboardSubTab::Config => {
                                    ui.label(crate::theme::section_label(tr("Load Preset")));
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        let selected = self.pending_preset
                                            .map(|i| crate::config::PRESET_NAMES[i])
                                            .unwrap_or(tr("— select —"));
                                        egui::ComboBox::from_id_salt("cfg_preset_combo")
                                            .selected_text(selected)
                                            .show_ui(ui, |ui| {
                                                for (i, name) in crate::config::PRESET_NAMES.iter().enumerate() {
                                                    ui.selectable_value(&mut self.pending_preset, Some(i), *name);
                                                }
                                            });
                                        if ui.button(tr("Load Preset")).clicked() {
                                            if let Some(idx) = self.pending_preset {
                                                crate::config::apply_preset(&mut self.config, crate::config::PRESET_DATA[idx]);
                                                self.config.save();
                                                self.config_io_status = tr("Preset loaded.").to_string();
                                            }
                                        }
                                    });

                                    ui.add_space(12.0);
                                    ui.separator();
                                    ui.add_space(8.0);

                                    ui.label(crate::theme::section_label(tr("Export")));
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(tr("Copies your dashboard layout as JSON to the clipboard."))
                                            .size(11.0).color(egui::Color32::GRAY),
                                    );
                                    ui.add_space(4.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config_export_minisettings, tr("Include mini-settings"));
                                    ui.add_space(4.0);
                                    if ui.button(format!("{}  {}", crate::icons::COPY, tr("Copy to clipboard"))).clicked() {
                                        ui.ctx().copy_text(crate::config::export_preset(&self.config, self.config_export_minisettings));
                                        self.config_io_status = tr("Copied to clipboard.").to_string();
                                    }

                                    ui.add_space(12.0);
                                    ui.separator();
                                    ui.add_space(8.0);

                                    ui.label(crate::theme::section_label(tr("Import")));
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(tr("Paste an exported JSON below and Import. Keys not in the JSON keep their current value."))
                                            .size(11.0).color(egui::Color32::GRAY),
                                    );
                                    ui.add_space(4.0);
                                    ui.add(
                                        egui::TextEdit::multiline(&mut self.config_import_buf)
                                            .desired_rows(6)
                                            .desired_width(f32::INFINITY)
                                            .code_editor()
                                            .hint_text(tr("Paste JSON here")),
                                    );
                                    ui.add_space(4.0);
                                    crate::theme::styled_checkbox(ui, &mut self.config_import_minisettings, tr("Include mini-settings"));
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        if ui.button(format!("{}  {}", crate::icons::FLOPPY, tr("Import"))).clicked() {
                                            if crate::config::import_preset(&mut self.config, &self.config_import_buf, self.config_import_minisettings) {
                                                self.config.save();
                                                self.config_import_buf.clear();
                                                self.config_io_status = tr("Imported.").to_string();
                                            } else {
                                                self.config_io_status = tr("Invalid JSON — nothing imported.").to_string();
                                            }
                                        }
                                        if !self.config_import_buf.is_empty()
                                            && ui.button(tr("Clear")).clicked()
                                        {
                                            self.config_import_buf.clear();
                                        }
                                    });
                                    if !self.config_io_status.is_empty() {
                                        ui.add_space(6.0);
                                        ui.label(
                                            egui::RichText::new(&self.config_io_status)
                                                .size(11.0).color(egui::Color32::from_rgb(120, 200, 120)),
                                        );
                                    }
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
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_fps_limit_enabled, tr("Render FPS limit:"));
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
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_smooth_rotation, tr("Smooth rotation"));
                                        crate::theme::styled_checkbox(ui, &mut self.config.minimap_use_movement_dir, tr("Use movement direction as rotation"));
                                    }
                                    crate::theme::styled_checkbox(ui, &mut self.config.minimap_mirror_edges, tr("Mirror map at edges"));
                                    crate::theme::styled_checkbox(ui, &mut self.config.minimap_show_compass, tr("Show compass"));
                                    ui.add_space(4.0);
                                    ui.label(tr("Zoom when driving (radius, metres):"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.minimap_zoom_driving_m, 50.0..=3000.0)
                                            .suffix(" m"),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(tr("Zoom when stopped (radius, metres):"));
                                    ui.add(
                                        egui::Slider::new(&mut self.config.minimap_zoom_stopped_m, 500.0..=6000.0)
                                            .suffix(" m"),
                                    );
                                    ui.add_space(8.0);
                                    ui.label(tr("Image quality:"));
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
                                            ui.label(tr("Pixels per metre:"));
                                            ui.add(
                                                egui::DragValue::new(&mut self.config.minimap_px_per_m)
                                                    .speed(0.001)
                                                    .range(0.01..=10.0),
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label(tr("World origin X (m at pixel 0):"));
                                            ui.add(
                                                egui::DragValue::new(&mut self.config.minimap_world_origin_x)
                                                    .speed(10.0),
                                            );
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label(tr("World origin Z (m at pixel 0):"));
                                            ui.add(
                                                egui::DragValue::new(&mut self.config.minimap_world_origin_z)
                                                    .speed(10.0),
                                            );
                                        });
                                        ui.add_space(4.0);
                                        if ui.button(tr("Reset to defaults")).clicked() {
                                            self.config.minimap_px_per_m = 0.3722;
                                            self.config.minimap_world_origin_x = -12540.0;
                                            self.config.minimap_world_origin_z = 10738.0;
                                        }
                                    });
                                    }
                                    MiniMapTab::Coop => {
                                        ui.label(crate::theme::section_label(tr("Tracer fade")));
                                        ui.add_space(4.0);
                                        ui.label(tr("Fade after (time):"));
                                        ui.add(egui::Slider::new(&mut self.config.coop_trail_fade_secs, 1.0..=60.0).suffix(" s"));
                                        ui.add_space(4.0);
                                        ui.label(tr("Fade after (distance):"));
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
                                            ui.label(egui::RichText::new(tr("Columns:")).size(11.0).color(egui::Color32::GRAY));
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
                        Tab::PowerCurve => {
                            ui.horizontal(|ui| {
                                ui.label(tr("RPM step size:"));
                                ui.add(
                                    egui::Slider::new(&mut self.config.power_curve_step, 25.0..=500.0)
                                        .step_by(25.0)
                                        .suffix(" rpm"),
                                );
                            });
                            ui.add_space(8.0);
                            crate::theme::styled_checkbox(ui, 
                                &mut self.config.power_curve_forced_induction,
                                tr("Forced induction detection"),
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(tr(
                                    "ON: hide boost graph if no positive pressure was captured.\n\
                                     OFF: always show the boost graph."
                                ))
                                .size(11.0)
                                .color(egui::Color32::GRAY),
                            );
                            if self.config.power_curve_forced_induction {
                                ui.add_space(8.0);
                                crate::theme::styled_checkbox(ui, 
                                    &mut self.config.power_curve_save_fi_state,
                                    tr("Save Forced Induction State"),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(tr(
                                        "Keep the boost graph visible after clearing data,\n\
                                         if FI was detected at least once for this car."
                                    ))
                                    .size(11.0)
                                    .color(egui::Color32::GRAY),
                                );
                            }
                        }
                        Tab::Gearbox => {
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
            let target = if hovered { 1.0_f32 } else { 0.5_f32 };
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
            Tab::Backfire => crate::ui::backfire::show_backfire(ui, self),
            Tab::Gearbox => crate::ui::gearbox::show_gearbox(ui, self),
            Tab::PowerCurve => crate::ui::power_curve::show(ui, self),
            Tab::EngineSwaps => crate::ui::engine_swaps::show(ui, self),
            Tab::Coop => crate::ui::coop::show(ui, self),
            Tab::Settings => crate::ui::settings::show(ui, self),
            Tab::Changelog => crate::ui::changelog::show(ui, self),
        });

        // FPS limiter
        if self.config.fps_limit_enabled {
            ctx.request_repaint_after(Duration::from_secs_f32(1.0 / self.config.fps_limit));
        } else {
            ctx.request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.config.save();
        if self.config.dsg_save_calibration {
            save_car_calibrations(&self.car_calibrations);
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
