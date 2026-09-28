# Changelog

All notable user-facing changes, newest first. Categories: **Added** (new
features), **Fixed** (bug/behaviour fixes), **Removed** (things taken out),
**Info** (notes worth knowing).

## [0.3.0] – 2026-09-28

### Added
- **In-game HUD overlay (Linux/Wayland)**: a compact HUD drawn right over Forza Horizon 6, replacing the stock one with the telemetry you pick — an RPM / gear / speed cluster in two styles (Pill or Halo), a minimap with a compass and your co-op teammates, your race position with lap time and a lap delta (hidden in free roam), and a drift counter. It switches between the race block and the drift counter by itself when it detects a drift event. Clicks and keys go straight through to the game, it hides when you pause, and it follows the game to whichever monitor it's on. Tested on Hyprland; should work on other compositors with wlr-layer-shell such as Sway and KDE Plasma. Not on GNOME or X11, which the Overlay tab tells you.
- **Overlay tab**: a new tab after Dashboard to set up the HUD — turn it on, drag the Minimap, Drive cluster and Race / Drift modules onto a 3×3 screen grid (modules in the same cell stack from the screen edge inward), and set scale, plate opacity, fade and per-module options. Monitor detection is Hyprland (built in), a custom command or a fixed monitor, with a live status line. Changes apply to the running HUD straight away, and the overlay settings travel with your profiles.
- **Hide HUD hotkey**: press **H** (rebindable, on the Overlay tab or in Setup → Hotkey) to hide or show the in-game HUD.
- **Drive mode on the gear**: while the Automatic Gearbox is on, the HUD shows its mode in front of the gear — **D** (Street), **S** (Sport) or **R** (Race), e.g. "D4". Reverse and neutral stay a plain R and N.
- **Drift counter styles**: the default **Position + Gain** shows your place (with the same green/red flash as in races) and the points you scored in the last interval, counting up. **Total** shows the event score instead, which Forza already shows itself.
- **Calmer speed readout**: a Drive Cluster option, *Update speed only every 0.5 s*, holds the speed number between updates while gear and revs stay live.
- **Shift cue from the gearbox's RPM calibration**: the HUD's shift cue now lights at the Automatic Gearbox's own shift point (Gearbox → Shift RPM) on the max rpm it has calibrated for your car, and the redline starts from that calibrated value too. It works with the Automatic Gearbox switched off. Until a car is calibrated (rev out and shift up manually once), the HUD uses the game's max rpm and the new *Shift cue before calibration* setting. The HUD also hides in menus and on loading screens.
- **Edge margin and module spacing**: two new sliders in the Overlay tab's Layout card set the HUD's distance from the screen edges and the space between modules stacked in one cell. Reset layout puts them back to the defaults.
- **Clear a hotkey**: press **Backspace** while rebinding to leave it **Not set**; Esc still cancels.
- **GNOME window detection**: Settings → Window Detection has a new "GNOME (Window Calls extension)" method, so "only when the game is focused" works on GNOME too. It needs the free Window Calls GNOME Shell extension (extensions.gnome.org/extension/4724); the Test button shows the active window, or why detection failed (e.g. the extension isn't installed).
- **Debug tab**: a new tab next to Setup (bug icon) that lists every raw value in the latest telemetry packet, updating live, with a **Copy** button to paste them into a bug report.
- **Window Detection card**: Setup's **Input** card is now called **Window Detection** and has a new *Only when game window is focused* option that hides the in-game HUD while another window is in front. Its status light now shows amber (not red) while the game window simply isn't focused.

### Fixed
- **Co-Op while driving**: your position now keeps reaching teammates while the game window covers the app — previously you only moved on their map while the app itself was visible.

## [0.2.3] – 2026-09-23

### Fixed
- **Backfire & Automatic Gearbox keep working while the window is hidden**: both now run on their own background thread instead of inside the window's redraw loop, so pops and shifts continue when the app is minimized or completely covered by the game — previously they stopped the moment the window stopped being drawn. The **G** / **B** / **Reset RPM Calibration** hotkeys work while hidden too, and the toggle state is shown correctly when you come back. Bringing the window back no longer replays the telemetry that piled up while it was hidden.
- **Global hotkeys can't fire from the wrong window**: while the app is hidden, the bare **G** / **B** / **F** keys only act when the *game* is focused — they no longer react to those letters typed into other applications.
- **Status bar reports a stopped Backfire/Gearbox**: if the background thread driving them ever fails, both status-bar indicators now read **Stopped (error)** in red instead of showing a frozen *Active*.

## [0.2.2] – 2026-07-20

### Added
- **Backfire drift detection**: a new **Drift detection** condition (on by default) stops the pop from firing while the car is sliding or spinning up — whenever any wheel's slip ratio goes above 1.1. Turn it off in the Backfire tab's **Conditions** card.

## [0.2.1] – 2026-07-17

### Added
- **Backfire "Online Mode" note**: the Backfire tab's first card is now **General** (was *Activation*) and shows a hint that Backfire only works in Online Mode.

### Fixed
- **Race widget left margin**: the Race view's rows now have the same small left inset as the Sprint view — values no longer sat flush against the widget's left edge.

## [0.2.0] – 2026-07-16

### Added
- **Profile Manager**: a new **PROFILES** card at the top of the Settings tab lets you keep multiple named settings profiles and switch between them — create, duplicate, rename, and delete. A new profile starts from your current settings (on Windows it also defaults to *Game window focused* + *Only send inputs when game focused*). Switching auto-saves the profile you're leaving first, so nothing is ever lost — there's no Save button.
- **Selective export/import**: Export and Import open a large two-pane dialog — on the left you tick exactly what to include by group (Dashboard layout / mini-settings, Settings categories, and tuning tabs), plus the source and destination; on the right a live JSON preview shows exactly what will be written. Copy it to the clipboard, or paste JSON to import — into a new profile *or* over an existing one, overwriting only the settings you ticked. The bundled Ale/Ritze presets are now built-in import sources.
- **Global hotkeys**: rebindable keys that work while the game is focused — default `G` toggles Automatic Gearbox, `B` toggles Backfire — plus rebindable `Ctrl+S` (open mini-settings) / `Ctrl+E` (Dashboard edit mode). Configure them in Settings → Hotkeys, with Telemetry-live or window-focus triggering (Hyprland / X11 / custom command on Linux, with a Detect button to capture the game's window name and a live focus status readout). Optionally suppress backfire/gearbox key injection unless the game is focused. On Linux this needs your user in the `input` group (a status light shows whether it's working).
- **Status-bar Backfire & Gearbox indicators**: the bottom status bar now shows Backfire and Automatic Gearbox state, centered with a divider between them, each with its tab icon and an **Active** / **Deactivated** label (Gearbox also shows **Uncalibrated** while it's enabled but hasn't engaged yet). A *Status bar: show text labels* toggle in the General mini-settings collapses both to icon-only. Hovering either indicator shows a tooltip naming the feature and its current state.
- **Split gearbox reset**: the single *Clear calibration* button is now two — **Clear RPM calibration** (wipes the detected redline + engagement, keeps the per-gear speed map) and **Clear gear map** (wipes the per-gear speeds, keeps the redline). The **Reset RPM Calibration** hotkey (default `F`) now clears only the RPM calibration. Each also updates the saved per-car profile so a car reload can't restore what you cleared.
- **Gearbox calibration from any gear**: the Automatic Gearbox no longer needs a clean 1st-gear pull to engage — it now calibrates and engages on your first manual upshift from *any* gear, so rolling starts and high-gear spawns work. The **Clear calibration** button is now always visible (disabled until there's a calibration to clear).
- **Mini-settings transparency toggle**: the cog-wheel mini-settings window fades translucent when you're not hovering it; a new *Mini-settings fade when not hovered* switch (General page, on by default) lets you turn that off and keep it fully opaque.
- **Curated default settings**: a fresh install (no `config.json` yet) now starts from a well-rounded real-world setup — dashboard layout, units, and tuning — instead of bare defaults. Personal Co-Op fields (name, colour, last join code) stay neutral. Existing configs are untouched.
- **Refreshed tab icons**: Automatic Gearbox now uses a cogs glyph, Engine Swaps an engine glyph, and Backfire a flame (were gamepad/wrench/bolt).

### Fixed
- **Consistent label style (no trailing colons)**: swept every tab and mini-settings page to drop the trailing `:` from field/section labels, which were mixed inconsistently across the UI — Settings, Dashboard, Co-Op, Automatic Gearbox, Backfire, Power Graph, Engines, and the accel/decel trackers now all read colon-free.
- **Backfire RPM labels spelled out**: the RPM Range sliders' `Min` / `Max` now read **Minimum RPM** / **Maximum RPM**.
- **Co-Op label clarity**: the identity fields are now **Player name** / **Player color** (aligned to the two-column row layout), and the pacing slider is **Packet Buffer Size**.
- **Engine Swaps search icon**: the web-lookup search now uses a vehicle-lookup glyph instead of the plain magnifying glass.
- **Race widget auto-fit**: the Race/Sprint widget's *Race* view now scales its rows to fit the cell in both width and height, matching the Sprint view — it previously rendered at fixed sizes and overflowed small widgets.
- **Steady tab bar with the current-page pill**: the Modern top bar's current-tab pill now reserves the width of the longest tab name, so switching tabs no longer nudges the icon tabs sideways.
- **Gearbox viz theming**: the Automatic Gearbox live-view now draws its chrome — borders, bar tracks, dim labels, neutral text — from the shared theme tokens instead of hard-coded greys/whites; the semantic gear-state colours are unchanged.
- **Steadier settings rows**: the host-port spinner is right-pinned and control-row heights are bounded, so right-aligned rows no longer drift toward the panel's vertical middle.
- **Category page top spacing**: the first category card no longer sits with a doubled gap below the tab bar; the top inset now matches the left/right inset on every card-based tab.
- **Centered status-bar cog**: the settings cog in the status bar is now ink-centred in its button, matching the tab-bar icons.
- **Settings scrolling**: the inner scroll panes no longer chain-scroll the outer pane at their edges, and always capture the wheel while hovered.

### Removed
- **Recording**: the telemetry recorder is gone entirely — the Settings tab's Recording card (Record/Stop, Export CSV, delete) and the status-bar REC indicator have been removed, along with the `.ftr` capture and CSV export.
- **Mini-settings *Config* tab**: the dashboard cog's Config sub-tab (Load Preset / Export / Import) is gone — its export/import is now the Settings → Profiles card, and the bundled presets are import sources there.
- **F10 map-orientation hotkey**: removed; use the "Lock map north-up" checkbox in the minimap mini-settings.

### Info
- **Settings tab restyled and reorganised**: cards are regrouped — left column holds **Profiles** and **Hotkey**; right column holds **Repository / Credits** (renamed from *Repository*), **Display**, **Network**, **Co-Op**, and **Input** — with a coloured connection status dot, auto-save, and no more Save button.
- **Styled radio buttons**: one-of-N choices (unit pickers, import target, engine display mode) now use a custom radio that matches the app's accent checkbox — a circle with a white centre dot — and the Display unit rows (km/h · mph, °C · °F, bar · PSI) are column-aligned.
- **Profile actions use dialogs**: New / Duplicate / Rename / Delete open a centered modal (with a name field where relevant, Enter to confirm, Esc to cancel) instead of inline rows, and the redundant active-profile dropdown is gone — the profile list is the single picker.

## [0.1.1] – 2026-07-15

### Added
- **Power Graph widget options**: the dashboard Power Graph widget's mini-settings (Dashboard → Graphs) now expose the full Power Graph tab's capture options too — RPM step size, forced-induction detection, and save-FI-state — so you can tune the widget without opening the Power Graph tab. The mini-settings tab is also renamed from "Power" to "Power Graph".
- **Clearer "waiting for telemetry" screen**: while no data is coming in, the dashboard now spells out the exact Data Out settings to enter in Forza — reminds you to scroll all the way down, and shows Data Out = On, IP Address = 127.0.0.1, and the Port to match your app's listen port — in a tidy card.

### Fixed
- **Aligned control rows**: labels now sit vertically centred against the slider, dropdown, or spinner beside them across the settings and tuning cards, instead of clinging to the top of the row; checkboxes share the same row height, so each label + control reads as one straight band.

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
- **Map "North up when stopped"**: in heading-up mode, the map now smoothly eases to north when you come to a stop and swings back to your heading as you drive off — timed to the same stop as the zoom-out.
- **Keyboard shortcuts**: Ctrl+S opens the mini-settings for the current tab; Ctrl+E toggles Dashboard edit mode.
- **Top bar styles**: a new General page in the mini-settings lets you pick the top bar look — Modern (app title + current-page pill with centered icon tabs), Simple (icon-only tabs), or Legacy (the full labelled buttons). Modern adds a "Show current tab pill" toggle, and a "High contrast icons" toggle draws the compact tab icons white instead of the accent tone.
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
- **Load Preset in Setup**: preset loading now lives only in the dashboard mini-settings (it was in both).
- **Forza Motorsport 7 mode**: dropped entirely — the app is now Forza Horizon 6 only, and the game-selection dropdown in Setup is gone.

### Info
- **Presets carry your mini-settings**: exporting a preset or your config now includes the per-widget mini-settings, not just the grid layout.
- **Setup tab**: "Settings" is renamed "Setup" and moved to the far right, next to What's New.
- **Tab order**: reordered to Dashboard, Power Curve, Co-Op, Backfire, Automatic Gearbox, Engine Swaps.
