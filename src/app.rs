use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use egui::Context;

use crate::config::{AllCarSettings, AppConfig, SpeedDeltaMode, Theme};
use crate::engines::{load_engines, EngineRecord};
use crate::listeners::perf_test::PerfTest;
use crate::listeners::power_capture::PowerCapture;
use crate::listeners::sprint_timer::SprintTimer;
use crate::network::{start_receiver, NetworkHandle};
use crate::packet::ForzaPacket;
use crate::telemetry::TelemetryState;

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
    pub max_lateral:       f32,
    pub max_longitudinal:  f32,
    pub max_vertical:      f32,
    pub peak_lateral:      f32,
    pub peak_longitudinal: f32,
    pub peak_reset_timer:  Option<Instant>,
}

impl GForceStats {
    pub fn update(&mut self, lat: f32, lon: f32, vert: f32) {
        let cur_mag  = (lat * lat + lon * lon).sqrt();
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

        if lat.abs()  >= 0.5 { self.max_lateral      = self.max_lateral.max(lat.abs()); }
        if lon.abs()  >= 0.1 { self.max_longitudinal = self.max_longitudinal.max(lon.abs()); }
        if vert.abs() >= 0.2 { self.max_vertical     = self.max_vertical.max(vert.abs()); }
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

#[derive(PartialEq, Clone, Copy, Default)]
pub enum DashboardSubTab {
    #[default]
    General,
    Kmh,
    Gear,
    SprintTimes,
    Tires,
    Shift,
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

    // Session stats
    pub suspension_stats: SuspensionStats,
    pub gforce_stats: GForceStats,

    // Cached car identity — persists when is_race_on == 0 (paused)
    pub cached_car_class_str:  String,
    pub cached_car_pi:         i32,
    pub cached_drivetrain_str: String,
    pub cached_num_cylinders:  i32,

    // Speed delta tracking
    pub speed_delta_kmh: f32,
    last_tracked_speed: f32,
    last_track_instant: Option<Instant>,
    speed_history: VecDeque<(Instant, f32)>,

    // Power curve plot: request auto-fit on next frame (set by Clear or middle-click)
    pub power_plot_auto_bounds: bool,

    // Page-specific settings popup
    pub page_settings_open: bool,
    pub page_settings_opacity: f32,
    pub page_settings_tab: Tab,
    pub page_dashboard_sub_tab: DashboardSubTab,

    receiver: Receiver<ForzaPacket>,
    _network: NetworkHandle,
}

impl ForzaApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
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
            cached_drivetrain_str: "XWD".to_string(),
            cached_num_cylinders:  0,
            speed_delta_kmh: 0.0,
            last_tracked_speed: 0.0,
            last_track_instant: None,
            speed_history: VecDeque::new(),
            power_plot_auto_bounds: false,
            page_settings_open: false,
            page_settings_opacity: 0.5,
            page_settings_tab: Tab::Dashboard,
            page_dashboard_sub_tab: DashboardSubTab::default(),
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

            // Session maxima + cache car identity
            if pkt.is_race_on != 0 {
                self.cached_car_class_str  = pkt.car_class_str().to_string();
                self.cached_car_pi         = pkt.car_performance_index;
                self.cached_drivetrain_str = pkt.drivetrain_str().to_string();
                self.cached_num_cylinders  = pkt.num_cylinders;
                if pkt.speed >= 0.1 {
                    self.max_power_ps  = self.max_power_ps.max(pkt.power_ps());
                    self.max_torque_nm = self.max_torque_nm.max(pkt.torque_nm());
                    self.max_boost_psi = self.max_boost_psi.max(pkt.boost);
                }

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
                match self.config.speed_delta_mode {
                    SpeedDeltaMode::Calculate => {
                        if let Some(&(_, oldest)) = self.speed_history.front() {
                            self.speed_delta_kmh = cur_kmh - oldest;
                        }
                    }
                    SpeedDeltaMode::Track => {
                        if self.last_track_instant
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
                ui.selectable_value(&mut self.current_tab, Tab::Dashboard,
                    format!("{} Dashboard", icons::DASHBOARD));
                ui.selectable_value(&mut self.current_tab, Tab::Acceleration,
                    format!("{} Acceleration", icons::BOLT));
                ui.selectable_value(&mut self.current_tab, Tab::Deceleration,
                    format!("{} Deceleration", icons::STOP));
                ui.selectable_value(&mut self.current_tab, Tab::PowerCurve,
                    format!("{} Power Curve", icons::LINE_CHART));
                ui.selectable_value(&mut self.current_tab, Tab::EngineSwaps,
                    format!("{} Engine Swaps", icons::WRENCH));
                ui.selectable_value(&mut self.current_tab, Tab::Settings,
                    format!("{} Settings", icons::COG));
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
                    let resp = ui.add(
                        egui::Label::new(
                            egui::RichText::new(icons::COG).color(cog_color).size(16.0),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if resp.clicked() {
                        self.page_settings_open = !self.page_settings_open;
                        self.page_settings_tab = self.current_tab;
                        if !self.page_settings_open {
                            self.config.save();
                            self.car_settings.save();
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
                .fixed_size([600.0, 600.0])
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -36.0))
                .frame(egui::Frame::window(&ctx.style()).multiply_with_opacity(opacity))
                .show(ctx, |ui| {
                    ui.set_opacity(opacity);
                    use crate::config::{SpeedDeltaMode, SprintType, TextAlign, TireSlipStyle};
                    use crate::icons;

                    // Main tab row (no Settings tab here)
                    ui.horizontal(|ui| {
                        for (tab, lbl) in [
                            (Tab::Dashboard,    "Dashboard"),
                            (Tab::Acceleration, "Accel"),
                            (Tab::Deceleration, "Decel"),
                            (Tab::PowerCurve,   "Power"),
                            (Tab::EngineSwaps,  "Engines"),
                        ] {
                            ui.selectable_value(&mut self.page_settings_tab, tab, lbl);
                        }
                    });
                    ui.separator();

                    ui.set_min_height(540.0);

                    match self.page_settings_tab {
                        Tab::Dashboard => {
                            // Sub-tab row
                            ui.horizontal(|ui| {
                                for (sub, lbl) in [
                                    (DashboardSubTab::General,     "General"),
                                    (DashboardSubTab::Kmh,         "Km/h"),
                                    (DashboardSubTab::Gear,        "Gear"),
                                    (DashboardSubTab::SprintTimes, "Sprint Times"),
                                    (DashboardSubTab::Tires,       "Tires"),
                                    (DashboardSubTab::Shift,       "Shift"),
                                ] {
                                    ui.selectable_value(&mut self.page_dashboard_sub_tab, sub, lbl);
                                }
                            });
                            ui.separator();
                            ui.add_space(8.0);

                            match self.page_dashboard_sub_tab {
                                DashboardSubTab::General => {
                                    ui.checkbox(&mut self.config.dynamic_width, "Dynamic width");
                                    ui.add_space(4.0);
                                    let lbl = if self.config.dynamic_width {
                                        "Minimum module width:"
                                    } else {
                                        "Module width:"
                                    };
                                    ui.label(lbl);
                                    ui.add(
                                        egui::Slider::new(
                                            &mut self.config.dashboard_block_width,
                                            200.0..=800.0,
                                        )
                                        .step_by(10.0)
                                        .suffix(" px"),
                                    );
                                }
                                DashboardSubTab::Kmh => {
                                    ui.horizontal(|ui| {
                                        ui.label("Alignment:");
                                        egui::ComboBox::from_id_salt("speed_align")
                                            .selected_text(match self.config.speed_align {
                                                TextAlign::Right            => "Right",
                                                TextAlign::Center           => "Center",
                                                TextAlign::RightPlaceholder => "Right w/ Placeholder",
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.speed_align, TextAlign::Right,            "Right");
                                                ui.selectable_value(&mut self.config.speed_align, TextAlign::Center,           "Center");
                                                ui.selectable_value(&mut self.config.speed_align, TextAlign::RightPlaceholder, "Right w/ Placeholder");
                                            });
                                    });
                                    ui.add_space(8.0);
                                    ui.checkbox(&mut self.config.show_speed_delta, "Show Accel/Decel Tracker");
                                    if self.config.show_speed_delta {
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Mode:");
                                            egui::ComboBox::from_id_salt("speed_delta_mode")
                                                .selected_text(match self.config.speed_delta_mode {
                                                    SpeedDeltaMode::Track     => "Track (1s comparison)",
                                                    SpeedDeltaMode::Calculate => "Calculate (frame-to-frame)",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut self.config.speed_delta_mode, SpeedDeltaMode::Track,     "Track (1s comparison)");
                                                    ui.selectable_value(&mut self.config.speed_delta_mode, SpeedDeltaMode::Calculate, "Calculate (frame-to-frame)");
                                                });
                                        });
                                    }
                                }
                                DashboardSubTab::Gear => {
                                    ui.horizontal(|ui| {
                                        ui.label("Alignment:");
                                        egui::ComboBox::from_id_salt("gear_align")
                                            .selected_text(match self.config.gear_align {
                                                TextAlign::Right | TextAlign::RightPlaceholder => "Right",
                                                TextAlign::Center => "Center",
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.gear_align, TextAlign::Right,  "Right");
                                                ui.selectable_value(&mut self.config.gear_align, TextAlign::Center, "Center");
                                            });
                                    });
                                }
                                DashboardSubTab::SprintTimes => {
                                    ui.horizontal(|ui| {
                                        ui.label("Type:");
                                        egui::ComboBox::from_id_salt("sprint_type")
                                            .selected_text(match self.config.sprint_type {
                                                SprintType::Incremental => "Incremental (segment times)",
                                                SprintType::Absolute    => "Absolute (0 to X times)",
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.sprint_type, SprintType::Incremental, "Incremental (segment times)");
                                                ui.selectable_value(&mut self.config.sprint_type, SprintType::Absolute,    "Absolute (0 to X times)");
                                            });
                                    });
                                    ui.add_space(8.0);
                                    ui.checkbox(&mut self.config.sprint_show_other,
                                        "Show other type in parentheses");
                                }
                                DashboardSubTab::Tires => {
                                    use crate::config::TireDisplayStyle;
                                    ui.horizontal(|ui| {
                                        ui.label("Style:");
                                        egui::ComboBox::from_id_salt("tire_display_style")
                                            .selected_text(match self.config.tire_display_style {
                                                TireDisplayStyle::Separate => "Separate",
                                                TireDisplayStyle::Combined => "Combined",
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.config.tire_display_style, TireDisplayStyle::Separate, "Separate");
                                                ui.selectable_value(&mut self.config.tire_display_style, TireDisplayStyle::Combined, "Combined");
                                            });
                                    });
                                    if self.config.tire_display_style == TireDisplayStyle::Separate {
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Slip display style:");
                                            egui::ComboBox::from_id_salt("tire_slip_style")
                                                .selected_text(match self.config.tire_slip_style {
                                                    TireSlipStyle::Values => "Values",
                                                    TireSlipStyle::Graph  => "Graph",
                                                    TireSlipStyle::Both   => "Both",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut self.config.tire_slip_style, TireSlipStyle::Values, "Values");
                                                    ui.selectable_value(&mut self.config.tire_slip_style, TireSlipStyle::Graph,  "Graph");
                                                    ui.selectable_value(&mut self.config.tire_slip_style, TireSlipStyle::Both,   "Both");
                                                });
                                        });
                                    }
                                }
                                DashboardSubTab::Shift => {
                                    ui.label("Shift indicator thresholds (% of engine max RPM):");
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label("Low (warn):");
                                        ui.add(
                                            egui::Slider::new(&mut self.config.shift_low_pct, 50.0..=99.0)
                                                .suffix("%"),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label("High (shift):");
                                        ui.add(
                                            egui::Slider::new(&mut self.config.shift_high_pct, 51.0..=100.0)
                                                .suffix("%"),
                                        );
                                    });
                                }
                            }
                        }
                        Tab::PowerCurve => {
                            ui.checkbox(
                                &mut self.config.power_curve_forced_induction,
                                "Forced induction detection",
                            );
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "ON: hide boost graph if no positive pressure was captured.\n\
                                     OFF: always show the boost graph."
                                )
                                .size(11.0)
                                .color(egui::Color32::GRAY),
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
                    // Always fill the full window height
                    let rem = ui.available_height();
                    if rem > 0.0 { ui.add_space(rem); }
                    let _ = icons::COG;
                });
            let hovered = win_resp
                .map(|r| {
                    let mut rect = r.response.rect;
                    rect.set_bottom(ctx.screen_rect().bottom());
                    ctx.input(|i| i.pointer.hover_pos().map(|p| rect.contains(p)).unwrap_or(false))
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
