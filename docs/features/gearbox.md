# Automatic Gearbox — DSG-style auto-shifter

A DSG-style auto-shifter that drives the game's manual up/down-shift keys
(`E` / `Q`) for you, calibrated live from telemetry rather than from a fixed
gear-ratio table.

## How it works

- **Calibration first.** The box stays completely hands-off until you do one
  manual full-throttle pull to redline and shift up yourself — from **any**
  gear, not just 1st. That first manual upshift "engages" the box and lets the
  redline detector lock the peak RPM it uses from then on. *(Why no first-gear
  requirement: races that start rolling, or spawning in a high gear, never gave
  a clean 1st-gear pull — so engagement now keys off the first manual upshift
  wherever you are.)* From there, every gear you drive through gets continuously
  calibrated: past 60% of the detected redline, with little tire slip, springs
  loaded, and moving in a straight line, it extrapolates that gear's
  speed-at-full-redline and keeps a rolling median of the last 10 samples — so a
  bad sample self-corrects instead of locking in.
- **Calibration messages on the HUD** (when notifications are on): `Calibration
  started` on a reset or new uncalibrated car, then `Shift at redline` (yellow
  dot) as soon as gear 1 has gear-map data while still uncalibrated, then
  `Calibration done: N rpm` once you shift and the box engages. The middle one
  fires once per cycle and re-arms on a car change, Clear RPM calibration or
  Clear gear map. See [[overlay]] (Notifications).
- **The redline detection and engagement run with the box switched off too**
  (`worker.rs` tracks `dynamic_max_rpm` unconditionally; `DsgListener::update`
  sets `engaged` before its `dsg_enabled` gate). The in-game HUD reuses them:
  its shift cue is this box's Shift RPM on the detected redline. See [[overlay]].
- **Clear RPM calibration** and **Clear gear map** — two buttons (Automatic
  Gearbox tab; always shown, each disabled until it has something to clear).
  *Clear RPM calibration* forgets the detected redline + engagement (the box
  goes hands-off until your next manual upshift re-locks it) and keeps the gear
  map; it also pushes the `Calibration started` HUD notification. *Clear gear
  map* forgets the per-gear speed map and keeps the redline. Each also has a
  global hotkey / controller action of the same name (see [[hotkeys]],
  [[gamepad]]): **Clear RPM calibration** is `HotkeyAction::ResetCalibration`
  (default `F`; the serde name is kept from the old "Reset RPM Calibration" so
  saved bindings still load), **Clear gear map** is `HotkeyAction::ClearGearMap`
  (no default key). Both rewrite the saved per-car profile to match
  (`persist_calibration`; removed once nothing is left), so a car reload can't
  restore what you just cleared. Implemented as `Command::ClearRpmCalibration` /
  `Command::ClearGearMap` and the matching hotkey arms in `listeners/worker.rs`.
  *Why two again:* the user wants to choose the half to clear, and, since one
  key or pad button can be bound to several actions, to bind both to one button
  to get a full reset. (They were briefly merged into one "Clear gearbox
  calibration".)
- **Shift decision**, in order, each packet:
  1. **Hard redline upshift** — once RPM reaches **Shift RPM** (% of the
     detected max RPM) and road speed has reached **Upshift min. speed** (%
     of that gear's calibrated top speed, to reject wheelspin rev spikes).
  2. **Cruise upshift** — eases into a taller gear once it would still sit at
     or above a throttle-demanded target RPM, so light-throttle cruising
     settles into tall gears instead of revving out every gear.
  3. **Downshift** — once RPM falls below a demand-based down-point, drops to
     the deepest gear the **Powerband buffer** (or **Kickdown powerband
     buffer**, on a full-throttle kickdown) allows without landing too close
     to the limiter.
  - A **wheelspin guard** holds the current gear whenever engine RPM implies
    far more speed than the wheels are actually making (spinning wheels), and
    an **airborne guard** holds the gear while all four wheels are near max
    suspension stretch (car's in the air).
  - **Accelerator gamma** reshapes the pedal the box reacts to
    (`effective = pedal^gamma`) before any of the above — this is what makes
    Street/Sport feel progressive instead of an on/off full-throttle switch.
- **Shift execution** is a small state machine: it fires all the key
  presses for a multi-gear kickdown up front, then waits for the *final*
  gear to actually appear (or a ~500 ms+ timeout, extended per extra gear in
  a batched shift) before commanding anything else — this avoids key spam and
  tolerates the brief neutral flash some cars show mid-shift. A timed-out
  shift is treated as a desync: the box accepts the gear it actually landed
  in and pauses briefly before commanding again.
- **Modes** — **Manual**, **Street**, **Sport**, **Race** — each has its own **Cruise
  RPM** and **Accelerator gamma** tuning. Race ignores the cruise/downshift
  settings entirely: it always wants the full powerband and only upshifts at
  the redline. **Auto Race mode in races** forces Race mode whenever you're
  in an actual race (race position ≥ P1), then reverts to your chosen mode
  back in free roam.
  - **Manual** (D25) — the box never shifts and the HUD treats it as the gearbox
    off (no drive-mode letter). With **Auto Race mode in races** on it still flips
    to Race in a race and falls back to Manual after. *Why:* the user drives manual
    in free roam and wants the auto gearbox only in races. The effective mode is
    re-derived from config + race/drift state on every packet (stateless), so
    "reverting" is automatic for any selected mode. Manual is serialized by name,
    so existing configs are unaffected. Manual has no tuning of its own (the
    Advanced sliders edit Sport's while it's selected).
  - **Disable in drift events** (D30) — with `dsg_disable_in_drift` on, a detected
    drift event turns the box off (no shifting, HUD shows it as off) until the event
    ends. Only applies when Auto Race mode is on or Race is the selected mode.
    *Why:* drifting wants the driver's own gear choice; the user's condition was
    scoping it to the race-oriented setups. Detection reuses the HUD's
    `ModeClassifier` (see [[overlay]], *Race vs drift detection*); `DsgListener`
    runs its own instance fed every packet, because the HUD tracker's copy only
    runs while the overlay is enabled. The pure decision is
    `AppConfig::dsg_resolved_mode(in_race, in_drift) -> Option<GearboxMode>`
    (`None` = off), used by both the shift loop and the HUD gear label.
- **Calibration persistence** — with **Remember calibration per car** on,
  each car's measured gear speeds and detected redline are saved to
  `automatic-gearbox-saved-calibrations.json` in the app data dir (keyed by
  car), and reloaded automatically next time you get in that car, skipping
  the manual calibration pull. The file is written on every car change and once
  more when the app quits.
- **Runs off the frame loop.** The box lives on its own listener thread
  (`src/listeners/worker.rs`), not inside the UI's per-frame packet drain, so it
  keeps shifting while the window is minimized or fully covered. *Why:* on
  GNOME/Wayland a hidden window gets no frame callbacks, winit then stops calling
  `eframe::App::update` (it gates `RedrawRequested` on that callback,
  `winit wayland/event_loop/mod.rs:486`), and everything in it — including the
  shift logic — simply stopped. The thread owns the live `DsgListener` and the
  per-car calibrations; the Automatic Gearbox tab reads a copy of its state
  (`DsgView`) refreshed once a frame, and the **Clear RPM
  calibration** / **Clear gear map** buttons send it a command. A displayed value can therefore be
  one frame stale, which is fine — nothing shown is critical, and in exchange the
  shift loop is never affected by the UI. See [[overview]] for the mailbox design.
  If that thread ever dies, the status-bar Gearbox indicator reads **Stopped
  (error)** rather than a frozen *Active*.
- **Ignore Backfire input** keeps the shift logic (and the live throttle-bar
  visualization) reacting only to your real pedal, not the synthetic key
  [[backfire]] briefly presses to fake its pop.

## Using it

The bottom **status bar** carries a live indicator (visible from any tab): the
gearbox icon plus **Active** (green), **Deactivated** (red), or pastel-amber
**Uncalibrated** while it's enabled but hasn't engaged yet (before your first
manual upshift). Backfire sits beside it (Active/Deactivated), separated by a
divider. The **General** mini-settings page has a *Status bar: show text
labels* toggle to collapse both down to just the icons — the same toggle also makes the
left side (connection status, Co-Op indicator) icon-only; see [[coop]].

Open the **Automatic Gearbox** tab — controls on the left, a live
visualization on the right.

- **General** — **Enabled**, **Ignore Backfire input**, **Shift RPM** and
  **Upshift min. speed** sliders, the **Gearbox mode** dropdown
  (Manual/Street/Sport/Race), **Auto Race mode in races**, **Disable in drift events**, **Remember calibration
  per car**, and the **Clear RPM calibration** / **Clear gear map**
  buttons (see above).
- **Advanced Settings** — a **Reset settings** button (resets the sliders
  below to a tuned baseline; leaves modes/toggles alone), **Accelerator
  gamma**, **Gear overlap** (Race only), and — hidden in Race, since Race
  ignores them — **Cruise RPM**, **Kickdown cooldown**, **Downshift
  deadzone**, **Full throttle threshold**. **Powerband buffer** always shows;
  **Kickdown powerband buffer** is hidden in Race.
- **Debug** (only shown once **Debug** is enabled via the status-bar cog's
  mini-settings) — live decision state (engaged/current/target gear,
  detected redline, upshift RPM, kickdown cooldown countdown, desync count)
  and a **Log shifts to CSV** toggle that appends every shift to
  `dsg_shift_log.csv` in the app data dir (cleared on each launch). The file write
  happens on a separate writer thread, so logging can never delay a shift; a row is
  dropped rather than made to wait.
- **Live visualization** (right column): **State** (big gear readout, target
  gear, active mode + decision rule, engaged/idle indicator, an RPM bar with
  down-point/target/shift-point markers), **Gear Map** (a stacked chart of
  each calibrated gear's real speed range with the live speed marked),
  **Accelerator** (the gamma curve plus a translucent overlay of which gear
  the box would pick at each pedal position, at the current speed), and
  **Inputs** (throttle/brake bars plus SPIN / KICK / DESYNC status lamps).

## Options

| Config key | Default | Meaning |
|---|---|---|
| `dsg_enabled` | off | Master toggle. |
| `dsg_shift_rpm_pct` | 98% | Redline upshift point, as % of detected max RPM. |
| `dsg_upshift_speed_pct` | 80% | Minimum % of a gear's calibrated top speed before a redline upshift can fire. |
| `dsg_gearbox_mode` | Sport | `Manual` / `Street` / `Sport` / `Race`. |
| `dsg_auto_race_mode` | on | Force Race mode whenever an actual race is detected (race position ≥ P1). |
| `dsg_disable_in_drift` | off | Gearbox off while a drift event is detected (needs Auto Race mode on or Race selected). |
| `dsg_tuning_street` / `dsg_tuning_sport` / `dsg_tuning_race` | cruise 35% / 50% / 85%, gamma 1.0 each | Per-mode `{ cruise_rpm_pct, accel_gamma }`. |
| `dsg_kickdown_cooldown_secs` | 5.0 s | How long the lower gear is held after a full-throttle kickdown once you lift off. |
| `dsg_downshift_deadzone_pct` | 60% | Highest the part-throttle rev target climbs to, as % of the shift point. |
| `dsg_full_throttle_pct` | 95% | Throttle % where the box switches from economical to full-powerband behaviour. |
| `dsg_race_gear_overlap_pct` | 10% | Race-only: extends each gear's range downward by this many %-points of max RPM. |
| `dsg_downshift_powerband_buffer_pct` | 20% | Headroom a downshift must leave below the shift point (as % of the inter-gear RPM jump). |
| `dsg_kickdown_powerband_buffer_pct` | 0% | Same, but for full-throttle kickdowns (usually smaller, so it drops deeper). |
| `dsg_save_calibration` | off | Persist per-car calibration and auto-load it next session. |
| `dsg_ignore_backfire_accel` | on | Shift logic ignores accel while a Backfire pop is firing. |
| `dsg_debug` | off | Show the live decision state in the Debug card. |
| `dsg_show_debug_panel` | off | Whether the Debug card is available at all (toggled from the status-bar cog). |
| `dsg_log_shifts` | off | Append every shift to `dsg_shift_log.csv`. |

Only `dsg_show_debug_panel` (and the adjacent `inputs_filter_backfire_accel`)
are in `MINISETTINGS_KEYS`; every other Backfire/DSG setting is local and
doesn't travel with exported presets or dashboard layouts.

Related: [[backfire]].
