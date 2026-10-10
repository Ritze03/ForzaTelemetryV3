//! The key-output listener thread: Backfire + the automatic gearbox, off the frame loop.
//!
//! **Why this thread exists.** GNOME/Wayland stops delivering frame callbacks for a window
//! that is minimized or fully occluded, and winit gates `RedrawRequested` on that callback
//! (`winit wayland/event_loop/mod.rs:486`), so `eframe::App::update` simply stops being
//! called. Anything that ran inside it stopped with it — which for Backfire and the gearbox
//! means the game loses its synthetic keypresses the moment the window is covered. Both
//! features must not depend on redraws, so they run here instead, straight off the UDP
//! channel, together with the state they need (per-car calibration, the detected redline,
//! the packet rate) and the global hotkeys that toggle them. Co-Op's outgoing relay runs here
//! for the same reason (peers must see us move while the game covers the window). Every
//! packet is then handed to the UI, which keeps doing everything else in `ForzaApp::drain_packets`.
//!
//! **Synchronisation** is three one-way mailboxes, each with its own mutex, plus an mpsc
//! channel for one-shot commands. No lock is ever held across a blocking call, disk IO or
//! UI drawing, and no side ever holds two at once:
//!
//! - **listener → UI**: [`ListenerView`]. The thread locks only to overwrite it from its
//!   local copy, then unlocks. The UI `try_lock`s once a frame, clones, drops the guard
//!   immediately, and on a miss keeps last frame's copy.
//! - **UI → listener**: [`ToListener`] — the config plus the two focus facts only egui
//!   knows. The UI writes it with `try_lock` (a miss is retried next frame); the thread
//!   picks it up with `try_lock` once per loop.
//! - **listener → UI packet queue**: an `Arc<Mutex<VecDeque<ForzaPacket>>>` capped at
//!   [`UI_BACKLOG_CAP`], oldest dropped on overflow. Same mailbox style, and unlike a
//!   channel it can't grow without bound while the UI isn't draining it.
//! - **commands** ([`Command`]) go down an `mpsc` channel, which never blocks the sender and
//!   never drops a one-shot the user asked for (a calibration reset, shutdown).
//!
//! - **listener → overlay** (optional, attached with [`ListenerHandle::set_hud_sink`]): a
//!   [`HudSnapshot`](crate::overlay::snapshot::HudSnapshot) written latest-wins into the
//!   overlay's slot plus a wake ping — on every packet (D17: one redraw per packet) and
//!   whenever the visibility target changes without one (pause/timeout/focus/Hide HUD).
//!
//! Why so loose: the UI may show a one-frame-stale value, which is fine because nothing in
//! it is critical, while the loop that actually drives the game is never affected by the UI.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::{
    load_car_calibrations, save_car_calibrations, AppConfig, CarCalibration, GateMode,
    HotkeyAction,
};
use crate::coop::CoopReader;
use crate::focus::FocusDetector;
use crate::input::InputSender;
use crate::listeners::backfire::{BackfireListener, BackfireView};
use crate::listeners::calib::next_max_rpm;
use crate::listeners::dsg::{DsgListener, DsgView};
use crate::listeners::hud::{hud_paused, HudTracker, VisFacts};
use crate::listeners::notify::Notifier;
use crate::overlay::snapshot::{hud_clock, HudSink};
use crate::packet::ForzaPacket;

/// Loop cadence when no packet arrives — also the worst-case shutdown latency, and the
/// granularity at which the HUD notices a lost stream (hidden 2.0–2.2 s after the last
/// packet) or a focus change while no packets arrive.
const IDLE_POLL: Duration = Duration::from_millis(200);
/// No packet for this long ⇒ telemetry isn't live (the `TelemetryLive` hotkey gate).
const STALE_AFTER: Duration = Duration::from_secs(2);
/// How long a pushed [`ToListener::our_focused`] / `wants_text` stays believable.
///
/// They only change when the UI is drawn, and a hidden window is never drawn — so without
/// an expiry they would freeze at whatever the last visible frame said, and the global
/// hotkey gate would be stuck open (bare `G`/`B`/`F` typed in *any* other app would toggle
/// the gearbox or wipe the calibration) or stuck shut (if a text field had focus). After
/// this long with no push we treat the window as neither focused nor typing, which is what
/// a hidden window actually is; the gate then falls back to "is the game focused?".
///
/// 1 s is ~5× the slowest frame the UI can legitimately take: `update` always re-arms a
/// repaint (`request_repaint_after(1/fps_limit)` or `request_repaint()`, `app.rs`), and the
/// FPS-limit slider floor is 5 fps = 200 ms (`ui/settings.rs`). A hand-edited `fps_limit`
/// below 1 would expire the facts early, which only ever fails *closed* (hotkeys then need
/// the game focused), never open.
const FOCUS_FACTS_TTL: Duration = Duration::from_secs(1);
/// Packets held for the UI at most. The UI drains every frame; while it isn't being drawn
/// the queue fills and the **oldest** are dropped, so a restore replays at most this many
/// packets instead of minutes of stale telemetry (and the queue can't grow without bound).
pub const UI_BACKLOG_CAP: usize = 200;

/// Largest packet-time step the navigator counts for one packet (ms): a stalled loop or the
/// first packet after a long gap is one step of this, not seconds spent "off the route".
const NAV_DT_CAP_MS: u128 = 250;
/// How often the co-op shared destination is looked at (one lock of the co-op state).
const NAV_COOP_POLL_MS: u64 = 100;

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
    /// Hide HUD hotkey state (D16). Listener-owned runtime state, not config: nothing the
    /// UI pushes can overwrite it, so it needs no `toggle_gen` protection, and it resets to
    /// shown on restart.
    pub hud_hidden: bool,
    /// Race-vs-drift mode from the DSG listener's classifier (always running, unlike the
    /// HUD's own copy which only runs while the overlay is on). Shown in the Debug tab.
    pub hud_mode: crate::overlay::snapshot::HudMode,
    /// Packets per second as the listener thread measures it on receipt (1 s windows; `0.0`
    /// once no packet arrived for [`PpsMeter::STALE`]). The status bar shows this: the UI
    /// thread's own count depends on when frames drain the queue, so any UI stall made it swing.
    pub pps: f32,
}

/// Packets-per-second over 1 s windows, counted when each packet is received.
struct PpsMeter {
    /// Last finished window's rate.
    rate: f32,
    count: u32,
    since: Instant,
    last: Option<Instant>,
}

impl PpsMeter {
    /// No packet for this long and [`Self::published`] reads 0: `rate` only moves on a packet,
    /// so it would otherwise stay frozen at the last value after the game stops sending.
    const STALE: Duration = Duration::from_secs(2);

    fn new(now: Instant) -> Self {
        Self { rate: 0.0, count: 0, since: now, last: None }
    }

    fn on_packet(&mut self, now: Instant) {
        // After a silence (game paused / closed) the open window would make the first packet
        // back report 1 / gap; start a fresh one instead.
        if self.last.is_some_and(|t| now.duration_since(t) > Duration::from_secs(1)) {
            self.count = 0;
            self.since = now;
        }
        self.last = Some(now);
        self.count += 1;
        let elapsed = now.duration_since(self.since).as_secs_f32();
        if elapsed >= 1.0 {
            self.rate = self.count as f32 / elapsed;
            self.count = 0;
            self.since = now;
        }
    }

    /// The value for the UI: `rate`, or 0 when the stream stopped.
    fn published(&self, now: Instant) -> f32 {
        if self.last.is_some_and(|t| now.duration_since(t) < Self::STALE) { self.rate } else { 0.0 }
    }
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
    /// is hidden, so the listener ages them out after `FOCUS_FACTS_TTL` rather than trusting
    /// a frozen value.
    our_focused: bool,
    wants_text: bool,
}

/// One-shot requests from the UI. Sent over a channel so none is ever lost.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))] // SetHudSink: the overlay exists on Linux and Windows only
pub enum Command {
    /// Clear the detected redline + engagement; keeps the per-gear speed map. Same as the
    /// "Clear RPM calibration" hotkey / controller action.
    ClearRpmCalibration,
    /// Clear the per-gear speed map; keeps the detected redline. Same as the "Clear gear map"
    /// hotkey / controller action.
    ClearGearMap,
    /// Attach (`Some`) or detach (`None`) the HUD overlay's mailbox. With none attached the
    /// listener builds no snapshots.
    SetHudSink(Option<HudSink>),
    /// Flush the per-car calibrations and end the loop.
    Shutdown,
}

/// Packets waiting for the UI, newest-wins (see [`UI_BACKLOG_CAP`]).
pub type PacketQueue = Arc<Mutex<VecDeque<ForzaPacket>>>;

/// The UI's end of the listener thread.
pub struct ListenerHandle {
    view: Arc<Mutex<ListenerView>>,
    inbox: Arc<Mutex<Option<ToListener>>>,
    packets: PacketQueue,
    cmd_tx: Sender<Command>,
    join: Option<JoinHandle<()>>,
}

impl ListenerHandle {
    /// Copy the published state, or `None` if the thread holds the lock right now — the UI
    /// then keeps last frame's copy rather than waiting.
    pub fn try_view(&self) -> Option<ListenerView> {
        self.view.try_lock().ok().map(|v| v.clone())
    }

    /// Blocking read, for the one caller that must not miss: `on_exit`, where a `try_view`
    /// miss would save a stale Backfire/Gearbox toggle. Safe to block on — the thread holds
    /// this lock only long enough to overwrite the struct.
    pub fn view_now(&self) -> Option<ListenerView> {
        self.view.lock().ok().map(|v| v.clone())
    }

    /// Take everything queued for the UI, leaving the deque empty. The lock is held only
    /// for the `take` — the caller processes packets after it is dropped.
    pub fn take_packets(&self) -> VecDeque<ForzaPacket> {
        match self.packets.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => VecDeque::new(),
        }
    }

    /// True once the listener thread has stopped while the app is still running — i.e. it
    /// panicked. Backfire and the gearbox are then dead, so the UI says so instead of
    /// showing a frozen "Active".
    pub fn is_dead(&self) -> bool {
        self.join.as_ref().is_some_and(|j| j.is_finished())
    }

    /// Hand over the current config + focus facts. Pushed every frame (an `AppConfig` clone
    /// per frame, exactly what `drain_packets` used to do for `fun_cfg`), so a push the
    /// thread happens to be holding the lock for is simply repeated next frame.
    pub fn push(&self, cfg: &AppConfig, ack_gen: u64, our_focused: bool, wants_text: bool) {
        // Cloned before taking the lock: the listener must never wait on our allocator.
        let msg = ToListener {
            cfg: cfg.clone(),
            ack_gen,
            our_focused,
            wants_text,
        };
        if let Ok(mut slot) = self.inbox.try_lock() {
            *slot = Some(msg);
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Attach the overlay (`Some`) or detach it (`None`); see [`Command::SetHudSink`].
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))] // the overlay exists on Linux and Windows only
    pub fn set_hud_sink(&self, sink: Option<HudSink>) {
        self.send(Command::SetHudSink(sink));
    }

    /// Ask the thread to flush calibrations and stop, then wait for it (≤ `IDLE_POLL`).
    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Start the thread. It owns `udp` (the UDP channel) and hands every packet to the UI
/// through the shared [`PacketQueue`], for the features that stay tied to redraws.
pub fn spawn(
    udp: Receiver<ForzaPacket>,
    hotkeys: Receiver<HotkeyAction>,
    input: InputSender,
    input_allowed: Arc<AtomicBool>,
    focus: Arc<FocusDetector>,
    cfg: AppConfig,
    coop: CoopReader,
) -> ListenerHandle {
    let view = Arc::new(Mutex::new(ListenerView {
        dsg_enabled: cfg.dsg_enabled,
        backfire_enabled: cfg.backfire_enabled,
        ..Default::default()
    }));
    let inbox: Arc<Mutex<Option<ToListener>>> = Arc::new(Mutex::new(None));
    let to_ui: PacketQueue = Arc::new(Mutex::new(VecDeque::new()));
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();

    let (view_t, inbox_t, to_ui_t) = (view.clone(), inbox.clone(), to_ui.clone());
    let join = std::thread::spawn(move || {
        run(Ctx {
            udp,
            to_ui: to_ui_t,
            hotkeys,
            cmd_rx,
            input,
            input_allowed,
            focus,
            view: view_t,
            inbox: inbox_t,
            cfg,
            coop,
        })
    });

    ListenerHandle {
        view,
        inbox,
        packets: to_ui,
        cmd_tx,
        join: Some(join),
    }
}

/// Everything the loop is handed at spawn (a struct purely to keep `run`'s signature sane).
struct Ctx {
    udp: Receiver<ForzaPacket>,
    to_ui: PacketQueue,
    hotkeys: Receiver<HotkeyAction>,
    cmd_rx: Receiver<Command>,
    input: InputSender,
    input_allowed: Arc<AtomicBool>,
    focus: Arc<FocusDetector>,
    view: Arc<Mutex<ListenerView>>,
    inbox: Arc<Mutex<Option<ToListener>>>,
    cfg: AppConfig,
    coop: CoopReader,
}

fn run(ctx: Ctx) {
    let Ctx {
        udp,
        to_ui,
        hotkeys,
        cmd_rx,
        input,
        input_allowed,
        focus,
        view,
        inbox,
        mut cfg,
        coop,
    } = ctx;

    let mut dsg = DsgListener::new();
    let mut backfire = BackfireListener::new();
    let mut cals = load_car_calibrations();
    let mut dynamic_max_rpm = 0.0_f32;
    let mut last_car_ordinal = 0_i32;
    let mut toggle_gen = 0_u64;
    let mut our_focused = false;
    let mut wants_text = false;
    // When the UI last pushed; None = never. Ages out via FOCUS_FACTS_TTL.
    let mut last_push: Option<Instant> = None;
    // Own packet-rate measurement (Backfire's dynamic press length needs it; the status bar
    // shows it too, via `ListenerView::pps`). It lives here, not on the UI thread, because the
    // UI drains in batches whenever a frame stalls, which made a UI-side count swing.
    let mut pps = PpsMeter::new(Instant::now());
    let mut last_packet: Option<Instant> = None;
    // HUD overlay: per-packet derived state, the attached mailbox, the Hide HUD toggle, and
    // the last published visibility (a change without a packet still has to be published).
    let mut hud = HudTracker::new();
    let mut hud_sink: Option<HudSink> = None;
    let mut hud_hidden = false;
    let mut hud_visible = false;
    let mut hud_force = false;
    let mut hud_cfg = Arc::new(cfg.overlay.effective(&cfg));
    // D26 on-HUD messages; `in_race` = race position ≠ 0 on the last race-on packet.
    let mut notifier = Notifier::default();
    let mut in_race = false;
    // The car on the last packet while the game is running and not paused (Calibration started).
    let mut driving_car: Option<i32> = None;
    // Co-Op: (class, PI) of the last race-on packet, restored into paused packets.
    let mut coop_car = (-1, 0);
    // Navigation (`nav`): progress / off-route / re-route per packet, here and not in the UI
    // because the UI loop stops while the game covers the window. The route itself is computed
    // on the `nav-route` thread; while no destination exists a tick is one atomic load.
    let mut nav = crate::nav::Tracker::global();
    let nav_start = Instant::now();
    let mut nav_prev_pkt: Option<Instant> = None;
    let mut nav_coop_seq = u64::MAX;
    let mut nav_coop_next = 0_u64;

    loop {
        // ── UI → listener: config + focus facts ────────────────────────────
        if let Ok(mut slot) = inbox.try_lock() {
            if let Some(msg) = slot.take() {
                let stale_toggles = msg.ack_gen != toggle_gen;
                let (dsg_on, bf_on) = (cfg.dsg_enabled, cfg.backfire_enabled);
                our_focused = msg.our_focused;
                wants_text = msg.wants_text;
                last_push = Some(Instant::now());
                if msg.cfg.overlay.enabled && !cfg.overlay.enabled {
                    hud.reset(); // no stale "previous packet" from before it was off
                    // why: enabling should show the HUD; a hidden state left over from
                    // before would make it look broken.
                    hud_hidden = false;
                }
                cfg = msg.cfg;
                // The HUD draws with the effective config (Dashboard values where "Use Dashboard ...
                // settings" is ticked), so the renderer never branches on those flags.
                let eff = cfg.overlay.effective(&cfg);
                if *hud_cfg != eff {
                    hud_cfg = Arc::new(eff);
                }
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
                    clear_rpm_calibration(&mut dsg, &mut dynamic_max_rpm);
                    notifier.calibration_reset(&cfg.overlay, last_car_ordinal, hud_clock());
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                Command::ClearGearMap => {
                    dsg.reset_calibration();
                    notifier.calibration_reset(&cfg.overlay, last_car_ordinal, hud_clock());
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                Command::SetHudSink(sink) => {
                    hud_sink = sink;
                    hud_force = true; // the new sink gets a snapshot right away
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
            // Stale focus facts mean the UI hasn't been drawn for a while — i.e. the window
            // is hidden, so it is neither focused nor typing. See FOCUS_FACTS_TTL.
            let fresh = last_push.map(|t| t.elapsed() < FOCUS_FACTS_TTL).unwrap_or(false);
            let (ours, typing) = if fresh { (our_focused, wants_text) } else { (false, false) };
            if !crate::app::global_hotkey_allowed(ours, typing, game_focused) {
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
                    clear_rpm_calibration(&mut dsg, &mut dynamic_max_rpm);
                    notifier.calibration_reset(&cfg.overlay, last_car_ordinal, hud_clock());
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                HotkeyAction::ClearGearMap => {
                    dsg.reset_calibration();
                    notifier.calibration_reset(&cfg.overlay, last_car_ordinal, hud_clock());
                    persist_calibration(&mut cals, last_car_ordinal, &dsg, dynamic_max_rpm);
                }
                // why: ignored while the overlay is off. The key may double as a game key
                // (H = FH6's horn), and an invisible flip would leave the next enable blank.
                HotkeyAction::HideHud if cfg.overlay.enabled => hud_hidden = !hud_hidden,
                HotkeyAction::HideHud => {}
                HotkeyAction::MiniSettings | HotkeyAction::DashboardEdit => {}
            }
        }

        // Synthetic-input focus gate — also moved off the frame loop, or key emission would
        // freeze at whatever the last drawn frame stored.
        input_allowed.store(
            !cfg.hotkeys.input_focus_gate || focus.focused(),
            Ordering::Relaxed,
        );

        // ── One packet. The only blocking call, and no lock is held across it. ──
        // A message queued above (reset hotkey / command) goes out at once instead of after
        // the idle wait, packet or no packet.
        let poll = if notifier.has_fresh() { Duration::from_millis(1) } else { IDLE_POLL };
        let mut got_packet = false;
        let mut nav_sample = None;
        match udp.recv_timeout(poll) {
            Ok(pkt) => {
                got_packet = true;
                // Tracked whenever the overlay is enabled, attached or not, so a lap
                // started before the overlay came up still has its trace.
                if cfg.overlay.enabled {
                    hud.on_packet(&pkt, &cfg.overlay, cfg.experimental_pause_detection, hud_clock());
                }
                last_packet = Some(Instant::now());
                pps.on_packet(Instant::now());

                if pkt.is_race_on != 0 {
                    in_race = pkt.race_position != 0;
                }
                // Same pause rule as the HUD (includes race-on).
                driving_car = (!hud_paused(&pkt, cfg.experimental_pause_detection) && pkt.car_ordinal != 0)
                    .then_some(pkt.car_ordinal);
                let nav_now = Instant::now();
                let dt_ms = nav_prev_pkt.map_or(0, |t| nav_now.duration_since(t).as_millis().min(NAV_DT_CAP_MS) as u32);
                nav_prev_pkt = Some(nav_now);
                nav_sample = Some(crate::nav::CarSample {
                    x: pkt.position_x,
                    y: pkt.position_y,
                    z: pkt.position_z,
                    dt_ms,
                    driving: driving_car.is_some(),
                    in_race,
                });
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
                    // "Calibration started" for an uncalibrated new car comes from the level
                    // rule in `Notifier::watch` (car change = new episode), not from here.
                }

                // Dynamic redline: highest RPM seen before the first manual upshift, ignoring
                // handbrake / slipping tyres (>0.5) — those inflate RPM without
                // real road speed. The gearbox's only redline source, so it lives here.
                // The conditions live in `listeners/calib.rs` (shared with the Debug tab).
                // Locked once the box is engaged (`dsg.engaged`): only Clear RPM calibration, a car
                // change or a restored profile changes it. See `MaxRpmChecks::unlocked`.
                dynamic_max_rpm = next_max_rpm(dynamic_max_rpm, &pkt, dsg.engaged);

                backfire.update(&pkt, &cfg, &input, pps.rate);
                // No grace here: `echo_grace` exists for consumers that read the window a
                // frame late behind an FPS limit, and this thread reads it the instant the
                // packet lands. See `backfire::echo_grace`.
                let suppress_gearbox_accel =
                    cfg.dsg_ignore_backfire_accel && input.synthetic_active(Duration::ZERO);
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

                // Co-Op relay to peers. Here, not in `drain_packets`, because the UI loop stops
                // while the game covers the window — exactly while we're driving.
                coop.push_local(&crate::coop::outgoing(&pkt, &mut coop_car));

                // Everything else (stats, telemetry, widgets) stays UI-side. Hand the
                // packet over and drop the oldest if the UI isn't draining (hidden window);
                // the lock covers only the push, never the processing above or below.
                if let Ok(mut q) = to_ui.lock() {
                    if q.len() >= UI_BACKLOG_CAP {
                        q.pop_front();
                    }
                    q.push_back(pkt);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break, // app dropped its sender
        }

        // ── Navigation: the room's shared destination, then the tracker (packet or idle tick) ──
        let nav_now = nav_start.elapsed().as_millis() as u64;
        if nav_now >= nav_coop_next {
            nav_coop_next = nav_now + NAV_COOP_POLL_MS;
            let seq = coop.destination_seq();
            if seq != nav_coop_seq {
                nav_coop_seq = seq;
                nav.set_shared(nav_now, coop.destination().map(|d| crate::nav::SharedIn {
                    x: d.x,
                    z: d.z,
                    hue: d.hue,
                    filter_bits: d.filters(),
                    curve: d.curve,
                    ts: d.ts,
                    setter: d.setter_name,
                }));
            }
        }
        nav.tick(nav_now, nav_sample.as_ref());

        // ── D26: queue what changed (hotkeys and UI settings alike). ──
        notifier.watch(&cfg, in_race, dsg.engaged, dsg.gear_redline_speeds[1] > 0.0, dsg.gear1_seq, dynamic_max_rpm, driving_car, hud_clock());
        hud_force |= notifier.take_new(); // a message must reach the HUD even without a packet

        // ── listener → overlay: every packet, plus any visibility change without one. ──
        if let Some(sink) = &hud_sink {
            let now = hud_clock();
            let facts = VisFacts {
                enabled: cfg.overlay.enabled,
                hud_hidden,
                focus_only: cfg.overlay.focus_only,
                game_focused: focus.focused(),
                ..Default::default()
            };
            // The gearbox's redline counts once the DSG itself trusts it: after the first
            // manual upshift (or a restored profile). Runs with the DSG switched off too.
            let calibrated = if dsg.engaged { dynamic_max_rpm } else { 0.0 };
            let mut snap = hud.snapshot(facts, &hud_cfg, &cfg, calibrated, now);
            notifier.hold_unseen(snap.visible, now);
            snap.notifications = notifier.items().to_vec();
            if got_packet || hud_force || snap.visible != hud_visible {
                hud_visible = snap.visible;
                hud_force = false;
                sink.publish(snap);
            }
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
                hud_hidden,
                hud_mode: dsg.hud_mode,
                pps: pps.published(Instant::now()),
            };
        }
    }

    if cfg.dsg_save_calibration {
        save_car_calibrations(&cals);
    }
}

/// Forget the detected redline and the engagement (the box goes hands-off until the next
/// manual upshift); the per-gear speed map stays. Shared by the hotkey / controller action and
/// the tab button. (The gear map is cleared by `DsgListener::reset_calibration`.)
fn clear_rpm_calibration(dsg: &mut DsgListener, dynamic_max_rpm: &mut f32) {
    dsg.reset_state();
    *dynamic_max_rpm = 0.0;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pps_meter_measures_the_stream_and_reads_zero_when_it_stops() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut m = PpsMeter::new(t0);
        assert_eq!(m.published(t0), 0.0, "no packet yet");
        // 70 Hz-ish (every 14 ms) for 2.5 s.
        for i in 1..=178 {
            m.on_packet(ms(i * 14));
        }
        let r = m.published(ms(178 * 14));
        assert!((r - 71.0).abs() < 2.0, "got {r}");
        // Stream stops: still the old rate for a moment, then 0.
        assert!(m.published(ms(178 * 14 + 1_000)) > 0.0);
        assert_eq!(m.published(ms(178 * 14 + 2_500)), 0.0);
    }

    #[test]
    fn pps_meter_restarts_its_window_after_a_gap() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut m = PpsMeter::new(t0);
        m.on_packet(ms(10));
        // Silent for 30 s, then a steady 100 Hz: must not report ~1/30 for the first window.
        for i in 0..=200 {
            m.on_packet(ms(30_000 + i * 10));
        }
        assert!((m.rate - 100.0).abs() < 2.0, "got {}", m.rate);
    }

    #[test]
    fn clearing_rpm_calibration_keeps_the_gear_map() {
        let mut dsg = DsgListener::new();
        dsg.gear_redline_speeds[1] = 60.0;
        dsg.engaged = true;
        let mut max_rpm = 8500.0_f32;

        clear_rpm_calibration(&mut dsg, &mut max_rpm);

        assert_eq!(max_rpm, 0.0, "redline cleared");
        assert!(!dsg.engaged, "engagement cleared");
        assert_eq!(dsg.gear_redline_speeds[1], 60.0, "gear map kept");
    }

    #[test]
    fn clearing_gear_map_keeps_the_rpm_calibration() {
        let mut dsg = DsgListener::new();
        dsg.gear_redline_speeds[1] = 60.0;
        dsg.gear_redline_speeds[2] = 100.0;
        dsg.engaged = true;
        let max_rpm = 8500.0_f32;

        dsg.reset_calibration(); // what Command::ClearGearMap / the hotkey run

        assert!(dsg.gear_redline_speeds.iter().all(|&s| s == 0.0), "gear map cleared");
        assert!(dsg.engaged, "engagement kept");
        assert_eq!(max_rpm, 8500.0, "redline kept");
    }

    #[test]
    fn both_clears_together_leave_nothing_to_persist() {
        // One key bound to both actions fires both, in either order.
        let mut dsg = DsgListener::new();
        dsg.gear_redline_speeds[1] = 60.0;
        let mut max_rpm = 8500.0_f32;
        clear_rpm_calibration(&mut dsg, &mut max_rpm);
        dsg.reset_calibration();
        // Mirrors persist_calibration's condition (the file write itself is not exercised).
        assert!(!(dsg.gear_redline_speeds[1] > 0.0 || max_rpm > 0.0));
    }
}
