# Map tab (`src/ui/map_tab.rs`, `src/ui/map_scene.rs`)

A top-level tab (icon `MAP`, `fa-map`; label **Map**, German **Karte**) between **Overlay** and
**Power Curve**. Two modes: the **viewer** (the whole tab is the map) and a full-size
**settings** mode for the maps (Minimap; Dashboard map & Viewer, which share settings; Map data).

*Why (D67, the user, 2026-10-08):* "Instead of having the settings for the minimap and the
dashboard map in the overlay, we should probably create a new [tab] that is only used for
configuring those two maps. It could be like a general map where the user can see the map in
full screen and then just switch to a full screen settings UI inside of that ... add tabs for
configuring the minimap, the dashboard map, one for the viewer itself and maybe one for the map
data editor so we can move it out of the setup screen. That would be really nice since that way
even the setup screen would be less bloated than it is right now." The pan / zoom and its reset
(D72) were a follow-up request: "an option to pan and zoom the Map in the UI (both the dashboard
and the new Map tab). It should just reset, once the player starts driving again."

## Viewer

The map fills the tab, drawn by the **same scene code as the Dashboard's Map widget**
(`map_scene::draw`, extracted from `dashboard::show_minimap_widget`; the shared renderer
`maprender::draw_base` + `draw_layers`, D61): satellite image, roads, POIs, race lines, trails, co-op
teammates, the own arrow, shared waypoints (click drops one, right-click clears, as on the
Dashboard map), compass, the co-op player list. It uses the Dashboard's already loaded map
texture (`app.minimap_texture`) and POI icons (`app.minimap_icons`): no second copy.
`ForzaApp::ensure_map_image` starts the load when the Dashboard map module is off (the startup
load skips it), and reloads when the season changed under such a texture. Without an install the
Dashboard's "Map needs your Forza Horizon 6 install" state shows (the buttons stay).

**The viewer has no settings of its own (D73).** It draws with the Dashboard map's: `minimap_layers`
and the `minimap_*` view options (north-up and its sub-options, mirror at edges, compass, right-stick
look, allow pan and zoom), the same yaw (`minimap_look.view_yaw(minimap_base_yaw())`) and the same
base zoom (the Dashboard's eased driving / stopped zoom, `ForzaApp::minimap_current_zoom`; a pan /
zoom the user did is only the temporary `ManualView` on top). *Why (the user, 2026-10-09):* "I think
that the dashboard map and the viewer should actually share the same settings. Because both are, like,
big and accessed through the UI, so it only makes sense." Before, the viewer had `viewer_layers`
and `viewer_*` (own look, `viewer_zoom_m` = 1500 m); those are gone, see *Settings mode*.

- **Drag** pans, **wheel / pinch** zooms (radius 50-8000 m). While the view follows the car the wheel
  zooms around the car (the car stays in the middle; there is nothing to anchor to); once panned
  it zooms **around the cursor** (the point under the pointer stays under it). Panning with a
  tilted camera keeps the grabbed point under the pointer too (`map_scene::panned` /
  `zoomed_at` use `Camera::unproject`; 3D: see Temporary pan / zoom).
- **Follow car** (top left): the view centre is the car by default. The button is shown **only
  while the view is manual** (panned or zoomed), like the Dashboard widget's
  (`map_scene::follow_button`); pressing it brings the view back at once (also resets a zoom).
  *Why (the user, 2026-10-09):* "only show the Follow Car button, while it isn't following the car
  at the moment". Before, it was always there (lit while following) and, with nothing manual,
  pressing it froze the view; that freeze (`ManualView::pin_centre`) is gone with the button.
- **Settings** (**bottom right**, cog) switches the tab to the settings mode. The zoom radius is shown
  bottom left. *Why bottom right (the user, 2026-10-09):* "the settings are in the top right, but
  that's also where the co-op thingy draws ... just move the settings button to the bottom right."
  The co-op player list (`map_scene::draw`) owns the top right. The compass is drawn top left by the
  scene, where Follow car is: while the compass is on, Follow car steps right of it
  (`map_scene::compass_rect`, the compass box the Dashboard widget's button uses too). Test:
  `viewer_controls_stay_inside_the_tab` (corners, no label overlap, clear of a simulated co-op list
  and the compass; EN + DE, 600-1235 px).
- **North-up** by default (`minimap_north_up`, the Dashboard map's setting); off = the map turns with
  the car's heading (the Dashboard's eased heading incl. the right-stick look).

### Temporary pan / zoom (D72)

A manual view is **temporary**: it resets to following the car at the configured zoom once the
player drives off again. Shared by the Dashboard map and the viewer: one state type,
`map_scene::ManualView` (the user's centre and zoom over the map's base view), one input function,
`ManualView::interact`, one rule, `DriveGate`.

- **Rule** (*why these numbers:* a few km/h is a crawl in a garage queue, ~11 km/h is clearly
  driving off): after the car has been **seen stopped** (< 2 m/s) since the view went manual, it
  must stay **>= 3 m/s for 0.6 s** before the view resets. The 2-3 m/s gap is hysteresis; the delay
  stops a short nudge (a bump, a tap on the throttle) from taking the map away; dropping below
  2 m/s cancels a running countdown. While stopped, or with **no telemetry** (`speed = None`), the
  view stays.
- **Panning while already driving** (a passenger looking ahead): it does not snap back a moment
  later; it resets the next time the car stops and drives off (`DriveGate::arm` takes the speed
  at the moment the view went manual).
- Speed is the packet's `speed` (m/s). The countdown requests a repaint every 100 ms so it fires
  without input.
- **Dashboard map** (`dashboard::show_minimap_widget`, state `ForzaApp::minimap_pan`, a `RefCell`
  because the widget draws from `&ForzaApp`): same drag / wheel; a small **Follow car** button
  appears in its corner while the view is manual. **Not in the dashboard's layout-edit mode:**
  there the grid owns the drag (the response only senses hover), and a leftover manual view is
  reset. The base zoom is the eased `minimap_current_zoom`.
- **Viewer:** the base zoom is the Dashboard's eased `minimap_current_zoom` (driving / stopped
  zoom). *Why not the last viewer zoom:* the viewer has no zoom setting any more (D73), and the
  Dashboard's value is what the user tuned for the map; the temporary manual zoom resets to it.
- **Option, default on:** `minimap_allow_pan_zoom` (Map tab, Dashboard map & Viewer page; one
  switch for both). Off = no pan / zoom sensing, the
  old click-only behaviour (the Dashboard map keeps its waypoint click).
- **In 3D** (View mode 3D, phase K, K4) pan and wheel zoom anchor on the **terrain surface under the
  pointer**, not the car-height plane. `map_scene::draw` leaves the drawn 3D look in the egui context,
  keyed by the map's `Ui`, and `ManualView::interact` solves for the view centre that keeps the
  grabbed or zoomed terrain point under the pointer. A panned view's camera height eases (about 0.2 s)
  towards the terrain under the centre rather than snapping to it. *Why:* the camera height shifts
  the whole picture, so a pivot that follows the terrain exaggerates a pan uphill by several times,
  cancels it downhill, and makes the view jump on the first drag frame; easing lets the pan solve
  exactly per frame and the new height settles smoothly afterwards (the earlier ground-plane anchor
  was a stable but wrong answer: the map moved with the pointer only on flat ground). `pick()`
  ray-marches the terrain, so waypoint clicks on slopes are exact. Tests:
  `pan_keeps_the_grabbed_terrain_point_under_the_pointer_in_3d`,
  `zoom_keeps_the_terrain_point_under_the_cursor_in_3d`, `the_panned_camera_height_eases_towards_the_terrain`.
  The 3D renderer's state and fallback are described in
  [minimap.md](minimap.md#3d-on-the-eframe-side-dashboard-map-and-viewer-phase-k-k4).
- Tests: `map_scene::tests` (`manual_view_resets_when_the_player_drives_off`,
  `a_nudge_does_not_reset_the_view`, `panning_while_driving_waits_for_the_next_stop`,
  `manual_view_overrides_the_base_and_resets`, pan / zoom anchoring).

## Settings mode

The cog on the viewer switches the whole tab to settings; **Back to map** (arrow, top left)
returns. A **module selector** (the Overlay tab's control: `theme::segmented` in a `WELL` frame,
`overlay_tab::page_selector_with`, one row or two where the labels don't fit) picks the page:

| Page | Content | Config |
|---|---|---|
| **Minimap** | the HUD minimap: Minimap card (Enabled, *Use Dashboard map settings*, view options, a **Co-Op** block: *Use Dashboard co-op settings*, teammates, shared waypoints, trails + fade time / distance, then *Reset map layers*) + the layer cards | `overlay.map_*`, `overlay.coop_*`, `overlay.map_layers`, `overlay.map_plate_opacity` |
| **Dashboard map & Viewer** (DE *Dashboard-Karte & Viewer*) | "Dashboard map & Viewer" card (**Show Dashboard map**, view options, **Allow pan and zoom**, **Render FPS limit**, *Reset map layers*), a **Co-Op** card (tracer fade time / distance, player list and its columns) + the layer cards; edits what both the Dashboard's Map widget and the Map tab viewer draw (D73) | `disabled_modules` (Map), `minimap_*`, `minimap_layers`, `minimap_allow_pan_zoom`, `minimap_fps_limit*`, `coop_trail_fade_*`, `coop_map_playerlist`, `coop_list_*` |
| **Map data** | the road-type map editor card, [map-editor.md](map-editor.md), and the **Map image** card (image quality, *Reload Map*, *Rebuild Map Cache*, the advanced calibration) | `minimap_quality`, `minimap_px_per_m`, `minimap_world_origin_x/z` |

- **Moved, not copied:** the Minimap and Dashboard map pages came from the Overlay tab
  (D63-D66) with identical content (the Dashboard page was renamed *Dashboard map & Viewer* in D73); the Map data card from Setup. The Overlay tab keeps the HUD
  modules only (General, Drive cluster, Race / Drift, Notifications), including the Minimap
  module's cell in its Layout card and, since D75, its **Minimap frame** (shape, size, outline,
  background).
- **Remembered across restarts:** `map_tab_settings` (settings vs viewer) and `map_tab_page`
  (`config::MapPage`), both in `config::EXPORT_EXCLUDE`, like `overlay_page` (where the user
  last looked is not a setting; kept across a profile switch). An old `overlay_page` of
  `minimap` / `dashboard_map` loads as General. An old `map_tab_page` of `viewer` (the page D73
  removed) loads as the shared Dashboard map & Viewer page (`#[serde(alias = "viewer")]` on
  `MapPage::DashboardMap`).
- **Removed config keys (D73):** `viewer_layers`, `viewer_north_up`, `viewer_mirror_edges`,
  `viewer_show_compass`, `viewer_allow_pan_zoom`, `viewer_zoom_m`, the `viewer_layers_default`
  function and the export group **Map -> Map viewer** (it was the last group, so no group index
  shifts). Their values are **not migrated**: the user wants the Dashboard map's look. An old
  `config.json` / profile that still has them loads normally: `AppConfig` has no
  `deny_unknown_fields`, so serde ignores them (no `.bad-` backup, nothing reset, see
  `config::from_value_lenient`), and the next save writes the file without them. An old preset that
  has them imports with the keys ignored. Test: `map_tab::tests::an_old_config_with_viewer_keys_still_loads`.
- The viewer's co-op trails (fade time / distance) and player list follow the Dashboard map's
  co-op settings (the Dashboard map & Viewer page's Co-Op card), as they always did.
- **Map data page layout:** the Map data card in the first column (three from 1100 px, else two),
  as wide as it was in Setup, and the Map image card in the second. The page runs the Game Install
  check itself (`Fh6Setup::poll`).

### Map settings are only here (D79)

*Why (the user, 2026-10-09):* "make sure that all of the map settings are gone from the mini-settings
menu and that they are placed inside of the map settings tab". **Mini-Settings has no map setting
any more**: its Dashboard -> **Map** sub-tab (General + Co-Op) and the **Minimap** and **Co-Op**
sections of its Overlay tab are gone (the Overlay tab keeps Notifications; `MiniMapTab` and
`ForzaApp::page_map_sub_tab` were removed with them). Where each control went:

| Was in Mini-Settings | Now |
|---|---|
| Lock north-up (F10), north up when stopped, smooth rotation, movement direction, mirror at edges, right stick, compass, zoom driving / stopped | already the View rows of the Dashboard map & Viewer / Minimap cards (`view_rows`) |
| Allow pan and zoom | already on the Dashboard map & Viewer card |
| Render FPS limit (+ slider) | **moved**: Dashboard map & Viewer card |
| Image quality, Reload Map, Rebuild Map Cache, Advanced calibration | **moved**: Map data page, Map image card (`map_tab::map_image_card`; the reload code is `ForzaApp::reload_map_image(rebuild)`) |
| Co-Op: tracer fade time / distance, player list + columns | **moved**: Dashboard map & Viewer page, Co-Op card |
| Overlay -> Minimap: use Dashboard map settings, view options, zoom | already on the Minimap card |
| Overlay -> Co-Op: use Dashboard co-op settings, teammates, waypoints, trails, fade | **moved**: Co-Op block of the Minimap card (while "use Dashboard" is on the rows show what the HUD uses, greyed) |

**D90 (the user, 2026-10-10):** "Co-Op map settings should also be in the map tab. The mini-settings
\> Dashboard > Map settings should all be moved to the map tab, so there are no settings left
there!!!" An audit of the whole Mini-Settings window and the Co-Op tab found one map control left:
**Dashboard -> Modules -> Map** (the checkbox that adds / removes the Dashboard's Map module,
`disabled_modules` holding `WidgetKind::MiniMap`). It is now **Show Dashboard map** at the top of
the Dashboard map & Viewer card (`map_tab::dashboard_map_shown`); switching it runs
`ForzaApp::dashboard_map_toggled` (loads the map image when it came on, drops the texture when it
went off), the code that used to sit in the Modules list. The other Modules (including
**Co-Op Players**, a roster widget, not a map layer) stay. The Co-Op tab holds no map settings
(identity, pacing, session, roster), and its **Player color** is the player's identity, not a map
setting. Test: `app::tests::mini_settings_window_touches_no_map_config` (the Mini-Settings window
body references no `minimap_*`, `map_layers`, `WidgetKind::MiniMap`, `overlay.map_*` or co-op map
key).

There is no "Map settings..." link in Mini-Settings (considered, left out: the Map tab is in the tab
bar with its own cog, and a link for settings that moved once is clutter). Test:
`app::tests::mini_settings_has_no_map_controls` (a source check, since the window is built inline in
`ForzaApp::update`), `map_tab::tests::map_settings_from_mini_settings_are_on_the_map_tab`.

### The layer cards

Both map pages call **one function**, `maprender::ui::layers_ui` (D61 spirit: one renderer, so
one settings UI, they can't drift apart). It lays the cards out in three columns from 1100 px
(first column: the page's own lead card + Image, View mode, Race lines; then Roads; then Points
of interest) or two, and edits a `MapLayerConfig`. Above the cards a status line shows the
layer store's state (no install / loading / loaded / error) and `MapLayers::note` (e.g. "your
saved road types were ignored").

- **Image:** satellite on/off, opacity, brightness, saturation (approximate, a grey veil); HUD
  only: **Map plate opacity** (`overlay.map_plate_opacity`, the minimap's own plate, not the
  General tab's *Plate opacity*); the same field is also on the Overlay tab -> Minimap page
  (**Minimap frame** card, D75).
- **Roads:** on/off, scale width with zoom (road width in metres, minimum / maximum px; off =
  one fixed width; the three px sliders go up to **100 px**, `maprender::ui::ROAD_PX_MAX`, D74), outline width and opacity, then **By type**: per type a block with
  visible, line colour, outline colour, width factor, dash, opacity, outline on/off. Turnarounds
  are never listed (D52). **Reset road styles** restores the "by type" preset.
- **Points of interest:** on/off, icon size, **Max zoom radius** (tooltip: POIs are hidden
  while the view radius is above it; the default is 10 km on the Dashboard map and 3 km on the Minimap),
  only near the car + radius, gate lines, and the categories as checkboxes grouped Events /
  Zones and gates / Places / Collectibles with All / None per group. Each shows the game's icon
  (this context's `IconTex`, `ForzaApp::minimap_icons`, the Dashboard's) or, without an
  install, the coloured fallback marker. A test guarantees every `style::POI_CATS` id has a
  checkbox.
- **Race lines:** mode (off / current race / nearest line / near the car / all lines), search
  radius (nearest / near), **Race line style** (D80, `RaceCfg::route`: *Road*, the default, = the race
  is drawn as a road of its own in the race colour (outline, rounded ends, as wide as a highway; in 3D
  a real road at the race's own heights), or *Line* = the thin line on top), width and opacity (greyed
  unless *Line*: a race road has its own width and is opaque), one **Race colour** for every race, circuit or sprint (default orange `#f97316`, not the road blue; D88; the field is `RaceCfg::color`, which reads the old `circuit_color` through a serde alias and ignores the old `sprint_color`; a saved old circuit default `#38bdf8` is migrated to orange on load by `migrate_circuit_color`), start /
  finish marks, then
  the section **In a race** (the in-race focus, D66, [minimap.md](minimap.md#in-race-focus-d66-other-roads-muted-pois-hidden)):
  *Other roads* (Normal / Muted / Hidden / **Race road only**, D82), and for Muted the colour, opacity and width factor,
  and *Hide points of interest in a race*. *Race road only* draws nothing of the road network in a
  race, only the race road (the user: "a setting, to not draw anything from the normal road mesh and
  only draw the circuit using the 3d renderer"); with the Line style it hides the other roads
  instead (`OtherRoads::effective`). *The focus and the 3D scene:* since D88 every drawn race
  line, the Line style and the start / finish marks are part of the GL scene (`gl3d::Race3d`) whatever
  the focus is; the focus (`gl3d::Focus3d`) is handed over only when *Other roads* is not Normal, and
  only drives the muting / hiding and *Race road only*. Tooltip: applies only in the Current race mode while
  the car is in a race and a line was detected. The rows are greyed in the other modes (the
  focus never applies there), the three muted-look rows unless *Muted* is chosen.
- **View mode:** Flat / Tilted / 3D, then the tilt rows and (3D only) the relief options; see below.
- **View options** (the lead card): lock north-up and its heading-up-only options, mirror,
  right-stick look, compass, zoom driving / stopped (50-6000 m). `maprender::ui::ViewCfg`
  copies them out of `AppConfig::minimap_*` or `OverlayConfig::map_*` so one function edits
  both. (Mini-Settings used to have a copy of these; it has none since D79.)
- **HUD tab with "Use Dashboard map settings" on:** the layer cards and view options show the
  *Dashboard's* values, greyed (what `OverlayConfig::effective` really uses), so the page never
  shows numbers that differ from what is drawn. The plate opacity stays editable: it is not
  copied. With the module or the overlay off the cards are greyed like the other module cards.

- **Road width limit (D74).** *Why (the user, 2026-10-09):* "when I zoom in a lot in the viewer ... at
  some point it just limits the width when I zoom in really far, and it shouldn't do that ... just
  raise the limit to, like, 50, and then I should be able to set it the way I actually want it to
  behave." The zoom rule is `clamp(px_per_metre * metres, min_px, max_px)`
  (`style::road_base_px`), so what looked like a bug was `max_px` (default 10, slider max was 30).
  The slider ranges (minimum, maximum, fixed width) are now 0.5-100 px; the **defaults are
  unchanged** (the user tunes them). Checked: `road_base_px` is the only clamp on the base width in
  `maprender/{style,paint2d}.rs` and `gl3d/roads.rs` (which calls `road_base_px` too); the per-type
  factor and outline only multiply / add. Maximum width and minimum width have tooltips.
  Test: `map_tab::tests::the_road_width_limit_goes_up_to_100_px`.

- *Why colours use egui's `color_edit_button_srgb`:* the config stores `"#rrggbb"` (`Rgb`),
  the picker edits the 3 bytes directly.

### View mode (D65, D67, phase K)

The card that used to be "Tilted view". Every map (HUD Minimap, Dashboard map and Viewer) has the same
three modes, picked with a segmented control (`theme::segmented`) at the top of the card:

- **Flat:** the 2D map, top-down. The tilt rows below are greyed.
- **Tilted:** a flat perspective of the 2D map (D65): **Angle**, **Perspective**, **Car position**,
  **Thinner lines in the distance** (`TiltCfg::{angle_deg, perspective_px, car_y, taper}`).
- **3D:** the same camera, now over the terrain (relief) with the roads as decks. The same tilt rows
  apply (angle, perspective and car position *are* the 3D camera's pitch / lens / car position), except
  *Thinner lines*, greyed because 3D sets line widths itself. Tooltip on the **3D** segment: "Uses your
  graphics card; falls back to Tilted if it isn't supported." On Windows the segment reads "3D
  (experimental)" and a checkbox **Allow 3D on Windows** (`OverlayConfig::map_3d_windows`, one flag for
  all three maps) appears while 3D is picked; until it is ticked the maps stay Tilted. Under the
  segmented control a **status line** appears in 3D mode only: "Loading terrain…" while the terrain is
  read, "3D not available: <reason>" when the GL renderer gave up (old OpenGL, shader error, "3D is too
  slow on this GPU") or there is no install; nothing when all is well (`maprender::ui::status_3d`). The
  map then draws Tilted by itself. Below the tilt rows a **3D** group appears
  (hidden in Flat and Tilted, not greyed: a block of rows that mean nothing there):
  - **Road height** (`ReliefCfg::road_height`): *Node heights* (default) or *On the terrain*.
    Tooltip: node heights put bridges and ramps at the height of the game's road network, the other
    option lays every road on the ground; cross-country is always on the ground and jumps are a taut string.
  - **Deck thickness** (`deck_m`, 0 to 20 m, step 0.5), **Height exaggeration** (`exaggeration`,
    0.5 to 3 x), **Hill shading** (`shading`, 0 to 100 %). The ranges are `ReliefCfg::*_RANGE`; the
    card applies `ReliefCfg::sane()` while the 3D rows show, so a hand-edited config can't leave them.
  - **Car marker** (`ReliefCfg::marker`, D78): *Arrow* (default, a 3D version of the flat arrow) or
    *Car* (a small low-poly sedan), a dropdown like Road height. Both are drawn by the GL renderer at
    the car's telemetry position **and height** (also down in a tunnel), with the trails at their
    recorded heights (D77); Flat and Tilted keep the flat arrow. Tooltip: "How your car is drawn in the
    3D view: a 3D arrow or a small car model. Both sit at the car's real position and height, also in
    tunnels." *Why a choice:* the user asked for both and to decide himself; Arrow keeps today's look.
    Models, sizing and the tunnel rule: `docs/features/minimap.md`, "3D: the own car and the trails in
    the scene".

The control writes only `tilt.on` / `tilt.relief.on` (`TiltCfg::set_view_mode`; the mode is derived by
`view_mode()`), so switching Flat -> 3D -> Tilted keeps every value the user set. Code:
`maprender::ui::{tilt_card, view_mode_picker, relief_rows}`; the card's "Copy to…" is
`LayerCategory::Tilt`, which copies `tilt` whole, relief included (test:
`copying_view_mode_copies_the_relief_fields`).

*Why:* **D67** put all map settings on the Map tab and gave all three maps 2D / tilted / 3D, so the
choice is one card instead of a checkbox plus a separate 3D switch; **D65** made tilted the flat
perspective and 3D the same camera plus relief, so the tilt rows are shared rather than duplicated;
**D51 / D61**: *Node heights* is the default because the user reversed the earlier "always drape"
decision (bridges and expressways should float), and the drape stays one click away. The tooltip
on 3D is there because the 3D view needs a GL context and falls back to Tilted without one.
Why the 3D rows are hidden, not greyed: they are a whole group that means nothing outside 3D, and
a card full of dead controls is noise (the tilt rows, which Flat can switch on, stay and grey).

### Copy to … (D68, simplified by D73)

*Why (the user, 2026-10-08):* "to make it easier to configure both the minimap and the dashboard
map for every category, there should be a button to basically apply, like for example, if I'm in
the minimap settings and then I change the color of the roads, then I would have to do it by hand
for the dashboard map too. And so it would be cool to just at the end of every category have like
a button that basically overrides the other map with the same settings, so I can easily port
them in between."

- **Where:** a small right-aligned **Copy to…** menu button at the end of every card: Image,
  View mode, Race lines (incl. the in-race focus), Roads, Points of interest, and the page's
  lead card (**View**). It lists the other map and **Both**. A pick overwrites **that
  category only** on the target; every other category is untouched. The button then reads
  "Copied" for 1.5 s (`copy_menu`, time kept in egui's temp memory).
- **Two maps since D73:** the viewer shares the Dashboard map's settings, so `MapId` has only
  `Minimap` and `Dashboard` (shown as "Dashboard map & Viewer"). The menu lists the one other map and
  **Both** (which is then the same single target; kept so the button looks the same on every card).
- **Code:** `MapLayerConfig::copy_category(&from, LayerCategory)` (`maprender/cfg.rs`, the one place
  that knows which field is in which category) and `maprender::ui::apply_copy(&mut AppConfig,
  &CopyRequest)`. `layers_ui` stays pure: it only edits the config it was given and **returns** the
  `CopyRequest` (`from: MapId`, `what: CopyWhat::Layer(cat) | View`, `to`); the page
  (`map_tab::apply_requests`) applies it **after** writing its own edits back, so the source
  includes this frame's changes and a stale clone never overwrites the target. *Why a request, not
  `&mut` access to the other configs:* testable without an app, no aliasing of three configs
  in one UI function. Tests: `maprender::cfg::tests::copy_category_*`, `maprender::ui::tests`
  (every category, both directions, Both, following Minimap, View), `map_tab::tests` (real
  pointer events through the menu).
- **Minimap following the Dashboard map** (`map_use_dashboard`): copying **into** it is pointless
  (it draws the Dashboard's values), so its entry and *Both* are greyed with a tooltip, and
  `apply_copy` refuses it anyway. Copying **from** its page copies the effective values (the
  Dashboard map's), the Copy buttons stay usable on the greyed cards.
- **View:** the view options live under different keys per map (`overlay.map_*` / `minimap_*`).
  Copying copies all of `ViewCfg` (north-up and its sub-options, mirror, right stick, compass, both
  zooms). The Image card's **Map plate opacity** (HUD-only, its own key) is not part of the copy.
  With the Minimap module off the Minimap card (and its View copy) is greyed like the rest of that
  card. (Before D73 the viewer was a third target with a reduced field set and no zoom pair; that
  special case is gone with it.)

## Tests (`ui/map_tab.rs`, `ui/map_data.rs`, `ui/map_scene.rs`)

`ui::test_render` panes at 700 / 1000 / 1100 / 1235 px, **English and German** (the language is
a process-wide static: such tests go through `i18n::with_language`, which serialises them): the
Minimap and Dashboard map & Viewer pages (six cards each, seven on the Dashboard page with its
Co-Op card; following the Dashboard, module off, every layer status), the Race lines card with
either style and *Race road only*, the controls that came from Mini-Settings (D79) incl. the Map
image card, the viewer's buttons (inside the tab, bottom right / left, Follow car only while
manual, never overlapping the compass or the co-op list), the module selector at the window minimum (three pages), the remembered
state (saved, not exported), old configs with `viewer_*` keys / `map_tab_page: "viewer"`, the road
width range; the Map data
card's pane and confirm-block tests moved with it.
