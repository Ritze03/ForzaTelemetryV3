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
- **Reset calibration** — the **Clear calibration** button (Automatic Gearbox
  tab, always shown but disabled until there's a calibration) or the
  **Reset Gearbox Calibration** hotkey (default `F`, see [[hotkeys]]) wipes
  the calibration and engagement; the box goes hands-off until your next manual
  upshift re-learns it, and any saved per-car profile is forgotten.
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
- **Modes** — **Street**, **Sport**, **Race** — each has its own **Cruise
  RPM** and **Accelerator gamma** tuning. Race ignores the cruise/downshift
  settings entirely: it always wants the full powerband and only upshifts at
  the redline. **Auto Race mode in races** forces Race mode whenever you're
  in an actual race (race position ≥ P1), then reverts to your chosen mode
  back in free roam.
- **Calibration persistence** — with **Remember calibration per car** on,
  each car's measured gear speeds and detected redline are saved to
  `automatic-gearbox-saved-calibrations.json` in the app data dir (keyed by
  car), and reloaded automatically next time you get in that car, skipping
  the manual calibration pull.
- **Ignore Backfire input** keeps the shift logic (and the live throttle-bar
  visualization) reacting only to your real pedal, not the synthetic key
  [[backfire]] briefly presses to fake its pop.

## Using it

The bottom **status bar** carries a live indicator (visible from any tab): the
gearbox icon plus **Active** (green), **Deactivated** (red), or pastel-amber
**Uncalibrated** while it's enabled but hasn't engaged yet (before your first
manual upshift). Backfire sits beside it (Active/Deactivated), separated by a
divider. The **General** mini-settings page has a *Status bar: show text
labels* toggle to collapse both down to just the icons.

Open the **Automatic Gearbox** tab — controls on the left, a live
visualization on the right.

- **General** — **Enabled**, **Ignore Backfire input**, **Shift RPM** and
  **Upshift min. speed** sliders, the **Gearbox mode** dropdown
  (Street/Sport/Race), **Auto Race mode in races**, **Remember calibration
  per car**, and a **Clear calibration** button (wipes both the in-memory
  gear data and the saved per-car profile).
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
  `dsg_shift_log.csv` in the app data dir (cleared on each launch).
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
| `dsg_gearbox_mode` | Sport | `Street` / `Sport` / `Race`. |
| `dsg_auto_race_mode` | on | Force Race mode whenever an actual race is detected (race position ≥ P1). |
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
