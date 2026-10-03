use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Name of the uinput device `InputSender` creates (Linux). The hotkey backend skips it by this
/// name: it only ever carries our own synthetic presses, never a real keyboard's.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const VIRTUAL_DEVICE_NAME: &str = "Forza Telemetry Input";

/// Shared "synthetic press may still echo in telemetry" window.
///
/// Written by the input WORKER at actual key emission time — not at enqueue
/// time — so queue backlog from other presses (e.g. a DSG multi-gear shift
/// burst ahead of a backfire pop) can't erode the window, and a dead worker
/// (no /dev/uinput access) never opens phantom suppression windows.
#[derive(Clone, Default)]
pub struct EchoWindow(Arc<Mutex<Option<Instant>>>);

impl EchoWindow {
    /// Called by the worker around a tracked press: covers the hold up-front
    /// (in case anything reads mid-hold), re-anchored at key-up.
    fn open(&self, ms: u64) {
        *self.0.lock().unwrap() = Some(Instant::now() + Duration::from_millis(ms));
    }

    /// True while the window (plus `grace` for consumers that process packets
    /// with a known delay, e.g. a low render-FPS limit) is still open.
    pub fn active(&self, grace: Duration) -> bool {
        self.0
            .lock()
            .unwrap()
            .map(|t| Instant::now() < t + grace)
            .unwrap_or(false)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TryRecvError};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use evdev::{AttributeSet, EventType, InputEvent, Key};
    use evdev::uinput::VirtualDeviceBuilder;

    use super::EchoWindow;

    enum Cmd {
        // `echo_ms`: for tracked presses, how long after key-up the game may
        // still report the synthetic input back in telemetry.
        Key { key: Key, hold_ms: u64, gap_ms: u64, echo_ms: Option<u64> },
        // Press-and-hold with a safety deadline; released by a later `Release`
        // (or auto-released at `max_hold_ms` if none arrives — e.g. packets stop).
        Hold { key: Key, max_hold_ms: u64, echo_ms: u64 },
        Release,
        // Wake the worker while its virtual device is missing: retry the build now instead of
        // at the next 2 s tick (Setup -> Re-check).
        Retry,
    }

    /// Readiness of the virtual device (stored in an `AtomicU8`).
    const PENDING: u8 = 0;
    const READY: u8 = 1;
    const FAILED: u8 = 2;

    /// Pure: the `AtomicU8` state as the tri-state the permission check uses.
    pub(super) fn ready_from_state(state: u8) -> Option<bool> {
        match state {
            READY => Some(true),
            FAILED => Some(false),
            _ => None,
        }
    }

    /// How long the worker waits after a failed build before trying again. *Why retry:* a one-shot
    /// build left the sender dead until restart even after the user fixed the permission.
    const RETRY_EVERY: Duration = Duration::from_secs(2);

    fn build_device() -> std::io::Result<evdev::uinput::VirtualDevice> {
        let mut keys = AttributeSet::<Key>::new();
        keys.insert(Key::KEY_W);
        keys.insert(Key::KEY_E);
        keys.insert(Key::KEY_Q);
        VirtualDeviceBuilder::new()?.name(super::VIRTUAL_DEVICE_NAME).with_keys(&keys)?.build()
    }

    #[derive(Clone)]
    pub struct InputSender {
        tx: SyncSender<Cmd>,
        echo: EchoWindow,
        gate: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        /// PENDING / READY / FAILED: whether the worker holds a working virtual device.
        state: Arc<AtomicU8>,
    }

    impl InputSender {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::sync_channel::<Cmd>(64);
            let echo = EchoWindow::default();
            let worker_echo = echo.clone();
            let state = Arc::new(AtomicU8::new(PENDING));
            let worker_state = state.clone();
            thread::spawn(move || {
                // Build the virtual device, retrying until it works (or the app is gone).
                let mut logged = false;
                let mut device = loop {
                    match build_device() {
                        Ok(d) => {
                            worker_state.store(READY, Ordering::Relaxed);
                            if logged { eprintln!("uinput: virtual device created, key input works now"); }
                            break d;
                        }
                        Err(e) => {
                            worker_state.store(FAILED, Ordering::Relaxed);
                            // Once, not every retry.
                            if !logged {
                                logged = true;
                                eprintln!("uinput: could not create virtual device: {e}");
                                eprintln!("uinput: ensure the current user is in the 'input' group or /dev/uinput is accessible; retrying every 2 s");
                            }
                        }
                    }
                    // Wait for the next try; presses are dropped at the sender while not READY, and
                    // anything that slipped in is discarded here so it can't fire long after.
                    let until = Instant::now() + RETRY_EVERY;
                    loop {
                        match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                            Ok(Cmd::Retry) | Err(RecvTimeoutError::Timeout) => break,
                            Ok(_) => {}
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    loop {
                        match rx.try_recv() {
                            Ok(_) => {}
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                };

                thread::sleep(Duration::from_millis(200));

                // A key held via `Cmd::Hold`: (key, safety deadline, echo_ms to
                // re-anchor at key-up). While set, we recv with a timeout so the
                // key auto-releases if no `Release` arrives (e.g. packets stop).
                let mut held: Option<(Key, std::time::Instant, u64)> = None;
                loop {
                    let cmd = match &held {
                        Some((_, deadline, _)) => match rx.recv_timeout(
                            deadline.saturating_duration_since(std::time::Instant::now()),
                        ) {
                            Ok(c) => Some(c),
                            Err(RecvTimeoutError::Timeout) => None, // deadline: release below
                            Err(RecvTimeoutError::Disconnected) => break,
                        },
                        None => match rx.recv() {
                            Ok(c) => Some(c),
                            Err(_) => break,
                        },
                    };
                    let syn = InputEvent::new(EventType::SYNCHRONIZATION, 0, 0);
                    match cmd {
                        // Timeout or Release: emit key-up for the held key, re-anchor echo.
                        // (`Retry` only matters while the device is missing: nothing to do here.)
                        None | Some(Cmd::Release) | Some(Cmd::Retry) => {
                            if let Some((key, _, echo)) = held.take() {
                                device.emit(&[InputEvent::new(EventType::KEY, key.code(), 0), syn]).ok();
                                worker_echo.open(echo);
                            }
                        }
                        Some(Cmd::Hold { key, max_hold_ms, echo_ms }) => {
                            // Shouldn't happen, but if something's already held, release it first.
                            if let Some((prev, _, echo)) = held.take() {
                                device.emit(&[InputEvent::new(EventType::KEY, prev.code(), 0), syn]).ok();
                                worker_echo.open(echo);
                            }
                            worker_echo.open(max_hold_ms + echo_ms);
                            device.emit(&[InputEvent::new(EventType::KEY, key.code(), 1), syn]).ok();
                            held = Some((
                                key,
                                std::time::Instant::now() + Duration::from_millis(max_hold_ms),
                                echo_ms,
                            ));
                        }
                        Some(Cmd::Key { key, hold_ms, gap_ms, echo_ms }) => {
                            if let Some(echo) = echo_ms {
                                worker_echo.open(hold_ms + echo);
                            }
                            device.emit(&[InputEvent::new(EventType::KEY, key.code(), 1), syn]).ok();
                            thread::sleep(Duration::from_millis(hold_ms));
                            device.emit(&[InputEvent::new(EventType::KEY, key.code(), 0), syn]).ok();
                            if let Some(echo) = echo_ms {
                                // Re-anchor at the real key-up (sleep may overshoot).
                                worker_echo.open(echo);
                            }
                            // Gap so back-to-back queued presses (a batched multi-gear kickdown) land as
                            // distinct key events instead of being coalesced into one.
                            if gap_ms > 0 {
                                thread::sleep(Duration::from_millis(gap_ms));
                            }
                        }
                    }
                }
            });
            let me = Self { tx, echo, gate: None, state };
            // Give the first build a moment so the startup permission probe sees a real answer
            // (ms in practice; success and EACCES both return at once) instead of "pending".
            me.wait_resolved(Duration::from_millis(500));
            me
        }

        fn wait_resolved(&self, max: Duration) {
            let end = Instant::now() + max;
            while self.state.load(Ordering::Relaxed) == PENDING && Instant::now() < end {
                thread::sleep(Duration::from_millis(5));
            }
        }

        /// Whether the virtual keyboard really exists: `Some(true)` it was created, `Some(false)`
        /// the last attempt failed (the worker keeps retrying), `None` still starting.
        pub fn uinput_ready(&self) -> Option<bool> {
            ready_from_state(self.state.load(Ordering::Relaxed))
        }

        /// Retry a failed virtual-device build now and wait briefly for the outcome (Setup ->
        /// Re-check). No-op while the device works.
        pub fn recheck(&self) {
            if self.state.compare_exchange(FAILED, PENDING, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                self.tx.try_send(Cmd::Retry).ok();
                self.wait_resolved(Duration::from_millis(300));
            }
        }

        fn is_ready(&self) -> bool { self.state.load(Ordering::Relaxed) == READY }

        /// Install a shared focus gate: when set and false, key emission is
        /// suppressed (so synthetic input never leaks into other apps).
        pub fn set_focus_gate(&mut self, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
            self.gate = Some(gate);
        }
        pub fn input_allowed(&self) -> bool {
            self.gate.as_ref().map_or(true, |g| g.load(std::sync::atomic::Ordering::Relaxed))
        }

        pub fn press(&self, key: Key, hold_ms: u64, gap_ms: u64) {
            if !self.input_allowed() || !self.is_ready() { return; }
            self.tx.send(Cmd::Key { key, hold_ms, gap_ms, echo_ms: None }).ok();
        }

        /// Press whose telemetry echo is trackable via [`Self::synthetic_active`].
        /// Non-blocking: a full queue drops the press (a skipped backfire pop is
        /// harmless; stalling the UI thread is not).
        pub fn press_tracked(&self, key: Key, hold_ms: u64, gap_ms: u64, echo_ms: u64) {
            if !self.input_allowed() || !self.is_ready() { return; }
            self.tx
                .try_send(Cmd::Key { key, hold_ms, gap_ms, echo_ms: Some(echo_ms) })
                .ok();
        }

        /// Press-and-hold `key` until a later [`Self::release`] (or auto-release
        /// after `max_hold_ms` as a stuck-key safety). Non-blocking, like
        /// `press_tracked`. Used for packet-based backfire (hold until next packet).
        pub fn hold_tracked(&self, key: Key, max_hold_ms: u64, echo_ms: u64) {
            if !self.input_allowed() || !self.is_ready() { return; }
            self.tx
                .try_send(Cmd::Hold { key, max_hold_ms, echo_ms })
                .ok();
        }

        /// Release a key held via [`Self::hold_tracked`]. Non-blocking.
        pub fn release(&self) {
            if !self.is_ready() { return; }
            self.tx.try_send(Cmd::Release).ok();
        }

        /// True while a tracked synthetic press may still echo back in telemetry.
        pub fn synthetic_active(&self, grace: Duration) -> bool {
            self.echo.active(grace)
        }
    }

    pub fn char_to_key(c: char) -> Option<Key> {
        match c {
            'w' | 'W' => Some(Key::KEY_W),
            'e' | 'E' => Some(Key::KEY_E),
            'q' | 'Q' => Some(Key::KEY_Q),
            _ => None,
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
    use std::thread;
    use std::time::Duration;

    use enigo::{Enigo, Key, Keyboard, Settings, Direction};

    use super::EchoWindow;

    #[derive(Clone, Copy, Debug)]
    pub struct KeyCode(pub Key);

    enum Cmd {
        Press { key: Key, hold_ms: u64, gap_ms: u64, echo_ms: Option<u64> },
        // Press-and-hold with a safety deadline; released by a later `Release`
        // (or auto-released at `max_hold_ms` if none arrives — e.g. packets stop).
        Hold { key: Key, max_hold_ms: u64, echo_ms: u64 },
        Release,
    }

    #[derive(Clone)]
    pub struct InputSender {
        tx: SyncSender<Cmd>,
        echo: EchoWindow,
        gate: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    }

    impl InputSender {
        pub fn new() -> Self {
            let (tx, rx) = mpsc::sync_channel::<Cmd>(64);
            let echo = EchoWindow::default();
            let worker_echo = echo.clone();
            thread::spawn(move || {
                let mut enigo = match Enigo::new(&Settings::default()) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("enigo: could not initialise input: {e}");
                        return;
                    }
                };
                // A key held via `Cmd::Hold`: (key, safety deadline, echo_ms to
                // re-anchor at key-up). While set, we recv with a timeout so the
                // key auto-releases if no `Release` arrives (e.g. packets stop).
                let mut held: Option<(Key, std::time::Instant, u64)> = None;
                loop {
                    let cmd = match &held {
                        Some((_, deadline, _)) => match rx.recv_timeout(
                            deadline.saturating_duration_since(std::time::Instant::now()),
                        ) {
                            Ok(c) => Some(c),
                            Err(RecvTimeoutError::Timeout) => None, // deadline: release below
                            Err(RecvTimeoutError::Disconnected) => break,
                        },
                        None => match rx.recv() {
                            Ok(c) => Some(c),
                            Err(_) => break,
                        },
                    };
                    match cmd {
                        // Timeout or Release: emit key-up for the held key, re-anchor echo.
                        None | Some(Cmd::Release) => {
                            if let Some((key, _, echo)) = held.take() {
                                enigo.key(key, Direction::Release).ok();
                                worker_echo.open(echo);
                            }
                        }
                        Some(Cmd::Hold { key, max_hold_ms, echo_ms }) => {
                            // Shouldn't happen, but if something's already held, release it first.
                            if let Some((prev, _, echo)) = held.take() {
                                enigo.key(prev, Direction::Release).ok();
                                worker_echo.open(echo);
                            }
                            worker_echo.open(max_hold_ms + echo_ms);
                            enigo.key(key, Direction::Press).ok();
                            held = Some((
                                key,
                                std::time::Instant::now() + Duration::from_millis(max_hold_ms),
                                echo_ms,
                            ));
                        }
                        Some(Cmd::Press { key, hold_ms, gap_ms, echo_ms }) => {
                            if let Some(echo) = echo_ms {
                                worker_echo.open(hold_ms + echo);
                            }
                            enigo.key(key, Direction::Press).ok();
                            thread::sleep(Duration::from_millis(hold_ms));
                            enigo.key(key, Direction::Release).ok();
                            if let Some(echo) = echo_ms {
                                // Re-anchor at the real key-up (sleep may overshoot).
                                worker_echo.open(echo);
                            }
                            // Gap so back-to-back queued presses (a batched multi-gear kickdown) land as
                            // distinct key events instead of being coalesced into one.
                            if gap_ms > 0 {
                                thread::sleep(Duration::from_millis(gap_ms));
                            }
                        }
                    }
                }
            });
            Self { tx, echo, gate: None }
        }

        /// Install a shared focus gate: when set and false, key emission is
        /// suppressed (so synthetic input never leaks into other apps).
        pub fn set_focus_gate(&mut self, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
            self.gate = Some(gate);
        }
        pub fn input_allowed(&self) -> bool {
            self.gate.as_ref().map_or(true, |g| g.load(std::sync::atomic::Ordering::Relaxed))
        }

        /// No virtual device to build (enigo): always ready.
        pub fn uinput_ready(&self) -> Option<bool> { Some(true) }
        pub fn recheck(&self) {}

        pub fn press(&self, key: KeyCode, hold_ms: u64, gap_ms: u64) {
            if !self.input_allowed() { return; }
            self.tx.send(Cmd::Press { key: key.0, hold_ms, gap_ms, echo_ms: None }).ok();
        }

        /// Press whose telemetry echo is trackable via [`Self::synthetic_active`].
        /// Non-blocking: a full queue drops the press (a skipped backfire pop is
        /// harmless; stalling the UI thread is not).
        pub fn press_tracked(&self, key: KeyCode, hold_ms: u64, gap_ms: u64, echo_ms: u64) {
            if !self.input_allowed() { return; }
            self.tx
                .try_send(Cmd::Press { key: key.0, hold_ms, gap_ms, echo_ms: Some(echo_ms) })
                .ok();
        }

        /// Press-and-hold `key` until a later [`Self::release`] (or auto-release
        /// after `max_hold_ms` as a stuck-key safety). Non-blocking, like
        /// `press_tracked`. Used for packet-based backfire (hold until next packet).
        pub fn hold_tracked(&self, key: KeyCode, max_hold_ms: u64, echo_ms: u64) {
            if !self.input_allowed() { return; }
            self.tx
                .try_send(Cmd::Hold { key: key.0, max_hold_ms, echo_ms })
                .ok();
        }

        /// Release a key held via [`Self::hold_tracked`]. Non-blocking.
        pub fn release(&self) {
            self.tx.try_send(Cmd::Release).ok();
        }

        /// True while a tracked synthetic press may still echo back in telemetry.
        pub fn synthetic_active(&self, grace: std::time::Duration) -> bool {
            self.echo.active(grace)
        }
    }

    pub fn char_to_key(c: char) -> Option<KeyCode> {
        match c {
            'w' | 'W' => Some(KeyCode(Key::Unicode('w'))),
            'e' | 'E' => Some(KeyCode(Key::Unicode('e'))),
            'q' | 'Q' => Some(KeyCode(Key::Unicode('q'))),
            _ => None,
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod stub {
    #[derive(Clone, Copy)]
    pub struct KeyCode;

    #[derive(Clone)]
    pub struct InputSender;

    impl InputSender {
        pub fn new() -> Self { Self }
        pub fn set_focus_gate(&mut self, _gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {}
        pub fn input_allowed(&self) -> bool { true }
        pub fn uinput_ready(&self) -> Option<bool> { Some(true) }
        pub fn recheck(&self) {}
        pub fn press(&self, _key: KeyCode, _hold_ms: u64, _gap_ms: u64) {}
        pub fn press_tracked(&self, _key: KeyCode, _hold_ms: u64, _gap_ms: u64, _echo_ms: u64) {}
        pub fn hold_tracked(&self, _key: KeyCode, _max_hold_ms: u64, _echo_ms: u64) {}
        pub fn release(&self) {}
        pub fn synthetic_active(&self, _grace: std::time::Duration) -> bool { false }
    }

    pub fn char_to_key(_c: char) -> Option<KeyCode> { None }
}

#[cfg(target_os = "linux")]
pub use linux::{InputSender, char_to_key};

#[cfg(target_os = "windows")]
pub use windows::{InputSender, KeyCode, char_to_key};

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub use stub::{InputSender, KeyCode, char_to_key};

// ── Permission check (Linux) ───────────────────────────────────────
// Hotkeys read /dev/input/event*, synthetic input writes /dev/uinput; both silently
// fail without the right group. `probe()` gathers the facts, `evaluate()` is pure.

/// Raw facts about input access. `Default` = everything fine (what Windows reports).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputProbe {
    /// A **keyboard** under `/dev/input/event*` is readable *and* the hotkey backend is reading
    /// at least one (no keyboard at all isn't a permission problem). See [`hotkeys_ok`].
    pub hotkeys_ok: bool,
    /// Key sending works: the virtual keyboard was really created ([`uinput_ok`]); before the
    /// sender has answered, `/dev/uinput` opening for writing stands in.
    pub uinput_ok: bool,
    /// `/dev/uinput` exists (else the `uinput` kernel module isn't loaded).
    pub uinput_exists: bool,
    /// `/dev/uinput` is group `input` with group rw — otherwise a udev rule is needed.
    pub uinput_group_input: bool,
    /// The running process is in the `input` group (effective groups: a `usermod` without a
    /// re-login does not count yet). Without it a user practically can't read keyboards or write
    /// `/dev/uinput`, so being outside it counts as **missing** (see [`evaluate`]).
    pub in_input_group: bool,
}

impl Default for InputProbe {
    fn default() -> Self {
        InputProbe { hotkeys_ok: true, uinput_ok: true, uinput_exists: true, uinput_group_input: true, in_input_group: true }
    }
}

pub const LBL_USERMOD: &str = "Add yourself to the input group (hotkeys and key input)";
pub const LBL_MODPROBE: &str = "Load the uinput kernel module";
pub const LBL_UDEV: &str = "Let the input group write /dev/uinput (key input)";
pub const CMD_USERMOD: &str = "sudo usermod -aG input $USER";
pub const CMD_MODPROBE: &str = "sudo modprobe uinput";
pub const CMD_UDEV: &str = "echo 'KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\"' | sudo tee /etc/udev/rules.d/99-uinput.rules && sudo udevadm control --reload && sudo udevadm trigger";

/// What is missing and the shell commands that fix it (deduplicated, in run order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputReport {
    pub hotkeys_missing: bool,
    pub uinput_missing: bool,
    /// Not a member of the `input` group.
    pub group_missing: bool,
    /// `(label, command)` pairs; the label is an English `tr()` key.
    pub commands: Vec<(&'static str, &'static str)>,
}

impl InputReport {
    pub fn any_missing(&self) -> bool { self.hotkeys_missing || self.uinput_missing || self.group_missing }
}

/// Pure: turn probe results into the missing items + fix commands. Not being in the `input` group
/// is itself "missing" (and offers `usermod`) even while the other two lights are green.
/// *Why:* the keyboard check alone was unreliable (a gaming mouse's key interface counted as a
/// working keyboard, so a user whose real keyboard was unreadable got no dialog), and without the
/// group a user practically can't read keyboards anyway. A false alarm (access set up by ACLs or
/// udev) is muted with "Don't remind me again".
pub fn evaluate(p: &InputProbe) -> InputReport {
    let mut r = InputReport {
        hotkeys_missing: !p.hotkeys_ok,
        uinput_missing: !p.uinput_ok,
        group_missing: !p.in_input_group,
        commands: Vec::new(),
    };
    if r.uinput_missing {
        if !p.uinput_exists {
            r.commands.push((LBL_MODPROBE, CMD_MODPROBE));
        } else if !p.uinput_group_input {
            r.commands.push((LBL_UDEV, CMD_UDEV));
        }
    }
    if r.group_missing {
        r.commands.insert(0, (LBL_USERMOD, CMD_USERMOD));
    }
    r
}

/// Pure: the hotkeys light. Permission-denied is red; "readable" but the backend opened zero
/// **physical** keyboards (`active_keyboards == 0`) is red too — *Why:* it must never stay green
/// while hotkeys are silently dead (virtual keyboards such as ydotoold's, or a mouse's key
/// interface, can be readable while the real keyboard isn't, and they don't count). No keyboard
/// at all is not a permission problem.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn hotkeys_ok(status: crate::hotkeys::HotkeyStatus, active_keyboards: usize) -> bool {
    use crate::hotkeys::HotkeyStatus::*;
    match status {
        NoPermission => false,
        Ok => active_keyboards > 0,
        NoDevice | Unsupported => true,
    }
}

/// Pure: the "key input" light. The **sender's own readiness** wins: `Some(true)` = the virtual
/// keyboard was really created, `Some(false)` = building it failed (the worker keeps retrying).
/// Only while it is still starting (`None`) the plain open-for-write test stands in. *Why:* the
/// open test alone stayed green while the device build failed and every key press was silently
/// dropped; and a created device is the proof that sending works, whatever a later open says.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn uinput_ok(open_ok: bool, sender_ready: Option<bool>) -> bool {
    sender_ready.unwrap_or(open_ok)
}

/// Pure: should the missing-permissions modal open now? Once per *transition* into "missing"
/// (or at startup), never repeatedly while it stays missing. `prev_missing` = what the previous
/// refresh saw (false at launch before the first one), `now_missing` = what this refresh sees.
/// *Why:* the status is live now, so access that breaks mid-session (a device unplugged, the
/// sender dying) must tell the user once, but a dismissed dialog must not pop up every 2 s.
pub fn modal_should_open(prev_missing: bool, now_missing: bool, remind: bool) -> bool {
    remind && now_missing && !prev_missing
}

/// Pure: does `/proc/self/status` list `gid` among the process's groups (`getgroups()` / `id -G`
/// semantics: the supplementary `Groups:` plus the effective gid, `Gid:`'s second field)?
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn status_has_gid(status: &str, gid: u32) -> bool {
    let nums = |line: &str| -> Vec<u32> { line.split_whitespace().skip(1).filter_map(|g| g.parse().ok()).collect() };
    status.lines().any(|l| {
        (l.starts_with("Groups:") && nums(l).contains(&gid))
            || (l.starts_with("Gid:") && nums(l).get(1) == Some(&gid))
    })
}

/// Probe this machine. `active_keyboards` = keyboards the hotkey backend is reading
/// (`HotkeyListener::active_keyboards`); `uinput_ready` = [`InputSender::uinput_ready`].
/// Windows (and anything non-Linux) needs no permissions.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn probe(active_keyboards: usize, uinput_ready: Option<bool>) -> InputProbe {
    #[cfg(target_os = "linux")]
    {
        // Dev/testing aid: pretend nothing is permitted so the modal can be reviewed.
        if std::env::var_os("FORZA_FAKE_NO_INPUT_PERMS").is_some_and(|v| v == "1") {
            return InputProbe { hotkeys_ok: false, uinput_ok: false, uinput_exists: true, uinput_group_input: false, in_input_group: false };
        }
        use std::os::unix::fs::MetadataExt;
        let input_gid = std::fs::read_to_string("/etc/group").ok().and_then(|g| {
            g.lines().find_map(|l| {
                let mut f = l.split(':');
                if f.next() != Some("input") { return None; }
                f.nth(1).and_then(|n| n.parse::<u32>().ok())
            })
        });
        // The process's own groups, so a fresh `usermod` without a re-login is correctly still
        // "not a member".
        let in_input_group = input_gid.is_some_and(|gid| {
            std::fs::read_to_string("/proc/self/status").ok().is_some_and(|s| status_has_gid(&s, gid))
        });
        let meta = std::fs::metadata("/dev/uinput").ok();
        InputProbe {
            hotkeys_ok: hotkeys_ok(crate::hotkeys::probe_status(), active_keyboards),
            uinput_ok: uinput_ok(std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok(), uinput_ready),
            uinput_exists: meta.is_some(),
            uinput_group_input: meta.is_some_and(|m| Some(m.gid()) == input_gid && m.mode() & 0o060 == 0o060),
            in_input_group,
        }
    }
    #[cfg(not(target_os = "linux"))]
    { InputProbe::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_all_ok_is_empty() {
        let r = evaluate(&InputProbe::default());
        assert!(!r.any_missing());
        assert!(r.commands.is_empty());
    }

    #[test]
    fn evaluate_not_in_group_suggests_usermod() {
        let p = InputProbe { hotkeys_ok: false, uinput_ok: false, in_input_group: false, ..Default::default() };
        let r = evaluate(&p);
        assert!(r.hotkeys_missing && r.uinput_missing);
        assert_eq!(r.commands, vec![(LBL_USERMOD, CMD_USERMOD)]);
    }

    #[test]
    fn evaluate_uinput_module_and_udev() {
        let p = InputProbe { uinput_ok: false, uinput_exists: false, ..Default::default() };
        assert_eq!(evaluate(&p).commands, vec![(LBL_MODPROBE, CMD_MODPROBE)]);
        let p = InputProbe { uinput_ok: false, uinput_group_input: false, in_input_group: false, ..Default::default() };
        assert_eq!(evaluate(&p).commands, vec![(LBL_USERMOD, CMD_USERMOD), (LBL_UDEV, CMD_UDEV)]);
    }

    #[test]
    fn evaluate_group_member_only_hotkeys_gives_no_command() {
        // Already in the group but still can't read: needs a re-login, no command helps.
        let r = evaluate(&InputProbe { hotkeys_ok: false, ..Default::default() });
        assert!(r.hotkeys_missing && r.commands.is_empty());
    }

    #[test]
    fn hotkeys_light_needs_permission_and_an_open_keyboard() {
        use crate::hotkeys::HotkeyStatus::*;
        assert!(!hotkeys_ok(NoPermission, 0));
        assert!(!hotkeys_ok(NoPermission, 2));
        assert!(!hotkeys_ok(Ok, 0), "readable but zero keyboards opened must be red");
        assert!(hotkeys_ok(Ok, 1));
        assert!(hotkeys_ok(NoDevice, 0));
    }

    #[test]
    fn uinput_light_follows_sender_readiness() {
        // Sender answered: its answer wins over the open test.
        assert!(!uinput_ok(true, Some(false)), "open ok but device build failed must be red");
        assert!(uinput_ok(false, Some(true)), "a created device proves sending works");
        assert!(uinput_ok(true, Some(true)));
        assert!(!uinput_ok(false, Some(false)));
        // Still pending: fall back to the open test.
        assert!(uinput_ok(true, None));
        assert!(!uinput_ok(false, None));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn sender_state_maps_to_tristate() {
        assert_eq!(linux::ready_from_state(0), None);
        assert_eq!(linux::ready_from_state(1), Some(true));
        assert_eq!(linux::ready_from_state(2), Some(false));
    }

    #[test]
    fn modal_reshows_once_per_transition_to_missing() {
        // Startup: missing and reminders on -> open.
        assert!(modal_should_open(false, true, true));
        // Still missing (even after the user closed it): no repeat.
        assert!(!modal_should_open(true, true, true));
        // Fine -> fine, and recovery: nothing.
        assert!(!modal_should_open(false, false, true));
        assert!(!modal_should_open(true, false, true));
        // Reminders off: never.
        assert!(!modal_should_open(false, true, false));
        // OK -> missing -> OK -> missing walks through two openings.
        let walk = [(false, true), (true, true), (true, false), (false, true)];
        assert_eq!(walk.iter().filter(|(p, n)| modal_should_open(*p, *n, true)).count(), 2);
    }

    #[test]
    fn group_membership_reads_the_process_groups() {
        let st = "Name:\tx\nGid:\t1000\t1000\t1000\t1000\nGroups:\t10 998 1000\n";
        assert!(status_has_gid(st, 998), "supplementary group");
        assert!(status_has_gid(st, 1000), "primary / effective gid");
        assert!(!status_has_gid(st, 104), "not a member (a fresh usermod without re-login)");
        assert!(!status_has_gid("Name:\tx\n", 998));
    }

    #[test]
    fn not_in_the_input_group_is_missing_on_its_own() {
        // Other lights green, user not in `input`: missing -> modal, with the usermod command.
        let r = evaluate(&InputProbe { in_input_group: false, ..Default::default() });
        assert!(r.group_missing && !r.hotkeys_missing && !r.uinput_missing);
        assert!(r.any_missing());
        assert_eq!(r.commands, vec![(LBL_USERMOD, CMD_USERMOD)]);
        assert!(modal_should_open(false, r.any_missing(), true));
        // In the group and everything else fine: nothing missing.
        let r = evaluate(&InputProbe { in_input_group: true, ..Default::default() });
        assert!(!r.any_missing() && !r.group_missing && r.commands.is_empty());
    }

    #[test]
    fn focus_gate_blocks_when_not_allowed() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let gate = Arc::new(AtomicBool::new(true));
        let mut sender = InputSender::new();
        sender.set_focus_gate(gate.clone());
        assert!(sender.input_allowed());
        gate.store(false, Ordering::Relaxed);
        assert!(!sender.input_allowed());
        // No gate set → always allowed.
        let plain = InputSender::new();
        assert!(plain.input_allowed());
    }

    #[test]
    fn echo_window_opens_and_expires() {
        let w = EchoWindow::default();
        assert!(!w.active(Duration::ZERO), "fresh window must be closed");
        w.open(10_000);
        assert!(w.active(Duration::ZERO), "opened window must be active");
        // An already-expired deadline is inactive without grace, active with it.
        w.open(0);
        std::thread::sleep(Duration::from_millis(2));
        assert!(!w.active(Duration::ZERO));
        assert!(w.active(Duration::from_secs(5)), "grace extends the window");
    }
}
