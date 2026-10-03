//! Global key capture. A background backend (Linux: evdev read of /dev/input;
//! Windows: GetAsyncKeyState poll) matches configured global-scope combos and
//! pushes the matched HotkeyAction down an mpsc channel drained on the listener
//! thread. Match-only: no other keystroke is stored, sent, or logged. See spec.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::HotkeyAction;
use crate::keymap::{HotKey, HotkeyBinding, Mods};

/// Shared list of global bindings the backend matches against.
pub type Bindings = Arc<Mutex<Vec<(HotkeyBinding, HotkeyAction)>>>;

/// Pure match: a combo matches when its key was pressed and modifiers match exactly. Returns
/// **every** action bound to that combo, in list order (the app sorts the list by action
/// order). Why all: one key may be bound to several actions and must fire all of them.
pub fn match_combo(binds: &[(HotkeyBinding, HotkeyAction)], key: HotKey, mods: Mods) -> Vec<HotkeyAction> {
    binds.iter().filter(|(b, _)| b.key == key && b.mods == mods).map(|(_, a)| *a).collect()
}

/// Send every action bound to `key` + `mods` (the caller has already checked the rebind guard).
/// Shared by both backends so neither can stop at the first match.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn dispatch(binds: &[(HotkeyBinding, HotkeyAction)], key: HotKey, mods: Mods, tx: &std::sync::mpsc::Sender<HotkeyAction>) {
    for action in match_combo(binds, key, mods) {
        let _ = tx.send(action);
    }
}

/// How long input sources stay muted after a rebind capture ends (see [`RebindGuard`]).
pub const REBIND_GRACE: Duration = Duration::from_millis(300);

/// "A rebind capture is active" — shared by the keyboard backend, the gamepad backend and the
/// UI. While [`blocked`](Self::blocked), no hotkey / pad action may be sent to the consumer.
///
/// The UI mirrors "any rebind armed" into it once per frame ([`set_active`](Self::set_active)).
/// When the capture ends, input stays blocked for a short [`REBIND_GRACE`] on top.
///
/// Why: the backends read the raw device (evdev / `GetAsyncKeyState` / XInput) on their own
/// threads and are always *ahead* of the UI: the key that completes a capture reaches them
/// before egui processes it and ends the capture, and the listener thread drains the queued
/// action later still. Without this, rebinding Backfire to B (its current key) fired
/// Backfire; the grace also covers the capture key being released and a pad press whose
/// capture the UI had not committed yet.
#[derive(Default)]
pub struct RebindGuard {
    active: AtomicBool,
    /// Muted until this instant after the capture ended.
    until: Mutex<Option<Instant>>,
}

impl RebindGuard {
    /// Mirror "a capture is armed" (call every frame; only the true→false edge starts the grace).
    pub fn set_active(&self, active: bool) { self.set_active_at(active, Instant::now()); }
    fn set_active_at(&self, active: bool, now: Instant) {
        if self.active.swap(active, Ordering::SeqCst) && !active {
            *self.until.lock().unwrap() = Some(now + REBIND_GRACE);
        }
    }
    /// True while capturing or inside the grace period after one.
    pub fn blocked(&self) -> bool { self.blocked_at(Instant::now()) }
    fn blocked_at(&self, now: Instant) -> bool {
        self.active.load(Ordering::SeqCst) || self.until.lock().unwrap().is_some_and(|t| now < t)
    }
}

/// Whether the capture backend can work here — drives the Setup "Input Permissions" light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // Unsupported is only built on non-Linux/Windows targets
pub enum HotkeyStatus { Ok, NoPermission, NoDevice, Unsupported }

/// Pure: `(is_keyboard, readable)` per `/dev/input/event*` node → status. `Ok` when at least
/// one **keyboard** can be read, `NoPermission` when keyboards exist but none can be, `NoDevice`
/// when there is no keyboard at all. Only keyboards count: *Why:* on GNOME / KDE / Fedora
/// systemd-logind `uaccess` ACLs make game controllers (and `/dev/uinput`) readable for the
/// seated user but not keyboards (those stay `root:input 0660`), so "any readable node" was
/// green with a gamepad plugged in while no hotkey could ever be read.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn classify(devs: &[(bool, bool)]) -> HotkeyStatus {
    let keyboards: Vec<bool> = devs.iter().filter(|(kb, _)| *kb).map(|(_, r)| *r).collect();
    if keyboards.is_empty() { HotkeyStatus::NoDevice }
    else if keyboards.iter().any(|r| *r) { HotkeyStatus::Ok }
    else { HotkeyStatus::NoPermission }
}

/// Pure: does a sysfs `capabilities/key` bitmap (space-separated hex words, most significant
/// first) contain `KEY_A` (bit 30, always in the last word)? World-readable, so it classifies a
/// node as keyboard / not without being able to open it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn key_bitmap_has_letters(bitmap: &str) -> bool {
    bitmap.split_whitespace().last()
        .and_then(|w| u64::from_str_radix(w, 16).ok())
        .is_some_and(|w| w & (1 << 30) != 0)
}

/// Probe `/dev/input/event*` for a readable keyboard (Linux). A node counts as a keyboard by
/// its sysfs capability bitmap (works for nodes we may not open), falling back to asking the
/// device itself when sysfs has no entry. Windows polls `GetAsyncKeyState`, which needs
/// nothing, so it is always `Ok`.
pub fn probe_status() -> HotkeyStatus {
    #[cfg(target_os = "linux")]
    {
        let Ok(dir) = std::fs::read_dir("/dev/input") else { return HotkeyStatus::NoDevice };
        let mut devs = Vec::new();
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("event") { continue; }
            let readable = std::fs::File::open(e.path()).is_ok();
            let is_kb = std::fs::read_to_string(format!("/sys/class/input/{name}/device/capabilities/key"))
                .ok()
                .map(|s| key_bitmap_has_letters(&s))
                .or_else(|| {
                    if !readable { return None; }
                    evdev::Device::open(e.path()).ok()
                        .map(|d| d.supported_keys().is_some_and(|k| k.contains(evdev::Key::KEY_A)))
                })
                .unwrap_or(false);
            devs.push((is_kb, readable));
        }
        classify(&devs)
    }
    #[cfg(target_os = "windows")]
    { HotkeyStatus::Ok }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    { HotkeyStatus::Unsupported }
}

/// Event nodes the Linux backend currently reads (empty elsewhere). Shared so a rescan skips
/// keyboards that already have a reader thread, and the UI can tell "backend has zero keyboards".
pub type OpenKeyboards = Arc<Mutex<std::collections::HashSet<std::path::PathBuf>>>;

pub struct HotkeyListener {
    binds: Bindings,
    tx: std::sync::mpsc::Sender<HotkeyAction>,
    guard: Arc<RebindGuard>,
    open: OpenKeyboards,
}

impl HotkeyListener {
    /// Starts the capture backend and hands back the matched-action receiver, which the
    /// listener thread (`listeners/worker.rs`) drains — global hotkeys must keep working
    /// while the window is hidden and the frame loop is stopped. The `HotkeyListener`
    /// itself stays on the UI side, purely to push rebound keys to the backend.
    pub fn new(initial: Vec<(HotkeyBinding, HotkeyAction)>) -> (Self, Receiver<HotkeyAction>) {
        let binds: Bindings = Arc::new(Mutex::new(initial));
        let (tx, rx) = std::sync::mpsc::channel();
        let guard = Arc::new(RebindGuard::default());
        let open = OpenKeyboards::default();
        let me = HotkeyListener { binds, tx, guard, open };
        me.rescan();
        (me, rx)
    }
    /// (Re)open keyboards that have no reader yet — the initial scan, and the Setup **Re-check**
    /// button, so access that appears later (a udev/ACL change, a hot-plugged keyboard) works
    /// without a restart. Synchronous: the open count is right as soon as it returns.
    pub fn rescan(&self) { backend::scan(&self.binds, &self.tx, &self.guard, &self.open); }
    /// How many keyboards the backend is actually reading (always 1 off Linux: nothing to open).
    pub fn active_keyboards(&self) -> usize {
        if cfg!(target_os = "linux") { self.open.lock().unwrap().len() } else { 1 }
    }
    /// A sender into the same action channel, for other input sources (the gamepad), so
    /// they pass through the listener thread's focus gate exactly like keyboard hotkeys.
    pub fn action_sender(&self) -> std::sync::mpsc::Sender<HotkeyAction> { self.tx.clone() }
    /// The shared rebind guard: the UI sets it while any rebind capture is armed; the gamepad
    /// backend gets a clone so pad actions are muted too.
    pub fn rebind_guard(&self) -> Arc<RebindGuard> { self.guard.clone() }
    pub fn set_bindings(&self, b: Vec<(HotkeyBinding, HotkeyAction)>) { *self.binds.lock().unwrap() = b; }
}

#[cfg(target_os = "linux")]
mod backend {
    use std::sync::mpsc::Sender;
    use std::thread;
    use evdev::{Device, EventType, Key};
    use std::sync::Arc;
    use super::{Bindings, OpenKeyboards, RebindGuard, dispatch};
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, Mods};

    /// Open every readable keyboard that has no reader thread yet and start one for it.
    pub fn scan(binds: &Bindings, tx: &Sender<HotkeyAction>, guard: &Arc<RebindGuard>, open: &OpenKeyboards) {
        // `enumerate` yields only devices we can open; non-keyboards are filtered out.
        let keyboards: Vec<(std::path::PathBuf, Device)> = evdev::enumerate()
            .filter(|(_, d)| d.supported_keys().map_or(false, |k| k.contains(Key::KEY_A)))
            .collect();
        {
            // One reader thread per keyboard; each tracks its own modifier state.
            for (path, mut dev) in keyboards {
                if !open.lock().unwrap().insert(path.clone()) { continue; } // already read
                let binds = binds.clone();
                let tx = tx.clone();
                let guard = guard.clone();
                let open = open.clone();
                thread::spawn(move || {
                    let mut mods = Mods::default();
                    loop {
                        let events = match dev.fetch_events() { Ok(e) => e, Err(_) => break };
                        for ev in events {
                            if ev.event_type() != EventType::KEY { continue; }
                            let key = Key::new(ev.code());
                            let down = ev.value() == 1; // 1=down, 0=up, 2=repeat
                            match key {
                                Key::KEY_LEFTCTRL | Key::KEY_RIGHTCTRL => { mods.ctrl = ev.value() != 0; }
                                Key::KEY_LEFTALT | Key::KEY_RIGHTALT => { mods.alt = ev.value() != 0; }
                                Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT => { mods.shift = ev.value() != 0; }
                                Key::KEY_LEFTMETA | Key::KEY_RIGHTMETA => { mods.sup = ev.value() != 0; }
                                _ if down => {
                                    if let Some(hk) = HotKey::from_evdev(key) {
                                        let list = binds.lock().unwrap();
                                        // Muted while a rebind capture runs (and just after).
                                        if !guard.blocked() { dispatch(&list, hk, mods, &tx); }
                                        // Non-matching keys are dropped here — never stored/logged.
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    open.lock().unwrap().remove(&path); // device gone: a rescan may reopen it
                });
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod backend {
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use super::{Bindings, OpenKeyboards, RebindGuard, dispatch};
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, Mods};

    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12; // Alt
    const VK_SHIFT: i32 = 0x10;
    const VK_LWIN: i32 = 0x5B;

    fn down(vk: i32) -> bool { (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0 }

    /// Starts the poll thread once (a rescan has nothing to reopen: `GetAsyncKeyState` needs no device).
    pub fn scan(binds: &Bindings, tx: &Sender<HotkeyAction>, guard: &Arc<RebindGuard>, open: &OpenKeyboards) {
        if !open.lock().unwrap().insert(std::path::PathBuf::new()) { return; }
        let (binds, tx, guard) = (binds.clone(), tx.clone(), guard.clone());
        thread::spawn(move || {
            // Previous key-down state per virtual key (not per action): the state is tracked
            // even while a rebind capture mutes sending, so a key that was held down when the
            // capture ended (the one just bound) has no rising edge afterwards.
            let mut prev: std::collections::HashMap<i32, bool> = std::collections::HashMap::new();
            loop {
                let mods = Mods { ctrl: down(VK_CONTROL), alt: down(VK_MENU), shift: down(VK_SHIFT), sup: down(VK_LWIN) };
                let list = binds.lock().unwrap().clone();
                let muted = guard.blocked();
                // One pass per distinct key (not per binding): the edge state is per key, so a
                // per-binding pass let only the first of several bindings on one key fire.
                let mut keys: Vec<HotKey> = Vec::new();
                for (b, _) in &list {
                    if !keys.contains(&b.key) { keys.push(b.key); }
                }
                for key in keys {
                    let vk = key.to_vk();
                    let key_down = down(vk);
                    let was = *prev.get(&vk).unwrap_or(&false);
                    // Rising edge of the base key, with modifiers matching now: fires every
                    // action bound to the combo.
                    if key_down && !was && !muted {
                        dispatch(&list, key, mods, &tx);
                    }
                    prev.insert(vk, key_down);
                }
                thread::sleep(Duration::from_millis(8));
            }
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use super::{Bindings, OpenKeyboards, RebindGuard};
    use crate::config::HotkeyAction;
    pub fn scan(_b: &Bindings, _tx: &Sender<HotkeyAction>, _g: &Arc<RebindGuard>, _o: &OpenKeyboards) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, HotkeyBinding, Mods};

    fn bind(ctrl: bool, key: HotKey) -> HotkeyBinding {
        HotkeyBinding { mods: Mods { ctrl, ..Default::default() }, key }
    }

    #[test]
    fn classify_needs_a_readable_keyboard() {
        // (is_keyboard, readable)
        assert_eq!(classify(&[]), HotkeyStatus::NoDevice);
        // Only a non-keyboard (gamepad) node exists: no keyboard at all.
        assert_eq!(classify(&[(false, true)]), HotkeyStatus::NoDevice);
        // The GNOME/uaccess case: gamepad readable, keyboard not -> must NOT be green.
        assert_eq!(classify(&[(false, true), (true, false), (true, false)]), HotkeyStatus::NoPermission);
        assert_eq!(classify(&[(false, true), (true, false), (true, true)]), HotkeyStatus::Ok);
    }

    #[test]
    fn key_bitmap_detects_letter_keys() {
        // Real keyboard (KEY_A is bit 30 of the last word) vs a gamepad (only BTN_* bits high up).
        assert!(key_bitmap_has_letters("402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe"));
        assert!(!key_bitmap_has_letters("7cdb000000000000 0 0 0"));
        assert!(!key_bitmap_has_letters("0"));
        assert!(!key_bitmap_has_letters(""));
    }

    #[test]
    fn matches_key_and_exact_modifiers() {
        let binds = vec![
            (bind(false, HotKey::G), HotkeyAction::ToggleGearbox),
            (bind(true, HotKey::E), HotkeyAction::DashboardEdit),
        ];
        // Plain G with no mods → gearbox.
        assert_eq!(match_combo(&binds, HotKey::G, Mods::default()), vec![HotkeyAction::ToggleGearbox]);
        // G but Ctrl held → no match (modifiers must match exactly).
        assert!(match_combo(&binds, HotKey::G, Mods { ctrl: true, ..Default::default() }).is_empty());
        // Ctrl+E → dashboard.
        assert_eq!(match_combo(&binds, HotKey::E, Mods { ctrl: true, ..Default::default() }), vec![HotkeyAction::DashboardEdit]);
    }

    #[test]
    fn one_key_bound_to_two_actions_sends_both_in_list_order() {
        let binds = vec![
            (bind(false, HotKey::F), HotkeyAction::ResetCalibration),
            (bind(false, HotKey::G), HotkeyAction::ToggleGearbox),
            (bind(false, HotKey::F), HotkeyAction::ClearGearMap),
        ];
        let (tx, rx) = std::sync::mpsc::channel();
        dispatch(&binds, HotKey::F, Mods::default(), &tx);
        assert_eq!(rx.try_recv().ok(), Some(HotkeyAction::ResetCalibration));
        assert_eq!(rx.try_recv().ok(), Some(HotkeyAction::ClearGearMap));
        assert!(rx.try_recv().is_err(), "the G binding does not fire");
        // Same key, different modifiers: not part of the combo.
        dispatch(&binds, HotKey::F, Mods { ctrl: true, ..Default::default() }, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn global_bindings_are_sorted_by_action_order() {
        let mut cfg = crate::config::AppConfig::default();
        let f = bind(false, HotKey::F);
        cfg.hotkeys.bind(HotkeyAction::ClearGearMap, f);
        cfg.hotkeys.bind(HotkeyAction::ResetCalibration, f);
        let list = crate::app::global_bindings(&cfg);
        let on_f: Vec<_> = list.iter().filter(|(b, _)| b.key == HotKey::F).map(|(_, a)| *a).collect();
        assert_eq!(on_f, vec![HotkeyAction::ResetCalibration, HotkeyAction::ClearGearMap]);
    }

    #[test]
    fn clear_gear_map_defaults_to_f_and_unbound_stays_unbound() {
        let mut hk = crate::config::HotkeyConfig::default();
        assert_eq!(hk.bindings[&HotkeyAction::ClearGearMap].key, HotKey::F);
        hk.unbind(HotkeyAction::ClearGearMap);
        crate::config::inject_missing_hotkeys(&mut hk); // a deliberate unbind is not re-injected
        assert!(!hk.bindings.contains_key(&HotkeyAction::ClearGearMap));
        assert_eq!(serde_json::to_string(&HotkeyAction::ResetCalibration).unwrap(), "\"ResetCalibration\"");
        assert_eq!(serde_json::to_string(&HotkeyAction::ClearGearMap).unwrap(), "\"ClearGearMap\"");
    }

    #[test]
    fn rebind_guard_blocks_during_capture_and_through_the_grace() {
        let g = RebindGuard::default();
        let t0 = Instant::now();
        assert!(!g.blocked_at(t0), "idle: actions flow");
        g.set_active_at(true, t0);
        assert!(g.blocked_at(t0 + Duration::from_secs(60)), "blocked for as long as the capture lasts");
        g.set_active_at(true, t0 + Duration::from_secs(1)); // per-frame re-assert changes nothing
        let end = t0 + Duration::from_secs(2);
        g.set_active_at(false, end);
        assert!(g.blocked_at(end), "the capturing key is still down the moment the capture ends");
        assert!(g.blocked_at(end + REBIND_GRACE - Duration::from_millis(1)));
        assert!(!g.blocked_at(end + REBIND_GRACE), "grace over: actions flow again");
        // Idle frames (false → false) must not restart the grace.
        g.set_active_at(false, end + Duration::from_secs(10));
        assert!(!g.blocked_at(end + Duration::from_secs(10)));
    }
}
