# Global Hotkeys — Design Spec

**Date:** 2026-07-16
**Status:** Approved design, pre-implementation
**Feature:** Rebindable hotkeys that work while the *game* is focused (not just the
telemetry app), on Windows, Linux/X11, and Linux/Wayland — plus a shared window-focus
detector reused to gate synthetic input.

---

## 1. Goal & motivation

Today the app's "hotkeys" (`F10`, `F11`, `Ctrl+S`, `Ctrl+E`) go through `ctx.input(...)`,
so they only fire while the **telemetry window** is focused. In Forza the *game* holds
focus, so none of them work while driving.

We want:

- **Global hotkeys** that fire while Forza is focused — default `G` = toggle Automatic
  Gearbox, `B` = toggle Backfire.
- **App-focused hotkeys** that stay UI-only — `Ctrl+S` (mini-settings), `Ctrl+E`
  (dashboard edit) — but are now **rebindable** instead of hardcoded.
- A **rebind UI** in the big Settings tab: click a button, press a key, done. Modifier
  combos supported.
- **Cross-platform**, including **Wayland**, without per-compositor global-shortcut APIs.

Bonus, requested during design: reuse the same window-focus detection to **gate synthetic
input** (backfire/DSG keypresses) so alt-tabbing out of the game doesn't spray keys into
other apps.

## 2. Non-goals

- **Consuming** the key (blocking it from the game). We *observe* only — the game still
  receives the key. Users pick keys the game doesn't use for driving (G, B are safe).
- Perfect focus detection on every Wayland compositor. We ship Hyprland + X11 + a Custom
  command escape hatch; others fall back to the Custom option or the telemetry-live gate.
- Hot-plugged keyboards mid-session on Linux (enumerated at startup only).
- `F10` map-orientation hotkey — **removed** (the mini-settings checkbox remains).
- `F11` fullscreen — stays hardcoded (Windows-only, not requested for rebinding).

## 3. Key insight: why Wayland isn't the wall it looks like

Wayland deliberately blocks apps from grabbing global keys *and* from querying the focused
window — the same security model breaks the two obvious approaches (global-shortcut grab,
foreground-window query).

We sidestep both:

- **Capture:** we already depend on `evdev` and already require `input`-group /
  `/dev/uinput` access for synthetic keypresses (`input.rs`). The same access lets us
  **read** `/dev/input/event*`, which sits *below* the display server — so it works
  identically on X11, Wayland, and console. This reads, it doesn't grab, so Wayland's
  restriction doesn't apply. No new dependency, no new permission, no D-Bus/portal.
- **Where they fire:** global hotkeys fire when the game *or* our own window is focused, and
  are ignored for any third app. "Our window focused" is known instantly from egui; "game
  focused" comes from either telemetry-is-live or a user-chosen window-focus method. No
  Wayland-blocked OS foreground query is required in the common path.

## 4. Architecture overview

```
                       ┌────────────────────────┐
  keyboard ──────────► │  Capture backend       │  (src/hotkeys.rs)
  (all keys)           │  Linux: evdev read     │
                       │  Windows: GetAsyncKey  │
                       └───────────┬────────────┘
                                   │ matched HotkeyAction only
                                   ▼  mpsc channel
  ┌─────────────────────────────────────────────────────────────┐
  │  app.rs update():                                             │
  │   • app-focused actions  ← ctx.input (egui, focused only)     │
  │   • global actions       ← hotkeys.try_recv() + focus gate    │
  └─────────────────────────────────────────────────────────────┘
                                   ▲
                                   │ cached "game focused?" AtomicBool
                       ┌───────────┴────────────┐
                       │  FocusDetector poll     │  (src/focus.rs)
                       │  Hyprland / X11 / Custom │  polled at N Hz
                       │  Windows: GetForeground │
                       └───────────┬────────────┘
                                   │ same cached bool
                                   ▼
                       InputSender focus gate (input.rs)
                       suppresses backfire/DSG keys when not focused
```

Mirrors the existing thread+channel pattern (`network.rs`, `input.rs`): background
producer, main thread consumes via `try_recv`, no shared-state locking on app state.

## 5. Config model (`src/config.rs`)

```rust
/// A bindable action. `scope` is fixed per action, not user-editable.
enum HotkeyAction {
    ToggleGearbox,   // Global   — flips config.dsg_enabled
    ToggleBackfire,  // Global   — flips config.backfire_enabled
    MiniSettings,    // AppFocused — flips page_settings_open
    DashboardEdit,   // AppFocused — flips config.dashboard_edit_mode (Dashboard tab only)
}

enum HotkeyScope { Global, AppFocused }

/// Canonical, serde-stable key identity. Own enum (do NOT depend on egui's serde
/// feature). Covers the bindable subset: letters, digits, F1–F12, and a few named
/// keys. Maps to egui::Key / evdev::Key / Windows VK via tables in hotkeys.rs.
enum HotKey { A, B, /* … */ Key0, /* … */ F1, /* … */ Space, Esc, /* … */ }

struct HotkeyBinding { ctrl: bool, alt: bool, shift: bool, sup: bool, key: HotKey }

enum GateMode { TelemetryLive, WindowFocus }        // global-hotkey gating
enum FocusMethod { Hyprland, X11, Custom }           // how WindowFocus reads active window
                                                     // (Windows ignores: uses GetForegroundWindow)

struct HotkeyConfig {
    #[serde(default)] bindings: HashMap<HotkeyAction, HotkeyBinding>, // defaults G, B, Ctrl+S, Ctrl+E
    #[serde(default)] gate_mode: GateMode,           // default TelemetryLive
    #[serde(default)] focus_method: FocusMethod,     // default Hyprland
    #[serde(default)] custom_cmd: String,            // for FocusMethod::Custom
    #[serde(default)] game_match: String,            // substring, default "Forza"; Detect fills it
    #[serde(default)] input_focus_gate: bool,        // gate synthetic input on focus; default OFF
    #[serde(default)] focus_poll_hz: f32,            // shared poll rate, default 4.0, range 1–20
}
```

Nested in `AppConfig` under `#[serde(default)]`. `AppConfig` does **not** use
`deny_unknown_fields`, and nothing is removed from existing structs, so old `config.json`
files load unchanged (defaults fill the new block). No migration needed.

## 6. Capture backend (`src/hotkeys.rs`)

`HotkeyListener` spawns the platform backend, shares the current bindings via
`Arc<Mutex<Vec<(Combo, HotkeyAction)>>>` (updated on every rebind), and exposes:

- `try_recv() -> Option<HotkeyAction>` — drained each frame like packets.
- `status() -> HotkeyStatus` (`Ok` / `NoPermission` / `NoDevice` / `Unsupported`) — drives
  the settings status light.
- `set_bindings(&[…])` — push updated global bindings after a rebind.

**Only global-scope actions are registered with the backend.** App-focused actions never
touch it.

### Linux — evdev read
- Enumerate `/dev/input/event*`, keep devices whose supported keys look like a keyboard.
- One blocking reader thread per keyboard device (typically 1–3). Track modifier state from
  *that device's own* events; on a non-modifier key-down, build the combo and match against
  the shared list. **Match only** — non-matching keys are dropped in-thread immediately,
  never stored, never sent, never logged.
- `status = Ok` if ≥1 keyboard opened; `NoPermission` if opens fail (user not in `input`
  group); `NoDevice` if none found.
- Enumerated at startup; hot-plugged keyboards need an app restart (documented limitation).

### Windows — GetAsyncKeyState poll
- One thread: `loop { for each bound combo, read its VKs, detect the rising edge (track
  previous state per combo), send on match; sleep ~8 ms }`.
- Reads global physical key state regardless of focus; non-consuming by nature (game still
  gets the key). More robust over fullscreen than a hook, and simpler (no `SetWindowsHookEx`,
  no message pump).
- **Better privacy story than a hook:** only ever queries the specific VKs we've bound —
  never observes any other key.
- `windows-sys` with `Win32_UI_Input_KeyboardAndMouse` (+ `…WindowsAndMessaging` for the
  focus detector's `GetForegroundWindow`).

Tradeoff: polling can miss a key tapped *and* released between two ~8 ms polls — faster than
a human hotkey tap, so a non-issue.

## 7. Focus detection (`src/focus.rs`)

One `FocusDetector` + **one poll thread** at `focus_poll_hz`, caching "is the game focused?"
in an `Arc<AtomicBool>` (plus an `AtomicU8` status for the light). Two consumers read the
same cached bool. The thread only runs when a consumer needs it (input gate on, or
`gate_mode == WindowFocus`).

**Methods** — each returns the active window's identifier string; `game_match` (substring,
case-insensitive) decides the match:
- **Hyprland:** `hyprctl activewindow -j`, read `class`/`title`.
- **X11:** `xprop -root _NET_ACTIVE_WINDOW` → window id → `xprop -id … WM_CLASS`/`_NET_WM_NAME`.
- **Custom:** run `custom_cmd`; its stdout is the active-window identifier. Escape hatch for
  GameScope, unusual compositors, etc.
- **Windows:** `GetForegroundWindow` → `GetWindowTextW` (no method dropdown; used always).

**Fail-open:** if the tool is missing or the command errors (status ≠ `Ok`), the detector
reports "focused/allowed" so input and hotkeys keep working, and the settings light goes
**red** so the failure is visible rather than silently blocking the feature.

### Consumer 1 — global hotkey gate (main thread)
Global hotkeys fire when **either** the game **or** our own telemetry window is focused, and
are ignored when a third app is focused. On each `hotkeys.try_recv()` global action:

```rust
let allow = if our_window_focused {
    !ctx.wants_keyboard_input()   // our app focused: fire unless a text field has keyboard focus
} else {
    game_focus_signal             // not our app: fire only if the game is focused
};
```

`game_focus_signal` is mode-dependent:
- `TelemetryLive`: `telemetry.is_connected` — no window query (can't exclude a third app while
  data still flows with auto-pause off; the accepted weakness of this mode).
- `WindowFocus`: `cached_focus` — the detector's active-window match; excludes third apps.

Our-window-focused is now a *sufficient* condition, so binds work while you're in the app;
the `wants_keyboard_input()` guard keeps `G` typing normally into a text field instead of
toggling gearbox.

### Consumer 2 — synthetic input gate (`input.rs`)
`InputSender` gains an optional focus gate (`Arc<AtomicBool>` clone + `input_focus_gate`
flag). When the flag is on and the cached focus is false, `press`/`hold` become no-ops, so
backfire/DSG don't inject keys into whatever else is focused. **Default off** (preserves
today's behaviour; opt-in).

## 8. Rebind & focus UI (Settings tab → new "HOTKEYS" category)

- **Two labelled subsections** so global binds are visually distinct:
  - **Global (while in-game):** Toggle Gearbox, Toggle Backfire.
  - **In-app:** Mini-settings, Dashboard edit.
- Each row: `action name | current combo ("Ctrl + E") | [Rebind]`. Click Rebind → row shows
  "Press a key…", captures the next non-modifier key + held modifiers from **egui input**
  (the app is focused during rebind, so no backend involved), Esc cancels. Simple conflict
  warning if two bindings collide.
- **Detection settings:**
  - `Gate mode` dropdown: Telemetry-live / Window-focus.
  - When Window-focus (Linux): `Method` dropdown (Hyprland / X11 / Custom); for Custom, a
    command text field with a **live preview** — "Active window: X — matches ✓/✗" — refreshed
    on a "Test" click and while the page is open (at the poll rate).
  - `Game match` text field (default "Forza") + **Detect** button: 3-second countdown, then
    one active-window query auto-fills the field (handles GameScope/opaque titles).
  - `Input focus gate` toggle + `Poll rate` slider (1–20 Hz, default 4).
- **Status light** (🟢/🔴) from `HotkeyListener::status()` / detector status, with help text:
  *"Reading hotkeys needs your user in the `input` group: `sudo usermod -aG input $USER`,
  then re-login."*

## 9. Main-thread wiring (`src/app.rs`)

Replace the hardcoded `F10`/`Ctrl+S`/`Ctrl+E` block (and delete the `F10` handler):
- App-focused actions: match their configured `HotkeyBinding` against `ctx.input(...)`
  (egui only delivers keys while focused → inherently UI-only).
- Global actions: drain `hotkeys.try_recv()`, run the gate per action, then flip
  `config.dsg_enabled` / `config.backfire_enabled`.
- `F11` fullscreen: unchanged.

## 10. Privacy & security

- `input`-group access = the ability to read all input. We only ever **match configured
  combos** and never persist, transmit, or log any other keystroke. Stated in the module
  docs. Linux drops non-matches in the reader thread; Windows only queries bound VKs.
- The green/red status light makes the `input`-group requirement honest and visible.

## 11. Known limitations (documented, not engineered around)

1. **Observe-only:** a bound key still reaches the game. Fine for G/B; a footgun only if you
   bind a real driving key. Noted in UI help, not policed.
2. **Two physical keyboards:** modifiers tracked per-device, so Ctrl on keyboard A + E on
   keyboard B won't combine. Negligible.
3. **Focus poll staleness:** hotkey/input gating reads a bool up to `1/Hz` old (~250 ms at
   4 Hz). Acceptable.
4. **Fail-open on detection failure:** input/hotkeys keep working when detection is
   unavailable; the red light is the signal.
5. **Hot-plug:** new keyboards need an app restart (Linux).

## 12. Future (easy additions, out of scope now)

- Sway (`swaymsg -t get_tree`) and other compositors as extra `FocusMethod` branches.
- udev monitor for keyboard hot-plug.
- Optional key consumption where the platform allows it.

## 13. Testing

- Unit: `HotKey` ↔ egui/evdev/VK mapping round-trips; combo match logic; config
  serde-default round-trip (old config without the hotkey block loads).
- Manual (user, on request — no unprompted app launch): rebind each action; G/B toggle while
  Forza focused; nothing toggles while typing in our app; Detect fills `game_match`; input
  gate suppresses backfire when alt-tabbed; status light reflects `input`-group membership.

## 14. Touched files

- New: `src/hotkeys.rs`, `src/focus.rs`, `docs/features/hotkeys.md`.
- Changed: `config.rs` (HotkeyConfig + enums), `app.rs` (wiring, delete F10 handler),
  `input.rs` (focus gate), `ui/settings.rs` (HOTKEYS category), `i18n.rs`
  (strings), `main.rs` (`mod hotkeys; mod focus;`), `Cargo.toml` (`windows-sys` on Windows),
  `CHANGELOG.md`, `docs/architecture/overview.md` (module map + hotkey step).
