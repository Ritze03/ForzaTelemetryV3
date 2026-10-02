# Debug

The tab is split into two columns (`ui.columns(2)`): **left** the raw packet (below), **right** the
"Derived from telemetry" card (see the end of this page).

A top-level tab just left of Setup (bug icon, `icons::BUG` = fa-bug U+F188). It shows every field of
the latest received `ForzaPacket` (`app.telemetry.latest`) raw, as a live, scrollable,
monospaced **name → value** grid inside one "Raw Telemetry" card, plus a **Copy** button
that puts the same `name: value` lines on the clipboard (handy for bug reports). With no
packet yet it shows "No telemetry yet".

## Where it lives

- `src/ui/debug_tab.rs` — `show()` and `fields(pkt)`.
- Wired in `src/app.rs` like every tab: `Tab::Debug`, `tab_title`, `max_pill_width`'s
  `TABS`, the tab bar's `right` array (second entry; the array is laid
  out right-to-left, so the bar reads "What's New | Debug | Setup" and Setup keeps the
  far-right edge users expect), and the `CentralPanel` dispatch. No mini-settings page.

## How the fields are enumerated

`fields()` formats the packet with its derived `{:#?}` and turns each `name: value,` line
into a row, in struct declaration order. Floats (a value containing `.`, `e`, `NaN` or
`inf`) are re-printed with 3 decimals; integers as-is. Field names are the Rust field
names and stay untranslated (they're data).

*Why:* the user asked for a "super simple" page with all raw readings. A hand-written list
of ~85 fields would go stale the first time `packet.rs` changes; the Debug output can't
drift. A unit test checks there's one row per field.


## Derived from telemetry (right column, D24)

Display only, nothing consumes it yet. *Why:* the user wanted to "easily see whether it gets it
right" before the app relies on these values anywhere.

| Row | Source |
| --- | --- |
| Experimental pause detection | `config.experimental_pause_detection` |
| Paused / Pause reason | `debug_tab::pause_reasons`: the same checks as `listeners::hud::hud_paused` (race off, `engine_max_rpm <= 0`, zero yaw/pitch/roll, and `garage_paused` only when experimental is on), one line per rule that fired. A `debug_assert` keeps it equal to `hud_paused`. |
| In race | `race_position != 0` (the gearbox's own in-race test) |
| HUD mode | `app.hud_mode` (Race / Drift; `HudMode` has no separate free-roam variant, so non-drift reads "Race / free roam"). Flow: `DsgListener.hud_mode` (its own `ModeClassifier`) → `ListenerView::hud_mode` → `ForzaApp::adopt_listener_view`. *Why the DSG's classifier:* it is fed every packet whether or not the overlay is on; the HUD tracker's copy only runs while the overlay is enabled. |
| Gearbox selected / effective / resolved | `config.dsg_gearbox_mode`, `dsg_effective_mode(in_race)`, `dsg_resolved_mode(in_race, in_drift)` with the real drift flag (`off` = None). |
| Calibrated max RPM | `app.dynamic_max_rpm` (the listener's per-car calibration, mirrored into the UI) |
| Season | `minimap::current_season()` (wall clock) |
| Car block | `CarDbState` (in `debug_tab.rs`, held in `app.debug_cars`): ordinal, language + whether served from cache, install path + car count, name (`display`), make, media name. |

**Car DB loading.** `gamedata::cars::CarDb::load` blocks (~0.4-1 s cold, ~4 ms warm), so
`CarDbState::poll` (called each frame from `show`) spawns a thread on the first open of the tab and
collects the result through an `mpsc` channel (same pattern as the minimap loader), asking for a
repaint every 100 ms while loading. It reloads if the UI language changes (`i18n::language_code()`:
EN/DE, the game's `<LANG>` codes; `load` itself falls back to EN when a zip is missing). States:
loading / failed (install not found, with the error text) / ready. *Why lazy, not at startup:* most
sessions never open the Debug tab, and the cold scan reads the whole install.
