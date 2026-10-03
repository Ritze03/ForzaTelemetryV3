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

/// Send every action bound to `key` + `mods` (the caller has already checked the rebind guard)
/// and return what was sent. Shared by both backends so neither can stop at the first match.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
fn dispatch(binds: &[(HotkeyBinding, HotkeyAction)], key: HotKey, mods: Mods, tx: &std::sync::mpsc::Sender<HotkeyAction>) -> Vec<HotkeyAction> {
    let actions = match_combo(binds, key, mods);
    for action in &actions {
        let _ = tx.send(*action);
    }
    actions
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

/// Pure: `(is_keyboard, readable)` per **physical** `/dev/input/event*` node → status. `Ok` when
/// at least one keyboard can be read, `NoPermission` when keyboards exist but none can be,
/// `NoDevice` when there is no keyboard at all. Only keyboards count: *Why:* on GNOME / KDE /
/// Fedora systemd-logind `uaccess` ACLs make game controllers (and `/dev/uinput`) readable for
/// the seated user but not keyboards (those stay `root:input 0660`), so "any readable node" was
/// green with a gamepad plugged in while no hotkey could ever be read. Callers pass physical
/// keyboards only ([`counts_as_physical`]): a readable *virtual* keyboard (ydotoold, a remapper,
/// Steam Input) says nothing about whether the real one can be read.
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

/// Pure: does a sysfs `capabilities/ev` bitmap say the node also reports relative or absolute
/// axes (`EV_REL` bit 2, `EV_ABS` bit 3)? Mice with a keyboard-ish interface, gamepads and
/// ydotool's catch-all device do; a plain keyboard doesn't. Shown in the diagnostics only
/// (some real keyboards have a trackpoint/touchpad on the same node, so it never decides).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ev_bitmap_has_axes(bitmap: &str) -> bool {
    bitmap.split_whitespace().last()
        .and_then(|w| u64::from_str_radix(w, 16).ok())
        .is_some_and(|w| w & 0b1100 != 0)
}

/// Pure: a node created through `/dev/uinput` lives under `/sys/devices/virtual/input/`.
/// *Why this path and not "virtual" anywhere:* Bluetooth keyboards arrive through `uhid`
/// (`/sys/devices/virtual/misc/uhid/…`) and are real hardware.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_virtual_sysfs_path(canonical: &str) -> bool {
    canonical.starts_with("/sys/devices/virtual/input/")
}

/// Pure: does this node count as a **physical** keyboard? Not our own uinput device
/// ([`crate::input::VIRTUAL_DEVICE_NAME`]) and not any other uinput device. *Why:* virtual
/// keyboards (ydotoold, a remapper, Steam Input, Bluetooth AVRCP, …) are often readable through
/// `uaccess`/ACLs while the real keyboard isn't, and counting them made the permission light
/// amber ("fine") while no real key could ever be read.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn counts_as_physical(name: &str, is_virtual: bool) -> bool {
    !is_virtual && name != crate::input::VIRTUAL_DEVICE_NAME
}

/// Pure: which keyboard-like nodes count as a **working keyboard**, from `(is_physical_keyboard,
/// also_pointer)` per node. Physical keyboard-likes that also report mouse/pad axes (a gaming
/// mouse's key-macro interface, a pad) only count when no plain keyboard exists. *Why:* such
/// interfaces can be readable while the real keyboard isn't, and counting them turned the light
/// amber ("fine") with no typed key readable; the fallback keeps a keyboard whose node does
/// report axes (some do) from turning the light red.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn working_mask(devs: &[(bool, bool)]) -> Vec<bool> {
    let any_plain = devs.iter().any(|(physical, pointer)| *physical && !*pointer);
    devs.iter().map(|(physical, pointer)| *physical && (!*pointer || !any_plain)).collect()
}

/// One keyboard-like (`KEY_A`) input node, for the Setup diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyboardInfo {
    /// `eventN`.
    pub node: String,
    pub name: String,
    /// `/dev/input/eventN` can be opened by us.
    pub readable: bool,
    /// The backend has a reader thread on it.
    pub opened: bool,
    /// A uinput device (including our own), not hardware.
    pub is_virtual: bool,
    /// Also reports relative/absolute axes (mouse, gamepad, catch-all virtual device).
    pub pointer: bool,
    /// Counts as a working keyboard (see [`working_mask`]): not virtual, not a mouse/pad interface.
    pub counts: bool,
}

/// One `/dev/input/event*` node as seen through sysfs + an open attempt (Linux).
#[cfg(target_os = "linux")]
struct Node {
    node: String,
    path: std::path::PathBuf,
    name: String,
    readable: bool,
    is_kb: bool,
    is_virtual: bool,
    pointer: bool,
}

/// Every `/dev/input/event*` node, ordered by number. A node is a keyboard by its world-readable
/// sysfs capability bitmap (works for nodes we may not open), falling back to asking the device
/// itself when sysfs has no entry. A failed path lookup counts as *not* virtual so a real
/// keyboard is never hidden by a sysfs hiccup.
#[cfg(target_os = "linux")]
fn inventory() -> Vec<Node> {
    let Ok(dir) = std::fs::read_dir("/dev/input") else { return Vec::new() };
    let mut out = Vec::new();
    for e in dir.flatten() {
        let node = e.file_name().to_string_lossy().into_owned();
        if !node.starts_with("event") { continue; }
        let sys = |f: &str| std::fs::read_to_string(format!("/sys/class/input/{node}/device/{f}")).ok();
        let readable = std::fs::File::open(e.path()).is_ok();
        let is_kb = sys("capabilities/key")
            .map(|s| key_bitmap_has_letters(&s))
            .or_else(|| {
                if !readable { return None; }
                evdev::Device::open(e.path()).ok()
                    .map(|d| d.supported_keys().is_some_and(|k| k.contains(evdev::Key::KEY_A)))
            })
            .unwrap_or(false);
        let is_virtual = std::fs::canonicalize(format!("/sys/class/input/{node}")).ok()
            .is_some_and(|p| is_virtual_sysfs_path(&p.to_string_lossy()));
        out.push(Node {
            name: sys("name").map(|n| n.trim().to_string()).unwrap_or_default(),
            pointer: sys("capabilities/ev").is_some_and(|s| ev_bitmap_has_axes(&s)),
            path: e.path(), node, readable, is_kb, is_virtual,
        });
    }
    out.sort_by_key(|n| n.node.trim_start_matches("event").parse::<u32>().unwrap_or(u32::MAX));
    out
}

/// For each node, whether it counts as a working keyboard ([`working_mask`]).
#[cfg(target_os = "linux")]
fn working_keyboards(nodes: &[Node]) -> Vec<bool> {
    let kinds: Vec<(bool, bool)> = nodes.iter()
        .map(|n| (n.is_kb && counts_as_physical(&n.name, n.is_virtual), n.pointer))
        .collect();
    working_mask(&kinds)
}

/// Probe `/dev/input/event*` for a readable **working** keyboard (Linux). Windows polls
/// `GetAsyncKeyState`, which needs nothing, so it is always `Ok`.
pub fn probe_status() -> HotkeyStatus {
    #[cfg(target_os = "linux")]
    {
        let nodes = inventory();
        let devs: Vec<(bool, bool)> = working_keyboards(&nodes).into_iter().zip(&nodes).map(|(w, n)| (w, n.readable)).collect();
        classify(&devs)
    }
    #[cfg(target_os = "windows")]
    { HotkeyStatus::Ok }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    { HotkeyStatus::Unsupported }
}

/// Event nodes the Linux backend currently reads (empty elsewhere), each with "is a physical
/// keyboard". Shared so a rescan skips keyboards that already have a reader thread, and the UI
/// can tell "backend has zero (physical) keyboards".
pub type OpenKeyboards = Arc<Mutex<std::collections::HashMap<std::path::PathBuf, bool>>>;

/// Pure: an already-open node keeps its reader but takes the **fresh** "counts as working
/// keyboard" verdict. Returns true when the node was already open. *Why:* the flag was frozen at
/// open time, so a node first classified wrong (a sysfs hiccup, the "no plain keyboard" fallback
/// before the real keyboard appeared) kept skewing the light for the whole session.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn refresh_open_flag(map: &mut std::collections::HashMap<std::path::PathBuf, bool>, path: &std::path::Path, counts: bool) -> bool {
    match map.get_mut(path) {
        Some(v) => { *v = counts; true }
        None => false,
    }
}

/// Pure: claim a node for a new reader: inserts it and returns true, or (already open) refreshes
/// the flag and returns false. One lock, so the periodic rescan thread and the Re-check button
/// can never both start a reader on the same node (that would fire every hotkey twice).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn claim_node(map: &mut std::collections::HashMap<std::path::PathBuf, bool>, path: &std::path::Path, counts: bool) -> bool {
    if refresh_open_flag(map, path, counts) { return false; }
    map.insert(path.to_path_buf(), counts);
    true
}

/// What the Setup diagnostics can show about the keyboard path. Cheap, shared between the
/// reader threads and the UI.
///
/// **Privacy:** the backend is match-only (non-matching keys are never stored). The diagnostics
/// keep only a press *counter* and its time/device always, and the **identity** of the last key
/// only while the Setup → Input Permissions card is on screen ([`listen`](Self::listen) is
/// called every frame it is drawn). *Why:* "which key did you see?" is the one thing that tells
/// "no events arrive" apart from "events arrive but are gated", but a key log must not run
/// while the user is typing a password elsewhere.
#[derive(Clone, Default)]
pub struct HotkeyDiag(Arc<Mutex<DiagInner>>);

#[derive(Default)]
struct DiagInner {
    presses: u64,
    /// Time + device of the latest key-down (key not recorded).
    last_press: Option<(Instant, String)>,
    /// Latest key-down with its identity, recorded only while listening.
    last_key: Option<(Instant, String, String)>,
    /// Latest action the backend pushed to the listener thread (before its focus gate).
    last_action: Option<(Instant, HotkeyAction)>,
    listen_until: Option<Instant>,
    /// Reader threads that ended (device unplugged / read error), newest last, max 5.
    ended: Vec<String>,
}

/// A copy of [`HotkeyDiag`] with ages instead of instants, for display.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagSnapshot {
    pub presses: u64,
    pub last_press: Option<(Duration, String)>,
    /// `(age, key name, device)`.
    pub last_key: Option<(Duration, String, String)>,
    pub last_action: Option<(Duration, HotkeyAction)>,
    pub ended: Vec<String>,
}

/// How long after the last [`HotkeyDiag::listen`] the key identity is still recorded.
const LISTEN_WINDOW: Duration = Duration::from_millis(1500);

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl HotkeyDiag {
    /// The diagnostics card is on screen: record key identities for the next moments.
    pub fn listen(&self) { self.0.lock().unwrap().listen_until = Some(Instant::now() + LISTEN_WINDOW); }
    /// A key went down on `device`. `key` is only evaluated while listening.
    fn press(&self, device: &str, key: impl FnOnce() -> String) {
        let now = Instant::now();
        let mut d = self.0.lock().unwrap();
        d.presses += 1;
        d.last_press = Some((now, device.to_string()));
        if d.listen_until.is_some_and(|t| now < t) { d.last_key = Some((now, key(), device.to_string())); }
    }
    fn sent(&self, actions: &[HotkeyAction]) {
        if let Some(a) = actions.last() { self.0.lock().unwrap().last_action = Some((Instant::now(), *a)); }
    }
    fn reader_ended(&self, what: String) {
        let mut d = self.0.lock().unwrap();
        d.ended.push(what);
        if d.ended.len() > 5 { d.ended.remove(0); }
    }
    pub fn snapshot(&self) -> DiagSnapshot {
        let d = self.0.lock().unwrap();
        let now = Instant::now();
        let age = |t: &Instant| now.saturating_duration_since(*t);
        DiagSnapshot {
            presses: d.presses,
            last_press: d.last_press.as_ref().map(|(t, dev)| (age(t), dev.clone())),
            last_key: d.last_key.as_ref().map(|(t, k, dev)| (age(t), k.clone(), dev.clone())),
            last_action: d.last_action.as_ref().map(|(t, a)| (age(t), *a)),
            ended: d.ended.clone(),
        }
    }
}

pub struct HotkeyListener {
    binds: Bindings,
    tx: std::sync::mpsc::Sender<HotkeyAction>,
    guard: Arc<RebindGuard>,
    open: OpenKeyboards,
    diag: HotkeyDiag,
    /// Tells the periodic rescan thread to end when the listener is dropped.
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for HotkeyListener {
    fn drop(&mut self) { self.stop.store(true, std::sync::atomic::Ordering::Relaxed); }
}

/// How often the backend looks for keyboards that have no reader (hot-plug, Bluetooth reconnect,
/// access that appeared).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const RESCAN_EVERY: Duration = Duration::from_millis(2500);

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
        let me = HotkeyListener { binds, tx, guard, open, diag: HotkeyDiag::default(), stop: Arc::default() };
        me.rescan();
        // Keep looking for keyboards with no reader. Here, not in the UI frame loop: hotkeys must
        // keep working while the window is hidden and no frames are drawn. A scan is sysfs reads
        // plus `open()`s (sub-millisecond) and only starts readers for nodes without one.
        #[cfg(target_os = "linux")]
        {
            let (binds, tx, guard, open, diag, stop) =
                (me.binds.clone(), me.tx.clone(), me.guard.clone(), me.open.clone(), me.diag.clone(), me.stop.clone());
            std::thread::spawn(move || loop {
                std::thread::sleep(RESCAN_EVERY);
                if stop.load(std::sync::atomic::Ordering::Relaxed) { break; }
                backend::scan(&binds, &tx, &guard, &open, &diag);
            });
        }
        (me, rx)
    }
    /// (Re)open keyboards that have no reader yet — the initial scan, the Setup **Re-check**
    /// button (a periodic background rescan does the same every [`RESCAN_EVERY`]), so access that
    /// appears later (a udev/ACL change, a hot-plugged keyboard) works without a restart.
    /// Synchronous: the open count is right as soon as it returns.
    pub fn rescan(&self) { backend::scan(&self.binds, &self.tx, &self.guard, &self.open, &self.diag); }
    /// How many **physical** keyboards the backend is actually reading (always 1 off Linux:
    /// nothing to open). Virtual keyboards it also reads don't count: see [`counts_as_physical`].
    pub fn active_keyboards(&self) -> usize {
        self.open.lock().unwrap().values().filter(|physical| **physical).count()
    }
    /// The keyboard-like nodes right now, with readable / open flags, for the diagnostics
    /// (empty off Linux). Reads sysfs and tries to open every event node: not for every frame.
    pub fn keyboards(&self) -> Vec<KeyboardInfo> {
        #[cfg(target_os = "linux")]
        {
            let open = self.open.lock().unwrap();
            let nodes = inventory();
            let working = working_keyboards(&nodes);
            nodes.into_iter().zip(working).filter(|(n, _)| n.is_kb).map(|(n, counts)| KeyboardInfo {
                opened: open.contains_key(&n.path),
                is_virtual: !counts_as_physical(&n.name, n.is_virtual),
                counts,
                node: n.node, name: n.name, readable: n.readable, pointer: n.pointer,
            }).collect()
        }
        #[cfg(not(target_os = "linux"))]
        { Vec::new() }
    }
    /// The shared diagnostics (last key seen, last action sent, ended readers).
    pub fn diag(&self) -> HotkeyDiag { self.diag.clone() }
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
    use super::{Bindings, HotkeyDiag, OpenKeyboards, RebindGuard, dispatch, inventory, working_keyboards};
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, Mods};

    /// Open every readable keyboard that has no reader thread yet and start one for it. Reads
    /// virtual keyboards too (a remapper such as keyd re-emits the physical keys there), except
    /// our own uinput device, which only carries our synthetic presses; only *working* keyboards
    /// ([`working_mask`]) get the `true` flag in [`OpenKeyboards`] that [`HotkeyListener::active_keyboards`] counts.
    pub fn scan(binds: &Bindings, tx: &Sender<HotkeyAction>, guard: &Arc<RebindGuard>, open: &OpenKeyboards, diag: &HotkeyDiag) {
        // One reader thread per keyboard; each tracks its own modifier state.
        let nodes = inventory();
        let working = working_keyboards(&nodes);
        for (n, counts) in nodes.into_iter().zip(working) {
            if !n.is_kb || !n.readable || n.name == crate::input::VIRTUAL_DEVICE_NAME { continue; }
            let path = n.path;
            // Already read: keep its reader, but refresh the "counts" verdict.
            if super::refresh_open_flag(&mut open.lock().unwrap(), &path, counts) { continue; }
            let Ok(mut dev) = Device::open(&path) else { continue };
            // Claim under one lock (the periodic scan and Re-check can race to here).
            if !super::claim_node(&mut open.lock().unwrap(), &path, counts) { continue; }
            let label = format!("{} ({})", n.name, n.node);
            let binds = binds.clone();
            let tx = tx.clone();
            let guard = guard.clone();
            let open = open.clone();
            let diag = diag.clone();
            thread::spawn(move || {
                let mut mods = Mods::default();
                'read: loop {
                    let events = match dev.fetch_events() {
                        Ok(e) => e,
                        Err(e) => {
                            // Never silent: the Setup diagnostics list ended readers.
                            eprintln!("hotkeys: reader for {label} ended: {e}");
                            diag.reader_ended(format!("{label}: {e}"));
                            break 'read;
                        }
                    };
                    for ev in events {
                        if ev.event_type() != EventType::KEY { continue; }
                        let key = Key::new(ev.code());
                        let down = ev.value() == 1; // 1=down, 0=up, 2=repeat
                        // Counter always; the key's identity only while the diagnostics card is open.
                        if down { diag.press(&label, || format!("{key:?}")); }
                        match key {
                            Key::KEY_LEFTCTRL | Key::KEY_RIGHTCTRL => { mods.ctrl = ev.value() != 0; }
                            Key::KEY_LEFTALT | Key::KEY_RIGHTALT => { mods.alt = ev.value() != 0; }
                            Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT => { mods.shift = ev.value() != 0; }
                            Key::KEY_LEFTMETA | Key::KEY_RIGHTMETA => { mods.sup = ev.value() != 0; }
                            _ if down => {
                                if let Some(hk) = HotKey::from_evdev(key) {
                                    let list = binds.lock().unwrap();
                                    // Muted while a rebind capture runs (and just after).
                                    if !guard.blocked() {
                                        let sent = dispatch(&list, hk, mods, &tx);
                                        diag.sent(&sent);
                                    }
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

#[cfg(target_os = "windows")]
mod backend {
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use super::{Bindings, HotkeyDiag, OpenKeyboards, RebindGuard, dispatch};
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, Mods};

    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12; // Alt
    const VK_SHIFT: i32 = 0x10;
    const VK_LWIN: i32 = 0x5B;

    fn down(vk: i32) -> bool { (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0 }

    /// Starts the poll thread once (a rescan has nothing to reopen: `GetAsyncKeyState` needs no device).
    pub fn scan(binds: &Bindings, tx: &Sender<HotkeyAction>, guard: &Arc<RebindGuard>, open: &OpenKeyboards, _diag: &HotkeyDiag) {
        if open.lock().unwrap().insert(std::path::PathBuf::new(), true).is_some() { return; }
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
    use super::{Bindings, HotkeyDiag, OpenKeyboards, RebindGuard};
    use crate::config::HotkeyAction;
    pub fn scan(_b: &Bindings, _tx: &Sender<HotkeyAction>, _g: &Arc<RebindGuard>, _o: &OpenKeyboards, _d: &HotkeyDiag) {}
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
    fn open_node_flag_is_refreshed_not_frozen() {
        use std::collections::HashMap;
        use std::path::{Path, PathBuf};
        let p = Path::new("/dev/input/event3");
        let mut m: HashMap<PathBuf, bool> = HashMap::new();
        assert!(!refresh_open_flag(&mut m, p, true), "unknown node: nothing to refresh");
        assert!(m.is_empty());
        m.insert(p.to_path_buf(), false);
        assert!(refresh_open_flag(&mut m, p, true));
        assert_eq!(m[p], true, "counts verdict must follow the fresh classification");
        assert!(refresh_open_flag(&mut m, p, false));
        assert_eq!(m[p], false);
    }

    #[test]
    fn claim_node_starts_one_reader_per_node() {
        use std::collections::HashMap;
        use std::path::{Path, PathBuf};
        let p = Path::new("/dev/input/event5");
        let mut m: HashMap<PathBuf, bool> = HashMap::new();
        assert!(claim_node(&mut m, p, true), "first claim wins");
        assert!(!claim_node(&mut m, p, false), "second claim must not start another reader");
        assert_eq!(m[p], false, "...but refreshes the flag");
        m.remove(p); // reader ended (device gone)
        assert!(claim_node(&mut m, p, true), "a replugged node can be claimed again");
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
    fn virtual_keyboards_never_count_as_physical() {
        // uinput devices live under /sys/devices/virtual/input/; hardware (USB, i8042, ...) doesn't.
        assert!(is_virtual_sysfs_path("/sys/devices/virtual/input/input30/event24"));
        assert!(!is_virtual_sysfs_path("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-7/1-7:1.0/0003:0416:0123.0002/input/input5/event4"));
        assert!(!is_virtual_sysfs_path("/sys/devices/platform/i8042/serio0/input/input3/event3"));
        // Bluetooth keyboards come through uhid, not uinput: real hardware.
        assert!(!is_virtual_sysfs_path("/sys/devices/virtual/misc/uhid/0005:046D:B342.0004/input/input40/event9"));
        assert!(counts_as_physical("Ducky One2 SF RGB", false));
        assert!(!counts_as_physical("ydotoold virtual device", true));
        // Our own uinput device never counts, even if a sysfs lookup failed and it looked physical.
        assert!(!counts_as_physical(crate::input::VIRTUAL_DEVICE_NAME, false));
    }

    #[test]
    fn readable_virtual_keyboard_does_not_hide_an_unreadable_real_one() {
        // The GNOME report: a virtual keyboard readable via ACL, the real one not. Only physical
        // keyboards are passed to `classify`, so this is NoPermission (red + modal), not Ok.
        let nodes = [("ydotoold virtual device", true, true), ("Real keyboard", false, false)]; // name, virtual, readable
        let physical: Vec<(bool, bool)> = nodes.iter().map(|(n, v, r)| (counts_as_physical(n, *v), *r)).collect();
        assert_eq!(classify(&physical), HotkeyStatus::NoPermission);
    }

    #[test]
    fn ev_bitmap_flags_mice_and_pads_but_not_keyboards() {
        assert!(!ev_bitmap_has_axes("120013")); // SYN KEY MSC LED REP: a keyboard
        assert!(ev_bitmap_has_axes("17")); // + REL: a mouse with a key interface
        assert!(ev_bitmap_has_axes("1b")); // + ABS: a pad
        assert!(!ev_bitmap_has_axes(""));
    }

    #[test]
    fn diag_records_the_key_only_while_listening() {
        let d = HotkeyDiag::default();
        d.press("kbd", || "KEY_J".into()); // card closed: counted, identity not kept
        let s = d.snapshot();
        assert_eq!(s.presses, 1);
        assert_eq!(s.last_press.as_ref().map(|(_, dev)| dev.as_str()), Some("kbd"));
        assert!(s.last_key.is_none(), "a key log must not run while the card is closed");
        d.listen();
        d.press("kbd", || "KEY_K".into());
        let s = d.snapshot();
        assert_eq!(s.presses, 2);
        assert_eq!(s.last_key.map(|(_, k, dev)| (k, dev)), Some(("KEY_K".into(), "kbd".into())));
        d.sent(&[HotkeyAction::ToggleGearbox, HotkeyAction::ClearGearMap]);
        assert_eq!(d.snapshot().last_action.map(|(_, a)| a), Some(HotkeyAction::ClearGearMap));
        for i in 0..8 { d.reader_ended(format!("r{i}")); }
        assert_eq!(d.snapshot().ended.len(), 5);
    }

    #[test]
    fn mouse_key_interfaces_only_count_without_a_plain_keyboard() {
        // (is_physical_keyboard, also_pointer): Ducky keyboard, Razer mouse's key interface, virtual.
        assert_eq!(working_mask(&[(true, false), (true, true), (false, true)]), vec![true, false, false]);
        // Only mouse-like nodes: keep them (a keyboard that reports axes must not turn the light red).
        assert_eq!(working_mask(&[(true, true), (false, false)]), vec![true, false]);
        // The report: readable mouse interface + unreadable real keyboard -> the real one decides.
        let kinds = [(true, false), (true, true)];
        let devs: Vec<(bool, bool)> = working_mask(&kinds).into_iter().zip([false, true]).collect(); // (counts, readable)
        assert_eq!(classify(&devs), HotkeyStatus::NoPermission);
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
