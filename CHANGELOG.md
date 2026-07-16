# Changelog

All notable user-facing changes, newest first. Categories: **Added** (new
features), **Fixed** (bug/behaviour fixes), **Removed** (things taken out),
**Info** (notes worth knowing).

## [0.1.0] – 2026-07-13

### Added
- **Split gearbox reset**: the single *Clear calibration* button is now two — **Clear RPM calibration** (wipes the detected redline + engagement, keeps the per-gear speed map) and **Clear gear map** (wipes the per-gear speeds, keeps the redline). The **Reset RPM Calibration** hotkey (default `F`, renamed from *Reset Gearbox Calibration*) now clears only the RPM calibration. Each also updates the saved per-car profile so a car reload can't restore what you cleared.
- **Status-bar Backfire & Gearbox indicators**: the bottom status bar now shows Backfire and Automatic Gearbox state, centered with a divider between them, each with its tab icon and an **Active** / **Deactivated** label (Gearbox also shows pastel-amber **Uncalibrated** while it's enabled but hasn't engaged — i.e. before your first manual upshift teaches it the redline). A new *Status bar: show text labels* toggle in the General mini-settings collapses both to icon-only.
- **Mini-settings transparency toggle**: the cog-wheel mini-settings window fades translucent when you're not hovering it; a new *Mini-settings fade when not hovered* switch (General page, on by default) lets you turn that off and keep it fully opaque.
- **Refreshed tab icons**: Automatic Gearbox now uses a cogs glyph and Engine Swaps an engine glyph (were gamepad/wrench).
- **Gearbox calibration from any gear**: the Automatic Gearbox no longer needs a clean 1st-gear pull to engage — it now calibrates and engages on your first manual upshift from *any* gear (so rolling race starts and high-gear spawns work). Added a **Reset Gearbox Calibration** hotkey (default `F`) that wipes the calibration so the next manual upshift re-learns it, and the **Clear calibration** button is now always visible (disabled until there's a calibration to clear).
- **Curated default settings**: a fresh install (no `config.json` yet) now starts from a well-rounded real-world setup — dashboard layout, units, and tuning — instead of bare defaults. Personal Co-Op fields (name, colour, last join code) stay neutral. Existing configs are untouched.
- **Global hotkeys**: rebindable keys that work while the game is focused — default `G` toggles Automatic Gearbox, `B` toggles Backfire — plus rebindable `Ctrl+S` / `Ctrl+E`. Configure them in Settings → Hotkeys, with Telemetry-live or window-focus triggering (Hyprland / X11 / custom command on Linux, and a Detect button to capture the game's window name). Optionally suppress backfire/gearbox key injection unless the game is focused. On Linux this needs your user in the `input` group (a status light shows whether it's working).
- **Power Graph widget options**: the dashboard Power Graph widget's mini-settings (Dashboard → Graphs) now expose the full Power Graph tab's capture options too — RPM step size, forced-induction detection, and save-FI-state — so you can tune the widget without opening the Power Graph tab. The mini-settings tab is also renamed from "Power" to "Power Graph".
- **Clearer "waiting for telemetry" screen**: while no data is coming in, the dashboard now spells out the exact Data Out settings to enter in Forza — reminds you to scroll all the way down, and shows Data Out = On, IP Address = 127.0.0.1, and the Port to match your app's listen port — in a tidy card.
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
- **Map "North up when stopped"**: in heading-up mode, the map now smoothly eases to north when you come to a stop and swings back to your heading as you drive off — timed to the same stop as the zoom-out.
- **Keyboard shortcuts**: Ctrl+S opens the mini-settings for the current tab; Ctrl+E toggles Dashboard edit mode.
- **Top bar styles**: a new General page in the mini-settings lets you pick the top bar look — Modern (app title + current-page pill with centered icon tabs), Simple (icon-only tabs), or Legacy (the full labelled buttons). Modern adds a "Show current tab pill" toggle, and a "High contrast icons" toggle draws the compact tab icons white instead of the accent tone.
- **Suspension invert + end labels** (on by default): the bars now read as ride height (extension up), with rotated Compressed/Extended labels beside them; a mini-setting toggles back to raw compression.
- **What's New viewer**: this changelog, opened from the top-right of the tab bar, with filters for each category.
- **Backfire icon**: the Backfire tab now uses a flame glyph instead of the bolt icon.

### Fixed
- **Race widget auto-fit**: the Race/Sprint widget's *Race* view (shown once you're in a race) now scales its rows to fit the cell in both width and height, matching the Sprint view — it previously rendered at fixed sizes and overflowed small widgets.
- **Co-Op label clarity**: the identity fields are now **Player name** / **Player color**, and the pacing slider is **Packet Buffer Size** (was Name / Color / Buffer).
- **Engine Swaps search icon**: the web-lookup search now uses a vehicle-lookup glyph instead of the plain magnifying glass.
- **Consistent label style (no trailing colons)**: swept every tab and mini-settings page to drop the trailing `:` from field/section labels, which were mixed inconsistently across the UI — Settings, Dashboard, Co-Op, Automatic Gearbox, Backfire, Power Graph, Engines, and the accel/decel trackers now all read colon-free.
- **Backfire RPM labels spelled out**: the RPM Range sliders' `Min` / `Max` (and `Min RPM` / `Max RPM`) now read **Minimum RPM** / **Maximum RPM**.
- **Steady tab bar with the current-page pill**: the Modern top bar's current-tab pill now reserves the width of the longest tab name, so switching to a longer- or shorter-named tab no longer nudges the icon tabs sideways. They stay centered and only shift — once, uniformly — when the window gets genuinely narrow.
- **Gearbox viz theming**: the Automatic Gearbox live-view (right half) now draws its chrome — borders, bar tracks, dim labels, neutral text — from the shared theme tokens instead of hard-coded greys/whites, so it matches the rest of the app. The semantic gear-state colours (green/amber/red/cyan) are unchanged.
- **Category page top spacing**: the first category card no longer sits with a doubled gap below the tab bar — the top inset now matches the left/right inset on every card-based tab (Backfire, Automatic Gearbox, Co-Op, Settings).
- **Centered status-bar cog**: the settings cog in the status bar is now ink-centred in its button (matching the tab-bar icons) instead of sitting slightly off-centre.
- **Aligned control rows**: labels now sit vertically centred against the slider, dropdown, or spinner beside them across the settings and tuning cards, instead of clinging to the top of the row; checkboxes share the same row height, so each label + control reads as one straight band.
- **Steady number spinners**: the value boxes in the Automatic Gearbox and Backfire tabs now reserve room for their widest value, so rows no longer shift as digits are added; Backfire percentages always show one decimal, and the Key Press mode dropdown is left-aligned.
- **Steady packet-rate readout**: the packets-per-second display in the status bar reserves a fixed width, so it no longer shifts sideways as the number gains or loses a digit.
- **Consistent widget spacing**: uniform margins across the Boost, Speed Trace, Sprint, and RPM widgets so they line up when placed side by side.
- **Engine layout**: value columns line up across lines, and the text is centred vertically in the widget instead of clinging to the top.
- **Titles**: widget titles render consistently; the Power Graph compact title sits over the graph without stealing space, and "Hide widget titles" now covers it too.

### Removed
- **F10 map-orientation hotkey**: removed; use the "Lock map north-up" checkbox in the minimap mini-settings.
- **Separate tyre style**: folded into the single adaptive "Tires" view (was three styles, now Tires + Bars).
- **Duplicate engine-type caption** in the Car widget — it now lives in the Engine widget instead.
- **Recording**: the telemetry recorder is gone entirely — the Settings tab's Recording card (Record/Stop, Export CSV, delete) and the status-bar REC indicator have been removed, along with the `.ftr` capture and CSV export.
- **Load Preset in Setup**: preset loading now lives only in the dashboard mini-settings (it was in both).
- **Forza Motorsport 7 mode**: dropped entirely — the app is now Forza Horizon 6 only, and the game-selection dropdown in Setup is gone.

### Info
- **Presets carry your mini-settings**: exporting a preset or your config now includes the per-widget mini-settings, not just the grid layout.
- **Setup tab**: "Settings" is renamed "Setup" and moved to the far right, next to What's New.
- **Tab order**: reordered to Dashboard, Power Curve, Co-Op, Backfire, Automatic Gearbox, Engine Swaps.
