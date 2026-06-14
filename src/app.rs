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

#[derive(PartialEq, Clone, Copy)]
pub enum Tab {
    Dashboard,
    Acceleration,
    Deceleration,
    PowerCurve,
    EngineSwaps,
    Settings,
}

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
            if pkt.car_ordinal != 0 && pkt.car_ordinal != self.last_car_ordinal {
                self.last_car_ordinal = pkt.car_ordinal;
                self.sprint_timer.reset();
                self.power_capture.on_car_changed();
                self.perf_test.reset();
            }

            // Update per-car max RPM
            if pkt.car_ordinal != 0 && pkt.is_race_on != 0 {
                let car = self.car_settings.get_or_default(pkt.car_ordinal);
                if pkt.current_engine_rpm > car.max_rpm_measured {
                    car.max_rpm_measured = pkt.current_engine_rpm;
                }
            }

            self.sprint_timer.update(&pkt);
            self.power_capture.update(&pkt, step);
            self.perf_test.update(&pkt, accel_s, accel_e, decel_s, decel_e);
            self.telemetry.update(pkt);
            self.last_packet_time = Some(Instant::now());

            drained += 1;
            if drained >= 200 {
                break; // don't stall the render thread
            }
        }

        // Mark disconnected after 2 s without a packet
        if let Some(t) = self.last_packet_time {
            if t.elapsed() > Duration::from_secs(2) {
                self.telemetry.is_connected = false;
            }
        }
    }

    pub fn current_car_settings(&self) -> Option<&crate::config::CarSettings> {
        if self.last_car_ordinal != 0 {
            self.car_settings.cars.get(&self.last_car_ordinal)
        } else {
            None
        }
    }
}

impl eframe::App for ForzaApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_packets();

        // Apply theme
        match self.config.theme {
            Theme::Dark => ctx.set_visuals(egui::Visuals::dark()),
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

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (color, icon, text) = if self.telemetry.is_connected {
                        (egui::Color32::from_rgb(60, 200, 90), icons::PLUG, "Connected")
                    } else {
                        (egui::Color32::from_rgb(200, 60, 60), icons::NO_SIGNAL, "Disconnected")
                    };
                    ui.colored_label(color, format!("{icon} {text}"));
                    ui.label(format!("{:.0} pps", self.telemetry.packets_per_sec));
                });
            });
            ui.add_space(2.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.current_tab {
                Tab::Dashboard => crate::ui::dashboard::show(ui, self),
                Tab::Acceleration => crate::ui::acceleration::show(ui, self),
                Tab::Deceleration => crate::ui::deceleration::show(ui, self),
                Tab::PowerCurve => crate::ui::power_curve::show(ui, self),
                Tab::EngineSwaps => crate::ui::engine_swaps::show(ui, self),
                Tab::Settings => crate::ui::settings::show(ui, self),
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
