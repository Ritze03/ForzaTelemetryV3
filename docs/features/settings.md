# Settings — network, units, display

The **Settings** tab (`Tab::Settings`, labelled **Setup** in the tab bar) holds the app-wide
options that aren't tied to a single dashboard widget. Rendered from `src/ui/settings.rs` as
bordered category cards laid out per the [styling guide](../ui/STYLING-GUIDE.md)
(label-left / control-right rows); values persist in `config.json` (see
[[state-and-config]]). Per-widget tuning lives in the cog **Mini-Settings** popup instead —
see [[presets]]. The in-game HUD overlay has its own **Overlay** tab — see [[overlay]] (module tabs). Both maps' settings (layers, view options, co-op, FPS limit, image quality / cache) and the map editor live on the **Map** tab — see [[map-tab]]; **Mini-Settings has no map settings** (D79/D90: the user wanted them all in one place; even the Dashboard's *Map* module switch is on the Map tab now), only the per-widget and per-tab options of the other tabs.

Cards: **Profiles**, **Hotkey**, **Controller** (see [[gamepad]]), **Display** (left column); **Getting started**, **Repository / Credits**,
**Network**, **Game Install**, **Input Permissions** (Linux only), **Window Detection** (right column). Hotkeys and window detection get
their own doc — see [[hotkeys]]; the Profiles card gets its own doc too — see [[profiles]].

## Getting started

First card of the right column: one **Open setup guide** button that re-opens the first-run
onboarding guide (I17) at its first step; see [[onboarding]]. The guide itself reuses the Game
Install, Input Permissions and Window Detection cards' logic.

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
- The detection method and game window title (with **Detect**), always shown, and the
  **Focus check rate**. *Why always (D101):* global hotkeys and controller buttons fire only
  while the detector says the game window is focused, so a user stuck on amber *Game window
  not focused* must always be able to see and fix the method / title. The **Active if**
  dropdown (Telemetry live / Game window focused) is gone: hotkeys always use the window
  detection now ([[hotkeys]]). The *Game Window Title* label has a tooltip saying hotkeys and
  controller buttons only work while that window is in front (no under-option text, D21);
  the first-run guide's Window Detection step says the same in its intro.
- A status dot with three states: green *Game window focused*, **amber** *Game window not
  focused* (a normal waiting state, not an error; also shown while this app's own window is in
  front, since that is not the game for hotkeys), red *Focus detection failed* (hotkeys then
  work everywhere, fail-open). It shows while the detector runs (a global hotkey / pad binding
  exists, the input gate or the overlay is on). The colours are the theme's `GOOD` / `WARN` /
  `DANGER` tokens.
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

The **Map data** card moved to the **Map tab** in D67 (Settings → Map data; Setup was bloated, the user wanted the map editor with the maps), see [[map-tab]]. It opens the road-type map editor in the browser, built from the install above, and shows which road types the app uses (project data or the user's saved file), the editor's state and its last Save. Buttons: **Open map editor** with a *Start from* Current / Raw choice, **Open data folder**, **Reset road types to project data**, **Rebuild map data** and **Contribute**. It adds no config keys. Full detail and the rules (override replaces the project file, Rebuild never deletes the override): [[map-editor]].

## Loading and recovery (config.json, profiles)

`AppConfig::load` never throws the whole config away over one bad value. Order: read the file →
fill keys missing from it with code defaults → migrations → `from_value_lenient`.

- **Per-key fallback:** if the config does not deserialize as a whole, every top-level key is
  probed on its own (defaults + that one key). A failing key is reset to its default. If the
  failing key is an object (`overlay`, `minimap_layers`, `hotkeys`…) its direct fields are probed
  the same way, so only the bad field resets (e.g. `minimap_layers.race_lines` when its `mode`
  is `"Off"` instead of `"off"`), not the whole group. Granularity: top-level key, or its direct
  field. Unknown keys are ignored as before.
- **Element-level recovery for lists:** if a top-level list has unreadable elements (typically a
  removed `WidgetKind` in `dashboard_widgets`), each element is probed alone (`[elem]` must
  deserialize) and only the bad ones are dropped (`dashboard_widgets[3]` in the log);
  `inject_missing_widget_kinds` then re-adds any kind that is now missing, so the rest of the
  dashboard layout survives. Fixed-size arrays fail every single-element probe and reset as a
  whole key, as before. *Missing* fields of a nested object (e.g. `accel_gamma` in a saved
  `dsg_tuning_*`) are filled from the code default silently — not reported, no backup — and the
  saved sibling fields are kept. *Why not struct-level `serde(default)` on `GearboxTuning`:* its
  `Default` would be one value for all three modes, but their defaults differ; filling from the
  `AppConfig` default per key gives the right one.
- **Backup before overwrite, save after:** if the file is not valid JSON / UTF-8, can't be read,
  is not usable as a whole, or its `active_profile` is unreadable, the original is copied to
  `config.json.bad-<unix seconds>` (`-2`, `-3`…
  on a clash) **before** anything can write, and the repaired config is then saved right away,
  so killing the app before the first change doesn't pile up another `.bad-` copy on every start
  (if a backup could not be written, nothing is saved). The reset keys are logged with
  `eprintln!`. The app never deletes these backups. A merely *unreadable value* (some other
  reset key) makes **no** backup and no rewrite since the forward-compatibility merge below
  keeps it in the file (a `.bad-` copy on every start of an older build would pile up).
- **Invalid UTF-8:** `config.json` and profiles are read as bytes and decoded lossily; the
  replacement characters only damage the strings they sit in, everything else is salvaged. The
  file is backed up either way.
- **Active-profile rule (config.json unusable as a whole):** `save()` mirrors the live config
  into `profiles/<active>.json`, so that file still holds the last good state. If `config.json`
  isn't valid JSON, isn't an object, or can't be read, `load()` takes the previous active
  profile's name from a plain text scan of the broken file (`"active_profile": "…"`, works on
  invalid JSON) or else the most recently modified `profiles/*.json`, and loads that file through
  the same lenient path instead of defaults. If `active_profile` alone is unreadable the rest of
  `config.json` is kept and only the name is guessed this way. *Why:* previously the broken file
  gave defaults with `active_profile = "Default"`, and the first save overwrote the user's real
  `profiles/Default.json` with defaults — and only `config.json` had been backed up.
  **In any recovery** (including a mere reset key) the active profile's file is also copied to
  `<name>.json.bad-<ts>` before the first `save()` rewrites it, so no profile content is ever
  overwritten without a copy.
- **Profiles** (`profiles/<name>.json`) load through the same lenient path
  (`apply_profile_file`): bad values keep the live config's value, an unparsable file is skipped
  and backed up as `<name>.json.bad-<ts>` first (the `.bad-` suffix keeps it out of
  the profile list, which lists `*.json`). Unreadable *values* are kept in the file (see
  *Forward compatibility*), so they make no backup. A profile that can't be read as text at all
  (read error) is backed up and skipped; invalid UTF-8 is salvaged and backed up. Preset / import
  overlays use the same lenient merge.
- **Gearbox calibrations** (`automatic-gearbox-saved-calibrations.json`) parse per car entry; a
  broken entry is not applied but stays in the file (forward compatibility); a file that is not
  a JSON object is backed up and replaced.

*Why:* a single value serde could not read (a renamed or differently cased enum, a number where
a bool is expected, a file written by another version) made `load()` fall back to defaults for
**everything**, and the ~1 s autosave then overwrote `config.json` with them: all settings lost
silently. There is no in-app notice yet (the app has no toast mechanism); the stderr log and the
`.bad-` file are the only trace.

## Forward compatibility (an older build must not eat a newer build's settings)

*Why:* the user switches between builds. An older build used to drop every key it did not know
(serde ignores them, `to_string_pretty(self)` never wrote them back) and to reset every value it
could not parse (a new enum variant such as a new view mode), then wrote that to `config.json`:
going back to the newer build had lost those settings. It must never throw away what a newer
build wrote. **This protects only from this version on**: builds released before keep their old
behaviour on load.

How (`config.rs`, `merge_for_save`): the save is a three-way merge of JSON trees, not
`#[serde(flatten)]` on the ~65 config structs (invasive, and flatten interacts badly with the
lenient per-key probe).
- At load the config keeps (in `AppConfig.fwd`, a `#[serde(skip)]` field behind a mutex because
  `save` takes `&self`; clones carry it) **`raw`** = the file's tree after the migrations and
  **`loaded`** = `to_value(config)` right after the load, plus **`resets`** = the paths the
  lenient load could not read (`from_value_lenient`'s `reset_keys`).
- At save, `cur = to_value(config)` is merged with them, walking objects recursively: an object
  key in `raw` but in neither `loaded` nor `cur` is unknown to this build and **kept**; a leaf
  under a reset path that the user did not change this session (`cur == loaded`) gets `raw`'s
  value back (an unreadable new enum value); anything else is `cur` (a changed value, a new key,
  and a map entry the user deleted this session: it is in `raw` and `loaded`, so it is not
  "unknown" and stays deleted). Arrays are taken whole (`cur` if changed, else `raw` where a reset
  touched them).
- After a successful write, `raw` := what was written and `loaded` := `cur`, so the next save
  diffs against the file as it is now.
- With nothing to keep the save is plain `to_string_pretty(self)` (struct field order, as
  before); a merged tree is written key-sorted.
- A profile load (`apply_profile_file`) is a fresh load: `raw` is the profile file's tree, so
  switching profiles carries the right unknown keys. A preset import (`import_selected`,
  `apply_preset`) keeps the loaded file's trees. A clone keeps them.
- `resets` is why the merge does not restore *every* difference between `raw` and `loaded`:
  values this build normalizes on purpose (injected widgets, an aliased old enum spelling, the
  trip `nav.destination` that a profile switch deliberately keeps) must be written as this build
  has them.

Rules that follow for anyone changing config structs:
- **A migration that reads an old key must remove it from the raw tree** (`normalize_raw`:
  `compact_tabs` is removed after it set `top_bar_style`; `circuit_color` is renamed to `color`
  in the raw tree, because the serde alias would otherwise leave both keys, which serde rejects).
  A kept `compact_tabs` would otherwise re-run on every load and overwrite the user's choice.
- **A field you remove goes into `RETIRED_KEYS`** (dotted path). Without it an older key is
  "unknown" and would be written back forever. `strip_retired` runs on every raw tree (config,
  profile, preset). The unit test `the_embedded_default_config_has_nothing_unknown` catches a
  retired key in `assets/default-config.json`.
- A new enum variant needs nothing: an older build keeps the new value in the file.

Known limits: the user changing a value that this build could not read counts as their choice
(the new value wins; the old unreadable one is gone); setting a field back to the default this
build substituted for an unreadable value cannot be told from "untouched" and keeps the file's
value; a changed array replaces the file's whole array (a widget kind only a newer build knows is
lost once the user moves a widget); a hand-edited garbage value now stays in the file until the
user changes that setting (it is logged on every start).

Other files: `automatic-gearbox-saved-calibrations.json` uses the same merge
(`save_car_calibrations`: unknown fields of an entry and entries this build can't read stay, a
car removed in the session is dropped). Not covered, on purpose: the road-type override
(`map_editor/fh6-road-types.user.json`, a versioned export format that rejects other versions and is
written only by the editor's Save), and caches (map tiles, terrain, car data, the editor's port
file, the cloudflared binary), which are regenerated.

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
