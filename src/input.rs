use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
    use std::thread;
    use std::time::Duration;

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
                let mut keys = AttributeSet::<Key>::new();
                keys.insert(Key::KEY_W);
                keys.insert(Key::KEY_E);
                keys.insert(Key::KEY_Q);

                let device = VirtualDeviceBuilder::new()
                    .and_then(|b| b.name("Forza Telemetry Input").with_keys(&keys))
                    .and_then(|b| b.build());

                let mut device = match device {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("uinput: could not create virtual device: {e}");
                        eprintln!("uinput: ensure the current user is in the 'input' group or /dev/uinput is accessible");
                        return;
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
                        None | Some(Cmd::Release) => {
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

        pub fn press(&self, key: Key, hold_ms: u64, gap_ms: u64) {
            if !self.input_allowed() { return; }
            self.tx.send(Cmd::Key { key, hold_ms, gap_ms, echo_ms: None }).ok();
        }

        /// Press whose telemetry echo is trackable via [`Self::synthetic_active`].
        /// Non-blocking: a full queue drops the press (a skipped backfire pop is
        /// harmless; stalling the UI thread is not).
        pub fn press_tracked(&self, key: Key, hold_ms: u64, gap_ms: u64, echo_ms: u64) {
            if !self.input_allowed() { return; }
            self.tx
                .try_send(Cmd::Key { key, hold_ms, gap_ms, echo_ms: Some(echo_ms) })
                .ok();
        }

        /// Press-and-hold `key` until a later [`Self::release`] (or auto-release
        /// after `max_hold_ms` as a stuck-key safety). Non-blocking, like
        /// `press_tracked`. Used for packet-based backfire (hold until next packet).
        pub fn hold_tracked(&self, key: Key, max_hold_ms: u64, echo_ms: u64) {
            if !self.input_allowed() { return; }
            self.tx
                .try_send(Cmd::Hold { key, max_hold_ms, echo_ms })
                .ok();
        }

        /// Release a key held via [`Self::hold_tracked`]. Non-blocking.
        pub fn release(&self) {
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
    /// `/dev/uinput` can be opened for writing.
    pub uinput_ok: bool,
    /// `/dev/uinput` exists (else the `uinput` kernel module isn't loaded).
    pub uinput_exists: bool,
    /// `/dev/uinput` is group `input` with group rw — otherwise a udev rule is needed.
    pub uinput_group_input: bool,
    /// The running process is in the `input` group.
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
    /// `(label, command)` pairs; the label is an English `tr()` key.
    pub commands: Vec<(&'static str, &'static str)>,
}

impl InputReport {
    pub fn any_missing(&self) -> bool { self.hotkeys_missing || self.uinput_missing }
}

/// Pure: turn probe results into the missing items + fix commands.
pub fn evaluate(p: &InputProbe) -> InputReport {
    let mut r = InputReport { hotkeys_missing: !p.hotkeys_ok, uinput_missing: !p.uinput_ok, commands: Vec::new() };
    if r.uinput_missing {
        if !p.uinput_exists {
            r.commands.push((LBL_MODPROBE, CMD_MODPROBE));
        } else if !p.uinput_group_input {
            r.commands.push((LBL_UDEV, CMD_UDEV));
        }
    }
    if r.any_missing() && !p.in_input_group {
        r.commands.insert(0, (LBL_USERMOD, CMD_USERMOD));
    }
    r
}

/// Pure: the hotkeys light. Permission-denied is red; "readable" but the backend opened zero
/// keyboards (`active_keyboards == 0`) is red too — *Why:* it must never stay green while
/// hotkeys are silently dead. No keyboard at all is not a permission problem.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn hotkeys_ok(status: crate::hotkeys::HotkeyStatus, active_keyboards: usize) -> bool {
    use crate::hotkeys::HotkeyStatus::*;
    match status {
        NoPermission => false,
        Ok => active_keyboards > 0,
        NoDevice | Unsupported => true,
    }
}

/// Probe this machine. `active_keyboards` = keyboards the hotkey backend is reading
/// (`HotkeyListener::active_keyboards`). Windows (and anything non-Linux) needs no permissions.
#[cfg_attr(not(target_os = "linux"), allow(unused_variables))]
pub fn probe(active_keyboards: usize) -> InputProbe {
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
        let in_input_group = input_gid.is_some_and(|gid| {
            std::fs::read_to_string("/proc/self/status").ok().is_some_and(|s| {
                s.lines().find(|l| l.starts_with("Groups:")).is_some_and(|l| {
                    l.split_whitespace().skip(1).any(|g| g.parse::<u32>().ok() == Some(gid))
                })
            })
        });
        let meta = std::fs::metadata("/dev/uinput").ok();
        InputProbe {
            hotkeys_ok: hotkeys_ok(crate::hotkeys::probe_status(), active_keyboards),
            uinput_ok: std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok(),
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
    fn evaluate_group_alone_never_nags() {
        // Everything works via ACLs but the user is not in `input`: nothing missing -> no modal.
        let r = evaluate(&InputProbe { in_input_group: false, ..Default::default() });
        assert!(!r.any_missing() && r.commands.is_empty());
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
