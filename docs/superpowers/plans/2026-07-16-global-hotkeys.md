# Global Hotkeys Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add rebindable hotkeys that fire while the game *or* our app is focused (Linux via evdev read, Windows via GetAsyncKeyState), plus a shared window-focus detector reused to gate synthetic input, all configurable from the Settings tab.

**Architecture:** A `keymap` module defines a serde-stable `HotKey` enum with mapping tables to egui/evdev/Windows-VK. A background capture backend (`hotkeys.rs`) matches configured combos and pushes `HotkeyAction`s down an mpsc channel drained on the main thread. A shared `FocusDetector` (`focus.rs`) polls the active window at N Hz into an `AtomicBool`, consumed by both the hotkey gate and a new `InputSender` focus gate. Config lives in a `HotkeyConfig` nested in `AppConfig`.

**Tech Stack:** Rust, egui/eframe 0.33, `evdev` 0.12 (Linux, already a dep), `windows-sys` (Windows, new dep), serde.

**Spec:** `docs/superpowers/specs/2026-07-16-global-hotkeys-design.md`

---

## File Structure

| File | Responsibility |
| --- | --- |
| `src/keymap.rs` *(new)* | `HotKey` enum + `Mods` + `HotkeyBinding`, mapping tables (egui/evdev/VK), combo label formatting. Pure, cross-platform. |
| `src/config.rs` *(mod)* | `HotkeyAction`, `HotkeyScope`, `GateMode`, `FocusMethod`, `HotkeyConfig`; `hotkeys` field on `AppConfig`. |
| `src/hotkeys.rs` *(new)* | `HotkeyListener`, combo-match logic, Linux evdev reader / Windows poll / stub backends, `HotkeyStatus`. |
| `src/focus.rs` *(new)* | `FocusDetector` poll thread, per-method active-window query, `game_match` matching, cached `AtomicBool` + status. |
| `src/input.rs` *(mod)* | Optional focus gate on `InputSender` (suppress emission when not focused). |
| `src/app.rs` *(mod)* | Construct listener + detector, drain each frame, gate decision, config-driven app-focused hotkeys, delete F10 handler. |
| `src/ui/settings.rs` *(mod)* | HOTKEYS card: rebind rows, detection settings, Detect button, Custom preview, status light. |
| `src/i18n.rs` *(mod)* | New English→German strings. |
| `src/main.rs` *(mod)* | `mod keymap; mod hotkeys; mod focus;`. |
| `Cargo.toml` *(mod)* | `windows-sys` under the Windows target. |
| `docs/features/hotkeys.md` *(new)*, `docs/architecture/overview.md` *(mod)*, `CHANGELOG.md` *(mod)* | Docs. |

**Ordering note:** Tasks 1–2 (keymap, config) are the shared scaffolding — do them first and sequentially. Tasks 3–5 (focus, backend, input gate) are independent of each other. Task 6 (wiring) depends on 1–5. Task 7 (UI) depends on 1–2. Tasks 8–9 last.

---

## Task 1: `keymap` module — HotKey enum + mappings

**Files:**
- Create: `src/keymap.rs`
- Modify: `src/main.rs:2` (add `mod keymap;`)
- Test: inline `#[cfg(test)]` in `src/keymap.rs`

- [ ] **Step 1: Add the module declaration**

In `src/main.rs`, add after `mod input;` (keep alphabetical-ish order near the others):

```rust
mod keymap;
```

- [ ] **Step 2: Write the failing test**

Create `src/keymap.rs` with only the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_roundtrip_is_identity_for_all_variants() {
        for &hk in HotKey::ALL {
            let back = HotKey::from_egui(hk.to_egui());
            assert_eq!(back, Some(hk), "egui round-trip failed for {hk:?}");
        }
    }

    #[test]
    fn combo_label_formats_modifiers_in_order() {
        let b = HotkeyBinding { mods: Mods { ctrl: true, alt: false, shift: false, sup: false }, key: HotKey::E };
        assert_eq!(b.label(), "Ctrl + E");
        let g = HotkeyBinding { mods: Mods::default(), key: HotKey::G };
        assert_eq!(g.label(), "G");
    }

    #[test]
    fn every_variant_has_a_nonempty_key_label() {
        for &hk in HotKey::ALL {
            assert!(!hk.label().is_empty(), "{hk:?} has empty label");
        }
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --lib keymap 2>&1 | tail -20`
Expected: FAIL — `cannot find type HotKey` / does not compile.

- [ ] **Step 4: Implement the enum, macro table, and helpers**

Prepend to `src/keymap.rs` (above the test module):

```rust
//! Cross-platform key identity. `HotKey` is serde-stable (serialized by variant
//! name), independent of egui's optional serde feature, and maps to egui::Key,
//! evdev::Key (Linux), and Windows virtual-key codes via one table. See the
//! global-hotkeys spec.

use serde::{Deserialize, Serialize};

/// Modifier flags for a key combo.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub sup: bool, // Super / Meta / Windows key
}

/// A full key combo: modifiers + a base key.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct HotkeyBinding {
    pub mods: Mods,
    pub key: HotKey,
}

impl HotkeyBinding {
    /// Human label, e.g. "Ctrl + E" or "G". Modifier order: Ctrl, Alt, Shift, Super.
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.mods.ctrl { parts.push("Ctrl"); }
        if self.mods.alt { parts.push("Alt"); }
        if self.mods.shift { parts.push("Shift"); }
        if self.mods.sup { parts.push("Super"); }
        parts.push(self.key.label());
        parts.join(" + ")
    }
}

// One row per bindable key: variant => egui::Key, evdev key ident, Windows VK, label.
// evdev idents are the `evdev::Key::KEY_*` names; VK are Win32 virtual-key codes.
macro_rules! hotkeys {
    ($($variant:ident => $egui:ident, $evdev:ident, $vk:expr, $label:expr);+ $(;)?) => {
        /// Serde-stable key identity (serialized as the variant name).
        #[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum HotKey { $($variant),+ }

        impl HotKey {
            /// Every variant, for iteration in tests and UI pickers.
            pub const ALL: &'static [HotKey] = &[$(HotKey::$variant),+];

            pub fn to_egui(self) -> egui::Key {
                match self { $(HotKey::$variant => egui::Key::$egui),+ }
            }
            pub fn from_egui(k: egui::Key) -> Option<Self> {
                match k { $(egui::Key::$egui => Some(HotKey::$variant),)+ _ => None }
            }
            pub fn label(self) -> &'static str {
                match self { $(HotKey::$variant => $label),+ }
            }
            #[cfg(target_os = "linux")]
            pub fn to_evdev(self) -> evdev::Key {
                match self { $(HotKey::$variant => evdev::Key::$evdev),+ }
            }
            #[cfg(target_os = "linux")]
            pub fn from_evdev(k: evdev::Key) -> Option<Self> {
                match k { $(evdev::Key::$evdev => Some(HotKey::$variant),)+ _ => None }
            }
            #[cfg(target_os = "windows")]
            pub fn to_vk(self) -> i32 {
                match self { $(HotKey::$variant => $vk),+ }
            }
        }
    };
}

hotkeys! {
    A => A, KEY_A, 0x41, "A";  B => B, KEY_B, 0x42, "B";  C => C, KEY_C, 0x43, "C";
    D => D, KEY_D, 0x44, "D";  E => E, KEY_E, 0x45, "E";  F => F, KEY_F, 0x46, "F";
    G => G, KEY_G, 0x47, "G";  H => H, KEY_H, 0x48, "H";  I => I, KEY_I, 0x49, "I";
    J => J, KEY_J, 0x4A, "J";  K => K, KEY_K, 0x4B, "K";  L => L, KEY_L, 0x4C, "L";
    M => M, KEY_M, 0x4D, "M";  N => N, KEY_N, 0x4E, "N";  O => O, KEY_O, 0x4F, "O";
    P => P, KEY_P, 0x50, "P";  Q => Q, KEY_Q, 0x51, "Q";  R => R, KEY_R, 0x52, "R";
    S => S, KEY_S, 0x53, "S";  T => T, KEY_T, 0x54, "T";  U => U, KEY_U, 0x55, "U";
    V => V, KEY_V, 0x56, "V";  W => W, KEY_W, 0x57, "W";  X => X, KEY_X, 0x58, "X";
    Y => Y, KEY_Y, 0x59, "Y";  Z => Z, KEY_Z, 0x5A, "Z";
    Num0 => Num0, KEY_0, 0x30, "0";  Num1 => Num1, KEY_1, 0x31, "1";
    Num2 => Num2, KEY_2, 0x32, "2";  Num3 => Num3, KEY_3, 0x33, "3";
    Num4 => Num4, KEY_4, 0x34, "4";  Num5 => Num5, KEY_5, 0x35, "5";
    Num6 => Num6, KEY_6, 0x36, "6";  Num7 => Num7, KEY_7, 0x37, "7";
    Num8 => Num8, KEY_8, 0x38, "8";  Num9 => Num9, KEY_9, 0x39, "9";
    F1 => F1, KEY_F1, 0x70, "F1";  F2 => F2, KEY_F2, 0x71, "F2";
    F3 => F3, KEY_F3, 0x72, "F3";  F4 => F4, KEY_F4, 0x73, "F4";
    F5 => F5, KEY_F5, 0x74, "F5";  F6 => F6, KEY_F6, 0x75, "F6";
    F7 => F7, KEY_F7, 0x76, "F7";  F8 => F8, KEY_F8, 0x77, "F8";
    F9 => F9, KEY_F9, 0x78, "F9";  F10 => F10, KEY_F10, 0x79, "F10";
    F11 => F11, KEY_F11, 0x7A, "F11";  F12 => F12, KEY_F12, 0x7B, "F12";
    Space => Space, KEY_SPACE, 0x20, "Space";
    Escape => Escape, KEY_ESC, 0x1B, "Esc";
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib keymap 2>&1 | tail -20`
Expected: PASS — 3 tests ok. (On Linux the `evdev` arms compile because `evdev` is a Linux dep.)

- [ ] **Step 6: Build**

Run: `cargo build 2>&1 | tail -3`
Expected: Finished, no errors.

- [ ] **Step 7: Commit**

```bash
git add src/keymap.rs src/main.rs
git commit -m "feat(hotkeys): add keymap module (HotKey enum + egui/evdev/VK mappings)"
```

---

## Task 2: Config model — HotkeyConfig on AppConfig

**Files:**
- Modify: `src/config.rs` (new types near the other config enums ~line 45; new field on `AppConfig` struct ~line 325; default in `impl Default` ~line 445)
- Test: inline `#[cfg(test)]` in `src/config.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `src/config.rs` (find it with `grep -n "mod tests" src/config.rs`; if none, add one at end of file):

```rust
#[test]
fn hotkey_config_deserializes_from_empty_via_defaults() {
    let hk: HotkeyConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(hk, HotkeyConfig::default());
}

#[test]
fn default_hotkey_bindings_are_g_b_ctrl_s_ctrl_e() {
    use crate::keymap::{HotKey, Mods};
    let hk = HotkeyConfig::default();
    assert_eq!(hk.bindings[&HotkeyAction::ToggleGearbox].key, HotKey::G);
    assert_eq!(hk.bindings[&HotkeyAction::ToggleBackfire].key, HotKey::B);
    let mini = &hk.bindings[&HotkeyAction::MiniSettings];
    assert_eq!(mini.key, HotKey::S);
    assert_eq!(mini.mods, Mods { ctrl: true, ..Default::default() });
    assert_eq!(hk.bindings[&HotkeyAction::DashboardEdit].key, HotKey::E);
}

#[test]
fn action_scopes_split_global_and_app() {
    assert_eq!(HotkeyAction::ToggleGearbox.scope(), HotkeyScope::Global);
    assert_eq!(HotkeyAction::MiniSettings.scope(), HotkeyScope::AppFocused);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib config::tests::hotkey 2>&1 | tail -20`
Expected: FAIL — `cannot find type HotkeyConfig`.

- [ ] **Step 3: Add the new types**

In `src/config.rs`, near the other small enums (after the block ending ~line 60), add:

```rust
/// A bindable hotkey action. Scope is fixed per action (see `scope`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HotkeyAction {
    ToggleGearbox,
    ToggleBackfire,
    MiniSettings,
    DashboardEdit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HotkeyScope {
    Global,
    AppFocused,
}

impl HotkeyAction {
    /// Every action, in display order (globals first).
    pub const ALL: &'static [HotkeyAction] = &[
        HotkeyAction::ToggleGearbox,
        HotkeyAction::ToggleBackfire,
        HotkeyAction::MiniSettings,
        HotkeyAction::DashboardEdit,
    ];
    pub fn scope(self) -> HotkeyScope {
        match self {
            HotkeyAction::ToggleGearbox | HotkeyAction::ToggleBackfire => HotkeyScope::Global,
            HotkeyAction::MiniSettings | HotkeyAction::DashboardEdit => HotkeyScope::AppFocused,
        }
    }
    /// English label for the settings row.
    pub fn label(self) -> &'static str {
        match self {
            HotkeyAction::ToggleGearbox => "Toggle Automatic Gearbox",
            HotkeyAction::ToggleBackfire => "Toggle Backfire",
            HotkeyAction::MiniSettings => "Open mini-settings",
            HotkeyAction::DashboardEdit => "Toggle dashboard edit",
        }
    }
}

/// How global hotkeys decide the game is focused.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum GateMode {
    #[default]
    TelemetryLive,
    WindowFocus,
}

/// Which active-window query the WindowFocus gate uses (Linux). Windows always
/// uses GetForegroundWindow and ignores this.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum FocusMethod {
    #[default]
    Hyprland,
    X11,
    Custom,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct HotkeyConfig {
    #[serde(default = "default_bindings")]
    pub bindings: std::collections::HashMap<HotkeyAction, crate::keymap::HotkeyBinding>,
    #[serde(default)]
    pub gate_mode: GateMode,
    #[serde(default)]
    pub focus_method: FocusMethod,
    #[serde(default)]
    pub custom_cmd: String,
    #[serde(default = "default_game_match")]
    pub game_match: String,
    #[serde(default)]
    pub input_focus_gate: bool,
    #[serde(default = "default_poll_hz")]
    pub focus_poll_hz: f32,
}

fn default_game_match() -> String { "Forza".to_string() }
fn default_poll_hz() -> f32 { 4.0 }

fn default_bindings() -> std::collections::HashMap<HotkeyAction, crate::keymap::HotkeyBinding> {
    use crate::keymap::{HotKey, HotkeyBinding, Mods};
    let mut m = std::collections::HashMap::new();
    m.insert(HotkeyAction::ToggleGearbox, HotkeyBinding { mods: Mods::default(), key: HotKey::G });
    m.insert(HotkeyAction::ToggleBackfire, HotkeyBinding { mods: Mods::default(), key: HotKey::B });
    m.insert(HotkeyAction::MiniSettings, HotkeyBinding { mods: Mods { ctrl: true, ..Default::default() }, key: HotKey::S });
    m.insert(HotkeyAction::DashboardEdit, HotkeyBinding { mods: Mods { ctrl: true, ..Default::default() }, key: HotKey::E });
    m
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            bindings: default_bindings(),
            gate_mode: GateMode::default(),
            focus_method: FocusMethod::default(),
            custom_cmd: String::new(),
            game_match: default_game_match(),
            input_focus_gate: false,
            focus_poll_hz: default_poll_hz(),
        }
    }
}
```

- [ ] **Step 4: Add the field to `AppConfig` + its default**

In `struct AppConfig` (near `dsg_enabled` ~line 325) add:

```rust
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
```

In `impl Default for AppConfig` (near `dsg_enabled: false` ~line 445) add:

```rust
            hotkeys: HotkeyConfig::default(),
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib config::tests::hotkey 2>&1 | tail -20` then `cargo test --lib config 2>&1 | tail -8`
Expected: new tests PASS; the existing `old_config_missing_new_keys_deserializes_via_defaults` still PASS (the top-level merge in `AppConfig::load` fills a missing `hotkeys` key).

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat(hotkeys): add HotkeyConfig (actions, scope, gate mode, focus method) to AppConfig"
```

---

## Task 3: FocusDetector — active-window query + poll thread

**Files:**
- Create: `src/focus.rs`
- Modify: `src/main.rs` (add `mod focus;`), `Cargo.toml` (windows-sys)
- Test: inline `#[cfg(test)]` in `src/focus.rs`

- [ ] **Step 1: Add windows-sys dependency**

In `Cargo.toml`, under `[target.'cfg(target_os = "windows")'.dependencies]` (after `enigo = "0.6"`):

```toml
windows-sys = { version = "0.59", features = ["Win32_UI_Input_KeyboardAndMouse", "Win32_UI_WindowsAndMessaging", "Win32_Foundation"] }
```

- [ ] **Step 2: Add the module declaration**

In `src/main.rs`, add `mod focus;` near the other decls.

- [ ] **Step 3: Write the failing test**

Create `src/focus.rs` with the test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_is_case_insensitive_substring() {
        assert!(window_matches("Forza Horizon 6", "forza"));
        assert!(window_matches("gamescope[123]: Forza", "Forza"));
        assert!(!window_matches("Firefox", "Forza"));
    }

    #[test]
    fn empty_match_string_never_matches() {
        // An empty game_match would match everything; treat it as "no match" so
        // hotkeys aren't accidentally allowed for every window.
        assert!(!window_matches("Forza", ""));
    }
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test --lib focus 2>&1 | tail -20`
Expected: FAIL — `cannot find function window_matches`.

- [ ] **Step 5: Implement `window_matches` + query + detector**

Prepend to `src/focus.rs`:

```rust
//! Shared "is the game the focused window?" detector. One poll thread updates a
//! cached AtomicBool at the configured rate; the hotkey gate and the synthetic-
//! input gate both read it. Fail-open: if a query errors, we report focused=true
//! and surface a red status, so the feature never silently blocks. See spec.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::FocusMethod;

/// Status shown by the settings light.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusStatus { Ok = 0, ToolMissing = 1, QueryFailed = 2, Idle = 3 }

/// Case-insensitive substring match; an empty needle never matches.
pub fn window_matches(active: &str, needle: &str) -> bool {
    !needle.is_empty() && active.to_lowercase().contains(&needle.to_lowercase())
}

/// Settings the poll thread reads each tick (cheap clone via Arc<Mutex>).
#[derive(Clone)]
pub struct FocusParams {
    pub method: FocusMethod,
    pub custom_cmd: String,
    pub game_match: String,
    pub poll_hz: f32,
    pub enabled: bool, // false → thread idles and reports focused=true
}

/// Cached detector state shared with consumers.
pub struct FocusDetector {
    focused: Arc<AtomicBool>,
    status: Arc<AtomicU8>,
    params: Arc<Mutex<FocusParams>>,
}

impl FocusDetector {
    pub fn new(params: FocusParams) -> Self {
        let focused = Arc::new(AtomicBool::new(true)); // fail-open default
        let status = Arc::new(AtomicU8::new(FocusStatus::Idle as u8));
        let params = Arc::new(Mutex::new(params));
        let d = FocusDetector { focused: focused.clone(), status: status.clone(), params: params.clone() };
        thread::spawn(move || poll_loop(focused, status, params));
        d
    }

    /// Latest cached "game focused?" (fail-open true when disabled/erroring).
    pub fn focused(&self) -> bool { self.focused.load(Ordering::Relaxed) }
    pub fn status(&self) -> FocusStatus {
        match self.status.load(Ordering::Relaxed) {
            0 => FocusStatus::Ok, 1 => FocusStatus::ToolMissing,
            2 => FocusStatus::QueryFailed, _ => FocusStatus::Idle,
        }
    }
    /// Push updated params (called when settings change).
    pub fn set_params(&self, p: FocusParams) { *self.params.lock().unwrap() = p; }

    /// One-shot active-window query for the Detect button / Custom preview.
    /// Returns the active window name or an error string.
    pub fn query_now(&self) -> Result<String, String> {
        let p = self.params.lock().unwrap().clone();
        query_active_window(p.method, &p.custom_cmd)
    }
}

fn poll_loop(focused: Arc<AtomicBool>, status: Arc<AtomicU8>, params: Arc<Mutex<FocusParams>>) {
    loop {
        let p = params.lock().unwrap().clone();
        if !p.enabled {
            focused.store(true, Ordering::Relaxed);
            status.store(FocusStatus::Idle as u8, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        match query_active_window(p.method, &p.custom_cmd) {
            Ok(name) => {
                focused.store(window_matches(&name, &p.game_match), Ordering::Relaxed);
                status.store(FocusStatus::Ok as u8, Ordering::Relaxed);
            }
            Err(_) => {
                // Fail-open: allow input/hotkeys, but flag the failure.
                focused.store(true, Ordering::Relaxed);
                status.store(FocusStatus::QueryFailed as u8, Ordering::Relaxed);
            }
        }
        let hz = p.poll_hz.clamp(1.0, 20.0);
        thread::sleep(Duration::from_secs_f32(1.0 / hz));
    }
}

/// Run the platform/method query, returning the active window's name/class.
pub fn query_active_window(method: FocusMethod, custom_cmd: &str) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    { linux_query(method, custom_cmd) }
    #[cfg(target_os = "windows")]
    { let _ = (method, custom_cmd); windows_query() }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    { let _ = (method, custom_cmd); Err("unsupported platform".into()) }
}

#[cfg(target_os = "linux")]
fn linux_query(method: FocusMethod, custom_cmd: &str) -> Result<String, String> {
    use std::process::Command;
    let run = |cmd: &str, args: &[&str]| -> Result<String, String> {
        Command::new(cmd).args(args).output()
            .map_err(|e| format!("{cmd}: {e}"))
            .and_then(|o| if o.status.success() {
                Ok(String::from_utf8_lossy(&o.stdout).into_owned())
            } else {
                Err(format!("{cmd} exited {}", o.status))
            })
    };
    match method {
        FocusMethod::Hyprland => {
            let out = run("hyprctl", &["activewindow", "-j"])?;
            // Pull "class" and "title" values without a JSON dep.
            let field = |k: &str| out.split(&format!("\"{k}\""))
                .nth(1).and_then(|s| s.split('"').nth(1)).unwrap_or("").to_string();
            Ok(format!("{} {}", field("class"), field("title")))
        }
        FocusMethod::X11 => {
            let root = run("xdotool", &["getactivewindow", "getwindowname"]);
            // Prefer xdotool if present; fall back to xprop.
            match root {
                Ok(s) => Ok(s),
                Err(_) => {
                    let id_line = run("xprop", &["-root", "_NET_ACTIVE_WINDOW"])?;
                    let id = id_line.rsplit(' ').next().unwrap_or("").trim().to_string();
                    let props = run("xprop", &["-id", &id, "WM_CLASS", "_NET_WM_NAME"])?;
                    Ok(props)
                }
            }
        }
        FocusMethod::Custom => {
            if custom_cmd.trim().is_empty() { return Err("empty custom command".into()); }
            let out = Command::new("sh").arg("-c").arg(custom_cmd).output()
                .map_err(|e| format!("custom: {e}"))?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err(format!("custom exited {}", out.status))
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_query() -> Result<String, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() { return Err("no foreground window".into()); }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len <= 0 { return Err("empty window title".into()); }
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }
}
```

- [ ] **Step 6: Run tests + build**

Run: `cargo test --lib focus 2>&1 | tail -20`
Expected: 2 tests PASS.
Run: `cargo build 2>&1 | tail -3`
Expected: Finished, no errors.

- [ ] **Step 7: Commit**

```bash
git add src/focus.rs src/main.rs Cargo.toml
git commit -m "feat(hotkeys): add FocusDetector (Hyprland/X11/Custom/Windows active-window query + poll thread)"
```

---

## Task 4: Capture backend — HotkeyListener

**Files:**
- Create: `src/hotkeys.rs`
- Modify: `src/main.rs` (add `mod hotkeys;`)
- Test: inline `#[cfg(test)]` in `src/hotkeys.rs`

- [ ] **Step 1: Add the module declaration**

In `src/main.rs`, add `mod hotkeys;`.

- [ ] **Step 2: Write the failing test**

Create `src/hotkeys.rs` with the test module first:

```rust
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
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --lib hotkeys 2>&1 | tail -20`
Expected: FAIL — `cannot find function match_combo`.

- [ ] **Step 4: Implement the shared match + listener skeleton + backends**

Prepend to `src/hotkeys.rs`:

```rust
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
    use std::thread;
    use evdev::{Device, EventType, Key};
    use super::{Bindings, HotkeyStatus, match_combo};
    use crate::config::HotkeyAction;
    use crate::keymap::{HotKey, Mods};

    pub fn spawn(binds: Bindings, status: std::sync::Arc<std::sync::Mutex<HotkeyStatus>>, tx: Sender<HotkeyAction>) {
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

    pub fn spawn(binds: Bindings, status: std::sync::Arc<std::sync::Mutex<HotkeyStatus>>, tx: Sender<HotkeyAction>) {
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
    use super::{Bindings, HotkeyStatus};
    use crate::config::HotkeyAction;
    pub fn spawn(_b: Bindings, status: std::sync::Arc<std::sync::Mutex<HotkeyStatus>>, _tx: Sender<HotkeyAction>) {
        *status.lock().unwrap() = HotkeyStatus::Unsupported;
    }
}
```

- [ ] **Step 5: Run tests + build**

Run: `cargo test --lib hotkeys 2>&1 | tail -20`
Expected: `matches_key_and_exact_modifiers` PASS.
Run: `cargo build 2>&1 | tail -3`
Expected: Finished, no errors.

- [ ] **Step 6: Commit**

```bash
git add src/hotkeys.rs src/main.rs
git commit -m "feat(hotkeys): add HotkeyListener capture backend (evdev read / GetAsyncKeyState poll)"
```

---

## Task 5: Input focus gate on InputSender

**Files:**
- Modify: `src/input.rs` (linux mod ~line 52, windows mod ~line 212, stub ~line 337)
- Test: inline `#[cfg(test)]` in `src/input.rs` (extend existing tests)

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `src/input.rs`:

```rust
#[test]
fn focus_gate_blocks_when_not_allowed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let gate = Arc::new(AtomicBool::new(true));
    let sender = InputSender::new();
    sender.set_focus_gate(gate.clone());
    // allowed → gate lets input through (we can't observe uinput here, but the
    // gate predicate is the unit under test):
    assert!(sender.input_allowed());
    gate.store(false, Ordering::Relaxed);
    assert!(!sender.input_allowed());
    // No gate set → always allowed.
    let plain = InputSender::new();
    assert!(plain.input_allowed());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib input::tests::focus_gate 2>&1 | tail -20`
Expected: FAIL — `no method named set_focus_gate`.

- [ ] **Step 3: Add the gate field + methods (Linux mod)**

In `src/input.rs`, in the `#[cfg(target_os = "linux")] mod linux` block, add to `struct InputSender`:

```rust
        gate: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
```

In `InputSender::new`, change the returned struct to `Self { tx, echo, gate: None }`. Then add these methods to `impl InputSender` and guard emission:

```rust
        pub fn set_focus_gate(&mut self, gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {
            self.gate = Some(gate);
        }
        pub fn input_allowed(&self) -> bool {
            self.gate.as_ref().map_or(true, |g| g.load(std::sync::atomic::Ordering::Relaxed))
        }
```

At the **top** of each of `press`, `press_tracked`, and `hold_tracked`, add:

```rust
            if !self.input_allowed() { return; }
```

(Leave `release` ungated — a held key must always be releasable.)

- [ ] **Step 4: Mirror the change in the Windows mod and stub**

In `#[cfg(target_os = "windows")] mod windows`: add the same `gate` field, `gate: None` in `new`, the same two methods, and the same guard at the top of `press`, `press_tracked`, `hold_tracked`.

In `#[cfg(not(any(...)))] mod stub`: add to `InputSender`:

```rust
    pub fn set_focus_gate(&mut self, _gate: std::sync::Arc<std::sync::atomic::AtomicBool>) {}
    pub fn input_allowed(&self) -> bool { true }
```

- [ ] **Step 5: Run tests + build**

Run: `cargo test --lib input 2>&1 | tail -20`
Expected: `focus_gate_blocks_when_not_allowed` and existing `echo_window_opens_and_expires` PASS.
Run: `cargo build 2>&1 | tail -3`
Expected: Finished.

- [ ] **Step 6: Commit**

```bash
git add src/input.rs
git commit -m "feat(hotkeys): add optional focus gate to InputSender (suppress emission when not focused)"
```

---

## Task 6: Wire into the app + replace hardcoded hotkeys

**Files:**
- Modify: `src/app.rs` — imports (~line 11), `ForzaApp` fields (~line 514/546), `new` (~line 709), `update` hotkey block (lines 1320–1347), a small pure gate helper.
- Test: inline `#[cfg(test)]` in `src/app.rs` for the gate helper.

- [ ] **Step 1: Write the failing test for the gate helper**

Add a `#[cfg(test)] mod hotkey_tests` at the end of `src/app.rs`:

```rust
#[cfg(test)]
mod hotkey_tests {
    use super::global_hotkey_allowed;

    #[test]
    fn fires_when_app_focused_and_not_typing() {
        assert!(global_hotkey_allowed(true, false, false));
    }
    #[test]
    fn blocked_when_app_focused_but_typing() {
        assert!(!global_hotkey_allowed(true, true, false));
    }
    #[test]
    fn fires_when_not_ours_but_game_focused() {
        assert!(global_hotkey_allowed(false, false, true));
    }
    #[test]
    fn blocked_when_third_app_focused() {
        assert!(!global_hotkey_allowed(false, false, false));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib hotkey_tests 2>&1 | tail -20`
Expected: FAIL — `cannot find function global_hotkey_allowed`.

- [ ] **Step 3: Add the pure gate helper**

Add near the top of `src/app.rs` (module level, after the `use` block):

```rust
/// Whether a global hotkey should fire. Fires when our app is focused (unless a
/// text field is capturing keys), or when our app is not focused but the game is.
/// A third app focused → ignore. See the global-hotkeys spec §7.
pub(crate) fn global_hotkey_allowed(our_focused: bool, wants_text: bool, game_focused: bool) -> bool {
    if our_focused { !wants_text } else { game_focused }
}
```

- [ ] **Step 4: Add fields + imports**

In `src/app.rs` imports (~line 11), add:

```rust
use crate::hotkeys::HotkeyListener;
use crate::focus::{FocusDetector, FocusParams};
```

Add to `struct ForzaApp` (near `input: InputSender,` ~line 514):

```rust
    hotkeys: HotkeyListener,
    focus: FocusDetector,
    input_allowed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Settings-tab rebind state: the action currently capturing a new key.
    pub rebinding: Option<crate::config::HotkeyAction>,
    /// Detect-button countdown deadline (active-window auto-fill).
    pub detect_until: Option<std::time::Instant>,
    /// Last Custom-preview result for the settings page.
    pub focus_preview: String,
```

- [ ] **Step 5: Construct them in `new`**

Helper to build the global-scope bind list, add at module level in `src/app.rs`:

```rust
pub(crate) fn global_bindings(cfg: &crate::config::AppConfig) -> Vec<(crate::keymap::HotkeyBinding, crate::config::HotkeyAction)> {
    use crate::config::HotkeyScope;
    cfg.hotkeys.bindings.iter()
        .filter(|(a, _)| a.scope() == HotkeyScope::Global)
        .map(|(a, b)| (*b, *a))
        .collect()
}
```

In `ForzaApp::new` (near `input: InputSender::new(),` ~line 709), replace that line and add the new fields. First build the shared allowed-flag and gate `input`:

```rust
            input: {
                let mut s = InputSender::new();
                s.set_focus_gate(input_allowed.clone());
                s
            },
```

But `input_allowed` must exist first. So at the **start** of the struct literal's surrounding function (just before the `Self { ... }` returned in `new`), add:

```rust
        let input_allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let focus = FocusDetector::new(FocusParams {
            method: config.hotkeys.focus_method,
            custom_cmd: config.hotkeys.custom_cmd.clone(),
            game_match: config.hotkeys.game_match.clone(),
            poll_hz: config.hotkeys.focus_poll_hz,
            enabled: config.hotkeys.input_focus_gate, // gate poll runs when a consumer needs it
        });
        let hotkeys = HotkeyListener::new(global_bindings(&config));
```

(`config` is already the loaded config in `new`; confirm the local variable name with `grep -n "let config" src/app.rs` and match it.)

Then add to the `Self { ... }` literal:

```rust
            hotkeys,
            focus,
            input_allowed,
            rebinding: None,
            detect_until: None,
            focus_preview: String::new(),
```

- [ ] **Step 6: Replace the hardcoded hotkey block in `update`**

In `src/app.rs`, **keep** the F11 Windows block (lines 1320–1325). Delete the F10 handler (lines 1327–1331), the Ctrl+S block (1333–1340), and the Ctrl+E block (1342–1347), and replace them with:

```rust
        // ── Hotkeys ────────────────────────────────────────────────
        // App-focused actions: matched from config against egui input (only
        // delivered while our window is focused → inherently UI-only).
        {
            use crate::config::{HotkeyAction, HotkeyScope};
            let m = ctx.input(|i| i.modifiers);
            for action in HotkeyAction::ALL.iter().copied() {
                if action.scope() != HotkeyScope::AppFocused { continue; }
                let Some(b) = self.config.hotkeys.bindings.get(&action).copied() else { continue; };
                let pressed = ctx.input(|i| i.key_pressed(b.key.to_egui()))
                    && m.ctrl == b.mods.ctrl && m.alt == b.mods.alt
                    && m.shift == b.mods.shift;
                if pressed { self.run_app_hotkey(action); }
            }
        }
        // Global actions: from the capture backend, gated on focus.
        let our_focused = ctx.input(|i| i.focused);
        let wants_text = ctx.wants_keyboard_input();
        while let Some(action) = self.hotkeys.try_recv() {
            let game_focused = match self.config.hotkeys.gate_mode {
                crate::config::GateMode::TelemetryLive => self.telemetry.is_connected,
                crate::config::GateMode::WindowFocus => self.focus.focused(),
            };
            if crate::app::global_hotkey_allowed(our_focused, wants_text, game_focused) {
                self.run_global_hotkey(action);
            }
        }
```

- [ ] **Step 7: Add the action dispatch methods**

Add to `impl ForzaApp` (near `drain_packets`):

```rust
    fn run_app_hotkey(&mut self, action: crate::config::HotkeyAction) {
        use crate::config::HotkeyAction::*;
        match action {
            MiniSettings => {
                self.page_settings_open = !self.page_settings_open;
                self.page_settings_tab = PageSettingsTab::Tab(self.current_tab);
                if !self.page_settings_open { self.config.save(); }
            }
            DashboardEdit => {
                if self.current_tab == Tab::Dashboard {
                    self.config.dashboard_edit_mode = !self.config.dashboard_edit_mode;
                }
            }
            _ => {}
        }
    }

    fn run_global_hotkey(&mut self, action: crate::config::HotkeyAction) {
        use crate::config::HotkeyAction::*;
        match action {
            ToggleGearbox => { self.config.dsg_enabled = !self.config.dsg_enabled; }
            ToggleBackfire => { self.config.backfire_enabled = !self.config.backfire_enabled; }
            _ => {}
        }
    }
```

- [ ] **Step 8: Run tests + build**

Run: `cargo test --lib hotkey_tests 2>&1 | tail -20`
Expected: 4 gate tests PASS.
Run: `cargo build 2>&1 | tail -3`
Expected: Finished. (Fix any `config` local-name mismatch flagged by the compiler.)

- [ ] **Step 9: Commit**

```bash
git add src/app.rs
git commit -m "feat(hotkeys): wire listener + focus detector into the app; config-driven hotkeys, delete F10"
```

---

## Task 7: Settings UI — HOTKEYS card

**Files:**
- Modify: `src/ui/settings.rs` (add a card in the right column, after Repository ~line 138), `src/app.rs` (a helper to push updated bindings/params after edits)
- No unit test (egui UI); manual verification.

- [ ] **Step 1: Add a re-sync helper on ForzaApp**

In `src/app.rs`, add to `impl ForzaApp`:

```rust
    /// Push current hotkey config to the live backend + focus detector. Call
    /// after any hotkey/detection setting changes.
    pub fn sync_hotkeys(&mut self) {
        self.hotkeys.set_bindings(crate::app::global_bindings(&self.config));
        self.focus.set_params(crate::focus::FocusParams {
            method: self.config.hotkeys.focus_method,
            custom_cmd: self.config.hotkeys.custom_cmd.clone(),
            game_match: self.config.hotkeys.game_match.clone(),
            poll_hz: self.config.hotkeys.focus_poll_hz,
            enabled: self.config.hotkeys.input_focus_gate,
        });
        self.input_allowed.store(true, std::sync::atomic::Ordering::Relaxed);
    }
```

Also drive the input gate each frame: in `update`, right after the global-hotkey `while` loop, add:

```rust
        // Feed the synthetic-input gate from the detector when enabled.
        let allow = !self.config.hotkeys.input_focus_gate || self.focus.focused();
        self.input_allowed.store(allow, std::sync::atomic::Ordering::Relaxed);
```

- [ ] **Step 2: Add the HOTKEYS card**

In `src/ui/settings.rs`, after the Repository `right.group` (before the closing `});` of `ui.columns` at ~line 140), add:

```rust
            right.add_space(8.0);
            right.group(|ui| {
                ui.label(crate::theme::section_label(tr("Hotkeys")));
                ui.add_space(4.0);

                use crate::config::{HotkeyAction, HotkeyScope, GateMode, FocusMethod};
                let mut changed = false;

                // Rebind rows, grouped by scope.
                for (scope, heading) in [(HotkeyScope::Global, tr("Global (while in-game)")),
                                         (HotkeyScope::AppFocused, tr("In-app"))] {
                    ui.label(RichText::new(heading).size(11.0).color(Color32::GRAY));
                    for action in HotkeyAction::ALL.iter().copied().filter(|a| a.scope() == scope) {
                        ui.horizontal(|ui| {
                            ui.label(tr(action.label()));
                            let capturing = app.rebinding == Some(action);
                            let text = if capturing {
                                tr("Press a key…").to_string()
                            } else {
                                app.config.hotkeys.bindings.get(&action).map(|b| b.label()).unwrap_or_default()
                            };
                            if ui.button(text).clicked() {
                                app.rebinding = if capturing { None } else { Some(action) };
                            }
                        });
                    }
                }

                // Capture the next key while rebinding.
                if let Some(action) = app.rebinding {
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        app.rebinding = None;
                    } else if let Some(hk) = ui.input(|i| {
                        i.events.iter().find_map(|e| match e {
                            egui::Event::Key { key, pressed: true, .. } => crate::keymap::HotKey::from_egui(*key),
                            _ => None,
                        })
                    }) {
                        if hk != crate::keymap::HotKey::Escape {
                            let m = ui.input(|i| i.modifiers);
                            app.config.hotkeys.bindings.insert(action, crate::keymap::HotkeyBinding {
                                mods: crate::keymap::Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, sup: false },
                                key: hk,
                            });
                            app.rebinding = None;
                            changed = true;
                        }
                    }
                }

                ui.add_space(6.0);

                // Detection mode.
                ui.horizontal(|ui| {
                    ui.label(tr("Trigger when:"));
                    egui::ComboBox::from_id_salt("hk_gate_mode")
                        .selected_text(match app.config.hotkeys.gate_mode {
                            GateMode::TelemetryLive => tr("Telemetry live"),
                            GateMode::WindowFocus => tr("Game window focused"),
                        })
                        .show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::TelemetryLive, tr("Telemetry live")).changed();
                            changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::WindowFocus, tr("Game window focused")).changed();
                        });
                });

                if app.config.hotkeys.gate_mode == GateMode::WindowFocus {
                    #[cfg(target_os = "linux")]
                    ui.horizontal(|ui| {
                        ui.label(tr("Method:"));
                        egui::ComboBox::from_id_salt("hk_focus_method")
                            .selected_text(format!("{:?}", app.config.hotkeys.focus_method))
                            .show_ui(ui, |ui| {
                                changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Hyprland, "Hyprland").changed();
                                changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::X11, "X11").changed();
                                changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Custom, tr("Custom")).changed();
                            });
                    });
                    #[cfg(target_os = "linux")]
                    if app.config.hotkeys.focus_method == FocusMethod::Custom {
                        ui.horizontal(|ui| {
                            ui.label(tr("Command:"));
                            changed |= ui.text_edit_singleline(&mut app.config.hotkeys.custom_cmd).changed();
                            if ui.button(tr("Test")).clicked() {
                                app.focus_preview = app.focus.query_now().unwrap_or_else(|e| format!("error: {e}"));
                            }
                        });
                        if !app.focus_preview.is_empty() {
                            ui.label(RichText::new(format!("→ {}", app.focus_preview)).size(11.0).color(Color32::GRAY));
                        }
                    }

                    // Game match + Detect.
                    ui.horizontal(|ui| {
                        ui.label(tr("Game window match:"));
                        changed |= ui.text_edit_singleline(&mut app.config.hotkeys.game_match).changed();
                        let label = match app.detect_until {
                            Some(t) => {
                                let secs = t.saturating_duration_since(std::time::Instant::now()).as_secs() + 1;
                                format!("{} {}", tr("Detecting…"), secs)
                            }
                            None => tr("Detect").to_string(),
                        };
                        if ui.button(label).clicked() && app.detect_until.is_none() {
                            app.detect_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                        }
                    });
                }

                // Input focus gate + poll rate.
                ui.horizontal(|ui| {
                    // styled_checkbox may return () — compare before/after instead of .changed().
                    let before = app.config.hotkeys.input_focus_gate;
                    crate::theme::styled_checkbox(ui, &mut app.config.hotkeys.input_focus_gate, tr("Only send inputs when game focused"));
                    changed |= app.config.hotkeys.input_focus_gate != before;
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Focus check rate:"));
                    changed |= ui.add(egui::Slider::new(&mut app.config.hotkeys.focus_poll_hz, 1.0..=20.0).step_by(1.0).suffix(" Hz")).changed();
                });

                // Status light (Linux capture permission).
                #[cfg(target_os = "linux")]
                {
                    use crate::hotkeys::HotkeyStatus;
                    let (dot, msg) = match app.hotkeys.status() {
                        HotkeyStatus::Ok => ("🟢", tr("Keyboard capture active")),
                        _ => ("🔴", tr("No input access — add your user to the 'input' group: sudo usermod -aG input $USER, then re-login")),
                    };
                    ui.label(format!("{dot} {msg}"));
                }

                if changed { app.sync_hotkeys(); }
            });
```

- [ ] **Step 3: Handle the Detect countdown in `update`**

In `src/app.rs` `update`, after the input-gate line from Task 7 Step 1, add:

```rust
        // Detect button: when the 3s countdown elapses, capture the active window.
        if let Some(t) = self.detect_until {
            if std::time::Instant::now() >= t {
                self.detect_until = None;
                if let Ok(name) = self.focus.query_now() {
                    // Use the first whitespace-separated token as a reasonable default match.
                    if let Some(first) = name.split_whitespace().next() {
                        self.config.hotkeys.game_match = first.to_string();
                        self.sync_hotkeys();
                    }
                }
            } else {
                ctx.request_repaint(); // keep the countdown ticking
            }
        }
```

- [ ] **Step 4: Build**

Run: `cargo build 2>&1 | tail -6`
Expected: Finished. If `styled_checkbox` doesn't return a `Response`, check its signature (`grep -n "pub fn styled_checkbox" src/theme.rs`) and adapt: wrap in `ui.horizontal` and set `changed = true` unconditionally on interaction, or compare the bool before/after.

- [ ] **Step 5: Commit**

```bash
git add src/ui/settings.rs src/app.rs
git commit -m "feat(hotkeys): Settings HOTKEYS card — rebind, detection mode, Custom preview, Detect, status light"
```

---

## Task 8: i18n strings

**Files:**
- Modify: `src/i18n.rs`

- [ ] **Step 1: Add the English→German entries**

Find the translation table (`grep -n "=>" src/i18n.rs | head`) and add entries in the same style for every new `tr("…")` key used in Tasks 6–7. Add:

```rust
        "Hotkeys" => "Tastenkürzel",
        "Global (while in-game)" => "Global (im Spiel)",
        "In-app" => "In der App",
        "Press a key…" => "Taste drücken…",
        "Toggle Automatic Gearbox" => "Automatikgetriebe umschalten",
        "Toggle Backfire" => "Fehlzündung umschalten",
        "Open mini-settings" => "Mini-Einstellungen öffnen",
        "Toggle dashboard edit" => "Dashboard-Bearbeitung umschalten",
        "Trigger when:" => "Auslösen wenn:",
        "Telemetry live" => "Telemetrie aktiv",
        "Game window focused" => "Spielfenster im Fokus",
        "Method:" => "Methode:",
        "Custom" => "Benutzerdefiniert",
        "Command:" => "Befehl:",
        "Test" => "Test",
        "Game window match:" => "Spielfenster-Abgleich:",
        "Detect" => "Erkennen",
        "Detecting…" => "Erkenne…",
        "Only send inputs when game focused" => "Eingaben nur bei fokussiertem Spiel senden",
        "Focus check rate:" => "Fokus-Prüfrate:",
        "Keyboard capture active" => "Tastaturerfassung aktiv",
        "No input access — add your user to the 'input' group: sudo usermod -aG input $USER, then re-login" => "Kein Eingabezugriff — Benutzer zur Gruppe 'input' hinzufügen: sudo usermod -aG input $USER, dann neu anmelden",
```

- [ ] **Step 2: Build (checks for duplicate-key compile warnings)**

Run: `cargo build 2>&1 | tail -6`
Expected: Finished; no "unreachable pattern" warnings for the new keys.

- [ ] **Step 3: Commit**

```bash
git add src/i18n.rs
git commit -m "i18n(hotkeys): add German translations for the hotkeys settings"
```

---

## Task 9: Docs + changelog

**Files:**
- Create: `docs/features/hotkeys.md`
- Modify: `docs/architecture/overview.md`, `docs/README.md`, `CHANGELOG.md`

- [ ] **Step 1: Write the feature doc**

Create `docs/features/hotkeys.md` summarising: the two scopes, capture backends (evdev read / GetAsyncKeyState — with the *why*: reads below the display server so Wayland works, and observe-only so the game still gets the key), the shared FocusDetector and its methods, the input gate, the `input`-group requirement, and the known limitations from spec §11. Link back to the spec.

- [ ] **Step 2: Update the architecture map**

In `docs/architecture/overview.md`:
- Add `hotkeys.rs`, `focus.rs`, `keymap.rs` rows to the `src/` module map table.
- Update the "Global hotkeys" step 5 in the Frame loop section to describe the new config-driven + capture-backend behaviour and the removal of F10.
- Add a "Global hotkeys → `hotkeys.rs` + `focus.rs`" entry to "Where to look for X".

- [ ] **Step 3: Update docs index + changelog**

In `docs/README.md`, add a link to `features/hotkeys.md`.

In `CHANGELOG.md`, under the top `## [0.1.0]` → `### Added`:

```markdown
- **Global hotkeys**: rebindable keys that work while the game is focused — default `G` toggles Automatic Gearbox, `B` toggles Backfire — plus rebindable `Ctrl+S` / `Ctrl+E`. Configure them in Settings → Hotkeys, with Telemetry-live or window-focus triggering (Hyprland / X11 / custom command on Linux, and a Detect button to capture the game's window name). Optionally suppress backfire/gearbox key injection unless the game is focused.
```

And under `### Removed`:

```markdown
- **F10 map-orientation hotkey**: removed; use the "Lock map north-up" checkbox in the minimap mini-settings.
```

- [ ] **Step 4: Final full build + test**

Run: `cargo build 2>&1 | tail -3 && cargo test 2>&1 | tail -6`
Expected: build Finished; all tests pass.

- [ ] **Step 5: Commit**

```bash
git add docs/ CHANGELOG.md
git commit -m "docs(hotkeys): feature doc, architecture map, changelog"
```

---

## Manual verification checklist (user, on request — no unprompted app launch)

- Rebind each action (incl. `Ctrl+S`/`Ctrl+E`); labels update; Esc cancels.
- With Forza focused: `G` toggles gearbox, `B` toggles backfire.
- While a Settings text field is focused, typing `G` types normally (no toggle).
- With a third app (browser) focused in Window-focus mode: hotkeys ignored.
- Detect button: 3-s countdown then `game_match` fills with the focused window's name.
- Enable "only send inputs when game focused": alt-tab out → backfire stops injecting.
- Status light: red when not in `input` group, green after adding + re-login (Linux).
