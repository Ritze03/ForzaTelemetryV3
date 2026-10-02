//! Controller input. Background readers (Linux: evdev on the *physical* pad, hot-plug rescan;
//! Windows: XInput polling) turn pad state into [`PadControl`] presses, which are mapped to the
//! global [`HotkeyAction`]s and sent down the same mpsc channel the keyboard hotkeys use — so
//! the listener thread's focus gate applies unchanged. The latest right-stick vector is exposed
//! through [`Gamepad::right_stick`]. Everything between the raw device and the actions is pure
//! (`norm_*`, `radial_deadzone`, [`Processor`]) and unit-tested without devices.
//! See `docs/features/gamepad.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::config::HotkeyAction;

/// A physical control on an Xbox-style pad. Serde-stable (variant names).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PadControl {
    A, B, X, Y,
    Lb, Rb,
    /// Trigger pulled past the press threshold (after its deadzone).
    Lt, Rt,
    Back, Start,
    L3, R3,
    DpadUp, DpadDown, DpadLeft, DpadRight,
    /// Right stick pushed past the press threshold in that direction (rising edge).
    RsUp, RsDown, RsLeft, RsRight,
}

impl PadControl {
    pub const ALL: &'static [PadControl] = &[
        PadControl::A, PadControl::B, PadControl::X, PadControl::Y,
        PadControl::Lb, PadControl::Rb, PadControl::Lt, PadControl::Rt,
        PadControl::Back, PadControl::Start, PadControl::L3, PadControl::R3,
        PadControl::DpadUp, PadControl::DpadDown, PadControl::DpadLeft, PadControl::DpadRight,
        PadControl::RsUp, PadControl::RsDown, PadControl::RsLeft, PadControl::RsRight,
    ];

    /// English label (goes through `tr()` at the call site).
    pub fn label(self) -> &'static str {
        match self {
            PadControl::A => "A", PadControl::B => "B", PadControl::X => "X", PadControl::Y => "Y",
            PadControl::Lb => "LB", PadControl::Rb => "RB", PadControl::Lt => "LT", PadControl::Rt => "RT",
            PadControl::Back => "Back", PadControl::Start => "Start",
            PadControl::L3 => "L3", PadControl::R3 => "R3",
            PadControl::DpadUp => "D-pad Up", PadControl::DpadDown => "D-pad Down",
            PadControl::DpadLeft => "D-pad Left", PadControl::DpadRight => "D-pad Right",
            PadControl::RsUp => "Right stick Up", PadControl::RsDown => "Right stick Down",
            PadControl::RsLeft => "Right stick Left", PadControl::RsRight => "Right stick Right",
        }
    }

    /// Bit in a held-controls mask (`ALL` has 20 entries, so a u32 is enough).
    pub fn bit(self) -> u32 {
        1 << Self::ALL.iter().position(|c| *c == self).unwrap()
    }
}

// ── Pure signal processing ───────────────────────────────────────────────────

/// A stick axis from its absinfo range → -1..1 around the range's centre
/// (`±32768`, `0..255` centred, …). A degenerate range gives 0.
pub fn norm_stick(raw: i32, min: i32, max: i32) -> f32 {
    if max <= min { return 0.0; }
    let centre = (min as f32 + max as f32) / 2.0;
    let half = (max as f32 - min as f32) / 2.0;
    ((raw as f32 - centre) / half).clamp(-1.0, 1.0)
}

/// A trigger axis from its absinfo range → 0..1 (`0..255`, `0..1023`, …).
pub fn norm_trigger(raw: i32, min: i32, max: i32) -> f32 {
    if max <= min { return 0.0; }
    ((raw as f32 - min as f32) / (max as f32 - min as f32)).clamp(0.0, 1.0)
}

/// Radial deadzone: inside `dz` the stick is 0; outside, the magnitude is rescaled so it still
/// reaches 1 at full deflection (no jump at the edge), the direction is kept.
pub fn radial_deadzone(x: f32, y: f32, dz: f32) -> (f32, f32) {
    let mag = (x * x + y * y).sqrt();
    if mag <= dz || mag <= f32::EPSILON { return (0.0, 0.0); }
    let scaled = ((mag - dz) / (1.0 - dz).max(f32::EPSILON)).min(1.0);
    (x / mag * scaled, y / mag * scaled)
}

/// Trigger deadzone: 0 below `dz`, then rescaled to reach 1.
pub fn trigger_deadzone(v: f32, dz: f32) -> f32 {
    if v <= dz { 0.0 } else { ((v - dz) / (1.0 - dz).max(f32::EPSILON)).min(1.0) }
}

/// Rising-edge press / falling-edge release thresholds (hysteresis band): a stick that
/// wobbles around the press point doesn't re-fire.
pub const STICK_PRESS: f32 = 0.6;
pub const STICK_RELEASE: f32 = 0.4;
pub const TRIGGER_PRESS: f32 = 0.5;
pub const TRIGGER_RELEASE: f32 = 0.35;

fn hyst(on: bool, v: f32, press: f32, release: f32) -> bool {
    if on { v > release } else { v >= press }
}

/// One pad's instantaneous state, already normalised. `y` of the right stick is **up-positive**.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct PadInput {
    /// Digital buttons (A..R3, D-pad) as a [`PadControl::bit`] mask.
    pub buttons: u32,
    pub lt: f32,
    pub rt: f32,
    /// Right stick, -1..1, before the deadzone.
    pub rs: (f32, f32),
}

/// Deadzones for [`Processor::update`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deadzones { pub stick: f32, pub trigger: f32 }

/// Turns [`PadInput`] into the set of currently-held controls (triggers and stick directions
/// with hysteresis) and tracks the previous set so presses are rising edges.
#[derive(Default)]
pub struct Processor { held: u32 }

impl Processor {
    /// Feed one state; returns the controls that were **newly** pressed by it (rising edge:
    /// nothing while held, fires again after a release).
    pub fn update(&mut self, input: &PadInput, dz: Deadzones) -> u32 {
        let was = self.held;
        let on = |c: PadControl| was & c.bit() != 0;
        let mut now = input.buttons;
        let mut set = |c: PadControl, v: bool| if v { now |= c.bit(); };
        set(PadControl::Lt, hyst(on(PadControl::Lt), trigger_deadzone(input.lt, dz.trigger), TRIGGER_PRESS, TRIGGER_RELEASE));
        set(PadControl::Rt, hyst(on(PadControl::Rt), trigger_deadzone(input.rt, dz.trigger), TRIGGER_PRESS, TRIGGER_RELEASE));
        let (x, y) = radial_deadzone(input.rs.0, input.rs.1, dz.stick);
        // Only the dominant axis counts, so a diagonal doesn't press two directions.
        let horiz = x.abs() >= y.abs();
        let dirs = [
            (PadControl::RsRight, if horiz { x } else { 0.0 }),
            (PadControl::RsLeft, if horiz { -x } else { 0.0 }),
            (PadControl::RsUp, if horiz { 0.0 } else { y }),
            (PadControl::RsDown, if horiz { 0.0 } else { -y }),
        ];
        for (c, v) in dirs { set(c, hyst(on(c), v, STICK_PRESS, STICK_RELEASE)); }
        self.held = now;
        now & !was
    }
}

/// Iterate the controls in a mask.
fn controls_in(mask: u32) -> impl Iterator<Item = PadControl> {
    PadControl::ALL.iter().copied().filter(move |c| mask & c.bit() != 0)
}

// ── Shared state + public handle ─────────────────────────────────────────────

/// What the reader threads need from the config, pushed by the UI each frame.
#[derive(Clone, PartialEq, Debug)]
pub struct PadParams {
    pub enabled: bool,
    pub stick_deadzone: f32,
    pub trigger_deadzone: f32,
    pub bindings: Vec<(PadControl, HotkeyAction)>,
}

impl PadParams {
    pub fn from_config(c: &crate::config::GamepadConfig) -> Self {
        let mut bindings: Vec<_> = c.bindings.iter().map(|(a, p)| (*p, *a)).collect();
        bindings.sort_by_key(|(p, _)| p.bit()); // stable order → cheap equality
        PadParams { enabled: c.enabled, stick_deadzone: c.stick_deadzone, trigger_deadzone: c.trigger_deadzone, bindings }
    }
    fn deadzones(&self) -> Deadzones { Deadzones { stick: self.stick_deadzone, trigger: self.trigger_deadzone } }
}

struct Shared {
    params: Mutex<PadParams>,
    /// Latest raw (pre-deadzone, y up) right stick per device id.
    sticks: Mutex<HashMap<u64, (f32, f32)>>,
    /// Connected pads: (device id, display name).
    devices: Mutex<Vec<(u64, String)>>,
    capture: AtomicBool,
    captured: Mutex<Option<PadControl>>,
    tx: Mutex<Sender<HotkeyAction>>,
}

impl Shared {
    /// One device's new state: publish the stick, then act on the presses (a capture swallows
    /// the first one; otherwise bound controls send their action).
    fn feed(&self, id: u64, proc: &mut Processor, input: &PadInput) {
        let params = self.params.lock().unwrap().clone();
        self.sticks.lock().unwrap().insert(id, input.rs);
        let pressed = proc.update(input, params.deadzones());
        if !params.enabled || pressed == 0 { return; }
        if self.capture.load(Ordering::Relaxed) {
            if let Some(c) = controls_in(pressed).next() {
                *self.captured.lock().unwrap() = Some(c);
                self.capture.store(false, Ordering::Relaxed);
            }
            return;
        }
        let tx = self.tx.lock().unwrap();
        for c in controls_in(pressed) {
            for (_, a) in params.bindings.iter().filter(|(p, _)| *p == c) {
                let _ = tx.send(*a);
            }
        }
    }
    fn add_device(&self, id: u64, name: String) { self.devices.lock().unwrap().push((id, name)); }
    fn remove_device(&self, id: u64) {
        self.devices.lock().unwrap().retain(|(i, _)| *i != id);
        self.sticks.lock().unwrap().remove(&id);
    }
}

/// Handle to the controller backend. Cheap to clone (an `Arc`); hand clones to other threads.
#[derive(Clone)]
pub struct Gamepad { shared: Arc<Shared> }

impl Gamepad {
    /// Start the backend. Actions go into `tx` — the hotkey channel the listener thread drains.
    pub fn spawn(tx: Sender<HotkeyAction>, params: PadParams) -> Gamepad {
        let shared = Arc::new(Shared {
            params: Mutex::new(params),
            sticks: Mutex::new(HashMap::new()),
            devices: Mutex::new(Vec::new()),
            capture: AtomicBool::new(false),
            captured: Mutex::new(None),
            tx: Mutex::new(tx),
        });
        backend::spawn(shared.clone());
        Gamepad { shared }
    }

    /// Push the config (the UI does this every frame; no-op when unchanged).
    pub fn set_params(&self, p: PadParams) {
        let mut cur = self.shared.params.lock().unwrap();
        if *cur != p { *cur = p; }
    }

    /// Right stick as `(x, y)`, each -1..1 after the configured radial deadzone, **x right,
    /// y up**. If several pads are connected, the one deflected furthest wins. `(0, 0)` while
    /// the feature is disabled or no pad is connected. Read it from any thread, any time.
    pub fn right_stick(&self) -> (f32, f32) {
        let p = self.shared.params.lock().unwrap().clone();
        if !p.enabled { return (0.0, 0.0); }
        let sticks = self.shared.sticks.lock().unwrap();
        let best = sticks.values().copied()
            .max_by(|a, b| (a.0 * a.0 + a.1 * a.1).total_cmp(&(b.0 * b.0 + b.1 * b.1)))
            .unwrap_or((0.0, 0.0));
        radial_deadzone(best.0, best.1, p.stick_deadzone)
    }

    /// Display names of the connected pads.
    pub fn devices(&self) -> Vec<String> {
        self.shared.devices.lock().unwrap().iter().map(|(_, n)| n.clone()).collect()
    }

    /// Arm a capture: the next control press is stored (and not sent as an action).
    pub fn arm_capture(&self) {
        *self.shared.captured.lock().unwrap() = None;
        self.shared.capture.store(true, Ordering::Relaxed);
    }
    pub fn cancel_capture(&self) { self.shared.capture.store(false, Ordering::Relaxed); }
    /// The captured control, once (taking it clears it).
    pub fn take_captured(&self) -> Option<PadControl> { self.shared.captured.lock().unwrap().take() }
}

// ── Linux backend: evdev on the physical pad ─────────────────────────────────

#[cfg(target_os = "linux")]
mod backend {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use evdev::{AbsoluteAxisType as Abs, Device, EventType, InputEvent, Key};

    use super::{norm_stick, norm_trigger, PadControl, PadInput, Processor, Shared};

    /// Steam Input's virtual pad (vendor 28de, product 11ff). Skipped on purpose: it only
    /// delivers input while the game window is focused; the physical pad always does.
    const STEAM_VIRTUAL: (u16, u16) = (0x28de, 0x11ff);
    /// Hot-plug rescan period (the xone wireless dongle creates its node late).
    const RESCAN: Duration = Duration::from_secs(2);

    pub fn spawn(shared: Arc<Shared>) {
        thread::spawn(move || {
            let active: Arc<Mutex<HashSet<PathBuf>>> = Arc::default();
            let next_id = Arc::new(AtomicU64::new(1));
            let mut ignored: HashSet<PathBuf> = HashSet::new();
            loop {
                if shared.params.lock().unwrap().enabled { scan(&shared, &active, &next_id, &mut ignored); }
                thread::sleep(RESCAN);
            }
        });
    }

    fn scan(shared: &Arc<Shared>, active: &Arc<Mutex<HashSet<PathBuf>>>, next_id: &Arc<AtomicU64>, ignored: &mut HashSet<PathBuf>) {
        let Ok(dir) = std::fs::read_dir("/dev/input") else { return };
        let mut seen = HashSet::new();
        for e in dir.flatten() {
            if !e.file_name().to_string_lossy().starts_with("event") { continue; }
            let path = e.path();
            seen.insert(path.clone());
            if ignored.contains(&path) || active.lock().unwrap().contains(&path) { continue; }
            let Ok(dev) = Device::open(&path) else { continue }; // no permission yet: retry next scan
            if !is_gamepad(&dev) { ignored.insert(path); continue; }
            active.lock().unwrap().insert(path.clone());
            let id = next_id.fetch_add(1, Ordering::Relaxed);
            let (shared, active) = (shared.clone(), active.clone());
            thread::spawn(move || {
                read_pad(&shared, id, dev);
                shared.remove_device(id);
                active.lock().unwrap().remove(&path);
            });
        }
        ignored.retain(|p| seen.contains(p)); // forget unplugged nodes (numbers get reused)
    }

    /// By capability, not name: face button + both right-stick axes; and not Steam's virtual pad.
    fn is_gamepad(dev: &Device) -> bool {
        let id = dev.input_id();
        if (id.vendor(), id.product()) == STEAM_VIRTUAL { return false; }
        dev.supported_keys().is_some_and(|k| k.contains(Key::BTN_SOUTH))
            && dev.supported_absolute_axes().is_some_and(|a| a.contains(Abs::ABS_RX) && a.contains(Abs::ABS_RY))
    }

    /// evdev button code → control (xpad / xone / xpadneo all use these).
    pub fn control_for_key(code: u16) -> Option<PadControl> {
        Some(match Key::new(code) {
            Key::BTN_SOUTH => PadControl::A,
            Key::BTN_EAST => PadControl::B,
            Key::BTN_NORTH => PadControl::X,
            Key::BTN_WEST => PadControl::Y,
            Key::BTN_TL => PadControl::Lb,
            Key::BTN_TR => PadControl::Rb,
            Key::BTN_SELECT => PadControl::Back,
            Key::BTN_START => PadControl::Start,
            Key::BTN_THUMBL => PadControl::L3,
            Key::BTN_THUMBR => PadControl::R3,
            Key::BTN_DPAD_UP => PadControl::DpadUp,
            Key::BTN_DPAD_DOWN => PadControl::DpadDown,
            Key::BTN_DPAD_LEFT => PadControl::DpadLeft,
            Key::BTN_DPAD_RIGHT => PadControl::DpadRight,
            _ => return None,
        })
    }

    /// Absinfo (min, max) per axis we use, read once at open (EVIOCGABS) so every driver's
    /// range normalises correctly (xone triggers 0..1023, sticks ±32768, ...).
    #[derive(Default, Clone, Copy)]
    struct Range { min: i32, max: i32 }

    #[derive(Default)]
    struct PadState {
        rx: i32, ry: i32, lt: i32, rt: i32,
        r_rx: Range, r_ry: Range, r_lt: Range, r_rt: Range,
        hat: (i32, i32),
        buttons: u32,
        lt_btn: bool, rt_btn: bool,
    }

    impl PadState {
        fn apply(&mut self, ev: &InputEvent) {
            let v = ev.value();
            if ev.event_type() == EventType::KEY {
                let down = v != 0;
                match Key::new(ev.code()) {
                    Key::BTN_TL2 => self.lt_btn = down,
                    Key::BTN_TR2 => self.rt_btn = down,
                    _ => if let Some(c) = control_for_key(ev.code()) {
                        if down { self.buttons |= c.bit() } else { self.buttons &= !c.bit() }
                    },
                }
            } else if ev.event_type() == EventType::ABSOLUTE {
                match Abs(ev.code()) {
                    Abs::ABS_RX => self.rx = v,
                    Abs::ABS_RY => self.ry = v,
                    Abs::ABS_Z => self.lt = v,
                    Abs::ABS_RZ => self.rt = v,
                    Abs::ABS_HAT0X => self.hat.0 = v,
                    Abs::ABS_HAT0Y => self.hat.1 = v,
                    _ => {}
                }
            }
        }

        fn input(&self) -> PadInput {
            let mut buttons = self.buttons;
            for (on, c) in [
                (self.hat.0 < 0, PadControl::DpadLeft), (self.hat.0 > 0, PadControl::DpadRight),
                (self.hat.1 < 0, PadControl::DpadUp), (self.hat.1 > 0, PadControl::DpadDown),
            ] { if on { buttons |= c.bit(); } }
            let trig = |raw, r: Range, btn: bool| norm_trigger(raw, r.min, r.max).max(if btn { 1.0 } else { 0.0 });
            PadInput {
                buttons,
                lt: trig(self.lt, self.r_lt, self.lt_btn),
                rt: trig(self.rt, self.r_rt, self.rt_btn),
                // evdev Y grows downward; the shared convention is up-positive.
                rs: (norm_stick(self.rx, self.r_rx.min, self.r_rx.max), -norm_stick(self.ry, self.r_ry.min, self.r_ry.max)),
            }
        }
    }

    /// Reader thread body: blocks in `fetch_events` (read-only, never grabs the device) and
    /// returns when the device goes away.
    fn read_pad(shared: &Shared, id: u64, mut dev: Device) {
        let name = dev.name().unwrap_or("Controller").to_string();
        let mut st = PadState::default();
        if let Ok(abs) = dev.get_abs_state() {
            let r = |a: Abs| { let i = abs[a.0 as usize]; Range { min: i.minimum, max: i.maximum } };
            (st.r_rx, st.r_ry, st.r_lt, st.r_rt) = (r(Abs::ABS_RX), r(Abs::ABS_RY), r(Abs::ABS_Z), r(Abs::ABS_RZ));
            let v = |a: Abs| abs[a.0 as usize].value;
            (st.rx, st.ry, st.lt, st.rt) = (v(Abs::ABS_RX), v(Abs::ABS_RY), v(Abs::ABS_Z), v(Abs::ABS_RZ));
        }
        if let Ok(keys) = dev.get_key_state() {
            for k in keys.iter() {
                if let Some(c) = control_for_key(k.code()) { st.buttons |= c.bit(); }
            }
        }
        shared.add_device(id, name);
        let mut proc = Processor::default();
        loop {
            let Ok(events) = dev.fetch_events() else { return };
            for ev in events { st.apply(&ev); }
            shared.feed(id, &mut proc, &st.input());
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn evdev_buttons_map_to_controls() {
            assert_eq!(control_for_key(Key::BTN_SOUTH.code()), Some(PadControl::A));
            assert_eq!(control_for_key(Key::BTN_NORTH.code()), Some(PadControl::X));
            assert_eq!(control_for_key(Key::BTN_WEST.code()), Some(PadControl::Y));
            assert_eq!(control_for_key(Key::BTN_THUMBR.code()), Some(PadControl::R3));
            assert_eq!(control_for_key(Key::KEY_A.code()), None);
        }

        #[test]
        fn state_handles_hat_and_digital_triggers() {
            let mut st = PadState {
                r_rx: Range { min: -32768, max: 32767 }, r_ry: Range { min: -32768, max: 32767 },
                r_lt: Range { min: 0, max: 1023 }, r_rt: Range { min: 0, max: 1023 },
                ..Default::default()
            };
            st.apply(&InputEvent::new(EventType::ABSOLUTE, Abs::ABS_HAT0Y.0, -1));
            st.apply(&InputEvent::new(EventType::ABSOLUTE, Abs::ABS_RZ.0, 1023));
            st.apply(&InputEvent::new(EventType::ABSOLUTE, Abs::ABS_RY.0, 32767)); // pushed down
            st.apply(&InputEvent::new(EventType::KEY, Key::BTN_TL2.code(), 1));
            let i = st.input();
            assert!(i.buttons & PadControl::DpadUp.bit() != 0);
            assert_eq!(i.rt, 1.0);
            assert_eq!(i.lt, 1.0);
            assert!(i.rs.1 < -0.99, "evdev down must become y = -1 (up-positive)");
        }
    }
}

// ── Windows backend: XInput polling ──────────────────────────────────────────

#[cfg(target_os = "windows")]
mod backend {
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::Input::XboxController::*;

    use super::{norm_stick, norm_trigger, PadControl, PadInput, Processor, Shared};

    const POLL: Duration = Duration::from_millis(8);
    /// `XInputGetState` on an empty slot is slow on some systems, so absent slots are only
    /// probed this often (also the hot-plug latency).
    const RESCAN: Duration = Duration::from_secs(2);

    fn read(i: u32) -> Option<XINPUT_GAMEPAD> {
        let mut s: XINPUT_STATE = unsafe { std::mem::zeroed() };
        (unsafe { XInputGetState(i, &mut s) } == 0).then_some(s.Gamepad)
    }

    fn to_input(g: &XINPUT_GAMEPAD) -> PadInput {
        let mut buttons = 0;
        for (flag, c) in [
            (XINPUT_GAMEPAD_A, PadControl::A), (XINPUT_GAMEPAD_B, PadControl::B),
            (XINPUT_GAMEPAD_X, PadControl::X), (XINPUT_GAMEPAD_Y, PadControl::Y),
            (XINPUT_GAMEPAD_LEFT_SHOULDER, PadControl::Lb), (XINPUT_GAMEPAD_RIGHT_SHOULDER, PadControl::Rb),
            (XINPUT_GAMEPAD_BACK, PadControl::Back), (XINPUT_GAMEPAD_START, PadControl::Start),
            (XINPUT_GAMEPAD_LEFT_THUMB, PadControl::L3), (XINPUT_GAMEPAD_RIGHT_THUMB, PadControl::R3),
            (XINPUT_GAMEPAD_DPAD_UP, PadControl::DpadUp), (XINPUT_GAMEPAD_DPAD_DOWN, PadControl::DpadDown),
            (XINPUT_GAMEPAD_DPAD_LEFT, PadControl::DpadLeft), (XINPUT_GAMEPAD_DPAD_RIGHT, PadControl::DpadRight),
        ] { if g.wButtons & flag != 0 { buttons |= c.bit(); } }
        PadInput {
            buttons,
            lt: norm_trigger(g.bLeftTrigger as i32, 0, 255),
            rt: norm_trigger(g.bRightTrigger as i32, 0, 255),
            // XInput Y is already up-positive.
            rs: (norm_stick(g.sThumbRX as i32, -32768, 32767), norm_stick(g.sThumbRY as i32, -32768, 32767)),
        }
    }

    pub fn spawn(shared: Arc<Shared>) {
        thread::spawn(move || {
            let mut procs: [Option<Processor>; 4] = Default::default();
            let mut last_scan = Instant::now() - RESCAN;
            loop {
                if !shared.params.lock().unwrap().enabled {
                    for (i, p) in procs.iter_mut().enumerate() {
                        if p.take().is_some() { shared.remove_device(i as u64); }
                    }
                    thread::sleep(RESCAN);
                    continue;
                }
                let scan = last_scan.elapsed() >= RESCAN;
                if scan { last_scan = Instant::now(); }
                for i in 0..4u32 {
                    let slot = &mut procs[i as usize];
                    if slot.is_none() && !scan { continue; }
                    match read(i) {
                        Some(g) => {
                            if slot.is_none() {
                                *slot = Some(Processor::default());
                                shared.add_device(i as u64, format!("XInput controller {}", i + 1));
                            }
                            shared.feed(i as u64, slot.as_mut().unwrap(), &to_input(&g));
                        }
                        None => if slot.take().is_some() { shared.remove_device(i as u64); },
                    }
                }
                thread::sleep(POLL);
            }
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    use std::sync::Arc;
    pub fn spawn(_s: Arc<super::Shared>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const DZ: Deadzones = Deadzones { stick: 0.15, trigger: 0.1 };
    fn rs(x: f32, y: f32) -> PadInput { PadInput { rs: (x, y), ..Default::default() } }
    fn pressed(m: u32, c: PadControl) -> bool { m & c.bit() != 0 }

    #[test]
    fn stick_normalises_signed_range() {
        assert!((norm_stick(32767, -32768, 32767) - 1.0).abs() < 1e-4);
        assert!((norm_stick(-32768, -32768, 32767) + 1.0).abs() < 1e-4);
        assert!(norm_stick(0, -32768, 32767).abs() < 1e-4);
        assert!((norm_stick(16384, -32768, 32768) - 0.5).abs() < 1e-4);
        assert_eq!(norm_stick(5, 3, 3), 0.0); // degenerate range
    }

    #[test]
    fn trigger_normalises_unsigned_range() {
        assert_eq!(norm_trigger(0, 0, 1023), 0.0);
        assert_eq!(norm_trigger(1023, 0, 1023), 1.0);
        assert!((norm_trigger(512, 0, 1023) - 0.5).abs() < 1e-3);
        assert!((norm_trigger(128, 0, 255) - 0.502).abs() < 1e-3);
        assert_eq!(norm_trigger(2000, 0, 1023), 1.0); // clamped
    }

    #[test]
    fn radial_deadzone_zeroes_inside_and_rescales_outside() {
        assert_eq!(radial_deadzone(0.1, 0.1, 0.15), (0.0, 0.0)); // |v| = 0.141 < 0.15
        let (x, y) = radial_deadzone(1.0, 0.0, 0.15);
        assert!((x - 1.0).abs() < 1e-5 && y == 0.0);
        let (x, _) = radial_deadzone(0.575, 0.0, 0.15); // halfway between dz and 1
        assert!((x - 0.5).abs() < 1e-5);
        // Direction is preserved, magnitude never exceeds 1 even from a corner (1, 1).
        let (x, y) = radial_deadzone(1.0, 1.0, 0.15);
        assert!((x - y).abs() < 1e-6 && (x * x + y * y).sqrt() <= 1.0 + 1e-5);
        // Circular, not per-axis: a small x with a large y still passes.
        assert!(radial_deadzone(0.1, 0.9, 0.15).1 > 0.0);
    }

    #[test]
    fn stick_direction_has_rising_edge_and_no_repeat_while_held() {
        let mut p = Processor::default();
        assert_eq!(p.update(&rs(0.0, 0.0), DZ), 0);
        assert!(pressed(p.update(&rs(1.0, 0.0), DZ), PadControl::RsRight));
        assert_eq!(p.update(&rs(1.0, 0.0), DZ), 0, "held: no repeat");
        assert_eq!(p.update(&rs(0.9, 0.0), DZ), 0);
        assert_eq!(p.update(&rs(0.0, 0.0), DZ), 0); // release
        assert!(pressed(p.update(&rs(1.0, 0.0), DZ), PadControl::RsRight), "re-fires after release");
    }

    #[test]
    fn stick_hysteresis_band_does_not_refire() {
        let mut p = Processor::default();
        // Raw 0.9 → after the 0.15 deadzone ≈ 0.88: pressed. Then wobble inside the band.
        assert!(pressed(p.update(&rs(0.9, 0.0), DZ), PadControl::RsRight));
        // Raw 0.6 → ≈ 0.53: below press (0.6) but above release (0.4): still held, no event.
        assert_eq!(p.update(&rs(0.6, 0.0), DZ), 0);
        assert_eq!(p.update(&rs(0.9, 0.0), DZ), 0, "crossing the press point again must not re-fire");
        // Raw 0.4 → ≈ 0.29: below release → released; then 0.6 is not enough to press again.
        assert_eq!(p.update(&rs(0.4, 0.0), DZ), 0);
        assert_eq!(p.update(&rs(0.6, 0.0), DZ), 0);
        assert!(pressed(p.update(&rs(0.9, 0.0), DZ), PadControl::RsRight));
    }

    #[test]
    fn stick_diagonal_presses_only_the_dominant_direction() {
        let mut p = Processor::default();
        let m = p.update(&rs(0.7, 0.95), DZ);
        assert!(pressed(m, PadControl::RsUp));
        assert!(!pressed(m, PadControl::RsRight));
        let mut p = Processor::default();
        let m = p.update(&rs(-0.95, -0.2), DZ);
        assert!(pressed(m, PadControl::RsLeft) && !pressed(m, PadControl::RsDown));
    }

    #[test]
    fn stick_inside_deadzone_never_presses() {
        let mut p = Processor::default();
        assert_eq!(p.update(&rs(0.14, 0.0), DZ), 0);
    }

    #[test]
    fn trigger_presses_past_threshold_with_hysteresis() {
        let mut p = Processor::default();
        let t = |v: f32| PadInput { lt: v, ..Default::default() };
        assert_eq!(p.update(&t(0.3), DZ), 0);
        assert!(pressed(p.update(&t(0.9), DZ), PadControl::Lt));
        assert_eq!(p.update(&t(0.9), DZ), 0);
        assert_eq!(p.update(&t(0.5), DZ), 0); // ≈0.44 after deadzone: inside the band, held
        assert_eq!(p.update(&t(0.1), DZ), 0); // released
        assert!(pressed(p.update(&t(1.0), DZ), PadControl::Lt));
    }

    #[test]
    fn digital_buttons_are_edges_too() {
        let mut p = Processor::default();
        let b = PadInput { buttons: PadControl::A.bit(), ..Default::default() };
        assert!(pressed(p.update(&b, DZ), PadControl::A));
        assert_eq!(p.update(&b, DZ), 0);
        assert_eq!(p.update(&PadInput::default(), DZ), 0);
        assert!(pressed(p.update(&b, DZ), PadControl::A));
    }

    #[test]
    fn pad_control_bits_are_unique_and_serde_stable() {
        let mut seen = 0u32;
        for &c in PadControl::ALL {
            assert_eq!(seen & c.bit(), 0, "{c:?} bit collides");
            seen |= c.bit();
            assert!(!c.label().is_empty());
            let j = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<PadControl>(&j).unwrap(), c);
        }
        assert_eq!(serde_json::to_string(&PadControl::RsUp).unwrap(), "\"RsUp\"");
    }

    #[test]
    fn bound_controls_become_actions_and_capture_swallows_the_press() {
        let (tx, rx) = std::sync::mpsc::channel();
        let params = PadParams {
            enabled: true, stick_deadzone: 0.15, trigger_deadzone: 0.1,
            bindings: vec![(PadControl::Y, HotkeyAction::ToggleGearbox)],
        };
        let shared = Shared {
            params: Mutex::new(params), sticks: Mutex::new(HashMap::new()), devices: Mutex::new(vec![]),
            capture: AtomicBool::new(false), captured: Mutex::new(None), tx: Mutex::new(tx),
        };
        let mut proc = Processor::default();
        let y = PadInput { buttons: PadControl::Y.bit(), ..Default::default() };
        shared.feed(1, &mut proc, &y);
        assert_eq!(rx.try_recv().ok(), Some(HotkeyAction::ToggleGearbox));
        shared.feed(1, &mut proc, &PadInput::default());
        shared.capture.store(true, Ordering::Relaxed);
        shared.feed(1, &mut proc, &y);
        assert!(rx.try_recv().is_err(), "capture must not also fire the action");
        assert_eq!(*shared.captured.lock().unwrap(), Some(PadControl::Y));
    }

    #[test]
    fn gamepad_config_bind_steals_the_control() {
        let mut c = crate::config::GamepadConfig::default();
        c.bind(HotkeyAction::ToggleGearbox, PadControl::A);
        c.bind(HotkeyAction::ToggleBackfire, PadControl::A);
        assert_eq!(c.bindings.get(&HotkeyAction::ToggleGearbox), None);
        assert_eq!(c.bindings[&HotkeyAction::ToggleBackfire], PadControl::A);
    }
}
