# Changelog

All notable user-facing changes, newest first. Categories: **Added** (new
features), **Fixed** (bug/behaviour fixes), **Removed** (things taken out),
**Info** (notes worth knowing).

## [0.1.0] – 2026-07-13

### Added
- **Consistent card layout**: Backfire, Automatic Gearbox, Co-Op, and the Power Curve titles now share the same bordered cards with blue section titles and a uniform 8px gap between them. Backfire now uses the Gearbox's two-column control rows (label + slider + value) in a left-aligned column, and checkboxes in these cards share one fixed width.
- **Co-Op tidy-up**: the connection status now sits inside the Session card, the name and join-code fields fill the available width, and the colour swatch moved to the right of the hue slider.
- **Engine widget display modes**: choose Current, Max, or Both values per line in the mini-settings, and optionally show the engine type (Electric / cylinder count) underneath.
- **Engine text auto-fit**: the readout scales to fit the widget in both width and height, so it stays readable when the widget is small.
- **G-Force text toggle**: hide the text column so the plot fills the whole widget; when shown, the text scales to fit and the plot keeps priority.
- **Inputs full-width bar style**: an alternative layout where each bar spans the full width with its label and value drawn inside; plus a compact-steering toggle.
- **Boost compact mode**: draw the value inside a vertical bar with the peak above it, using the full width — a tidy, space-saving readout.
- **Power Graph compact style**: a version with no title/legend/axes that annotates peak power, torque, and boost right on the graph — and a show/hide grid-lines toggle for both styles.
- **Adaptive Tires tiles**: the tile view now arranges the four tyres to match the widget shape — a wide row, a 2×2 grid, or a tall column.
- **Tyre bar labels**: rotated Temp/Slip labels in the Bars style so it's clear which half of each bar is which.
- **RPM bar**: a more compact bar with the current/peak values drawn on it and the warning/shift lines colour-coded.
- **Dashboard config export/import**: copy your dashboard layout and mini-settings to/from JSON, with a checkbox to include or skip the mini-settings on each side; plus a preset loader in the mini-settings.
- **Hide widget titles**: a master mini-setting (General) that hides the title on every widget that has one, for a cleaner, denser dashboard.
- **G-Force label toggle**: show or hide the "Current" / "Peak" row labels; the peak readout now matches the orange of the peak marker on the plot.
- **Gearbox reset button**: reset the Automatic Gearbox sliders and numeric values to the default tune in one click — the mode dropdown, toggles, and per-car calibrations are left untouched.
- **Backfire dynamic key-press duration** (on by default): the throttle tap now lasts one game frame, derived from the packet rate, so it adapts to the game's frame rate instead of a fixed length; can be turned off to use a fixed value.
- **Backfire packet-based tap mode**: a second dynamic mode that holds the throttle key until the next packet arrives — an exact one-frame tap rather than an estimate; a safety timeout releases the key if telemetry stops.
- **Map "North up when stopped"**: in heading-up mode, the map now smoothly eases to north when you come to a stop and swings back to your heading as you drive off.
- **Keyboard shortcuts**: Ctrl+S opens the mini-settings for the current tab; Ctrl+E toggles Dashboard edit mode.
- **Top bar styles**: a new General page in the mini-settings lets you pick the top bar look — Modern (app title + current-page pill with centered icon tabs), Simple (icon-only tabs), or Legacy (the full labelled buttons). Modern adds a "Title and Selection" choice — show just the title, just the current-page pill, or both.
- **Suspension invert + end labels** (on by default): the bars now read as ride height (extension up), with rotated Compressed/Extended labels beside them; a mini-setting toggles back to raw compression.
- **What's New viewer**: this changelog, opened from the top-right of the tab bar, with filters for each category.

### Fixed
- **Steady number spinners**: the value boxes in the Automatic Gearbox and Backfire tabs now reserve room for their widest value, so rows no longer shift as digits are added; Backfire percentages always show one decimal, and the Key Press mode dropdown is left-aligned.
- **Steady packet-rate readout**: the packets-per-second display in the status bar reserves a fixed width, so it no longer shifts sideways as the number gains or loses a digit.
- **Consistent widget spacing**: uniform margins across the Boost, Speed Trace, Sprint, and RPM widgets so they line up when placed side by side.
- **Engine layout**: value columns line up across lines, and the text is centred vertically in the widget instead of clinging to the top.
- **Titles**: widget titles render consistently; the Power Graph compact title sits over the graph without stealing space, and "Hide widget titles" now covers it too.

### Removed
- **Separate tyre style**: folded into the single adaptive "Tires" view (was three styles, now Tires + Bars).
- **Duplicate engine-type caption** in the Car widget — it now lives in the Engine widget instead.
- **Recording replay**: the replay/loop playback was removed; recording, CSV export, and delete stay.
- **Load Preset in Setup**: preset loading now lives only in the dashboard mini-settings (it was in both).
- **Forza Motorsport 7 mode**: dropped entirely — the app is now Forza Horizon 6 only, and the game-selection dropdown in Setup is gone.

### Info
- **Presets carry your mini-settings**: exporting a preset or your config now includes the per-widget mini-settings, not just the grid layout.
- **Setup tab**: "Settings" is renamed "Setup" and moved to the far right, next to What's New.
- **Tab order**: reordered to Dashboard, Power Curve, Co-Op, Backfire, Automatic Gearbox, Engine Swaps.
