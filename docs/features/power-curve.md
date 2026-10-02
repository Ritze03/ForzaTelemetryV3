# Power Curve — live RPM vs power/torque

The **Power Curve** tab plots power and torque against RPM in real time while you hold
full throttle, plus a boost-vs-RPM bar chart for forced-induction cars. Useful for
seeing where an engine makes its power, comparing before/after a tune, or checking a
turbo's spool point.

## How it works

- Every incoming packet is fed to a capture (`src/listeners/power_capture.rs`). A point
  is only recorded when the car is **moving** (`speed > 0.1`) and **at full throttle**
  (`Accel >= 245` out of 255) — light throttle or a stationary rev would show
  artificially high figures, so those are skipped.
- The current RPM is snapped into a **bucket** (`power_curve_step`, default 100 RPM) and
  each bucket keeps only the **highest** power/torque/boost value seen there, so a
  slightly noisy pull still produces a clean curve.
- Power is converted from the packet's raw Watts to **PS** (÷ 735.499); torque is the
  packet's Nm value as-is; boost is the packet's PSI value (converted to bar for display
  if you use bar units — see below).
- Switching cars clears the capture automatically.

## Using it

- **Clear live** — wipes the current in-progress curve and count.
- **Save reference** — freezes the live curve as a translucent "Saved" overlay (separate
  power/torque/boost series) so you can capture a second run (e.g. after a tune or engine
  swap) and see both curves at once. Also clears live so you can start the next run
  immediately.
- **Clear reference** — removes the saved overlay.
- Middle-click either chart to snap it back to auto-fit bounds.
- The **Power & Torque** chart's RPM axis runs from 0 to the highest recorded RPM (live
  or saved) + 1000; before any data exists it falls back to the car's measured max RPM
  (or 8000 if that isn't known yet).
- The **Boost** chart only appears if there's positive boost data to show (see below),
  and its bars turn a darker shade wherever the live run is lower than the saved
  reference at the same RPM (or vice versa), so you can spot exactly where one run
  gained or lost.

## Options

Set from the tab's cog / mini-settings popup (`Tab::PowerCurve`, labelled **Power Graph**).
The same three options also appear under Dashboard → **Graphs** in the dashboard's
mini-settings, since they drive the same `power_curve_*` config fields the dashboard
Power Graph widget reads — one control set, rendered by `power_curve::options_ui`:

- **RPM step size** — bucket width for the boost bar chart, 25–500 RPM (default 100).
- **Forced induction detection** — when ON, boost is hidden until real boost
  (> `FI_BOOST_THRESHOLD_PSI` = 1 PSI) has actually been captured, so naturally-aspirated
  cars don't show a boost chart. When OFF, boost always shows. Applies to this tab's
  Boost chart and to the Dashboard's Power Graph boost line and Boost Graph module.
- **Save Forced Induction State** — (only shown when detection is ON) once boost has
  been detected for the current car, keeps boost visible even after "Clear live"
  wipes the data — so clearing between pulls doesn't hide the chart again.

### Forced-induction detection internals

One rule, `boost_visible()` in `src/listeners/power_capture.rs`, used by all three
boost plots: `!detection || any plotted series has a point > 1 PSI || (save_state &&
latched)`. The latch is `PowerCapture::fi_detected()` — set by any race-on packet with
boost > 1 PSI (no throttle/speed requirement), kept across `clear()` ("Clear live",
brake+handbrake), reset by `on_car_changed()`. It is per *current car* only (a car
you switch back to must be re-detected). Unit tests in the same file cover NA vacuum,
turbo spool, clear-vs-car-change and detection OFF.

- **Why 1 PSI:** the packet's `Boost` is manifold pressure relative to atmosphere. An NA
  engine sits in vacuum (negative) and only reaches ~0 at full throttle; a turbo or
  supercharger goes several PSI positive under load. The old 0.05 PSI cutoff left no
  margin for an NA car's near-zero WOT reading.
- **Why judge the plotted series:** the Dashboard modules plot live data, falling back to
  the saved reference. Visibility used to also pass if the *saved* reference had boost,
  so a saved turbo run made an NA car's live vacuum data show. The Power Curve tab plots
  live and saved together, so it passes both.
- **Bug fixed (0.4.1):** the Dashboard Power Graph's optional boost line (*Show Boost*)
  never consulted detection at all, so it was drawn (flat at 0, with a boost axis) for
  every car.
- **Boost / pressure unit** (bar vs PSI) is a global unit setting on the main
  [[settings]] page, not per-tab — it also affects other boost readouts in the app.

## Notes

- The capture is in-memory only — closing the app or switching cars discards it unless
  you've hit "Save reference". There's no way to export a captured curve to a file.
