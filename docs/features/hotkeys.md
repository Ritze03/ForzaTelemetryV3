# Hotkeys

Rebindable keyboard shortcuts, configured in **Setup → Hotkey** (the tab is `Tab::Settings`,
labelled Setup). Two scopes, one rebind UI. Full design + rationale: [[hotkeys-design]]
(`docs/features/hotkeys-design.md`).

Controller (gamepad) bindings for the global actions are a separate card (Setup → Controller) that
feeds the same action channel and gate: see [[gamepad]].

## Two scopes

- **Global (while in-game)** — fire while the *game* holds focus (or our app does).
  Defaults: `G` = toggle Automatic Gearbox, `F` = clear gearbox calibration (gear map + max RPM), `B` =
  toggle Backfire, `H` = **Hide HUD** (toggle the in-game overlay, see [[overlay]]). Routed
  through the capture backend + focus gate.
- **In-app** — fire only while our telemetry window is focused. Defaults: `Ctrl+S` =
  mini-settings, `Ctrl+E` = dashboard edit. Handled via egui input (`ctx.input`), so they
  are inherently UI-only. Rebindable because the combo is read from config.

A binding's *scope* is fixed per action (`HotkeyAction::scope`), not user-chosen.

**Hide HUD** is special in two ways: its state (`hud_hidden`) is runtime-only on the listener
thread, never config, and it does nothing while the overlay is disabled (details and *why* in
[[overlay]]). Its binding is the one `hotkeys.bindings[HideHud]`, editable both here and in
the Overlay tab's General card (D28).

## Rebinding and unbinding

Click a binding's button → "Press a key…". Every rebind button (Setup → Hotkey and the
Overlay tab's Hide HUD row) arms `app.rebinding`; the key is taken by one shared
`ForzaApp::capture_rebind`, which runs **before** the tabs are drawn:

- **Esc** cancels; **Backspace** or **Delete** clears the binding ("Not set", shown faint); any other
  bindable key binds with the held Ctrl/Alt/Shift.
- While a capture is armed the key doesn't also fire an in-app hotkey, and the listener is
  told `wants_text` (`app.rs`, the per-frame `listener.push`), so global hotkeys are gated
  too: binding G doesn't also toggle the gearbox, H doesn't hide the HUD.
- The key that ends a capture (bind / Backspace / Delete / Esc) is **consumed** from egui's input,
  so it doesn't also reach a widget.
- The capture **disarms without binding** on a primary press anywhere but the armed button
  (its id + rect are recorded each frame by `ForzaApp::track_rebind_button`), when another
  widget holds keyboard focus (e.g. a text field), and on a tab switch. *Why:* otherwise an
  armed capture outlived the page and silently rebound Hide HUD to a key meant for something
  else (Ctrl+S on the Dashboard, Backspace in a text field). The armed button itself may
  hold focus (armed via Tab + Enter), so the check is "another widget is focused", not
  `wants_keyboard_input()`.
- Unbinding goes through `HotkeyConfig::unbind`, which removes the binding **and** records
  the action in `hotkeys.unbound`, so `inject_missing_hotkeys` doesn't restore the default on
  the next load. `HotkeyConfig::bind` clears that mark. Always edit through `bind` /
  `unbind`, never `bindings.insert` directly.
- *Why one capture in `app.rs`:* two tabs edit the same binding, and a per-tab capture would
  duplicate the Esc/Backspace/Delete logic (the old Setup-only capture inserted directly and would
  have skipped the unbound bookkeeping).
- **No action fires during a rebind** (keyboard *or* pad). `ForzaApp::update` mirrors
  `rebinding.is_some() || pad_rebinding.is_some()` into the shared `RebindGuard`
  (`hotkeys.rs`); the evdev / `GetAsyncKeyState` backends and the gamepad backend check
  `guard.blocked()` before sending. After the capture ends it stays blocked for
  `REBIND_GRACE` (300 ms), which also covers the capturing key being released. *Why:* the
  raw-device backends are always ahead of egui, so rebinding Backfire to B (its current key)
  delivered B to the backend, and the queued action reached the listener thread after the
  UI had already ended the capture (the old per-frame "typing" flag was a frame stale) —
  Backfire toggled. On Windows the rising-edge state is kept per virtual key and updated
  even while muted, so the key just bound has no edge when the guard lifts.

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
in an `AtomicBool`, read by the hotkey gate, the input gate and the overlay's
**Only when game window is focused** option. The detector also runs whenever the overlay is
enabled, and its thread does the overlay's monitor detection (see [[overlay]]).

- **Methods:** Hyprland (`hyprctl activewindow`), X11 (`xdotool`/`xprop`), GNOME (`gdbus`
  → the **Window Calls** Shell extension's `List()`, since GNOME on Wayland has no built-in
  focused-window API; see [[hotkeys-design]] §7), Custom (a user command), and native
  `GetForegroundWindow` on Windows. Each yields the active
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
  pushes both to the listener thread each frame — and they **expire after 1 s**
  (`worker::FOCUS_FACTS_TTL`), after which the listener treats the window as neither focused
  nor typing. *Why the expiry:* a hidden window is never drawn, so the pushed values freeze.
  Frozen at *focused, not typing* the gate is stuck **open** — a bare `G`/`B`/`F` typed into
  any other app would toggle the gearbox or wipe your calibration. Frozen at *focused,
  typing* it is stuck **shut** for as long as the window stays hidden. Expired, the gate
  falls back to "is the game focused?", which is the correct rule for a hidden window. 1 s is
  ~5× the slowest frame the UI can legitimately take (the FPS-limit slider floors at 5 fps
  and `update` always re-arms a repaint).

## Requirements & limitations

- **Linux `input` group:** reading `/dev/input` needs the user in the `input` group
  (`sudo usermod -aG input $USER`, then re-login). The settings status light shows 🟢/🔴.
- **Input-permission check (D13, Linux only):** at startup `input::probe()` gathers the facts
  (`/dev/input/event*` readable via `hotkeys::probe_status()`, `/dev/uinput` writable,
  `/dev/uinput` existing / group `input` rw, process in the `input` group) and the pure
  `input::evaluate()` turns them into *what's missing* + the fix commands: `sudo usermod -aG
  input $USER` (not in the group), `sudo modprobe uinput` (no `/dev/uinput`), or a udev rule
  (`KERNEL=="uinput", GROUP="input", MODE="0660"` into `/etc/udev/rules.d/99-uinput.rules`,
  then `udevadm control --reload && udevadm trigger`) when the node exists but isn't
  group-`input` writable. `evaluate()` returns `(label, cmd)` pairs; each command sits under a
  numbered label (1., 2., 3. by position shown) saying what it does, and with two or more the
  closing line is "Run all commands above, then log out and back in." (Why: users couldn't tell
  whether they needed both.) If anything is missing a **modal** opens once per launch (X = closed
  for this session; *Don't remind me again* sets `input_perm_dont_remind`). Setup has an
  **Input Permissions** category below *Window Detection* (a light per requirement, the same
  copyable commands, a *Remind me on startup* checkbox = inverse of the flag, *Re-check*).
  Hidden on Windows. *Why:* both failures are silent otherwise (no hotkeys, dead gearbox /
  backfire). *Why a probe, not the listener:* the backends run on worker threads and only
  `return` on failure, so the UI can't ask them; opening the nodes is cheap and exact.
  *Why the flag is `EXPORT_EXCLUDE`:* it's per-machine, not a tuning setting.
  *Dev/testing aid:* `FORZA_FAKE_NO_INPUT_PERMS=1` makes `probe()` report hotkeys unreadable,
  uinput not writable and not in the `input` group (node present but not group-writable, so the
  udev-rule and `usermod` commands show). *Why:* a machine with every permission never shows
  the modal, so it couldn't be reviewed.
- Observe-only (a bound key still reaches the game); modifiers tracked per keyboard device;
  focus reads can be up to `1/Hz` stale; keyboards hot-plugged after launch need a restart.
  See spec §11.
