# Settings — network, units, display

The **Settings** tab (`Tab::Settings`, labelled **Setup** in the tab bar) holds the app-wide
options that aren't tied to a single dashboard widget. Rendered from `src/ui/settings.rs` as
bordered category cards laid out per the [styling guide](../ui/STYLING-GUIDE.md)
(label-left / control-right rows); values persist in `config.json` (see
[[state-and-config]]). Per-widget tuning lives in the cog **Mini-Settings** popup instead —
see [[presets]]. The in-game HUD overlay has its own **Overlay** tab — see [[overlay]] (module tabs; both maps' layer settings are there too, Mini-Settings keeps the quick options).

Cards: **Profiles**, **Hotkey**, **Controller** (see [[gamepad]]), **Display** (left column); **Repository / Credits**,
**Network**, **Game Install**, **Map data**, **Input Permissions** (Linux only), **Window Detection** (right column). Hotkeys and window detection get
their own doc — see [[hotkeys]]; the Profiles card gets its own doc too — see [[profiles]].

## Profiles

First card in the left column. Switch between named full-config snapshots, and create /
duplicate / rename / delete them, plus selectively export/import settings by group. Save
is continuous (the live config mirrors the active profile on every change), so there's no
Save button and switching always persists the outgoing profile. Full detail in
[[profiles]].

## Network

- **Listen port** — the UDP port the app binds to receive FH6 telemetry. Type a
  new port and press **Apply** to rebind the receiver. Configure the game to send
  to this port under **SETTINGS > HUD AND GAMEPLAY > Data Out**.
- Avoid ports **5200–5300** — the game binds its own outgoing socket there.

## Status bar connection indicator

Bottom bar, left side (`src/app.rs`). Governed by *Status bar: show text labels*:

- **Text on** — icon + "Connected"/"Disconnected" + "70 pps" form one hover zone.
- **Text off** — plug / no-signal icon, then the bare packet rate ("70", no unit) painted in
  the icon's colour directly against it (no item spacing; fixed 3-digit-wide box so the
  Co-Op icon doesn't shift when the rate changes digit count). The gap to the Co-Op icon is
  unchanged (see the reverted `96ef8e1`/`c86cec7` spacing experiment).
- Either way hovering the icon or number shows a tooltip: the state word, then
  "N packets per second" (never "pps").

*Why:* dropping the unit saves space, so the tooltip is where the unit is spelled out; one
zone for the group avoids dead gaps between icon and number.

The **Co-Op** category (Host port) also moved off this page — it is Cloudflare-only, so it
now lives on the Co-Op tab under the Session card; see [[coop]].

## Display

- **Language** — English or German (all strings go through `tr(...)`; see
  [[ui-architecture]]).
- **Speed unit**, **Tire temp unit**, **Boost / pressure unit** — pick the units
  used across every readout. The boost/pressure toggle also drives the
  [[power-curve]] boost axis and other boost readouts.
- **FPS limit** — cap the render rate independently of the packet rate (the
  limiter uses `request_repaint_after`, so it renders at most this often even
  though telemetry still arrives at ~60 Hz).
- **Always on top** — a checkbox and config field (`always_on_top`), but **currently not
  read anywhere**, so it has no effect.

## Hotkey & Window Detection

The **Hotkey** card lists every binding in two groups (Global / In-app), including **Hide
HUD** (default J, the same binding the Overlay tab edits). Click a button and press a key;
**Esc** cancels, **Backspace** clears it to a faint **Not set**. Details in [[hotkeys]].

The **Input Permissions** category (Linux only, below Window Detection) shows three status dots,
green or red (no amber): *Hotkeys: read keyboard devices*, *Key input: write /dev/uinput* and
*Member of the input group*, plus the copyable fix commands, a *Remind me on startup* checkbox and
*Re-check*. Not being in the `input` group is red on its own and opens the "input permissions
missing" dialog. *Why:* the keyboard check alone is unreliable (a gaming mouse's key interface
can count as a keyboard), a user outside the group practically can't read keyboards, and a false
alarm is muted with *Don't remind me again*. Details in [[hotkeys]].

The **Window Detection** card (formerly **Input**; its duplicate "Window Detection"
sub-heading is gone) holds the focus detection that several features share:

- **Detection method default**: a fresh config picks it from the desktop (`config::focus_method_for_desktop`, fed by `XDG_CURRENT_DESKTOP` / `XDG_SESSION_TYPE` / `HYPRLAND_INSTANCE_SIGNATURE`): Hyprland -> Hyprland, GNOME (incl. `ubuntu:GNOME`) -> GNOME, other X11 session -> X11, unknown Wayland compositor -> Hyprland. Saved configs keep their stored value; a saved `hotkeys` object missing the field gets plain Hyprland (serde default), not detection. *Why:* since overlay + focus-only became default-on, a GNOME user's fresh install ran `hyprctl`, failed, and broke focus gating. The overlay's monitor method (`monitor_method`) still defaults to Hyprland (no GNOME monitor method exists).
- **Active if** (hotkey gate: Telemetry live / Game window focused); the detection method
  and game window title (with **Detect**), shown when window focus gates something (Active
  if = Game window focused, the input gate, the overlay's focus-only option, or the overlay
  being enabled); and the
  **Focus check rate**. *Why the enabled overlay counts:* its monitor detection only trusts
  the focused monitor once the detector matches FH6, so a user stuck on amber *Game window
  not focused* must be able to see and fix the method / title.
- A status dot with three states: green *Game window focused*, **amber** *Game window not
  focused* (a normal waiting state, not an error), red *Focus detection failed*. It shows
  while the detector runs (including whenever the overlay is enabled). The colours are the
  theme's `GOOD` / `WARN` / `DANGER` tokens.
- **Overlay → Only when game window is focused** (`overlay.focus_only`) is **not** here any
  more: it lives on the Overlay tab (General card), with a tooltip saying it uses this card's
  detection method. *Why:* the user looked for it on the Overlay tab and didn't find it in
  Setup (supersedes D5).
- **Send Input → Only send inputs when game focused** (the synthetic-input gate).

Full detail in [[hotkeys]].

## Game Install

The **Game Install** card (Windows and Linux, third in the right column, below Repository / Credits and Network) sets
where Forza Horizon 6 is installed; the Debug tab reads car names from it (see
[[fh6-cars-names-icons]]). One path field (config `fh6_install_dir`, empty = auto via Steam; the
game folder or its `media` folder), three buttons and a status line:

- **Auto-detect** runs the Steam detection and fills the field (or reports *Not found*).
- **Detect from running game** reads the install folder from the running FH6 process (tooltip:
  *Start Forza first*; reports *Forza Horizon 6 is not running* otherwise).
- **Clear** (shown while the field is set) returns to automatic detection.
- Status dot: green = the `media` folder (and the car count), amber = *found but not readable*,
  red = *media folder not found*. The check runs on a thread once the field loses focus or a
  button changes it (not on every keystroke: it loads the car DB for the count).

Changing the path restarts the Debug tab's car-DB load. *Why per-machine:* an install path is
specific to a computer, so it is excluded from profile export/import (like
`input_perm_dont_remind`). Code: `game_install_card` in `src/ui/settings.rs`.

## Map data

The **Map data** card (right column, directly below Game Install, same platforms) opens the road-type map editor in the browser, built from the install above, and shows which road types the app uses (project data or the user's saved file), the editor's state and its last Save. Buttons: **Open map editor** with a *Start from* Current / Raw choice, **Open data folder**, **Reset road types to project data**, **Rebuild map data** and **Contribute**. It adds no config keys. Full detail and the rules (override replaces the project file, Rebuild never deletes the override): [[map-editor]].

## Save

Settings save automatically — there is no Save button. A change is written within ~1 s
(`config::AutoSave`, polled once per frame in `ForzaApp::update`; also on a clean exit and at the
explicit save points such as closing Mini-Settings or switching profile). While a check is held
back by the 1 s interval, `AutoSave::recheck_in` makes `update()` call `request_repaint_after`, so
one more frame arrives when it ends. *Why:* egui is reactive; without it an edit made <1 s after
the last check sat unsaved until the next input. Idle cost is nil (no change, no wake-ups). While
the window is minimised redraws may not be delivered, so the save is not guaranteed then.

*Why a content-compare autosave:* most Setup edit sites (the Window Detection text box, the
Detect button, ports, sliders) just mutate `app.config.*` and never called `save()`, so a value
only reached disk when Mini-Settings closed or the app exited gracefully — closing any other
way (launcher kill, compositor close, SIGTERM, crash) lost it ("Window Detection title never
saves", bug report). A per-site `save()` would have to be remembered for every new widget; comparing
the config's JSON once a second needs no cooperation from the edit site, and typing saves at
most once per second instead of per keystroke.
