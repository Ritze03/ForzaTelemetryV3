use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egui::Context;

use crate::config::{AllCarSettings, AppConfig, Theme};
use crate::engines::{load_engines, EngineRecord};
use crate::listeners::perf_test::PerfTest;
use crate::listeners::power_capture::PowerCapture;
use crate::listeners::sprint_timer::SprintTimer;
use crate::network::{start_receiver, NetworkHandle};
use crate::packet::ForzaPacket;
use crate::telemetry::TelemetryState;

// ── Session stats ──────────────────────────────────────────────────

#[derive(Default)]
pub struct SuspensionStats {
    pub min: [f32; 4],   // FL, FR, RL, RR — normalized 0..1
    pub max: [f32; 4],
    initialized: bool,
}

impl SuspensionStats {
    pub fn update(&mut self, vals: [f32; 4]) {
        if !self.initialized {
            self.min = vals;
            self.max = vals;
            self.initialized = true;
        } else {
            for i in 0..4 {
                self.min[i] = self.min[i].min(vals[i]);
                self.max[i] = self.max[i].max(vals[i]);
            }
        }
    }
    pub fn initialized(&self) -> bool {
        self.initialized
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Default)]
pub struct GForceStats {
    pub max_lateral:       f32,
    pub max_longitudinal:  f32,
    pub max_vertical:      f32,
    pub peak_lateral:      f32,
    pub peak_longitudinal: f32,
    pub peak_fade_start:   Option<Instant>,
}

impl GForceStats {
    pub fn update(&mut self, lat: f32, lon: f32, vert: f32) {
        self.max_lateral      = self.max_lateral.max(lat.abs());
        self.max_longitudinal = self.max_longitudinal.max(lon.abs());
        self.max_vertical     = self.max_vertical.max(vert.abs());

        let mag_sq = lat * lat + lon * lon;
        let peak_sq = self.peak_lateral * self.peak_lateral
            + self.peak_longitudinal * self.peak_longitudinal;
        if mag_sq > peak_sq {
            self.peak_lateral = lat;
            self.peak_longitudinal = lon;
        }

        let peak_mag = peak_sq.sqrt();
        let cur_mag  = mag_sq.sqrt();
        if cur_mag >= peak_mag - 0.3 {
            self.peak_fade_start = None;
        } else if self.peak_fade_start.is_none() {
            self.peak_fade_start = Some(Instant::now());
        }
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ── Tabs ───────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
pub enum Tab {
    Dashboard,
    Acceleration,
    Deceleration,
    PowerCurve,
    EngineSwaps,
    Settings,
}

// ── App ────────────────────────────────────────────────────────────

pub struct ForzaApp {
    pub config: AppConfig,
    pub car_settings: AllCarSettings,
    pub engines: Vec<EngineRecord>,
    pub telemetry: TelemetryState,
    pub current_tab: Tab,

    pub sprint_timer: SprintTimer,
    pub power_capture: PowerCapture,
    pub perf_test: PerfTest,

    // Acceleration test config
    pub accel_start_kmh: f32,
    pub accel_end_kmh: f32,

    // Deceleration test config
    pub decel_start_kmh: f32,
    pub decel_end_kmh: f32,

    // Engine swaps search filter
    pub engine_search: String,

    // Settings: pending port change
    pub pending_port: u16,

    pub last_car_ordinal: i32,
    pub last_packet_time: Option<Instant>,

    // Session maxima (reset on car change)
    pub max_power_ps:  f32,
    pub max_torque_nm: f32,
    pub max_boost_psi: f32,

    // Session stats (reset by Brake+HandBrake @ 100%)
    pub suspension_stats: SuspensionStats,
    pub gforce_stats: GForceStats,

    // Cached car identity — persists when is_race_on == 0 (paused)
    pub cached_car_class_str:  String,
    pub cached_car_pi:         i32,
    pub cached_drivetrain_str: String,
    pub cached_num_cylinders:  i32,

    // Page-specific settings popup
    pub page_settings_open: bool,
    pub page_settings_tab: Tab,

    receiver: Receiver<ForzaPacket>,
    _network: NetworkHandle,
}

impl ForzaApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        // Load Hack Nerd Font as the primary typeface so NF icon codepoints render.
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "hack_nerd".to_owned(),
            egui::FontData::from_static(include_bytes!("../assets/HackNerdFont-Regular.ttf")).into(),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "hack_nerd".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "hack_nerd".to_owned());
        _cc.egui_ctx.set_fonts(fonts);

        let config = AppConfig::load();
        let car_settings = AllCarSettings::load();
        let engines = load_engines();

        let (sender, receiver) = mpsc::channel();
        let network = start_receiver(config.listen_port, sender);
        let pending_port = config.listen_port;

        Self {
            config,
            car_settings,
            engines,
            telemetry: TelemetryState::new(),
            current_tab: Tab::Dashboard,
            sprint_timer: SprintTimer::new(),
            power_capture: PowerCapture::new(),
            perf_test: PerfTest::new(),
            accel_start_kmh: 0.0,
            accel_end_kmh: 100.0,
            decel_start_kmh: 100.0,
            decel_end_kmh: 0.0,
            engine_search: String::new(),
            pending_port,
            last_car_ordinal: 0,
            last_packet_time: None,
            max_power_ps: 0.0,
            max_torque_nm: 0.0,
            max_boost_psi: 0.0,
            suspension_stats: SuspensionStats::default(),
            gforce_stats: GForceStats::default(),
            cached_car_class_str:  String::new(),
            cached_car_pi:         0,
            cached_drivetrain_str: String::new(),
            cached_num_cylinders:  0,
            page_settings_open: false,
            page_settings_tab: Tab::Dashboard,
            receiver,
            _network: network,
        }
    }

    /// Restart the UDP receiver on a new port (called from Settings).
    pub fn restart_receiver(&mut self, port: u16) {
        let (sender, receiver) = mpsc::channel();
        let network = start_receiver(port, sender);
        self.receiver = receiver;
        self._network = network;
        self.config.listen_port = port;
    }

    pub fn drain_packets(&mut self) {
        let step = self.config.power_curve_step;
        let accel_s = self.accel_start_kmh;
        let accel_e = self.accel_end_kmh;
        let decel_s = self.decel_start_kmh;
        let decel_e = self.decel_end_kmh;

        let mut drained = 0;
        while let Ok(pkt) = self.receiver.try_recv() {
            // Car change: reset per-car state
            if pkt.car_ordinal != 0 && pkt.car_ordinal != self.last_car_ordinal {
                self.last_car_ordinal = pkt.car_ordinal;
                self.sprint_timer.reset();
                self.power_capture.on_car_changed();
                self.perf_test.reset();
                self.max_power_ps = 0.0;
                self.max_torque_nm = 0.0;
                self.max_boost_psi = 0.0;
            }

            // Update per-car max RPM
            if pkt.car_ordinal != 0 && pkt.is_race_on != 0 {
                let car = self.car_settings.get_or_default(pkt.car_ordinal);
                if pkt.current_engine_rpm > car.max_rpm_measured {
                    car.max_rpm_measured = pkt.current_engine_rpm;
                }
            }

            // Session maxima + cache car identity
            if pkt.is_race_on != 0 {
                self.cached_car_class_str  = pkt.car_class_str().to_string();
                self.cached_car_pi         = pkt.car_performance_index;
                self.cached_drivetrain_str = pkt.drivetrain_str().to_string();
                self.cached_num_cylinders  = pkt.num_cylinders;
                self.max_power_ps  = self.max_power_ps.max(pkt.power_ps());
                self.max_torque_nm = self.max_torque_nm.max(pkt.torque_nm());
                self.max_boost_psi = self.max_boost_psi.max(pkt.boost);

                let lat  = pkt.acceleration_x / 9.81;
                let lon  = pkt.acceleration_z / 9.81;
                let vert = pkt.acceleration_y / 9.81;
                self.gforce_stats.update(lat, lon, vert);

                self.suspension_stats.update([
                    pkt.normalized_suspension_travel_fl,
                    pkt.normalized_suspension_travel_fr,
                    pkt.normalized_suspension_travel_rl,
                    pkt.normalized_suspension_travel_rr,
                ]);
            }

            // Brake + HandBrake both at 100% → reset session stats & power curve
            if pkt.brake >= 255 && pkt.hand_brake >= 255 {
                self.gforce_stats.reset();
                self.suspension_stats.reset();
                self.power_capture.clear();
            }

            self.sprint_timer.update(&pkt);
            self.power_capture.update(&pkt, step);
            self.perf_test.update(&pkt, accel_s, accel_e, decel_s, decel_e);
            self.telemetry.update(pkt);
            self.last_packet_time = Some(Instant::now());

            drained += 1;
            if drained >= 200 {
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
}

impl eframe::App for ForzaApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_packets();

        // Apply theme
        match self.config.theme {
            Theme::Dark  => ctx.set_visuals(egui::Visuals::dark()),
            Theme::Light => ctx.set_visuals(egui::Visuals::light()),
        }

        // Tab bar
        egui::TopBottomPanel::top("tab_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                use crate::icons;
                ui.selectable_value(
                    &mut self.current_tab,
                    Tab::Dashboard,
                    format!("{} Dashboard", icons::DASHBOARD),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    Tab::Acceleration,
                    format!("{} Acceleration", icons::BOLT),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    Tab::Deceleration,
                    format!("{} Deceleration", icons::STOP),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    Tab::PowerCurve,
                    format!("{} Power Curve", icons::LINE_CHART),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    Tab::EngineSwaps,
                    format!("{} Engine Swaps", icons::WRENCH),
                );
                ui.selectable_value(
                    &mut self.current_tab,
                    Tab::Settings,
                    format!("{} Settings", icons::COG),
                );

            });
            ui.add_space(2.0);
        });

        // ── Bottom status bar ──────────────────────────────────────
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                use crate::icons;
                // LEFT: connection status + pps
                let (color, icon, text) = if self.telemetry.is_connected {
                    (egui::Color32::from_rgb(60, 200, 90), icons::PLUG, "Connected")
                } else {
                    (egui::Color32::from_rgb(200, 60, 60), icons::NO_SIGNAL, "Disconnected")
                };
                ui.colored_label(color, format!("{icon} {text}"));
                ui.label(format!("  {:.0} pps", self.telemetry.packets_per_sec));

                // RIGHT: cog toggle
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let cog_color = if self.page_settings_open {
                        egui::Color32::from_rgb(255, 200, 60)
                    } else {
                        egui::Color32::GRAY
                    };
                    if ui.add(
                        egui::Label::new(
                            egui::RichText::new(icons::COG).color(cog_color).size(16.0),
                        )
                        .sense(egui::Sense::click()),
                    )
                    .clicked()
                    {
                        self.page_settings_open = !self.page_settings_open;
                        self.page_settings_tab = self.current_tab;
                    }
                });
            });
        });

        // ── Page settings floating window ──────────────────────────
        if self.page_settings_open {
            egui::Window::new("page_settings_win")
                .title_bar(false)
                .resizable(false)
                .fixed_size([400.0, 400.0])
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -36.0))
                .show(ctx, |ui| {
                    use crate::icons;
                    ui.horizontal(|ui| {
                        for (tab, lbl) in [
                            (Tab::Dashboard,    "Dashboard"),
                            (Tab::Acceleration, "Accel"),
                            (Tab::Deceleration, "Decel"),
                            (Tab::PowerCurve,   "Power"),
                            (Tab::EngineSwaps,  "Engines"),
                            (Tab::Settings,     "Settings"),
                        ] {
                            ui.selectable_value(&mut self.page_settings_tab, tab, lbl);
                        }
                    });
                    ui.separator();

                    match self.page_settings_tab {
                        Tab::Dashboard => {
                            ui.add_space(8.0);
                            ui.label("Block width:");
                            ui.add(
                                egui::Slider::new(
                                    &mut self.config.dashboard_block_width,
                                    200.0..=800.0,
                                )
                                .step_by(10.0)
                                .suffix(" px"),
                            );
                        }
                        _ => {
                            ui.centered_and_justified(|ui| {
                                ui.label(
                                    egui::RichText::new("No options for this page")
                                        .color(egui::Color32::GRAY),
                                );
                            });
                        }
                    }
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.current_tab {
                Tab::Dashboard   => crate::ui::dashboard::show(ui, self),
                Tab::Acceleration => crate::ui::acceleration::show(ui, self),
                Tab::Deceleration => crate::ui::deceleration::show(ui, self),
                Tab::PowerCurve  => crate::ui::power_curve::show(ui, self),
                Tab::EngineSwaps => crate::ui::engine_swaps::show(ui, self),
                Tab::Settings    => crate::ui::settings::show(ui, self),
            }
        });

        // FPS limiter
        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / self.config.fps_limit));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.config.save();
        self.car_settings.save();
    }
}
