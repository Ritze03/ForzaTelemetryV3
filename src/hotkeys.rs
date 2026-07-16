//! Global key capture. A background backend (Linux: evdev read of /dev/input;
//! Windows: GetAsyncKeyState poll) matches configured global-scope combos and
//! pushes the matched HotkeyAction down an mpsc channel drained on the main
//! thread. Match-only: no other keystroke is stored, sent, or logged. See spec.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use crate::config::HotkeyAction;
use crate::keymap::{HotKey, HotkeyBinding, Mods};

/// Shared list of global bindings the backend matches against.
pub type Bindings = Arc<Mutex<Vec<(HotkeyBinding, HotkeyAction)>>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyStatus { Ok, NoPermission, NoDevice, Unsupported }

/// Pure match: a combo matches when its key was pressed and modifiers match exactly.
pub fn match_combo(binds: &[(HotkeyBinding, HotkeyAction)], key: HotKey, mods: Mods) -> Option<HotkeyAction> {
    binds.iter().find(|(b, _)| b.key == key && b.mods == mods).map(|(_, a)| *a)
}

pub struct HotkeyListener {
    rx: Receiver<HotkeyAction>,
    binds: Bindings,
    status: Arc<Mutex<HotkeyStatus>>,
}

impl HotkeyListener {
    pub fn new(initial: Vec<(HotkeyBinding, HotkeyAction)>) -> Self {
        let binds: Bindings = Arc::new(Mutex::new(initial));
        let status = Arc::new(Mutex::new(HotkeyStatus::Unsupported));
        let (tx, rx) = std::sync::mpsc::channel();
        backend::spawn(binds.clone(), status.clone(), tx);
        HotkeyListener { rx, binds, status }
    }
    pub fn try_recv(&self) -> Option<HotkeyAction> {
        match self.rx.try_recv() {
            Ok(a) => Some(a),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
    pub fn set_bindings(&self, b: Vec<(HotkeyBinding, HotkeyAction)>) { *self.binds.lock().unwrap() = b; }
    pub fn status(&self) -> HotkeyStatus { *self.status.lock().unwrap() }
}

#[cfg(target_os = "linux")]
mod backend {
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use evdev::{Device, EventType, Key};
    use super::{Bindings, HotkeyStatus, match_combo};
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, Mods};

    pub fn spawn(binds: Bindings, status: Arc<Mutex<HotkeyStatus>>, tx: Sender<HotkeyAction>) {
        thread::spawn(move || {
            let keyboards: Vec<(std::path::PathBuf, Device)> = evdev::enumerate()
                .filter(|(_, d)| d.supported_keys().map_or(false, |k| k.contains(Key::KEY_A)))
                .collect();
            if keyboards.is_empty() {
                // Could be no keyboards, or (more likely) no read permission.
                *status.lock().unwrap() = HotkeyStatus::NoPermission;
                return;
            }
            *status.lock().unwrap() = HotkeyStatus::Ok;
            // One reader thread per keyboard; each tracks its own modifier state.
            for (_, mut dev) in keyboards {
                let binds = binds.clone();
                let tx = tx.clone();
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
                                        if let Some(action) = match_combo(&list, hk, mods) {
                                            let _ = tx.send(action);
                                        }
                                        // Non-matching keys are dropped here — never stored/logged.
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                });
            }
        });
    }
}

#[cfg(target_os = "windows")]
mod backend {
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use super::{Bindings, HotkeyStatus, match_combo};
    use crate::config::HotkeyAction;
    use crate::keymap::Mods;

    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12; // Alt
    const VK_SHIFT: i32 = 0x10;
    const VK_LWIN: i32 = 0x5B;

    fn down(vk: i32) -> bool { (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0 }

    pub fn spawn(binds: Bindings, status: Arc<Mutex<HotkeyStatus>>, tx: Sender<HotkeyAction>) {
        *status.lock().unwrap() = HotkeyStatus::Ok;
        thread::spawn(move || {
            // Per-action previous key-down state for rising-edge detection.
            let mut prev: std::collections::HashMap<HotkeyAction, bool> = std::collections::HashMap::new();
            loop {
                let mods = Mods { ctrl: down(VK_CONTROL), alt: down(VK_MENU), shift: down(VK_SHIFT), sup: down(VK_LWIN) };
                let list = binds.lock().unwrap().clone();
                for (b, action) in &list {
                    let key_down = down(b.key.to_vk());
                    let was = *prev.get(action).unwrap_or(&false);
                    // Rising edge of the base key, with modifiers matching now.
                    if key_down && !was {
                        if match_combo(&list, b.key, mods) == Some(*action) {
                            let _ = tx.send(*action);
                        }
                    }
                    prev.insert(*action, key_down);
                }
                thread::sleep(Duration::from_millis(8));
            }
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod backend {
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use super::{Bindings, HotkeyStatus};
    use crate::config::HotkeyAction;
    pub fn spawn(_b: Bindings, status: Arc<Mutex<HotkeyStatus>>, _tx: Sender<HotkeyAction>) {
        *status.lock().unwrap() = HotkeyStatus::Unsupported;
    }
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
    fn matches_key_and_exact_modifiers() {
        let binds = vec![
            (bind(false, HotKey::G), HotkeyAction::ToggleGearbox),
            (bind(true, HotKey::E), HotkeyAction::DashboardEdit),
        ];
        // Plain G with no mods → gearbox.
        assert_eq!(match_combo(&binds, HotKey::G, Mods::default()), Some(HotkeyAction::ToggleGearbox));
        // G but Ctrl held → no match (modifiers must match exactly).
        assert_eq!(match_combo(&binds, HotKey::G, Mods { ctrl: true, ..Default::default() }), None);
        // Ctrl+E → dashboard.
        assert_eq!(match_combo(&binds, HotKey::E, Mods { ctrl: true, ..Default::default() }), Some(HotkeyAction::DashboardEdit));
    }
}
