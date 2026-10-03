# Hotkeys

Rebindable keyboard shortcuts, configured in **Setup → Hotkey** (the tab is `Tab::Settings`,
labelled Setup). Two scopes, one rebind UI. Full design + rationale: [[hotkeys-design]]
(`docs/features/hotkeys-design.md`).

Controller (gamepad) bindings for the global actions are a separate card (Setup → Controller) that
feeds the same action channel and gate: see [[gamepad]].

## Two scopes

- **Global (while in-game)** — fire while the *game* holds focus (or our app does).
  Defaults: `G` = toggle Automatic Gearbox, `F` = **Clear RPM calibration**, `F` =
  **Clear gear map** (same key: one press clears both, see "One key, several actions"),
  `B` = toggle Backfire, `J` = **Hide HUD** (toggle the in-game overlay, see [[overlay]]).
  *Why J and the shared F:* these two actions are new, and their defaults are the user's own
  bindings. Routed through the capture backend + focus gate.
- **In-app** — fire only while our telemetry window is focused. Defaults: `Ctrl+S` =
  mini-settings, `Ctrl+E` = dashboard edit. Handled via egui input (`ctx.input`), so they
  are inherently UI-only. Rebindable because the combo is read from config.

A binding's *scope* is fixed per action (`HotkeyAction::scope`), not user-chosen.

**Hide HUD** is special in two ways: its state (`hud_hidden`) is runtime-only on the listener
thread, never config, and it does nothing while the overlay is disabled (details and *why* in
[[overlay]]). Its binding is the one `hotkeys.bindings[HideHud]`, editable both here and in
the Overlay tab's General card (D28).

## One key, several actions

Any number of actions can share one key combo (e.g. **Clear RPM calibration** + **Clear gear
map** on `F`); pressing it **fires every action bound to it**, in `HotkeyAction::ALL` order
(`HotkeyAction::order`; `app::global_bindings` sorts the list handed to the backend). Nothing
warns about or blocks a shared key. In code: `hotkeys::match_combo` returns *all* matching
actions and `hotkeys::dispatch` sends each one; both backends use it. *Why:* the user wants one
button to do several things (both clears at once). It used to fire only the first: the Linux
backend took the first match, and the Windows poll looped per binding with the edge state kept
per key, so the first binding on a key consumed the rising edge and the rest never fired. The
Windows poll now loops over distinct keys. The same applies to controller controls
([[gamepad]]).

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
  focused=true so hotkeys/input keep working, and the settings shows a red status. A
  *successful* answer that doesn't match `game_match` is **not** an error: focused=false, hotkeys
  (in *Game window focused* mode) and gated input are dropped. `FocusDetector::snapshot()`
  keeps the last window, match flag, error, last-match time and the 5 most recent distinct
  names for the Hotkey Diagnostics card.

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
  (`sudo usermod -aG input $USER`, then re-login) **or** an ACL that grants the seat user the
  keyboard nodes. The settings status light shows 🟢/🔴.
- **Input-permission check (D13, Linux only):** `input::probe()` gathers the facts (a
  **keyboard** under `/dev/input/event*` readable via `hotkeys::probe_status()` and
  opened by the backend, the **key sender's virtual keyboard really created** (see *What the
  lights mean* below),
  `/dev/uinput` existing / group `input` rw, process in the `input` group) and the pure
  `input::evaluate()` turns them into *what's missing* + the fix commands: `sudo usermod -aG
  input $USER` (not in the group), `sudo modprobe uinput` (no `/dev/uinput`), or a udev rule
  (`KERNEL=="uinput", GROUP="input", MODE="0660"` into `/etc/udev/rules.d/99-uinput.rules`,
  then `udevadm control --reload && udevadm trigger`) when the node exists but isn't
  group-`input` writable. `evaluate()` returns `(label, cmd)` pairs; each command sits under a
  numbered label (1., 2., 3. by position shown) saying what it does, and with two or more the
  closing line is "Run all commands above, then log out and back in." (Why: users couldn't tell
  whether they needed both.) If anything is missing a **modal** opens (at launch, and again on a
  transition into "missing", see *Live status* below; X = closed until then; *Don't remind me
  again* sets `input_perm_dont_remind`). Setup has an
  **Input Permissions** category below *Window Detection* (a light per requirement, the same
  copyable commands, a *Remind me on startup* checkbox = inverse of the flag, *Re-check*).
  Hidden on Windows. *Why:* both failures are silent otherwise (no hotkeys, dead gearbox /
  backfire). *Why a probe, not the listener:* the backends run on worker threads and only
  `return` on failure, so the UI can't ask them; opening the nodes is cheap and exact.
  *Why the flag is `EXPORT_EXCLUDE`:* it's per-machine, not a tuning setting.
  **What the lights mean (both are functional checks).** A green light means the thing works
  *right now*, not merely that a permission looks right. *Why:* the user's bug report was exactly
  a green light while reading or sending was dead (a friend on GNOME, not in `input`).
  - **Hotkeys** green = at least one **physical** keyboard is readable *and* the backend holds an
    open reader on it (details below). Real `open()`s, no guessing from group membership.
  - **Key input** green = `InputSender` **created its uinput virtual keyboard** (`Forza Telemetry
    Input`; `InputSender::uinput_ready()` is `Some(true)`), decided by `input::uinput_ok()`. The
    old test (`open("/dev/uinput")` for write) only proved the node opens; the device build on
    the worker thread could still fail and then every press was silently dropped. The sender's
    answer wins over the open test; only while it hasn't answered (`None`) does the open test stand
    in. *Why the constructor waits:* `InputSender::new()` waits up to 500 ms for the first build
    (ms in practice) and the app creates it **before** the startup probe, so the first probe and
    the startup modal see the real answer instead of "pending".
  - **The sender retries.** The worker loops: build the device; on failure mark it failed, log
    **once** (`uinput: could not create virtual device: …`), drop queued commands, wait 2 s (or
    until **Re-check** nudges it) and try again; it stops only when the app is gone. While the
    device is not ready, `press*`/`hold_tracked`/`release` return at once (nothing queues up, and
    the UI thread can't block on a full queue). *Why:* it used to try once and end, so fixing the
    permission later (`modprobe`, a udev rule) left key sending dead until a restart, even though
    the light then went green. Re-check calls `InputSender::recheck()` (retry now, wait up to
    300 ms for the outcome). Windows / other platforms report ready (no virtual device).
  **What the hotkeys light tests:** "at least one *working keyboard* event node is readable".
  `hotkeys::classify()` (pure) maps `(is_keyboard, readable)` per node to `Ok` (≥1 readable
  keyboard) / `NoPermission` (keyboards exist, none readable) / `NoDevice` (no keyboard; not a
  permission problem, light stays green). A node is a keyboard by its world-readable sysfs
  bitmap `/sys/class/input/eventN/device/capabilities/key` having `KEY_A` (bit 30), so it can be
  classified without opening it; if sysfs has no entry the device is asked directly.
  `input::hotkeys_ok()` additionally turns the light red when the probe says readable but the
  backend has **zero** open keyboards, so hotkeys can't be dead behind a green light.
  *Why keyboards only:* on GNOME / KDE / Fedora, systemd-logind `uaccess` ACLs (and Steam's
  udev rules) make game controllers and `/dev/uinput` accessible to the seated user, but
  keyboards are not `uaccess`-tagged and stay `root:input 0660`. The old "any readable event
  node" check was therefore a false positive with a gamepad plugged in (green light, modal
  never shown, no hotkey ever read). `/dev/uinput` stays an open-for-write test: a writable
  ACL there is genuinely fine.
  **Working keyboard = physical, not mouse-like** (v0.4.2 follow-up; GNOME report: light amber,
  no modal, hotkeys dead). `hotkeys::inventory()` lists every event node once (name, readable,
  `KEY_A`, virtual, pointer-like) and three pure rules decide which ones count
  (`working_mask`, `counts_as_physical`, `is_virtual_sysfs_path`): (1) **not virtual**, i.e. the
  node's canonical sysfs path isn't under `/sys/devices/virtual/input/` (the uinput devices:
  ydotoold, a remapper, Steam Input, Bluetooth AVRCP, and **our own** `Forza Telemetry Input`,
  `input::VIRTUAL_DEVICE_NAME`, which has only W/E/Q so it was never a `KEY_A` keyboard anyway but
  is also excluded by name); (2) **not mouse-like** (also reports `EV_REL`/`EV_ABS`, e.g. a gaming
  mouse's key-macro interface or a pad), *unless* no plain keyboard exists at all (some real
  keyboards report axes, so that case falls back instead of going red). *Why:* all of those can be
  readable through `uaccess`/ACLs while the real keyboard (`root:input 0660`) isn't, and the old
  "any `KEY_A` node" count made the light amber with zero readable typed keys. Bluetooth keyboards
  (`uhid`, `/sys/devices/virtual/misc/uhid/…`) are real hardware and count. **The scan still reads
  virtual and mouse-like nodes** (except our own device): a remapper such as keyd re-emits the
  physical keys on a virtual keyboard and grabs the real one, so reading only physical nodes
  would break that setup. They just don't count toward `active_keyboards()` / the light.
  `OpenKeyboards` is now a map `node path → counts as working`; a rescan **refreshes** that flag
  for nodes that are already open (`refresh_open_flag`) instead of freezing the verdict from the
  moment the node was opened. *Why:* a node first classified wrong (a sysfs hiccup, or the
  "no plain keyboard yet" fallback) would otherwise skew the light for the whole session.
  **Hotkey Diagnostics card** (Setup, below Input Permissions, Linux): (a) *Keyboards*: every
  `KEY_A` node with readable / open / virtual / also-mouse flags (dot: green = working and open,
  red = working but not readable / open, amber = doesn't count); (b) *Hotkey events*: last key
  seen + device + age, last hotkey sent to the listener thread, ended readers; (c) *Hotkey gate*:
  the mode, the verdict **with the game in front** (`app::gate_verdict(.., our_focused=false, ..)`,
  built on the same `global_hotkey_allowed` the listener thread calls), the focus detector's last
  window + match yes/no, "game window last matched N s ago / never" and the recent distinct window
  names (tooltip). **Copy diagnostics** puts the same facts on the clipboard as English text.
  *Why this exists:* the failure is silent (no events, or events dropped by the gate), and we
  can't test on GNOME; "no key seen" vs "key seen, hotkey sent, still nothing" vs "gate says no"
  points at the culprit from one screenshot. *Privacy:* the backend stays match-only; the card
  keeps a press **counter** + device + time always, but the **identity** of the last key only
  while the card is drawn (`HotkeyDiag::listen()` each frame, 1.5 s window), so no key log runs
  in the background. *Limit:* the listener thread's own drop decision isn't recorded
  (`src/listeners/` is separate); the card recomputes the verdict from the same facts instead.
  A reader thread that ends logs `hotkeys: reader for … ended: …` to stderr and shows in the card.
  **Gate facts for GNOME:** with the default *Telemetry live* mode the window query isn't used for
  hotkeys at all (only packets in the last 2 s, or our own window focused); in *Game window
  focused* mode a **successful but non-matching** answer blocks hotkeys (fail-open covers only
  query *errors*), so a wrong `game_match` (e.g. the Proton window's title/class lacks
  "Forza") drops them silently: the card's recent-windows list shows what the game window is
  actually called.
  **Modal / group line:** the modal fires only when hotkeys or uinput are actually missing
  (`evaluate().any_missing()`); being outside the `input` group alone doesn't nag, because
  access may come from ACLs. The group line is amber then, red only when something is missing.
  **Live status (v0.4.2 follow-up).** `ui::settings::refresh_input_facts()` runs from
  `ForzaApp::update` every frame, throttled to once per ~2 s (plus at once on **Re-check**): it
  re-reads the keyboard list and `input_probe` (sysfs reads, `open()` of the event nodes, the
  uinput open and the sender's readiness: sub-millisecond). So the Setup lights, the Controller
  card's "can't read /dev/input" line and the modal's self-close always show fresh data, whether or
  not the Setup tab is open. *Why:* the probe used to run only at startup (and while Setup was
  drawn), so a fix or a later breakage never reached the lights or the modal.
  **Modal rule** (`input::modal_should_open(prev_missing, now_missing, remind)`): the modal opens
  at launch if something is missing, and again **once per transition** from "all fine" to
  "something missing" (device unplugged, sender died) while *Remind me on startup* is on. It
  never re-opens while the status simply stays missing, so closing it with X keeps it closed until
  the status has been fine and then breaks again. It closes by itself when nothing is missing any
  more. *Why:* the status is live now, so a mid-session breakage must be announced once, but a
  dismissed dialog must not reappear every 2 s.
  **Automatic reopen / hot-plug.** `HotkeyListener::new` starts a small rescan thread
  (`RESCAN_EVERY`, 2.5 s, Linux) that calls the same `scan()` as **Re-check**: it only opens
  keyboards that have no reader yet. A reader thread removes its node on exit, so an unplugged,
  suspended or Bluetooth-reconnected keyboard is picked up within ~2.5 s with no click. *Why in the
  backend, not the UI frame loop:* hotkeys must keep working while the window is hidden and no
  frames are drawn. `claim_node` makes the check-and-insert atomic so the thread and the Re-check
  button can't start two readers on one node (every hotkey would fire twice). The thread ends
  when the `HotkeyListener` is dropped.
  **Re-check** calls `HotkeyListener::rescan()` and `InputSender::recheck()` before re-probing, so
  access that appears later works without a restart (group membership itself still needs a
  re-login; then the app is restarted anyway).
  *Dev/testing aid:* `FORZA_FAKE_NO_INPUT_PERMS=1` makes `probe()` report hotkeys unreadable,
  uinput not writable and not in the `input` group (node present but not group-writable, so the
  udev-rule and `usermod` commands show). *Why:* a machine with every permission never shows
  the modal, so it couldn't be reviewed.
- Observe-only (a bound key still reaches the game); modifiers tracked per keyboard device;
  focus reads can be up to `1/Hz` stale; keyboards hot-plugged after launch are picked up by the
  periodic rescan (~2.5 s) or at once by Setup → Input Permissions → **Re-check**.
  See spec §11.
