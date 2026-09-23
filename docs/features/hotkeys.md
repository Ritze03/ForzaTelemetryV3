# Hotkeys

Rebindable keyboard shortcuts, configured in **Settings → Hotkeys**. Two scopes,
one rebind UI. Full design + rationale: [[hotkeys-design]] (`docs/features/hotkeys-design.md`).

## Two scopes

- **Global (while in-game)** — fire while the *game* holds focus (or our app does).
  Defaults: `G` = toggle Automatic Gearbox, `F` = reset RPM calibration, `B` =
  toggle Backfire. Routed through the capture backend + focus gate.
- **In-app** — fire only while our telemetry window is focused. Defaults: `Ctrl+S` =
  mini-settings, `Ctrl+E` = dashboard edit. Handled via egui input (`ctx.input`), so they
  are inherently UI-only. Rebindable because the combo is read from config.

A binding's *scope* is fixed per action (`HotkeyAction::scope`), not user-chosen.

## Capture backend (`src/hotkeys.rs`)

`HotkeyListener` runs a background backend that matches configured **global** combos and
pushes the matched `HotkeyAction` down an mpsc channel. `HotkeyListener::new` hands that
channel's `Receiver` to the **listener thread** (`src/listeners/worker.rs`), which drains it
once per loop and applies the action there; only the in-app actions are handled in the frame
loop. *Why:* a hidden window gets no frames on GNOME/Wayland, so a frame-loop drain would
leave you with an auto-shifter you can't switch off while the game is fullscreen over it.
`HotkeyListener` itself stays on the UI side purely to push rebound keys to the backend
(`set_bindings`). The gearbox/backfire toggles live in `AppConfig`, so the listener thread
publishes them back to the UI in its snapshot (with a generation counter, so the UI's next
config push can't undo a toggle it hasn't seen yet — see [[overview]]).

- **Linux — evdev read of `/dev/input/event*`.** *Why this way:* reading input devices sits
  *below* the display server, so it works identically on X11, Wayland, and console — no
  compositor-specific global-shortcut API, no D-Bus/portal. It reads, it doesn't grab, so
  Wayland's "no global key grab" restriction doesn't apply. Reuses the `input`-group /
  `evdev` access the synthetic-input feature already needs.
- **Windows — `GetAsyncKeyState` poll.** *Why not a hook:* polling only the specific VKs we
  bind is simpler (no `SetWindowsHookEx` / message pump), robust over fullscreen, and a
  cleaner privacy story than a low-level hook that sees every keystroke.

**Observe-only** on both: the game still receives the key, so bind keys the game doesn't use
for driving (G, B are safe). **Match-only:** non-matching keystrokes are dropped in the
backend immediately — never stored, sent, or logged.

## Focus detection (`src/focus.rs`)

One `FocusDetector` + one poll thread (at the configured Hz) caches "is the game focused?"
in an `AtomicBool`, read by both the hotkey gate and the input gate.

- **Methods:** Hyprland (`hyprctl activewindow`), X11 (`xdotool`/`xprop`), Custom (a
  user command), and native `GetForegroundWindow` on Windows. Each yields the active
  window's name; `game_match` (case-insensitive substring, default "Forza") decides.
- **Detect button:** 3-second countdown, then one query auto-fills `game_match` — handles
  opaque titles (e.g. GameScope).
- **Fail-open:** if a query errors (tool missing, bad command), the detector reports
  focused=true so hotkeys/input keep working, and the settings shows a red status.

## Gate rules

- **Global hotkeys fire** when our app is focused (unless a text field is capturing keys,
  via `wants_keyboard_input`) **or** the game is focused; a third app focused → ignored.
  "Game focused" comes from either *Telemetry-live* (`telemetry.is_connected`, lightweight,
  can't exclude a third app when auto-pause is off) or *Window-focus* (the detector).
- **Synthetic-input gate** (opt-in, `input_focus_gate`): when on, backfire/DSG key injection
  is suppressed unless the game is focused, so alt-tabbing out never sprays keys elsewhere.
  The flag is driven from the listener thread — driving it from the frame loop froze it at
  whatever the last drawn frame stored, which could leave key output dead while hidden.
- *"Our app focused"* and *"a text field wants keys"* only exist on the UI thread, so the UI
  pushes both to the listener thread each frame. They stop updating while the window is
  hidden, which is harmless: a hidden window is neither focused nor typing.

## Requirements & limitations

- **Linux `input` group:** reading `/dev/input` needs the user in the `input` group
  (`sudo usermod -aG input $USER`, then re-login). The settings status light shows 🟢/🔴.
- Observe-only (a bound key still reaches the game); modifiers tracked per keyboard device;
  focus reads can be up to `1/Hz` stale; keyboards hot-plugged after launch need a restart.
  See spec §11.
