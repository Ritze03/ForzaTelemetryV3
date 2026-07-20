# Backfire — synthetic anti-lag / throttle-blip

Simulates the throttle-blip pop-and-bang effect FH6 doesn't do on its own, by
briefly tapping the accelerator key at the right moment. The game reads it as
a real (tiny) throttle input and produces its own backfire/anti-lag sound.

## How it works

- Every packet, `BackfireListener::update` checks a set of conditions and, if
  all are true, sends a synthetic **W** key-press through the same input path
  used by the Automatic Gearbox (`InputSender` — `evdev`/uinput on Linux,
  `enigo` on Windows):
  - Off throttle (`Accel == 0`) and not braking.
  - Current RPM inside the active RPM range.
  - Not currently accelerating (speed isn't rising) — a fresh lift-off, not
    mid-pull.
  - At least **RPM interval** RPM below the last pop, so it doesn't
    machine-gun.
  - Not drifting — no wheel's slip-ratio magnitude is above `1.1`
    (`DRIFT_SLIP_MAX`). A slide or wheelspin isn't a clean lift-off, so the pop
    is suppressed while it lasts. Gated behind **Drift detection**.
- **RPM range** is either fixed (Minimum RPM / Maximum RPM absolute values) or
  **Dynamic RPM** — a percentage of the car's detected max RPM (Minimum RPM /
  Maximum RPM as %), so one setting works across every car.
- **Key-press duration** is either a fixed number of ms, or **Dynamic key
  press duration**, matched to the game's own frame length:
  - *Time-based* — estimated from the current packet rate (`1000 / pps`,
    clamped 4–40 ms).
  - *Packet-based* — holds the key down until the very next packet arrives,
    an exact one-frame tap (the default).
- An **echo window** tracks how long the game's telemetry may still reflect
  the fake key press after release. While it's open, the listener ignores
  that self-inflicted accel blip instead of mistaking it for the driver
  lifting back on the throttle — so backfire pops can't retrigger each other.
- **Disable if standing still** skips the effect below ~1 km/h. **Test mode**
  bypasses the throttle/RPM/speed conditions entirely, useful for confirming
  the key press and game reaction work at all.
- The synthetic key press requires the app's input backend to work: on Linux
  that means `/dev/uinput` access (the running user must be in the `input`
  group); on Windows it goes through `enigo`. If the virtual device can't be
  created, backfire silently does nothing.

## Using it

Open the **Backfire** tab:

- **General** — **Enabled** turns the whole feature on or off. A hint notes
  that Backfire only works in Online Mode.
- **RPM Range** — **Dynamic RPM** toggle; when on, **Minimum RPM**/**Maximum
  RPM** are shown as % of the car's max RPM (with a live "Range: X – Y RPM"
  readout below); when off, they are absolute RPM values. **RPM interval** sets
  the minimum RPM drop between two pops.
- **Key Press** — **Dynamic key press duration** toggle with a **Time-based
  / Packet-based** mode dropdown; when off, a fixed **Key press duration**
  slider (ms) appears instead.
- **Conditions** — **Disable if standing still**, **Drift detection (no pop
  while sliding)**, and **Test mode (ignores throttle/RPM conditions)**.

## Options

| Config key | Default | Meaning |
|---|---|---|
| `backfire_enabled` | off | Master toggle. |
| `backfire_dynamic_rpm` | on | Use a % of max RPM instead of fixed RPM values. |
| `backfire_dynamic_min_pct` / `backfire_dynamic_max_pct` | 60% / 95% | Dynamic RPM range as % of max RPM. |
| `backfire_min_rpm` / `backfire_max_rpm` | 4000 / 8000 | Fixed RPM range (used when Dynamic RPM is off). |
| `backfire_interval_rpm` | 100 | Minimum RPM drop between two pops. |
| `backfire_dynamic_duration` | on | Derive the key-press length from the frame rate instead of a fixed ms value. |
| `backfire_dynamic_mode` | Packet-based | `TimeBased` or `PacketBased` — how the dynamic duration is derived. |
| `backfire_accel_time_ms` | 8 ms | Fixed key-press duration (used when Dynamic key press duration is off). |
| `backfire_disable_standstill` | on | Suppress the effect below ~1 km/h. |
| `backfire_drift_detection` | on | Suppress the pop while any wheel's slip-ratio magnitude exceeds `1.1` (a slide/wheelspin). |
| `backfire_test_mode` | off | Ignore all throttle/RPM/speed conditions — always fires on lift-off. |
| `inputs_filter_backfire_accel` | on | Dashboard **Inputs** widget shows Accel as 0 while a backfire pop is actively firing, so the synthetic tap doesn't show up as a real pedal input. |

None of the Backfire settings are part of `MINISETTINGS_KEYS` or
`LAYOUT_KEYS`, so they don't travel with exported presets or dashboard
layouts — they're per-install.

The [[gearbox]] feature has its own **Ignore Backfire input** option so the
auto-shifter's shift logic doesn't react to Backfire's synthetic key either.
