# Gamepad (Controller input)

Xbox-style controller input: bind pad controls to the global hotkey actions, and expose the
right-stick vector for other features (the map rotation task). Configured in **Setup →
Controller** (card right under Hotkey). Code: `src/gamepad.rs`; UI: `controller_card` in
`src/ui/settings.rs`; config: `AppConfig.gamepad` (`GamepadConfig`). Research tool:
`tools/gamepad-probe.py`.

## What it does

- **Bindings** for the global actions (Toggle Automatic Gearbox, Clear RPM calibration, Clear gear map,
  Toggle Backfire, Hide HUD). Click a row's button, press a pad control (**Esc** cancels,
  **Backspace** or **Delete** clears the binding and ends the capture; there is no clear button). Default: only **Toggle Automatic Gearbox = L3** (the user's own binding; `GamepadConfig::default`). Several actions can share one control:
  pressing it **fires all of them** (in `HotkeyAction::ALL` order; `PadParams::from_config`
  sorts by control then action order, `Shared::feed` sends every match). `GamepadConfig::bind`
  no longer takes the control away from other actions. *Why:* the user wants one button to do
  several things (e.g. both clears); same rule as the keyboard hotkeys ([[hotkeys]]).
- **Bindable controls** (`PadControl`): A/B/X/Y, LB/RB, LT/RT (past a threshold), Back/Start,
  L3/R3, the D-pad's 4 directions, and the **right stick's** Up/Down/Left/Right.
- **Deadzones**: stick (radial, default 0.27) and trigger (default 0.10).
- The in-app hotkeys (mini-settings, dashboard edit) are not bindable: they are UI-only and
  the pad backend lives off the UI thread.

## Architecture

- **Actions reuse the keyboard hotkey channel.** `Gamepad::spawn` is handed
  `HotkeyListener::action_sender()` (a clone of the `mpsc::Sender<HotkeyAction>` the listener
  thread drains), so a pad press passes through the *same* focus gate (`global_hotkey_allowed`,
  `GateMode`) as a key, with no edit to `listeners/worker.rs`. *Why:* one gate, one place for
  the "game focused" rule; the pad cannot drift from the keyboard behaviour.
- **Pure core.** `norm_stick`/`norm_trigger` (absinfo range -> -1..1 / 0..1),
  `radial_deadzone`, `trigger_deadzone`, and `Processor` (held set + rising edges) have no
  device dependency and are unit-tested. `Shared::feed` is the one place a device state turns
  into a right-stick update and actions; both backends call it.
- **Edges.** Triggers and the right stick's directions become presses with **hysteresis**
  (press at 0.6 / release at 0.4 after the deadzone for the stick; 0.5 / 0.35 for triggers),
  rising edge only: nothing while held, fires again after a release. Only the *dominant* stick
  axis counts, so a diagonal presses one direction.
- **Capture.** `arm_capture()` makes the next rising edge be stored (`take_captured()`)
  instead of sent as an action, so binding Y doesn't also toggle the gearbox. The UI polls it
  in `controller_card` (`app.pad_rebinding`); a tab switch cancels.
- **Muted during any rebind.** `Shared::feed` sends no action while the shared
  `RebindGuard` (`hotkeys.rs`) is blocked: active while a keyboard *or* pad rebind is armed,
  plus a 300 ms grace after. *Why:* the capture flag clears the moment the press is stored,
  but the UI commits it a frame later, so rebinding to the control it already had (B → B)
  could still fire its old action in that gap.

### Linux (`backend`, evdev)

- **Thread model.** One scanner thread (rescan every 2 s) plus one blocking reader thread per
  pad. Reads are read-only: **no EVIOCGRAB**, the game still gets the pad.
- **Discovery by capability, not name:** a node with `BTN_SOUTH` and `ABS_RX` + `ABS_RY`.
  Non-pads are remembered in an ignore list (pruned when the node vanishes) so they aren't
  reopened every scan.
- **Hot-plug** = the 2 s rescan (simple; inotify not needed). *Why:* the xone wireless dongle
  creates its node late and the pad can appear any time. A reader thread ends when its device
  goes away (`ENODEV`) and the path becomes eligible again.
- **Normalisation per device** from `EVIOCGABS` (`get_abs_state`) at open: xone/xpadneo
  triggers 0..1023, xpad 0..255 (Steam virtual), sticks +-32768. Never hard-coded.
- **Drivers:** xpad, xone, xpadneo. All report the right stick as `ABS_RX`/`ABS_RY`, triggers
  as `ABS_Z`/`ABS_RZ` (digital `BTN_TL2`/`BTN_TR2` also honoured), the D-pad as
  `ABS_HAT0X/Y` (or `BTN_DPAD_*`). evdev Y grows downward; it is inverted so the shared
  convention is **up-positive**.
- **Needs the `input` group**, the same as hotkeys (see [[hotkeys]] / Input Permissions). The
  card shows a red status when `/dev/input` is unreadable.

### Windows (`backend`, XInput)

`XInputGetState` polled every 8 ms for user indices 0-3; empty slots are probed only every
2 s (a probe of an absent slot can be slow; this is also the hot-plug latency). Needs the
windows-sys feature `Win32_UI_Input_XboxController`. *Why XInput:* it keeps reporting while
FH6 is focused; **Windows.Gaming.Input only delivers while its own window is focused.**

## D15 findings: read the physical pad, skip Steam's virtual one

Steam Input exposes a virtual pad (vendor:product `28de:11ff`, named like an Xbox 360 pad)
next to the real device. A live probe in-game showed **Steam does not grab the physical pad**:
both the physical device (xpad, Elite 2) and Steam's virtual pad delivered right-stick events,
the virtual one tracking the physical with a small offset/lag. The backend therefore **skips
`28de:11ff`** and reads the physical pad.

*Why:* Steam's virtual pad only delivers input while the game window is focused; the
physical device is the reliable source regardless of focus. (`tools/gamepad-probe.py` lists
both and tags the Steam one.)

## Right-stick vector (for the map-rotation task)

`app.gamepad` is a `Gamepad` (cheap `Clone`, an `Arc`; hand a clone to any thread).
`gamepad.right_stick() -> (x, y)`: each -1..1 **after** the configured radial deadzone,
**x right, y up**, `(0, 0)` while the feature is disabled or no pad is connected. With several
pads, the one deflected furthest wins. It locks two tiny mutexes; call it once per frame.
The deadzone is applied at read time, so slider changes take effect without stick movement.

## Config (`AppConfig.gamepad`, export group "Hotkeys & Input")

`enabled` (true), `stick_deadzone` (0.27), `trigger_deadzone` (0.10), `bindings`
(`HashMap<HotkeyAction, PadControl>`, default `{ ToggleGearbox: L3 }`; a config whose `gamepad` object exists keeps its own
bindings, an object missing the `bindings` key gets the default). The UI pushes the config to the backend every
frame (`Gamepad::set_params`, no-op when unchanged).

## Limits

Xbox-layout pads only (PlayStation/Switch pads without `BTN_SOUTH`/`ABS_RX` aren't detected as
such); no guide/share buttons; no rumble; bindings are global-scope actions only.

## Controller card layout

Each row is `control_row` (label | control halves); inside the right half the bind button is a single `ui.scope_builder` (max_rect = the target rect, centered-and-justified layout) placed into a **fixed rect = the column's full width × `h`** (from `ui.available_rect_before_wrap()`), with the label `truncate()`d. There is deliberately **no clear button**: while a row captures, Backspace/Delete unbinds it and ends the capture (also disarming the backend capture, so `RebindGuard` drops like it does for a bind); skipped when a different widget holds keyboard focus. *Why no ✕:* the keyboard hotkey rows already clear with Backspace/Delete, so the pad rows work the same, and the always-reserved clear slot is gone. *Why a fixed rect, not cursor flow:* egui's `Region::expand_to_include_rect` grows a Ui's **max_rect** (not just min_rect) on overflow, so any widget wider than its slot widens the card by its overflow; the next row's `ui.columns` then splits a wider rect, its right column starts further right, and the rows stagger (an earlier version with a ✕ button whose natural width, glyph + 2×`button_padding.x` ≈ 30px, exceeded its h×h budget staggered ~4px/row). Nothing may overflow the column here — keep long capture text truncated. Also don't wrap a `put` in `add_enabled_ui` / `ui.scope`: those child Uis start at the parent's cursor, so their min_rect spans from the cursor to the put rect and overflows the same way.
