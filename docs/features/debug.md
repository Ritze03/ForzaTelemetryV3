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

### Calibration checks

A section of the Derived card (above the Car block, `debug_tab::calibration_section`) that shows
every condition behind RPM calibration as its own row: a green/red `●`, the name, and the raw
value next to its threshold (e.g. `Tyre slip  FL 0.12 FR 0.10 RL 0.92 RR 0.09  |slip| < 0.8`).
Per-wheel checks (slip, suspension travel) are one row with the four wheels coloured
individually; the row's dot is "all four pass". Three groups, each with an overall dot
(`capturing now` / `engaged` / `sampling now`):

1. **Max RPM capture**: race on, power > 0, handbrake released, slip <= 0.5 per wheel; plus
   the max RPM captured so far.
2. **Calibrated (box engages)**: engaged yes/no, previous forward gear in 1..=9, current gear
   in 2..=10, upshift. (These three are per-packet, so they only light up on the shift itself;
   the *Engaged* row is the sticky result.)
3. **Gear-map sample**: race on, forward gear, redline known, speed > 5 km/h, RPM >= 60 % of
   redline, slip < 0.8, suspension >= 0.1, moving straight; then a compact per-gear table
   (samples in the rolling window of 10, and the median redline speed) for gears that have data.

Flow: pure check fns in `listeners/calib.rs` (`MaxRpmChecks`, `EngageChecks`, `GearMapChecks`,
bundled with the raw values as `CalibChecks`) -> `DsgListener.calib` (refreshed every packet,
also while paused) -> `DsgView.calib` / `gear_sample_counts` -> `ListenerView.dsg` ->
`ForzaApp.dsg` (read as `app.dsg.calib`). The real logic (`worker.rs` max-RPM capture,
`dsg.rs` engage and sampling) gates on the same fns' `all()` / `triggers()`.
*Why:* "RPM calibration isn't working for some cars" was undiagnosable; showing each check
separately says exactly which one fails, and routing the real logic through the same fns means
the panel can't disagree with the code. The gear-map checks are evaluated against the *captured*
redline, so before step 1 has succeeded the "Redline known" row is the failing one.

**Car DB loading.** `gamedata::cars::CarDb::load` blocks (~0.4-1 s cold, ~4 ms warm), so
`CarDbState::poll` (called each frame from `show`) spawns a thread on the first open of the tab and
collects the result through an `mpsc` channel (same pattern as the minimap loader), asking for a
repaint every 100 ms while loading. It reloads if the UI language changes (`i18n::language_code()`:
EN/DE, the game's `<LANG>` codes; `load` itself falls back to EN when a zip is missing). States:
loading / failed (install not found, with the error text) / ready. *Why lazy, not at startup:* most
sessions never open the Debug tab, and the cold scan reads the whole install.
