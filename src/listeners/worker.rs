//! The key-output listener thread: Backfire + the automatic gearbox, off the frame loop.
//!
//! **Why this thread exists.** GNOME/Wayland stops delivering frame callbacks for a window
//! that is minimized or fully occluded, and winit gates `RedrawRequested` on that callback
//! (`winit wayland/event_loop/mod.rs:486`), so `eframe::App::update` simply stops being
//! called. Anything that ran inside it stopped with it — which for Backfire and the gearbox
//! means the game loses its synthetic keypresses the moment the window is covered. Both
//! features must not depend on redraws, so they run here instead, straight off the UDP
//! channel, together with the state they need (per-car calibration, the detected redline,
//! the packet rate) and the global hotkeys that toggle them. Every packet is then forwarded
//! to the UI, which keeps doing everything else in `ForzaApp::drain_packets`.
//!
//! **Synchronisation** is two one-way mailboxes, each with its own mutex, plus an mpsc
//! channel for one-shot commands. Neither lock is ever held across a blocking call, disk IO
//! or UI drawing, and neither side ever holds both:
//!
//! - **listener → UI**: [`ListenerView`]. The thread locks only to overwrite it from its
//!   local copy, then unlocks. The UI `try_lock`s once a frame, clones, drops the guard
//!   immediately, and on a miss keeps last frame's copy.
//! - **UI → listener**: [`ToListener`] — the config plus the two focus facts only egui
//!   knows. The UI writes it with `try_lock` (a miss is retried next frame); the thread
//!   picks it up with `try_lock` once per loop.
//! - **commands** ([`Command`]) go down an `mpsc` channel, which never blocks the sender and
//!   never drops a one-shot the user asked for (a calibration reset, shutdown).
//!
//! Why so loose: the UI may show a one-frame-stale value, which is fine because nothing in
//! it is critical, while the loop that actually drives the game is never affected by the UI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::{
    load_car_calibrations, save_car_calibrations, AppConfig, CarCalibration, GateMode,
    HotkeyAction,
};
use crate::focus::FocusDetector;
use crate::input::InputSender;
use crate::listeners::backfire::{echo_grace, BackfireListener, BackfireView};
use crate::listeners::dsg::{DsgListener, DsgView};
use crate::packet::ForzaPacket;

/// Loop cadence when no packet arrives — also the worst-case shutdown latency.
const IDLE_POLL: Duration = Duration::from_millis(200);
/// No packet for this long ⇒ telemetry isn't live (the `TelemetryLive` hotkey gate).
const STALE_AFTER: Duration = Duration::from_secs(2);

/// Everything the UI displays out of the listener thread.
#[derive(Clone, Default)]
pub struct ListenerView {
    pub dsg: DsgView,
    pub backfire: BackfireView,
    /// Highest RPM seen while making power — the detected redline, per car.
    pub dynamic_max_rpm: f32,
    /// The enable flags as the thread has them; a global hotkey can flip either while the
    /// window is hidden and the UI is not running.
    pub dsg_enabled: bool,
    pub backfire_enabled: bool,
    /// Bumped on every hotkey toggle. The UI adopts the two flags whenever it sees a new
    /// generation, and echoes the generation back in [`ToListener::ack_gen`].
    pub toggle_gen: u64,
}

/// What the UI hands the thread each frame.
struct ToListener {
    cfg: AppConfig,
    /// The `toggle_gen` this config was built against. If it's behind ours, the UI hadn't
    /// seen our last hotkey toggle yet, so its enable flags are stale and we keep our own —
    /// otherwise the next config push would silently undo the key the user just pressed.
    ack_gen: u64,
    /// `ctx.input(|i| i.focused)` and `ctx.wants_keyboard_input()` — the global-hotkey gate
    /// needs both and neither exists off the UI thread. They stop updating while the window
    /// is hidden, which is harmless: a hidden window isn't focused and isn't typing.
    our_focused: bool,
    wants_text: bool,
}

/// One-shot requests from the UI. Sent over a channel so none is ever lost.
pub enum Command {
    /// Clear the detected redline + engagement; keeps the per-gear speed map.
    ClearRpmCalibration,
    /// Clear the per-gear speed map; keeps the detected redline.
    ClearGearMap,
    /// Flush the per-car calibrations and end the loop.
    Shutdown,
}

/// The UI's end of the listener thread.
pub struct ListenerHandle {
    view: Arc<Mutex<ListenerView>>,
    inbox: Arc<Mutex<Option<ToListener>>>,
    cmd_tx: Sender<Command>,
    join: Option<JoinHandle<()>>,
}

impl ListenerHandle {
    /// Copy the published state, or `None` if the thread holds the lock right now — the UI
    /// then keeps last frame's copy rather than waiting.
    pub fn try_view(&self) -> Option<ListenerView> {
        self.view.try_lock().ok().map(|v| v.clone())
    }

    /// Hand over the current config + focus facts. Pushed every frame (an `AppConfig` clone
    /// per frame, exactly what `drain_packets` used to do for `fun_cfg`), so a push the
    /// thread happens to be holding the lock for is simply repeated next frame.
    pub fn push(&self, cfg: &AppConfig, ack_gen: u64, our_focused: bool, wants_text: bool) {
        if let Ok(mut slot) = self.inbox.try_lock() {
            *slot = Some(ToListener {
                cfg: cfg.clone(),
                ack_gen,
                our_focused,
                wants_text,
            });
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Ask the thread to flush calibrations and stop, then wait for it (≤ `IDLE_POLL`).
    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Start the thread. It owns `packets` (the UDP channel) and forwards every packet on
/// `to_ui` for the UI-side features that stay tied to redraws.
pub fn spawn(
    packets: Receiver<ForzaPacket>,
    to_ui: Sender<ForzaPacket>,
    hotkeys: Receiver<HotkeyAction>,
    input: InputSender,
    input_allowed: Arc<AtomicBool>,
    focus: Arc<FocusDetector>,
    cfg: AppConfig,
) -> ListenerHandle {
    let view = Arc::new(Mutex::new(ListenerView {
        dsg_enabled: cfg.dsg_enabled,
        backfire_enabled: cfg.backfire_enabled,
        ..Default::default()
    }));
    let inbox: Arc<Mutex<Option<ToListener>>> = Arc::new(Mutex::new(None));
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();

    let (view_t, inbox_t) = (view.clone(), inbox.clone());
    let join = std::thread::spawn(move || {
        run(Ctx {
            packets,
            to_ui,
            hotkeys,
            cmd_rx,
            input,
            input_allowed,
            focus,
            view: view_t,
            inbox: inbox_t,
            cfg,
        })
    });

    ListenerHandle {
        view,
        inbox,
        cmd_tx,
        join: Some(join),
    }
}

/// Everything the loop is handed at spawn (a struct purely to keep `run`'s signature sane).
struct Ctx {
    packets: Receiver<ForzaPacket>,
    to_ui: Sender<ForzaPacket>,
    hotkeys: Receiver<HotkeyAction>,
    cmd_rx: Receiver<Command>,
    input: InputSender,
    input_allowed: Arc<AtomicBool>,
    focus: Arc<FocusDetector>,
    view: Arc<Mutex<ListenerView>>,
    inbox: Arc<Mutex<Option<ToListener>>>,
    cfg: AppConfig,
}

fn run(ctx: Ctx) {
    let Ctx {
        packets,
        to_ui,
        hotkeys,
        cmd_rx,
        input,
        input_allowed,
        focus,
        view,
        inbox,
        mut cfg,
    } = ctx;

    let mut dsg = DsgListener::new();
    let mut backfire = BackfireListener::new();
    let mut cals = load_car_calibrations();
    let mut dynamic_max_rpm = 0.0_f32;
    let mut last_car_ordinal = 0_i32;
    let mut toggle_gen = 0_u64;
    let mut our_focused = false;
    let mut wants_text = false;
    // Own packet-rate measurement (Backfire's dynamic press length needs it). The UI's
    // `TelemetryState::packets_per_sec` is computed per frame and stops when frames do.
    let mut pps = 0.0_f32;
    let mut pps_count = 0_u32;
    let mut pps_since = Instant::now();
    let mut last_packet: Option<Instant> = None;

    loop {
        // ── UI → listener: config + focus facts ────────────────────────────
        if let Ok(mut slot) = inbox.try_lock() {
            if let Some(msg) = slot.take() {
                let stale_toggles = msg.ack_gen != toggle_gen;
                let (dsg_on, bf_on) = (cfg.dsg_enabled, cfg.backfire_enabled);
                our_focused = msg.our_focused;
                wants_text = msg.wants_text;
                cfg = msg.cfg;
                if stale_toggles {
                    cfg.dsg_enabled = dsg_on;
                    cfg.backfire_enabled = bf_on;
                }
            }
        }

        // ── One-shot commands ──────────────────────────────────────────────
        let mut stop = false;
        for cmd in cmd_rx.try_iter() {
            match cmd {
                Command::ClearRpmCalibration => {
                    dsg.reset_state();
                    dynamic_max_rpm = 0.0;
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                Command::ClearGearMap => {
                    dsg.reset_calibration();
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                Command::Shutdown => stop = true,
            }
        }
        if stop {
            break;
        }

        // ── Global hotkeys ─────────────────────────────────────────────────
        // Here rather than in the frame loop: an auto-shifter the user can't switch off
        // while the window is hidden would be worse than one that stops.
        while let Ok(action) = hotkeys.try_recv() {
            let game_focused = match cfg.hotkeys.gate_mode {
                GateMode::TelemetryLive => {
                    last_packet.map(|t| t.elapsed() < STALE_AFTER).unwrap_or(false)
                }
                GateMode::WindowFocus => focus.focused(),
            };
            if !crate::app::global_hotkey_allowed(our_focused, wants_text, game_focused) {
                continue;
            }
            match action {
                HotkeyAction::ToggleGearbox => {
                    cfg.dsg_enabled = !cfg.dsg_enabled;
                    toggle_gen += 1;
                }
                HotkeyAction::ToggleBackfire => {
                    cfg.backfire_enabled = !cfg.backfire_enabled;
                    toggle_gen += 1;
                }
                HotkeyAction::ResetCalibration => {
                    dsg.reset_state();
                    dynamic_max_rpm = 0.0;
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                _ => {}
            }
        }

        // Synthetic-input focus gate — also moved off the frame loop, or key emission would
        // freeze at whatever the last drawn frame stored.
        input_allowed.store(
            !cfg.hotkeys.input_focus_gate || focus.focused(),
            Ordering::Relaxed,
        );

        // ── One packet. The only blocking call, and no lock is held across it. ──
        match packets.recv_timeout(IDLE_POLL) {
            Ok(pkt) => {
                last_packet = Some(Instant::now());
                pps_count += 1;
                let elapsed = pps_since.elapsed().as_secs_f32();
                if elapsed >= 1.0 {
                    pps = pps_count as f32 / elapsed;
                    pps_count = 0;
                    pps_since = Instant::now();
                }

                if pkt.car_ordinal != 0 && pkt.car_ordinal != last_car_ordinal {
                    last_car_ordinal = pkt.car_ordinal;
                    dsg.reset_calibration();
                    dsg.reset_state();
                    dynamic_max_rpm = 0.0;
                    // Opt-in: flush saved calibrations to disk (car change is a natural
                    // checkpoint), then restore the new car's profile and skip the manual
                    // 1st→2nd pull.
                    if cfg.dsg_save_calibration {
                        save_car_calibrations(&cals);
                        if let Some(cal) = cals.get(&pkt.car_ordinal) {
                            dsg.gear_redline_speeds = cal.gear_redline_speeds;
                            dsg.engaged = true;
                            dynamic_max_rpm = cal.max_rpm;
                        }
                    }
                }

                // Dynamic redline: highest RPM seen while the engine is making power,
                // ignoring handbrake / slipping tyres (>0.5) — those inflate RPM without
                // real road speed. The gearbox's only redline source, so it lives here.
                if pkt.is_race_on != 0
                    && pkt.power > 0.0
                    && pkt.hand_brake == 0
                    && pkt.tire_slip_ratio_fl.abs() <= 0.5
                    && pkt.tire_slip_ratio_fr.abs() <= 0.5
                    && pkt.tire_slip_ratio_rl.abs() <= 0.5
                    && pkt.tire_slip_ratio_rr.abs() <= 0.5
                {
                    dynamic_max_rpm = dynamic_max_rpm.max(pkt.current_engine_rpm);
                }

                backfire.update(&pkt, &cfg, &input, pps);
                let suppress_gearbox_accel =
                    cfg.dsg_ignore_backfire_accel && input.synthetic_active(echo_grace(&cfg));
                dsg.update(&pkt, &cfg, &input, dynamic_max_rpm, suppress_gearbox_accel);

                // Opt-in: keep the in-memory per-car calibration current; written to its own
                // file on car change and on exit.
                if cfg.dsg_save_calibration
                    && pkt.car_ordinal != 0
                    && dsg.gear_redline_speeds[1] > 0.0
                {
                    cals.insert(
                        pkt.car_ordinal,
                        CarCalibration {
                            gear_redline_speeds: dsg.gear_redline_speeds,
                            max_rpm: dynamic_max_rpm,
                        },
                    );
                }

                // Everything else (stats, Co-Op, telemetry, widgets) stays UI-side.
                if to_ui.send(pkt).is_err() {
                    break; // UI gone
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break, // app dropped its sender
        }

        // ── listener → UI: overwrite the mailbox from our local copy, then unlock. ──
        // A plain `lock` is fine: the UI only ever holds this for a clone, so the longest
        // possible wait is a memcpy — never a draw, a packet or disk IO.
        if let Ok(mut published) = view.lock() {
            *published = ListenerView {
                dsg: dsg.view(),
                backfire: backfire.view(),
                dynamic_max_rpm,
                dsg_enabled: cfg.dsg_enabled,
                backfire_enabled: cfg.backfire_enabled,
                toggle_gen,
            };
        }
    }

    if cfg.dsg_save_calibration {
        save_car_calibrations(&cals);
    }
}

/// Rewrite (or drop) the saved per-car profile to match the live calibration, so a car
/// reload can't restore a part we just cleared. Entry removed once nothing's left to save.
fn persist_calibration(
    cals: &mut HashMap<i32, CarCalibration>,
    car_ordinal: i32,
    dsg: &DsgListener,
    dynamic_max_rpm: f32,
) {
    if dsg.gear_redline_speeds[1] > 0.0 || dynamic_max_rpm > 0.0 {
        cals.insert(
            car_ordinal,
            CarCalibration {
                gear_redline_speeds: dsg.gear_redline_speeds,
                max_rpm: dynamic_max_rpm,
            },
        );
    } else {
        cals.remove(&car_ordinal);
    }
    save_car_calibrations(cals);
}
