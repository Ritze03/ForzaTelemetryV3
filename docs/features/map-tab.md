# Map tab (`src/ui/map_tab.rs`, `src/ui/map_scene.rs`)

A top-level tab (icon `MAP`, `fa-map`; label **Map**, German **Karte**) between **Overlay** and
**Power Curve**. Two modes: the **viewer** (the whole tab is the map) and a full-size
**settings** mode for every map.

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

- **Drag** pans, **wheel / pinch** zooms (radius 50-8000 m). While the view follows the car the wheel
  zooms around the car (the car stays in the middle; there is nothing to anchor to); once panned
  it zooms **around the cursor** (the point under the pointer stays under it). Panning with a
  tilted camera keeps the grabbed point under the pointer too (`map_scene::panned` /
  `zoomed_at` use `Camera::unproject`).
- **Follow car** (top left, lit while following; the default): the view centre is the car.
  Panning turns it off; pressing the button brings the view back at once (also resets a zoom).
  With nothing manual, pressing it freezes the view where it is.
- **Settings** (top right, cog) switches the tab to the settings mode. The zoom radius is shown
  bottom left.
- **North-up** by default (`viewer_north_up`); off = the map turns with the car's heading
  (reusing the Dashboard's eased heading, `minimap_smoothed_yaw`).

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
- **Viewer:** the base zoom is `viewer_zoom_m` (Viewer page, default 1500 m).
- **Option, default on:** `minimap_allow_pan_zoom` (Mini-Settings → Dashboard → Map, and the Map tab's
  Dashboard map page), `viewer_allow_pan_zoom` (Viewer page). Off = no pan / zoom sensing, the
  old click-only behaviour (the Dashboard map keeps its waypoint click).
- Tests: `map_scene::tests` (`manual_view_resets_when_the_player_drives_off`,
  `a_nudge_does_not_reset_the_view`, `panning_while_driving_waits_for_the_next_stop`,
  `manual_view_overrides_the_base_and_resets`, pan / zoom anchoring).

## Settings mode

The cog on the viewer switches the whole tab to settings; **Back to map** (arrow, top left)
returns. A **module selector** (the Overlay tab's control: `theme::segmented` in a `WELL` frame,
`overlay_tab::page_selector_with`, one row or two where the labels don't fit) picks the page:

| Page | Content | Config |
|---|---|---|
| **Minimap** | the HUD minimap: Minimap card (Enabled, *Use Dashboard map settings*, view options, co-op teammates, *Reset map layers*) + the layer cards | `overlay.map_*`, `overlay.map_layers`, `overlay.map_plate_opacity` |
| **Dashboard map** | Dashboard map card (view options, **Allow pan and zoom**, *Reset map layers*) + the layer cards | `minimap_*`, `minimap_layers`, `minimap_allow_pan_zoom` |
| **Viewer** | Viewer card (lock north-up, mirror, compass, **Allow pan and zoom**, **Zoom**, *Reset map layers*) + the layer cards | `viewer_*` |
| **Map data** | the road-type map editor card, [map-editor.md](map-editor.md) | none |

- **Moved, not copied:** the Minimap and Dashboard map pages came from the Overlay tab
  (D63-D66) with identical content; the Map data card from Setup. The Overlay tab keeps the HUD
  modules only (General, Drive cluster, Race / Drift, Notifications), including the Minimap
  module's cell in its Layout card.
- **Remembered across restarts:** `map_tab_settings` (settings vs viewer) and `map_tab_page`
  (`config::MapPage`), both in `config::EXPORT_EXCLUDE`, like `overlay_page` (where the user
  last looked is not a setting; kept across a profile switch). An old `overlay_page` of
  `minimap` / `dashboard_map` loads as General.
- The viewer's settings are the KeyGroup **Map → Map viewer** (`viewer_layers`, `viewer_north_up`,
  `viewer_mirror_edges`, `viewer_show_compass`, `viewer_allow_pan_zoom`, `viewer_zoom_m`);
  `minimap_allow_pan_zoom` is in the Mini-settings group. All `serde(default)`.
- **Viewer defaults** (`config::viewer_layers_default`): the Dashboard map's look
  (`MapLayerConfig::dashboard()`) with POIs visible up to at least an 8000 m radius, i.e. at every viewer
  zoom (a viewer is mostly used zoomed out, where the POIs are the point). Since D71 the
  Dashboard default is 10 000 m, so the viewer just takes that (`max(.., 8000)`). North-up, mirror at the edges, no compass, 1500 m.
- The viewer's co-op trails (fade time / distance) and player list follow the Dashboard map's
  co-op settings (Mini-Settings → Dashboard → Map → Co-Op), not a set of their own.
- **Map data page layout:** one card in the first column (three from 1100 px, else two), as wide as
  it was in Setup. It runs the Game Install check itself (`Fh6Setup::poll`).

### The layer cards

All three map pages call **one function**, `maprender::ui::layers_ui` (D61 spirit: one renderer, so
one settings UI, they can't drift apart). It lays the cards out in three columns from 1100 px
(first column: the page's own lead card + Image, Tilted view, Race lines; then Roads; then Points
of interest) or two, and edits a `MapLayerConfig`. Above the cards a status line shows the
layer store's state (no install / loading / loaded / error) and `MapLayers::note` (e.g. "your
saved road types were ignored").

- **Image:** satellite on/off, opacity, brightness, saturation (approximate, a grey veil); HUD
  only: **Map plate opacity** (`overlay.map_plate_opacity`, the minimap's own plate, not the
  General tab's *Plate opacity*).
- **Roads:** on/off, scale width with zoom (road width in metres, minimum / maximum px; off =
  one fixed width), outline width and opacity, then **By type**: per type a block with
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
  radius (nearest / near), width, circuit and sprint colours, opacity, start / finish marks, then
  the section **In a race** (the in-race focus, D66, [minimap.md](minimap.md#in-race-focus-d66-other-roads-muted-pois-hidden)):
  *Other roads* (Normal / Muted / Hidden), and for Muted the colour, opacity and width factor,
  and *Hide points of interest in a race*. Tooltip: applies only in the Current race mode while
  the car is in a race and a line was detected. The rows are greyed in the other modes (the
  focus never applies there), the three muted-look rows unless *Muted* is chosen.
- **Tilted view:** on/off, angle, perspective, car position, thinner lines in the distance.
- **View options** (the lead card): lock north-up and its heading-up-only options, mirror,
  right-stick look, compass, zoom driving / stopped (50-6000 m). `maprender::ui::ViewCfg`
  copies them out of `AppConfig::minimap_*` or `OverlayConfig::map_*` so one function edits
  both. Mini-Settings keeps its quick options; both edit the same keys.
- **HUD tab with "Use Dashboard map settings" on:** the layer cards and view options show the
  *Dashboard's* values, greyed (what `OverlayConfig::effective` really uses), so the page never
  shows numbers that differ from what is drawn. The plate opacity stays editable: it is not
  copied. With the module or the overlay off the cards are greyed like the other module cards.

- *Why colours use egui's `color_edit_button_srgb`:* the config stores `"#rrggbb"` (`Rgb`),
  the picker edits the 3 bytes directly.

### Copy to … (D68)

*Why (the user, 2026-10-08):* "to make it easier to configure both the minimap and the dashboard
map for every category, there should be a button to basically apply, like for example, if I'm in
the minimap settings and then I change the color of the roads, then I would have to do it by hand
for the dashboard map too. And so it would be cool to just at the end of every category have like
a button that basically overrides the other map with the same settings, so I can easily port
them in between."

- **Where:** a small right-aligned **Copy to…** menu button at the end of every card: Image,
  Tilted view, Race lines (incl. the in-race focus), Roads, Points of interest, and the page's
  lead card (**View**). It lists the other two maps and **Both**. A pick overwrites **that
  category only** on the target; every other category is untouched. The button then reads
  "Copied" for 1.5 s (`copy_menu`, time kept in egui's temp memory).
- **Code:** `MapLayerConfig::copy_category(&from, LayerCategory)` (`maprender/cfg.rs`, the one place
  that knows which field is in which category) and `maprender::ui::apply_copy(&mut AppConfig,
  &CopyRequest)`. `layers_ui` stays pure: it only edits the config it was given and **returns** the
  `CopyRequest` (`from: MapId`, `what: CopyWhat::Layer(cat) | View`, `to`); the page
  (`map_tab::apply_requests`) applies it **after** writing its own edits back, so the source
  includes this frame's changes and a stale clone never overwrites the target. *Why a request, not
  `&mut` access to the other configs:* testable without an app, no aliasing of three configs
  in one UI function. Tests: `maprender::cfg::tests::copy_category_*`, `maprender::ui::tests`
  (every category, every map pair, Both, following Minimap, View), `map_tab::tests` (real
  pointer events through the menu).
- **Minimap following the Dashboard map** (`map_use_dashboard`): copying **into** it is pointless
  (it draws the Dashboard's values), so its entry and *Both* are greyed with a tooltip, and
  `apply_copy` refuses it anyway. Copying **from** its page copies the effective values (the
  Dashboard map's), the Copy buttons stay usable on the greyed cards.
- **View:** the view options live under different keys per map (`overlay.map_*` / `minimap_*` /
  `viewer_*`). HUD <-> Dashboard map copies all of `ViewCfg` (north-up and its sub-options, mirror,
  right stick, compass, both zooms). To or from the **viewer** only what it has: lock north-up,
  mirror at edges, compass, and *Allow pan and zoom* between the viewer and the Dashboard map (the HUD
  cannot pan). *Why not the zoom:* the viewer has one zoom, not a driving / stopped pair, so there is no
  honest mapping. The Image card's **Map plate opacity** (HUD-only, its own key) is not part of the
  copy. With the Minimap module off the Minimap card (and its View copy) is greyed like the rest of
  that card.

## Tests (`ui/map_tab.rs`, `ui/map_data.rs`, `ui/map_scene.rs`)

`ui::test_render` panes at 700 / 1000 / 1100 / 1235 px, **English and German** (the language is
a process-wide static: such tests go through `i18n::with_language`, which serialises them): the
Minimap / Dashboard map / Viewer pages (six cards each, following the Dashboard, module off, every
layer status), the viewer's buttons (inside the tab, never overlapping), the module selector at
the window minimum, the remembered state (saved, not exported), the viewer defaults; the Map data
card's pane and confirm-block tests moved with it.
