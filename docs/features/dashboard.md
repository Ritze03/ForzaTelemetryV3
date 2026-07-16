# Dashboard — widgets & edit mode

The Dashboard tab is a resizable grid of draggable widgets, each showing a slice of
live telemetry. Widgets are placed on a `grid_cols` × `grid_rows` cell grid (both
configurable, up to 40); each widget occupies a rectangle of cells (`col`/`row` +
`col_span`/`row_span`) and layouts persist in `config.json` (and travel with
[[coop|presets]] via `dashboard_widgets` in `LAYOUT_KEYS`).

## Edit Mode

Toggle **Edit Mode** from the cog "mini-settings" popup → **Dashboard → General**
(a pencil-icon button). While active:

- Each widget gets a **center square handle** — drag it to move the widget; it
  snaps to the grid and won't overlap another widget's cells.
- Each widget's four edges get an **8px resize strip** — drag an edge to grow or
  shrink the widget by whole cells (a live "`w×h`" preview shows the new size while
  dragging).
- Empty cells are outlined so open grid space is visible.
- Widget content is rendered but disabled (no clicks pass through) so drag/resize
  gestures don't fight with widget interactions (e.g. the minimap's click-to-drop
  waypoint).

Outside Edit Mode the grid still optionally shows outlines/gridlines
(`dashboard_show_outlines` / `dashboard_show_grid`, forced on while editing).
**Reset Layout** restores the built-in default arrangement.

## Widgets

Available widget kinds (`WidgetKind` in `src/config.rs`), rendered by
`render_widget` in `src/ui/dashboard.rs`:

- **Speed** — current speed, big numeral (km/h or mph per Settings unit).
- **Gear** — current gear as a single large character (R / 1–9 / N).
- **RPM** — full-width shift bar: current RPM against the calibrated shift
  low-warning/shift-point thresholds, scaled to the effective max RPM (game-provided
  or dynamically detected redline, per `max_rpm_mode`).
- **Inputs** — Accel/Brake/Clutch/HandBrake bars plus a steering indicator; can
  optionally hide the Accel blip that Backfire's synthetic keypress causes.
- **Car** — the car's class/PI badge and drivetrain (FWD/RWD/AWD) label art.
- **Engine** — Power/Torque/Boost as current, max, or both, with an optional
  cylinder-count/"Electric" caption.
- **Position** — raw world position (X/Y/Z) and orientation (Yaw/Pitch/Roll).
- **Race / Sprint** — before a race position is reported, shows 0–100/100–200/…
  sprint segment times (incremental or absolute) from the built-in sprint timer;
  once racing, switches to race position, lap number, current/last/best lap and
  total race time/distance. Both views auto-fit their rows to the cell (width +
  height, clamped so text never shrinks below half size).
- **Tires** — per-tire temperature and slip, as tiles or bars (`tire_display_style`),
  laid out 1×4/2×2/4×1 depending on the widget's aspect ratio.
- **G-Forces** — a lateral/longitudinal scatter plot with a peak ring, plus optional
  Current/Peak text readouts (lat/long/vertical g).
- **Suspension** — per-corner suspension travel (compression or, inverted, ride
  height).
- **Map** — the live minimap; see [[minimap]].
- **Co-Op Players** — a live speed-bar leaderboard (name, distance, speed, gear)
  of everyone in the current [[coop]] session, sorted fastest-first; shows a
  placeholder when not in a session.
- **Speed Trace** — a hand-drawn rolling speed + RPM sparkline over the last ~30s
  of active driving time (pauses don't cost plot width).
- **Boost** — current boost (PSI or bar) with a session-peak tick, as a horizontal
  or vertical bar depending on the widget's aspect ratio, or a compact
  value-in-bar style.
- **Session Stats** — top speed, peak power/torque, peak boost, peak lat/long G,
  and max RPM recorded so far this session.
- **Power Graph** — live RPM vs Power/Torque plot (falls back to the last saved
  Power Curve capture when no run is in progress), with an optional boost line.
  Its Dashboard → **Graphs** mini-settings carry the widget's own toggles (Show
  Boost / Compact / Show grid) plus the full [[power-curve]] tab's capture options
  (RPM step size, forced-induction detection, save-FI-state) — they share config.
- **Boost Graph** — RPM vs boost bar chart from the same capture data.

## Enabling / disabling widgets

The mini-settings **Dashboard → Modules** sub-tab lists every widget kind as a
checkbox: unchecking one adds it to `disabled_modules` (hidden from the grid
without losing its saved position/size). **Position** is disabled by default.
Right-clicking a module's checkbox re-parks it below the rest of the layout
(useful after re-enabling one that no longer fits).

## Per-widget options

The cog "mini-settings" popup (`DashboardSubTab` in `src/app.rs`) has one sub-tab
per concern: **General** (edit mode, grid size, grid/outline/title visibility),
**Modules**, **Km/h**, **Gear**, **RPM**, **Sprint**, **Tires**, **Suspension**,
**Shift**, **Engine**, **G-Force**, **Inputs**, **Boost**, **Power Graph**,
and **Map** — each tunes the corresponding widget(s) above. (Config export/import
moved out of here into the Settings → Profiles card — see [[profiles]].)
