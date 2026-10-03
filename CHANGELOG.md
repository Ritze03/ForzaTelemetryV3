# Changelog

All notable user-facing changes, newest first. Categories: **Added** (new
features), **Fixed** (bug/behaviour fixes), **Removed** (things taken out),
**Info** (notes worth knowing).

## [0.4.2] – 2026-10-04

### Fixed
- **Hotkeys on GNOME/Fedora: the permission check no longer shows a false green**: *Setup → Input Permissions* counted any readable input device, so a game controller made it green even when the keyboard could not be read, and the "input permissions missing" dialog never appeared while hotkeys stayed dead. It now checks that a **keyboard** is readable (and that hotkeys actually opened one), shows red with the fix commands otherwise, and **Re-check** reopens keyboards without a restart. Not being in the `input` group is only a hint (amber) while everything works.
- **Automatic gearbox: the detected max RPM (redline) is now locked once calibration finishes**: later over-revs (limiter bounce, downshift spikes) kept raising it and moved the shift point. Only **Clear RPM calibration**, switching cars, or loading a saved per-car calibration changes it now. The Debug tab's Calibration checks show the lock ("Redline not locked" row, heading "locked").

## [0.4.1] – 2026-10-02

### Fixed
- **Max RPM now detected on cars with fast rev limiters**: the capture required positive engine power, but such cars report zero power while bouncing off the limiter, so their max RPM was never captured. The check is gone (it only guarded against mis-shifts, which the current calibration protocol makes practically impossible).
- **Forced-induction detection on the Dashboard**: the Power Graph module's boost line ignored *Forced induction detection* and was drawn for every car, so naturally aspirated cars got a flat boost line at 0 plus a boost scale. It now follows the detection like the Power Curve tab and the Boost Graph module do. Detection is also stricter: a car only counts as turbo/supercharged once boost goes above 1 PSI (was 0.05 PSI, too close to an NA engine's near-zero reading at full throttle), it judges the data actually shown rather than a saved reference from another car, and it starts over when you switch cars.
- **Right-stick look-around no longer jolts**: holding the right stick while *North up when stopped* kicked in (or the map snapped back to heading-up as you drove off) made the map jerk toward north and then swing back to where the stick pointed. Now, while you hold the stick, the map stays exactly where the stick points relative to your car, whatever the map's own rotation does underneath; when you let go it eases smoothly (the short way round) to north-up if you are stopped with that option on, or to heading-up. Dashboard map and HUD minimap alike.
- **Right-stick map rotation in north-up**: with the map locked north-up, looking around with the right stick turned the map relative to north (stick right always put east at the top), so the view seemed randomly offset depending on where the car was heading. The stick now always looks relative to your car, like the game camera: right shows what is to the car's right, down looks behind it, in north-up and heading-up alike, on the Dashboard map and the HUD minimap. Releasing the stick still eases back to north.
- **Rebinding no longer triggers the action**: while you are binding a key or controller button (Setup → Hotkey / Controller, Overlay → Hide HUD), no hotkey or controller action fires, and the key or button you just pressed (or its release) stays silent for a moment afterwards. Before, rebinding Backfire to B while it was already on B toggled Backfire.
- **Setup layout**: the Controller card's bind buttons now all line up at the same position and size in every row (before, each row sat a few pixels further right than the one above), the x clear buttons are gone (press Backspace or Delete while binding instead), and the Network card moved above Game Install in the right column.
- **Controller stick deadzone** now defaults to 0.27 (was 0.15), which ignores the drift of worn sticks. Existing settings keep their saved value.
- **HUD Pill gear position**: the gear indicator in the Pill style of the Drive cluster sat 1 px too low and now sits 1 px higher.

### Added
- **In-game HUD overlay on Windows (experimental, untested)**: the HUD can now be drawn over a borderless/windowed Forza on Windows too, not just on Linux. It is a click-through, always-on-top, non-activating transparent window on one monitor, using the same drawing, settings, visibility rules (pause, Window Detection focus, Hide HUD hotkey) and notifications as the Linux overlay. Exclusive fullscreen can't be overlaid, so set the game to borderless. The Overlay tab is enabled on Windows, with a tooltip noting that only Borderless or Windowed works. Monitor Detection defaults to *Active window* there (the monitor the focused game window is on; Hyprland and Custom command are Linux-only). The Fixed monitor field takes `DISPLAY2`, `\\.\DISPLAY2` or just `2` (default: the primary monitor). Written without access to a Windows machine (only smoke-tested under Wine), so expect rough edges; `FORZA_OVERLAY_TEST=1` shows a test pattern without the game.
- **Debug: Calibration checks**: the Debug tab's "Derived from telemetry" card now lists every condition behind RPM calibration, each with a green or red dot, its live value and its threshold: Max RPM capture (race on, power, handbrake, tyre slip per wheel), Calibrated (the first manual upshift that engages the gearbox) and Gear-map sample (speed, RPM vs redline, tyre slip and suspension per wheel, driving straight), plus the max RPM captured so far and the samples and redline speed per gear. If calibration will not work for a car, you can now see which check is failing.
- **Solo trail (white)**: your own trail now also shows on the Dashboard map and the HUD minimap when you are not in a co-op session, in white like your arrow, with the same fade settings (Mini-Settings → Dashboard → Map → Co-Op "Tracer fade"; the HUD's Co-Op trail fade). In a session it turns your co-op colour as before; the trail you already drove is kept when a session starts or ends. On the HUD, **Show trails** switches it off together with the co-op trails.
- **Delete clears a hotkey too**: while rebinding a keyboard hotkey (Setup → Hotkey, Overlay → Hide HUD), **Delete** now clears it to *Not set* just like **Backspace**; Esc still cancels.
- **HUD minimap = Dashboard map**: the in-game minimap now draws like the Dashboard map. Your own arrow uses the Dashboard's arrow in your co-op colour (Co-Op → Your Identity; white outside a session) with your trail behind it, teammates get the same arrows, names, edge pointers, paused-grey and fading trails, and shared waypoints show up too. New options in Mini-Settings → Overlay: **Mirror map at edges**, a Co-Op section (teammates, waypoints, trails, trail fade), and two tick boxes, **Use Dashboard map settings** and **Use Dashboard co-op settings**, that make the HUD use the Dashboard's values for the map view or for co-op (separately) and hide the HUD's own controls for that part.
- **Look around with the right stick**: new option *Rotate with right stick* for the Dashboard map (Mini-Settings → Dashboard → Map) and the HUD minimap (Mini-Settings → Overlay → Minimap), both on by default. While you push the controller's right stick past its deadzone the map turns to look where the stick points relative to your car (up = ahead, right = the car's right, down = looking back); on release it eases back. Works in heading-up and north-up, the compass stays correct, and it works independently of any right-stick button bindings.
- **Controller input**: a new **Controller** card in Setup (below Hotkey). Bind controller buttons to the in-game actions (Toggle Automatic Gearbox, Clear RPM calibration, Clear gear map, Toggle Backfire, Hide HUD): click a row, press the button on your pad (Esc cancels; Backspace or Delete clears the binding). Works with the A/B/X/Y, bumpers, triggers, Back/Start, stick clicks, D-pad and the right stick's four directions. Stick and trigger deadzones are adjustable. Xbox-style pads on Linux (xpad, xone incl. the wireless dongle, xpadneo) and Windows (XInput); pads plugged in later are picked up automatically, and the real pad is read rather than Steam's virtual one so it keeps working while the game is focused. Linux needs the same `input` group access as the hotkeys.
- **Setup: Game Install**: a new card where you set the Forza Horizon 6 install folder (used for car names). *Auto-detect* finds it through Steam (all Steam libraries; on Windows also via the registry), *Detect from running game* reads it from the running game (Linux/Proton and Windows), or type the path yourself. A status light shows whether the folder was found and how many cars it holds.
- **HUD notifications**: short messages on the in-game HUD when something changes: *Gearbox: ON / OFF*, *Gearbox mode: Race* (also on the automatic switch to Race in a race and back), *Backfire: ON / OFF*, *Calibration started* and *Calibration done: N rpm*, with a *Shift at redline* hint (yellow dot) in between as soon as gear 1 has collected (new) gear-map data while the gearbox is still uncalibrated (once per calibration; it tells you to rev out and shift). *Calibration started* shows the moment you Clear RPM calibration or Clear gear map (hotkey, controller or button; also while paused, where it waits until the HUD is back), and whenever you are driving a car that is not calibrated yet (first car after starting the app, a car change), once per car, not on every unpause. *Shift at redline* then only appears once you have actually driven since the reset (new gear-1 data), so the *Calibration started* message is no longer overwritten at once. Each kind of message has its own slot: a new one replaces the one of the same kind that is still showing (the three calibration messages share one pill, Gearbox ON then OFF becomes one pill saying OFF) instead of stacking up. They fade out after about 2.5 s, up to 5 at once, always stacked vertically. Switch the whole thing and each message type on or off in Mini-Settings → Overlay → Notifications, and pick where on the screen they appear (3×3 anchor, default centre) on the Overlay tab.
- **HUD minimap settings in Mini-Settings**: the cog-wheel Mini-Settings gets an **Overlay** tab next to Dashboard with the in-game minimap's view options, like the Dashboard map's: lock north-up (the arrow turns instead of the map), north up when stopped, smooth rotation, use movement direction, compass and the two zoom radii.
- **Input permission check (Linux)**: at startup the app checks that it can read `/dev/input` (hotkeys) and write `/dev/uinput` (Gearbox / Backfire key presses). If not, a dialog shows what is missing and the exact command to fix it (with a Copy button; log out and back in afterwards). The X closes it for this session; *Don't remind me again* silences it. Setup has a new **Input Permissions** category below Window Detection with a status light per item and a *Remind me on startup* checkbox.
- **Debug tab: "Derived from telemetry"**: the Debug tab is now split in two halves. The left keeps the raw packet fields; the right shows what the app derives from them: whether the game counts as paused and which rule fired, the calibrated max RPM, the gearbox's selected / effective mode, the map season, and the car's make and model read from your own FH6 install. Display only for now.
- **Experimental pause detection**: a new checkbox in Setup → Network (on by default). The in-game HUD, and the gearbox's drift detection, now treat the garage as paused: when the car is level, completely motionless and the handbrake is fully on, the HUD hides as it does on the pause menu. Hover the checkbox for details; it may miss a garage view where the car is rotated.
- **Backfire: Limit max. duration**: a new checkbox under *RPM interval* on the Backfire tab. When on, a slider (100-5000 ms, default 1500) caps how long backfire may run continuously; after that it stops even if RPM is still in range, and starts again once you touch the throttle or downshift.
- **Automatic Gearbox: Manual mode**: a new first entry in the Gearbox mode dropdown. In Manual the gearbox never shifts and the in-game HUD shows it exactly as if the gearbox were off. With *Auto Race mode in races* on, it still switches to Race mode in an actual race and goes back to Manual afterwards, so you can drive free roam by hand and use the gearbox only in races.
- **Automatic Gearbox: Disable in drift events**: a new checkbox under *Auto Race mode in races*. While a drift event is detected (the same detection the HUD uses to show the drift counter) the gearbox turns off, HUD included, and comes back when the event ends. It applies with *Auto Race mode in races* on or with Race mode selected.

### Info
- **New defaults for overlay and new features (tuned values)**: everything you have not configured yet now starts with tuned values instead of the old neutral ones. All Overlay settings (overlay on, only when the game window is focused, race / drift block bottom-right, 4 px edge margin, engine RPM in the Drive cluster, minimap zoom 500 m while driving, follow movement direction and right stick, co-op settings from the Dashboard, no *Gearbox mode* notification, notifications in the centre), the right-stick map rotation, Backfire *Limit max. duration* (on, 1500 ms), the Co-Op transport (Trystero; auto-connect stays off), the Hide HUD key (J) and Clear gear map key (F), and the controller binding Toggle Automatic Gearbox = L3. Settings you already saved keep their values; machine-specific ones (monitor selection, install folder, co-op name/colour/room) stay neutral.
- **Clear RPM calibration and Clear gear map are separate hotkeys / controller actions**: the old *Reset RPM Calibration* action is now named **Clear RPM calibration** (your existing binding is kept) and a new **Clear gear map** action sits next to it (default key F, the same as Clear RPM calibration, so one press clears both; rebind it under Setup → Hotkey / Controller). The Automatic Gearbox tab has the two buttons *Clear RPM calibration* and *Clear gear map*.
- **One key or button, several actions**: you can now bind the same keyboard key or controller button to more than one action, and pressing it runs all of them (in the order they are listed in Setup), for example both clears on one button. Before, only one of them ran on a keyboard hotkey, and binding a controller button that another action used took it away from that action. The red "Also bound to ..." note under Overlay → Hide HUD is gone.
- **Dashboard modules fit any cell size**: Session Stats, Position, Co-Op, Boost and the Boost / Power Graph now work like the other modules: they draw inside their own cell and shrink their text to fit instead of needing a minimum size, so nothing overflows or wraps letter by letter in a small cell. Titles cut off with "…" instead of wrapping. Session Stats flows into several columns in a wide, short cell and stacks values under their labels in a narrow one; Co-Op drops the distance and gear columns (then uses two lines per player) when narrow; Position stacks its two blocks when narrow; the graphs switch to the Compact look on their own when the cell is too small for axes.
- **Dashboard Boost Graph matches the Power Graph**: same small blue title (no more big wrapping heading), same axis padding, grid and RPM range, and it follows the same Mini-Settings → Dashboard → Power Graph options: *Compact* (title over the plot, no axes, peak marked with a line and value) and *Show grid*. Without detected boost it shows "No boost detected" instead of an empty chart.
- **Dashboard map compass**: the Dashboard map now uses the same compass as the in-game minimap (disc with a red/white needle pointing north) instead of the old circle with an "N".
- **Overlay: "Only when game window is focused" moved** from Setup → Window Detection to the Overlay tab (General). Hover it for a note that it uses the Window Detection method from Setup. The setting itself is unchanged.
- **Co-Op: Host port moved** from the Setup page to the Co-Op tab, in a Cloudflare card under the Session card (shown while Cloudflare is selected), since only that transport uses it.
- **Co-Op: long room IDs fit**: the Room ID hint shows a full-length example and the shown room code shrinks so the whole ID is visible next to Copy.
- **Overlay: Drive cluster style** is now a dropdown with the entries **Pill** and **Halo** instead of two radio buttons.
- **Status bar connection indicator**: in icon-only mode the packet rate now shows just the number (no "pps"), in the connection icon's colour and right next to the icon. Hovering the icon or the number (or the whole icon + text group with text labels on) shows the connection state and the rate as "N packets per second".
- **Longer Co-Op room IDs**: generated Trystero Room IDs are now 32 characters (8 groups of 4) so shared public rooms practically never collide. Older, shorter IDs keep working.
- **Less clutter under options (Co-Op, Power Graph, Engine Swaps, Acceleration, Deceleration)**: removed the grey intro/explanation lines under options; the useful ones (packet buffer, forced-induction options, Dynamic mode) are now tooltips. The Room ID security warning stays visible.
- **Less clutter under options**: the small grey explanation lines under settings on the Overlay, Gearbox, Backfire and Setup tabs are now tooltips on the setting itself. Warnings and status messages (e.g. a key bound twice, Test results) stay visible.

## [0.4.0] – 2026-09-30

### Added
- **Co-Op: Trystero connection**: a new `Trystero | Cloudflare` switch at the top of the Co-Op session card. Trystero connects players directly peer-to-peer through a shared Room ID (with a Generate button) — no tunnel and no host needed. The last room ID is remembered, and an *Auto-connect on startup* option rejoins it when the app launches. Not yet tested over real networks; if two players can't reach each other (strict NAT), use the Cloudflare option. Credited in Settings → Repository / Credits.
- **Icon-only status bar, left side too**: with *Status bar: show text labels* off, the connection status is now just the plug / no-signal icon (hover for the word) and the Co-Op indicator is just the players icon and the player count. The Co-Op indicator turns yellow only while the Co-Op page shows "Negotiating…" (Trystero) or the session is connecting (Cloudflare), and green otherwise; hover it for the full role, count and status. The text version gets the same yellow while connecting.

## [0.3.0] – 2026-09-28

### Added
- **In-game HUD overlay (Linux)**: a compact HUD drawn right over Forza Horizon 6, replacing the stock one with the telemetry you pick — an RPM / gear / speed cluster in two styles (Pill or Halo), a minimap with a compass and your co-op teammates, your race position with lap time and a lap delta (hidden in free roam), and a drift counter. It switches between the race block and the drift counter by itself when it detects a drift event. Clicks and keys go straight through to the game, it hides when you pause, and it follows the game to whichever monitor it's on. Tested on Hyprland and GNOME; should work on other compositors with wlr-layer-shell such as Sway and KDE Plasma. GNOME and other desktops without layer-shell (and X11 sessions) use a fallback through XWayland, picked automatically with no setup needed.
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
- **Backfire & Automatic Gearbox keep working while the window is hidden**: both now run on their own background thread instead of inside the window's redraw loop, so pops and shifts continue when the app is minimized or completely covered by the game — previously they stopped the moment the window stopped being drawn. The **G** / **B** / **Clear RPM calibration** hotkeys work while hidden too, and the toggle state is shown correctly when you come back. Bringing the window back no longer replays the telemetry that piled up while it was hidden.
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
