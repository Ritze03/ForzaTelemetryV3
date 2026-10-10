# Minimap

The **Map** widget (`show_minimap_widget` in `src/ui/dashboard.rs`) renders the
car's position over a cached top-down map image, using a world-metres → screen-pixel
transform calibrated to the game's coordinate space. It's one of the standard
[[dashboard]] widgets.

## Map image & seasons

FH6's overworld map skin rotates weekly (Spring → Summer → Autumn → Winter,
`current_season()` in `src/minimap.rs`, a wall-clock rotation keyed off a fixed epoch,
not read from packets). The app loads and colour-caches the matching map image for the
detected season in a background thread (`app.rs:map_load_thread`), at a configurable
**Image quality** (20–100%, lower = faster load/less memory); **Reload Map** re-fetches,
**Rebuild Map Cache** clears the on-disk cache (`app_data_dir()/map_cache`, including the
HUD overlay's copy) first. The season is re-checked continuously, so the image swaps
automatically when the in-game season changes.

## Map image source

The season maps are **read from the user's own Forza Horizon 6 install at runtime**
(`src/gamedata/tiles.rs`): `<media>/UI/Textures/Data_Bound/Map_Brio_<Season>.zip` holds a
tile pyramid of BC1 `swatchbin` tiles (format: `docs/game-data/fh6-game-files.md` §2). Level 3
(8×8 tiles) is the 8192² map, level 2 the 4096² one. Which level feeds which **Image quality**:

| Quality | Source level | Cached size |
|---|---|---|
| 100 % | L3 mosaic | 8192² |
| 50 % (the HUD's, and a Dashboard set to 50 %) | L2 directly | 4096² |
| anything else | L3, triangle-resized | quality % of 8192 |

The cache header's "original size" is **always 8192²**, whatever the cached size, because
`MapCalibration` works in 8192-px space (`MapCalibration::DEFAULT` was calibrated on the
bundled jpgs, which are re-encodes of level 3, so it applies unchanged).

*Why from the install:* the app used to bundle four 8192² JPEGs (~109 MB, the binary was
145 MB; now ~31 MB). The map is Playground Games' IP, so the app must not ship it; it reads
the user's own copy instead (licensing rule, see `docs/game-data/fh6-game-files.md`). A bonus:
decoding the tiles is ~60 ms (one thread per tile row, `std::thread::scope`) against ~2.3 s
for the JPEG decode. *Why L2 directly for 50 %:* resizing level 3 down to 4096² costs ~1.7 s
(longer than the whole level-3 decode) and level 2 is the same imagery.

*Why the credit for the bundled images is gone:* the credits link (Setup → Repository /
Credits) thanking the Reddit user whose seasonal map images were bundled was kept only while
those images were used, and left with them.

**Rebuild Map Cache** is disabled without an install. Why: it deletes `map_cache/`, which without an install is the only copy of the map.

**No install:** the map needs the game on this machine (found through Setup → Game Install,
`FH6_INSTALL_DIR` or Steam). Without it, and without a cache from an older build, the load
fails with a `MapLoadError` (`NoInstall`, `NotReadable`, `MissingZip`, `Decode`):
- the **Dashboard map** shows "Map needs your Forza Horizon 6 install" / "Set it in Setup →
  Game Install" instead of the spinner (`ForzaApp::minimap_error`); it retries on **Reload
  Map**, **Rebuild Map Cache**, when the Map widget is re-enabled, when the season changes,
  and when the Game Install folder changes;
- the **HUD minimap** just stays without a map; its loader forgets the failed attempt so the
  once-a-minute season check retries.

The install folder reaches the two loader threads (they have no config access) through a
process-wide `install::set_user_dir`, which the app sets from `config.fh6_install_dir` at
startup and whenever it changes; `find_media` tries the explicit argument, that folder,
`FH6_INSTALL_DIR`, then Steam.

## Shared code (`src/minimap.rs`)

The season logic, the image loading/cache and the map maths live in `src/minimap.rs`,
shared by this widget and the in-game HUD overlay's minimap ([[overlay]]). Nothing in it
takes `&ForzaApp`, so the overlay thread can call it.

- **Cache:** `load_map_color_image(season, quality)` builds the image from the install's tiles
  once (`decode_and_cache_season`), writes `map_cache/<season>_q<quality>.bin` (format and path
  unchanged from the JPEG era, so older caches stay valid and keep working without an
  install), and reads that file afterwards. Both return `Result<_, MapLoadError>`. The write is
  **atomic** (unique temp file, then rename). *Why:* the Dashboard loader and the overlay's
  `hud-map` thread can build or read the same file at once, and a reader must never see a
  partial file.
- **Maths:** `MapCalibration` (world ↔ UV in original-image pixels), `MapView` (heading-up
  offsets, UV per mesh vertex, compass direction, arrow angle), `target_yaw` /
  `ease_yaw` / `ease_zoom`, and the stopped rule (`STOPPED_KMH` 5, `STOPPED_SECS` 1.5).
  The Dashboard camera in `app.rs` uses these too.
- **The overlay's copy:** `overlay_map_image(season)` is the **q50 (4096²)** cache file,
  uploaded with `OVERLAY_MAP_TEXTURE_OPTIONS`: linear filtering with **trilinear mipmaps**
  (egui_glow builds the chain on upload) and **MirroredRepeat** wrapping. *Why mipmaps:* the HUD map is
  heavily minified and rotates, which shimmers without them. The Dashboard map has no
  mipmaps (it aliases; out of scope so far) and its texture wraps with MirroredRepeat
  (visible when **Mirror map at edges** lets UVs past the edge).
  The overlay texture now wraps with `MirroredRepeat` too, so **Mirror map at edges** works on both
(the HUD cuts the pill to the image when mirroring is off, see [[overlay]]). The q50 file is shared with a Dashboard set
  to 50 % quality. The overlay keeps no RAM copy: it uploads, then drops the image.

## Calibration

World coordinates map to image pixels via three tunable constants under
**Advanced calibration**:

- `minimap_px_per_m` — pixels per metre.
- `minimap_world_origin_x` / `minimap_world_origin_z` — world X/Z at pixel (0,0).

These default to values derived from in-game reference points; **Reset to
defaults** restores them if the car dot drifts off the map after tuning. The HUD overlay's
minimap uses the same three config values (carried in its snapshot).

**Known duplicate:** the default numbers (0.3722, −12540, 10738) exist twice, as
`MapCalibration::DEFAULT` in `src/minimap.rs` and in `AppConfig::default()` in
`src/config.rs`. Change both together.

## Orientation: north-up vs heading-up

- **F10** (or the **Lock map north-up** checkbox) toggles between:
  - **North-up** — the map is fixed; only the car arrow rotates.
  - **Heading-up** (default) — the map rotates under a fixed, up-pointing car
    arrow, using either the raw yaw or (`Use movement direction as rotation`)
    the velocity vector's heading instead.
- **North up when stopped** — in heading-up mode, once speed drops under 5 km/h
  for 1.5s the map eases back to north-up, then eases back to heading-up as soon
  as the car moves again.
- **Smooth rotation** — lerps the map's rotation each frame instead of snapping;
  the ease-to-north animation always lerps regardless of this setting.

## Zoom

The visible radius (metres from the widget's centre to its nearest edge) is
interpolated toward one of two targets: **Zoom when driving** (default smaller
radius, engages immediately above 5 km/h) or **Zoom when stopped** (default larger
radius, engages 1.5s after dropping under 5 km/h) — both configurable, so the map
auto-zooms in while driving and back out once parked.

## Other display options

- **Mirror map at edges** — instead of clipping at the image boundary, the mesh's
  UVs are allowed outside [0,1] and the texture sampler mirrors/repeats, so panning
  past the map edge shows a reflected continuation rather than a hard cutoff.
- **Show compass** — a compass disc with a two-colour needle in the top-left corner
  pointing to world-north (`MapView::north_dir`, so it is right in north-up and heading-up
  modes alike). It is the HUD Minimap's compass, drawn by the shared
  `hud::minimap::draw_compass`, scaled with the widget (`min(w,h)/200`, clamped 0.8-1.6).
  *Why:* the user prefers the HUD look, and one implementation keeps the two identical.
- **Render FPS limit** — throttles how often the car's cached position/yaw are
  refreshed for the minimap, independent of the app's global FPS limit.

## Look-around (right stick)

Option **Rotate with right stick** (`minimap_look_stick`, on by default; Map tab -> settings -> "Dashboard map & Viewer" page (view options); the
HUD Minimap has `OverlayConfig::map_look_stick`, see [[overlay]]). While the right stick
(`Gamepad::right_stick`, post-deadzone, see [[gamepad]]) is deflected, the view turns to look where the
stick points **relative to the car**, like the game's camera.

**Intended behaviour (both modes, both maps):** stick angle `θ = minimap::look_offset((x, y)) =
atan2(x, y)` (up = 0, right = +90 deg, down = 180 deg; clockwise, matching the map yaw). With the stick
held, the view yaw becomes `heading + θ`, where `heading` is the heading-up yaw
(`minimap::target_yaw`, i.e. the car's yaw or movement direction). So stick right puts the car's right
at the top, stick down looks behind it, stick up shows "ahead" (in north-up that turns the map
heading-up for as long as it is held). The car stays at the view centre (the rotation pivot); the car
arrow (`arrow_angle` = `raw yaw - view yaw` = `-θ` in raw-yaw heading) and the compass (`north_dir`)
follow from the same `MapView` yaw, so they stay geometrically correct. Released (or option off), the
view eases back to the mode's own orientation (north, or heading).

**Implementation:** `minimap::LookAround` (`ForzaApp::minimap_look`, `MapAnim::look`) owns the
drawn **view yaw**. The map's own rotation stays as before, the **base** yaw
(`ForzaApp::minimap_base_yaw()`: 0 when north-up, else the smoothed heading-up yaw; the HUD's eased
`MapAnim::yaw`). Each frame `look.step(stick, enabled, heading, base, base_target, dt)` returns the
view yaw (wrapped to (-PI, PI]):

- **Held** (option on, stick past the deadzone): `view = heading + off`, with `off` kept *relative to
  the heading* and eased (`ease_yaw`, rate 6/s, shortest arc) toward `θ`. The base is not read at
  all, so north-up-when-stopped, smooth rotation or the north-up lock changing underneath can't move
  the view; it follows the heading exactly and the stick with the easing.
- **Released** (or option off): `view = base + off`, with `off` kept *relative to the base* and
  decaying to 0 at the same rate; once 0 the view is the base yaw again, with no added lag.
- **On each switch** `off` is re-referenced from the previous frame's view (continuous). On release
  it is `wrap(view - base_target) - wrap(base - base_target)`, unwrapped (up to ±2π) and decayed
  linearly: the base eases to its target (`base_target`: 0 north-up / stopped-north, else the heading)
  by its own short arc and `off` carries the rest, so the view's total turn is the short way to the
  target (a plain `wrap(view - base)` could add two short arcs into a long one while the base is
  still turning).

Both maps call this one type; only `heading` / `base` / `base_target` come from their own state
(Dashboard: `minimap_cached_yaw` / `minimap_base_yaw()` / the `base_target` computed next to the
smoothing in `app.rs`; HUD: `target_yaw(pkt, map_use_movement_dir)` / its eased yaw /
`map_target_yaw`). The HUD's "still animating" check is `LookAround::easing()`.

**Fixed bug (jolt with north-up-when-stopped):** the previous version eased an offset added to the
base, `view = base + look`, with `look` chasing `wrap(heading + θ - base)` at 6/s. When the base
moved, the target moved by minus that change in the same frame, but `look` only caught up at 6/s, so
the view swung with the base and then back. For the base easing to north (`b(t) = b0·e^(-6t)`) the
view error is `e(t) = -6·b0·t·e^(-6t)`, peaking at `-b0/e` (about 37 % of the base's turn) after 1/6 s:
"jolts toward north, then smoothly back to the stick". A snapping base (smooth rotation off, driving
off after a stop) jumped by the whole difference. *Why the new structure:* the user wants the held
view to stay exactly where the stick points whatever the mode does underneath, and to ease from
wherever it is to the mode's orientation on release; keeping the held offset relative to the heading
makes the first true by construction instead of by compensation. Tests:
`minimap::tests::look_held_ignores_north_up_when_stopped` (smooth on/off: held view constant while the
base goes north, monotone release to north, no jump driving off, release to heading),
`look_held_follows_only_heading_in_every_mode` (north-up / heading-up × smooth × stopped-north, turning
+ stop + drive-off: view = heading + θ every frame, never moves more than the heading),
`look_release_takes_the_short_way_while_the_base_still_turns`, and
`hud::minimap::tests::map_anim_held_look_ignores_north_up_when_stopped` (through the HUD's `MapAnim`).

**Fixed bug (north-up offset):** the first version added `θ` straight onto the base yaw
(`view yaw = base + θ`). In heading-up `base ≈ heading`, so that was car-relative; but in north-up
`base = 0`, so the stick turned the map relative to *north* (stick right = east at the top whatever the
car's heading): the result was off by the car's heading, which reads as a "weird offset". The geometry
(pivot, arrow, compass) was always consistent; the reference direction was wrong. Tests:
`minimap::tests::look_is_car_relative_in_north_up_and_heading_up` (view yaw, car at centre, the
stick's world direction at the top, arrow angle, compass, and north-up == heading-up views) and
`hud::minimap::tests::map_anim_look_is_car_relative_in_north_up`.

Why:
- *Car-relative in every mode*: the right stick is the game's camera stick, which looks relative to
  the car; matching it makes the stick mean the same thing in north-up and heading-up (the user
  reported the north-relative version as "weirdly offset").
- *Angle only, not scaled by deflection*: the deadzone already gates it, and scaling the angle by
  magnitude would make a half-pushed stick point at the wrong direction.
- *Its own easing* rather than folded into the base's target yaw: it eases even with "Smooth
  rotation" off, and leaves north-up / ease-to-north unchanged once released. While held, the view
  follows heading changes exactly (no lag) in every mode, since the offset is relative to the
  heading; only stick changes are eased.

It is independent of right-stick *button bindings*: a bound direction still fires its action and still
rotates the map. HUD plumbing: `HudSink::with_stick` stamps `HudSnapshot::look_stick` at publish time
(the listener thread has no gamepad), so it updates per packet.

## Shared drawing (`hud/map_shared.rs`)

The Dashboard map and the HUD Minimap draw their markers with the same functions: `draw_own_arrow`,
`draw_remotes` (teammate arrows, edge pointers, names, paused grey; in 3D the arrow is the GL car, `remote_markers_3d`, D89), `draw_trail` (+ `TrailFade`),
`draw_waypoint`, `draw_destination` (the navigation pin, phase L: [navigation.md](navigation.md) §3) and `draw_compass` (in `hud/minimap.rs`). Each takes a `MapCanvas` (painter,
`MapView`, centre, bounds rect, size factor `s`, fade alpha `a`): the Dashboard passes `s = 1`,
`a = 1`, the HUD its design scale and show/hide fade. Trail recording (`Trail`, `trail_push`) is
shared in `src/minimap.rs`. *Why:* the user wants the HUD map to match the Dashboard's, and one
implementation means a tweak to an arrow, label or trail lands on both. The buffers differ: the
Dashboard's is `ForzaApp::minimap_trails` (UI thread), the HUD's is `CoopLayer` (overlay thread). In 3D the own car and the trails are drawn by the GL renderer instead (`trail_3d`, `gl3d::add_marker`;
see "3D: the own car and the trails in the scene").

## Shared renderer & layers (`src/maprender/`)

Phase J. The Dashboard map and the HUD minimap draw the base image, roads by type, jump lines, race
lines and points of interest through one renderer, `src/maprender/`: both call the very same
`draw_base` / `draw_layers` (the HUD since I29b, with the rounded pill clip, see [[overlay]] "Minimap
M2′"). **Why (D61):** the user wants the two maps to look alike, and one drawing path means they
can't drift. `hud::map_shared` already shares the *markers*; `maprender` shares the *world*. A 3D mode
(phase K) will be one more renderer shared the same way.

| Module | What it holds |
|---|---|
| `cfg.rs` | `MapLayerConfig` (image look, roads + per-type style, POIs, race lines incl. the in-race `focus`, tilt, the navigation route's look `nav_route`) and its serde; `::dashboard()` (= `Default`) and `::hud()`. |
| `data.rs` | `MapLayers { rev, roads, pois, races, icons, race_class, note }` (`Arc`s), `build_roads`, `RoadLayer::joins` (`Joins`: an `End` per chain end + the `Overpass`es, D81), `GameData::load` (nav + POIs + race lines + danger signs + icons), `PoiLayer` (250 m cell grid, the chests apart for the weekly pick), `RaceLayer` (100 m segment grid + arc length per point `cum`). |
| `icontex.rs` | `IconTex`: uploads the store's icon pixels as a texture **per egui context** and builds that context's `IconAtlas`. |
| `store.rs` | The process-wide loader / cache: `layers()`, `refresh_now()`, thread `map-layers`. |
| `view.rs` | `Camera` (flat or tilted; `from_cfg`, `focal_for`, `depth_scale_at_row`), `world_aabb`, `thin`, `clip_convex`, `clip_polyline_convex`, `clip_segment_convex`, `fan`. |
| `style.rs` | Road draw order, the zoom-dependent width rule, dash patterns, the POI category table. |
| `racesel.rs` | `RaceSel`: which race lines to draw, incl. the "current race" candidate tracking (D76) and the in-race focus (`RoadFocus`: which roads lie along what is drawn of the picked line). |
| `ui.rs` | The settings UI (D63): `layers_ui` (the Image / View mode / Race lines / Roads / Points of interest cards, used by both Overlay-tab map tabs), `view_rows` + `ViewCfg` (zoom / orientation options of either config), `status_ui` (store status + `MapLayers::note`). |
| `paint2d.rs` | `draw_base` (image mesh, far-edge fade) and `draw_layers_parts` / `draw_layers_or_route` (roads, jumps, race lines, the navigation route, gate lines, POIs, the current chest) onto an egui `Painter`; `IconAtlas`, `CornerClip`. |

### Data model

All coordinates are world metres (x east, z north; heights ride along for phase K).

- **Roads**: `RoadLayer.by_type[0..10]` (slot 0 = edge without a type, 1..=9 = `RoadType::index()`),
  each a list of `Chain { pts, y, bbox }`; plus `jumps` (take-off x,z,y to landing x,z,y) and per-type
  edge counts. `build_roads` is a port of `preview_2d.py:build`: positions from the nav polylines,
  overridden by the road-type file's `moved` / `points`; per nav polyline the consecutive edges are
  walked, removed edges break the chain, the type comes from the file (missing = slot 0), and
  **consecutive same-type edges are chained** (so dashes run on across nodes); jump edges become
  `jumps` (take-off from `jump_from`, else the first node); `added` links follow as single segments.
  On the project data: 1 798 chains / 41 380 vertices (edges: road 21 701, offroad 7 237, other 317,
  trail 3 943, cross-country 144, tunnel 904, highway 4 996, turnaround 340, jump 18; a real-install
  test pins them). **Joins (D81):** `RoadLayer::joins()` says how every chain end meets the rest
  (`End::Dead` / `End::Join { next, slot }` = exactly one other piece goes on / `End::Junction` = 3+
  arms) and lists the `Overpass`es; see "Road joins and junctions" below. *Why not `mapedit::data::road_graph`:* it rounds to 0.1 m, loses the polyline
  order (no chains) and has no `jump_from`.
- **POIs**: the reader's `Poi`s plus the 15 **danger signs** (`Pois::load_danger_signs`, ~0.2 s, they
  live in the 40 GB GeoChunk0), each mapped to a *category* (`style::POI_CATS`, the demo's category
  ids: `barn_find`, `car_meet`, `speed_zone`, `danger_sign` ...). Items with `|x|` or `|z|` beyond 12 km
  are dropped: 11 parking areas sit at z ~ 18 km, off the map. A 250 m cell grid culls by the view
  box. The chests (`TreasureChest` + `TreasureChestBoard`) are also kept in `PoiLayer::chests` for the
  weekly pick.
- **Icons and race classes**: `MapLayers::icons` is the game's POI icon set (`gamedata::icons::PoiIcons`,
  CPU pixels, ~22 ms) read once on the loader thread, so both maps get the same data; `race_class`
  maps a route id to its `RaceClass` for the race pin icon (below).
- **Race lines**: the reader's `RaceLine`s (170, decimated to 5 m, ~171 k points) plus a 100 m
  segment grid over all segments.

### Store lifecycle

`maprender::layers()` returns `Layers { status, data }` (`NoInstall | Loading | Ready | Error`, and
`Option<Arc<MapLayers>>`). A map calls it only while one of its layers is on, so a user who never
enables them never pays the load (~70 ms release for nav + POIs + race lines; the icons and the danger
signs are read on two scoped threads next to them, ~0.25 s in all, logged as `map layers: loaded in
N ms`).

Two cache levels: the install-derived `GameData` is keyed on the install's `media` path (looked up at
most every 2 s, since Steam detection hits the filesystem) **and the game's season**; the `RoadLayer` is
keyed on `(media, season, mtime + len of the road-type override file)`; the override file (the same key
`MapData::poll` uses for the Setup card) and the season (`minimap::current_season()`, a clock read) are
checked at most once a second. A change of the override file spawns the `map-layers` thread, which
re-reads `RoadTypes::current(override_path(), &nav)` and rebuilds only the roads (~4 ms release); POIs and race
lines are shared `Arc`s, and the old data is served until the new arrives. So an editor **Save** or
**Reset** shows up on the Dashboard within about a second, independently of the editor server and of
the egui frame loop; `app.rs:poll_map_editor` calls `maprender::refresh_now()` on `MapEvent::Saved` to
remove that second. A failed load is remembered with its key and **not retried** until the key changes
(no retry loop); a panic in the loader becomes `Error`. Without an install the status is `NoInstall`
and the map is the image alone, as before.

**Season change = full re-read.** When the season (it turns weekly, Thursday 14:30 UTC) differs from
the one the data was loaded for, the loader thread re-reads *everything* from the install (nav, POIs,
danger signs, race lines, icons) and rebuilds the roads; the old `Arc` stays drawn until the new one is
ready and `rev` goes up. *Why (the user: "on season change, it should re-read those"):* game updates
add things to the files (the 6 Oct update added treasure chests 016-019), and the app keeps running for
days; re-reading weekly picks them up without a restart. *Why everything, not only POIs:* the nav is the
bulk of the ~70 ms and a full reload is the simplest correct thing (one key, one code path); it runs on
the loader thread, so nothing stalls. A failed re-read keeps the old data on screen and is not retried
within that season (`failed` is keyed too, so no retry loop). Test: `store::tests::a_season_change_*`
(fake source).

*Why a global store (not a field of the app, not the HUD snapshot):* the UI frame loop stops while the
game covers the window, which is exactly when the HUD is used, so anything the UI thread has to
forward to the overlay would stall then; the overlay thread polls the store itself (like the install
folder, mirrored into `gamedata::install::USER_DIR` for the helper threads). One load serves both maps.

### Drawing

Order on the Dashboard: base image, then **`draw_layers_parts`** (roads by type, jump lines, race lines with
start / finish marks, the **navigation route** (D84, below), POIs), then trails, teammates, own arrow, waypoints,
the navigation destination pin, compass.

- **CPU `Painter`, no baking.** One `Shape::line` per visible chain, every type's casing first, then
  every type's fill (D81; chain ends mitred or round-capped, overpasses outlined, see "Road joins and
  junctions" below), chains culled by their bbox against the view's world box (`Camera::world_aabb`), vertices
  thinned to >= 2 px apart on screen (`view::thin` keeps the first and last point and is idempotent).
  *Why:* road types change on every editor Save and are toggled per map, and widths are in screen px
  and change with the eased zoom; baking into the map texture or pre-tessellating in world space would
  have to be redone constantly, and the measured cost is small (below).
- **Order** (bottom to top): untyped (grey, dashed: a nav that does not fit the road-type data shows),
  cross-country, other, offroad, trail, tunnel, road, highway. Asphalt ends on top.
- **Turnarounds are never drawn** (D52): they exist so the game's AI can get back onto the right road.
  The config has no entry for them; `RoadStyles::get(Turnaround)` is `None`.
- **Width** (the demo's `roadBase`): `base = clamp(px_per_metre * metres, min_px, max_px)` when
  `scale_with_zoom`, else `base_px`; a type's line is `max(0.7, base * its factor)`, its casing that
  plus `casing_px`. Defaults (D62): 10 m, 1 to 10 px, base 3 px, casing +1.4 px at alpha 1; factors road
  1.0, highway 1.44, offroad 0.875, other 0.81, trail 0.75, cross-country 0.75, tunnel 0.94 (alpha .85),
  jump 0.875. At a 5 km radius (0.042 px/m on a 420 px map) the base is the 1 px
  minimum; at 300 m it is 7 px.
- **Dashes**: dashed types (trail, jump) use the demo's `[6,4]` pattern scaled by `max(1, 0.6 w)`, but
  only above 0.02 px/m (below that they are sub-pixel and draw solid). The casing is always solid.
- **POIs**: a marker per enabled category within the view, nothing above `max_zoom_m` (Dashboard default 10 km, HUD 3 km; since D71 the
  Dashboard shows them at its default zooms, the HUD hides them above a 3 km view), optionally only within
  `radius_m` of the car. Back to front by screen y. **Icons (D64)**: each category draws the game's own
  icon (`gamedata::icons::PoiIcons`, one atlas texture per egui context, `IconAtlas` = a UV rect per
  category plus per race class and per mascot region); a category without an icon (landmarks, creature
  zones, parking areas, flag-rush flags, pinatas, eliminators) or a missing atlas is drawn as a
  coloured circle / diamond / square / ring (the demo's colours). A `race_pin` takes the icon of its
  route's class: the user's hand marks of the route (`RoadTypes::races`: road, rally, cross-country,
  street, touge, drag) plus circuit or point-to-point (`data::race_class_of`; "rally = mixed surface"
  is a guess); an unmarked route gets the class-less pin icon, a mascot the icon of its region.
  Icons scale with the perspective, never below 0.4x.
- **Gate lines**: with `pois.gates` on, the speed traps, speed zones, trailblazers and drift zones draw
  the line across the road between their `Poi::gate` end points, 2.5 px in the category colour over a
  dark casing, under the icons, at least 7 px long about the gate's midpoint (a 15 m gate is
  sub-pixel at 5 km). Only for enabled categories.
- **Danger signs** (`danger_sign`) come from `Pois::load_danger_signs`, merged into the POI layer.
- **Current treasure chest** (`treasure_chest_current`): of the ~20 chests only the one the week names
  is drawn, with the chest icon 1.5x bigger and on top; `PoiLayer::current_chest(week_index_now())` is
  asked every frame (a few dozen items), so the Thursday 14:30 UTC rollover needs no timer. **The
  weekly rule is inferred from one in-game sighting**: chest number = week index - 52 (the week of
  Thursday 14:30 UTC; week 68 = 016, week 69 = 017). Evidence: the user found the current chest in game
  on 2026-10-08 just after 14:30 UTC (week 69) and it was chest 017; the Festival Playlist series are 28
  days with four chests each (`Pois::current_treasure_chest`, `docs/game-data/fh6-game-files.md`). The
  install has no date field to confirm it. **Next check:** chest 018 should go live on 2026-10-15
  14:30 UTC. *Why not "the highest number" like the map viewer:* since the 6 Oct update the file holds
  016-019 ahead of time.
- **Image look**: opacity and brightness are the mesh vertex colour. **Saturation is approximate:**
  egui cannot desaturate a texture, so below 1 a grey veil (alpha `0.6 * (1 - saturation)`) is drawn
  over the same shape.

### Road joins and junctions (D81)

*Why (the user, 2026-10-09):* "In general, we need a better way of connecting the roads to each other.
All intersections look a bit broken. Good enough for a demo, not for a product." and, with a 3D
screenshot of an L-corner with a notch: "It should also connect the nodes to each other in a way, that
allows intersections to be drawn more cleanly". Before D81 every chain was drawn on its own: square
ends at every chain end (a notch on the outside of a corner where two chains meet, an overlap inside),
in 2D each type's casing drawn right before its fill (so a road's outline ran across the mouth of every
lower-ranked road joining it), in 3D the casing a band of each ribbon (the same).

**Topology** (`data::Joins`, built once per road layer on the loader thread, `RoadLayer::joins()`): chain
vertices at bit-identical (x, z) are one nav node (`build_roads` takes every position from the node id,
so a shared node gives identical floats). Every chain end gets an `End`: **`Dead`** (nothing else
there), **`Join { next, slot }`** (exactly one other piece goes on: a type change, an L-corner of two
chains, a ring closing on itself; `next` = that piece's neighbouring vertex) or **`Junction`** (3+
arms; a vertex inside a chain counts two arms, a chain end one). Turnarounds (never drawn) and jump
lines (drawn apart) are no arms, so a road ending at a turnaround is a dead end. Real install: 775
dead ends (674 of them the turnarounds' own), 830 joined ends, 1 991 at a junction. **Roads that only
cross share no vertex and are no junction**: a proper crossing of two segments without a shared vertex,
both heights known and differing by `OVERPASS_DY_M` = 4 m or more, is an **`Overpass`** (503 on the
island; the upper road's slot / chain / segment, the point, its height, the lower road's slot and the
crossing angle), found with a 64 m cell grid. The table is rebuilt for the call when chains were pushed
after it was built (synthetic layers), and a cloned `RoadLayer` drops it. Build ~7 ms release, most
of it the overpass grid (the road rebuild on an editor Save goes from ~5 to ~12 ms, on the loader
thread).

**One rule for both renderers: every casing under every fill.** The outline then runs round the union
of the roads that meet, never across one of them, so a junction is the union of its arms: no polygon
of its own is needed. **Joined ends are mitred** (both pieces end on the corner's bisector, so a road
reads as one line through a type change or round an L-corner); **dead ends and junction ends get a
round cap** of the road's own width (at a junction the caps of the arms and the through road overlap
at the node; the top-ranked arm's fill is on top, so the patch takes the "biggest" road's style, a
highway's rounded end over the road it joins).

*Why not a junction polygon with the ribbons trimmed to it* (the first idea): the road widths are
screen px (the zoom rule, clamped to `min_px..max_px`, tapered with depth), so the trim points move
with every zoom step and, in 3D, differ per vertex depth; a world-space polygon cannot be precomputed,
and computing it per frame on the CPU (2D) or in the shader (3D) buys nothing the casing-first rule
does not give for free. Round caps also need no knowledge of the other arms' widths or types, so a
hidden or switched-off arm simply leaves the others' caps (no stub: a cap belongs to its own road).

**2D** (`paint2d::road_pass`): every type's lines are collected first (culled, projected, thinned,
tapered), then all casings are drawn (bottom to top type order), then all fills. A `Join` end is drawn
on 1 px along the corner's bisector (egui mitres the corner onto that short last segment and ends it
square to it, i.e. along the bisector, where the next piece ends the same way; the 1 px overlap hides
the anti-aliasing seam) plus a **mitre wedge** (`Deco::Wedge`: the triangle node, outer edge point, mitre
tip, in each pass's width and colour, because egui bevels that short segment's outer corner). A
`Dead` / `Junction` end gets a **round cap** (`Deco::Cap`: a half disc reaching 1 px back into the
line). The fill's caps and wedges are skipped for a translucent or dashed fill (a translucent one would
darken where it meets its own line, a dashed one ending in a gap would get a dot); the casing's always
come. A join onto a type that is switched off is a cap. **Overpasses**: casing-first alone would merge
an overpass with the road under it (the pre-D81 drawing did that only for crossings of one type: the
interchange ramps were one yellow blob). After the fills, the upper road's stretch over each overpass
is drawn once more, its casing over the lower road and its fill over that, 1.5 px past the casing's
ends; the stretches of one road that overlap are merged and drawn lowest first. Crossings flatter than
~15 degrees get none (`OVERPASS_MIN_SIN`), and a stretch that would run over a road lying under it
almost in parallel is dropped (it showed as a patch of the upper road over the lower one at a stacked
interchange); only cased, solid, opaque upper types, both types drawn in the pass.
Caps, wedges and overpass outlines of a line narrower than 3 px are not drawn (`DECO_MIN_R`: unseen
at that size, and the zoomed-out island has ~4 000 of them; see Performance). *Trade-offs:* the in-race
focus's muted pass keeps plain square ends (translucent, no casing); two roads stacked almost in
parallel at different heights are drawn by type rank as before (2D cannot show that stacking).

**3D** (`mesh3d`, `gl3d::shaders::ROAD_VS`, `scene.rs`): the mesh's tangent at every sample is the
bisector of its two segments times the mitre factor `1 / cos(turn / 2)` (at most `MITER_MAX` = 2, a
120 degree turn; the shader offsets by `(-tz, tx) * half_width`, so the ribbon keeps its width through
a bend). At a `Join` the end takes the bisector with the next piece's first segment, so **both ribbons
end in the very same two vertices** (tested: an L-corner's vertices are the mitre's inner and outer
corner exactly) at the node's one height. A `Dead` / `Junction` end gets a **round cap**: 7 rim samples
(`CAP_RIM`) after the piece's own, whose left vertex lies on the rim and whose right vertices are the
cap's centre (pad byte 1: the shader puts it on the point with side 0), a fan of 8 triangles round the
centre (near and far set), plus the rim wall and the underside in the near set, so a dead end's deck is
closed. All of a cap's vertices sit on the end's point, so the shader gives them the ribbon's half-width.
The renderer draws the roads in **two passes** (`uPass`): **casing** (every cased road at fill + casing
px in its casing colour, with its deck) and **fill** (every road at its fill width, nearer the eye by
`BIAS_FILL` = 0.05 % of the depth; the casing pass ranks its types with a step of 0.005 %, so every fill
is over every casing and the topmost fill, 0.41 %, stays under the trails' 0.44 %). A cased road's deck
is the casing pass's (the fill pass keeps its bottom vertices up: walls and underside collapse); a road
without a casing (the untyped slot, muted roads) is drawn wholly in the fill pass. The fill's edge over
its casing is anti-aliased in the fragment shader (`vSide`, `fwidth`; no MSAA in the scene FBO), radial in
a cap. Real install: 2 092 caps, +14 644 samples (135 911), +66 944 near / +16 736 far triangles (1.02 M /
71 k).

**In-race focus and per-edge styling (for the route recolouring, D80 / #215).** Nothing is shared
between two roads: a join is two half-mitres, a junction patch is each arm's own cap, so every piece
keeps its own style and the focus can mute or hide any run of any chain without leaving a stub
(tested: a 4-way with one arm hidden, 2D and 3D). 2D: a run's own ends inside its chain (where the
relevance changes) stay square; a run that ends at a chain end takes that end's join or cap. 3D: cap
samples carry the end sample's `SampleSrc` (`cap: true`), so `build_rel` flags them with their road.
A route colour (#215) can therefore be one more per-run / per-vertex style exactly like the focus: in
2D a pass (or a slot style override) over the route's runs, in 3D a per-vertex value next to `aRel`; a
recoloured run's caps and wedges come with it. The one thing to watch: a `Join` between a route piece
and a non-route piece is mitred against the other piece's geometry, which stays right whatever colour
either gets.

**Tests:** `data::tests` (type change, L-corner, ring, T / X / Y junctions, turnaround and jump no arms,
overpass vs at-grade vs shared vertex, a stale table rebuilt; the real install pins 775 / 830 / 1 991 ends
and 503 overpasses), `mesh3d::tests` (an L-corner's and a type change's ends share the mitre's vertices
exactly, caps only on dead ends, a cap as wide as its road, cap faces outward, a hidden arm takes its cap
along), `paint2d::tests` (every casing before every fill and no outline across a junction's mouth by
tessellated coverage, the L-corner's outer corner filled to the mitre tip and no further, a type change's
colour boundary on the bisector, a hidden arm leaves nothing, an overpass outlined and an at-grade
crossing not). PNGs (`--ignored`, `GL3D_PNG_DIR`, `JOIN_TAG=before|after`): `gl3d_synthetic_joins` (the
scenes above in flat 2D and 3D, plus the 4-way with an arm hidden) and `gl3d_real_install_joins` (a city
crossing and T, a highway junction, an L-corner, a type change, a shallow Y of the real island, 2D at
120 / 400 m and 3D at 40 / 150 m).

### Race lines and the "current race" guess

Modes: `off`, `current` (default), `nearest`, `near` (within `radius_m`, default 1 500 m), `all`
(40 000-vertex budget). Drawn as a **race road** by default (D80, below); with
`race_lines.route = "line"` as the thin line of before: 4 px, alpha .85. One colour for every race, `RaceCfg::color` (default `#f97316`, orange), with start /
finish marks (green dot + chequered flag for sprints, chequered flag for circuits).
*Why one colour (D88; the user, 2026-10-10):* "Sprints and circuits should have the same color."
(It replaces D86's separate circuit and sprint colours. Old configs: `circuit_color` is read as `color`
through a serde alias, `sprint_color` is ignored, and a saved `circuit_color` still at the old blue
`#38bdf8` is rewritten to orange by `config::migrate_circuit_color`; only the old key, so a `color` the
user sets to that blue stays.)

The telemetry has **no race id**, so "current" is a **best-effort inference**, not verified against
live races. *Why it keeps candidates (D76; the user, 2026-10-09):* "does it keep track of which route was
determined? It should only display the route up to the possible fork and then display more, once the
right route is established. Example: Route 1: A, B, C, D, E, F, G; Route 2: A, B, C, H, I, J, K ... Only
display until C (no finish line yet). Once the user drives further, we can determine the right route
really fast (D or H). So it mainly shouldn't just assume one route and only display further paths, once
it can be sure that it is the right route." Real data shows why this matters: 103 of the 170 routes share
their start with at least one other route (up to 7), some for kilometres (route 2411 shares 4 km with
5555 and 30105; 11007 differs from 5201 only in its last ~50 m).

**Candidates** (`racesel::Cand { line, s, off_m }`, `s` = the car's progress along the line in metres of
arc length; `RaceLayer::cum` holds the arc length per point, `racesel::Poly` is the arc-length view):

- *Pick-up:* while `race_position != 0` and there are no candidates, every line within
  `|half-width| + 4 m` of the car whose direction agrees with the car's heading (`cos > 0.5`,
  `ACQUIRE_COS`; a crossing road must not start a candidate) becomes one. Players join mid-route, so
  this is "every route on this road", not only those that start here.
- *Tracking, every frame* (segment grid, 40 m query): a candidate matches while the car is inside its
  corridor with `cos > 0` and its new progress is within 300 m back / 300 m ahead of the last one (a
  rewind or a packet gap is fine; a closed circuit wraps). A match updates `s` and resets `off_m`.
- *Drop:* a candidate that does not match, or is more than 8 m (`DOMINATED_M`) farther from the car than
  the nearest matching one, adds the distance driven to `off_m`; at 15 m (`DROP_M`) it is dropped. The
  dominance rule is what makes a fork resolve fast: the corridor of the branch not taken is wide, but
  the branch is clearly the farther one much sooner. Candidates are only dropped while another one still
  matches; if none does (a shortcut, a spin, a wide corner) the set is kept; if none does and a line
  with a well agreeing heading is under the car, that line set replaces them (as a new match replaced
  the single pick before).
- *Lock (the user, 2026-10-09, lap 2 of a circuit whose start a sprint shares):* "after I got back to
  the finish line, so another lap again, it just switched to a sprint circuit … it had already
  determined that the beginning part is shared by two different tracks, and then it correctly
  determined that it was the circuit race, but for some reason, after starting the new lap, it just
  threw away that information and just started fresh … it should stay with its decision". Once the set
  is certain (one candidate, or duplicates) that route is **locked** for the rest of the race
  (`RaceSel::locked`, `track_locked`): only its own stretch under the car is looked at (any progress,
  so lap wraps are free), no route is picked up, and the lock is only given up after 200 m driven off it
  (`LOCK_DROP_M`; then the usual rules, the route stays the pick until another one matches). *The cause
  of the reset:* a circuit line that does not close within 3 m (`RaceLine::closed` false) was tracked
  like a sprint, so the progress jump from its end back to its start at the line failed the 300 m
  window, the candidate fell off, and the pick-up re-added every route through the start (the sprint
  too). The progress window now also wraps for any `circuit` line, and the lock covers the rest.
  (The telemetry's lap counter would be a further signal, but `RaceSel::update` gets no packet; the
  call sites would have to pass it.)
- *Reset:* `race_position == 0` (race over), mode change, or new race data.

**What is drawn** (`RaceSel::picked()` + `RaceSel::span(line) -> Option<Span { s0, s1, start }>`):

- One candidate left: the whole line with both marks, as before (`span` is `None`).
- Several: the **part they share**. The reference is the shortest candidate; the others are compared
  with it in 4 m steps, forward from the car up to the first point where any other candidate is more
  than `SAME_TOL_M` (4 m) away (the fork), and backward down to the first difference or the start. Only
  that stretch of the reference line is drawn, with the start mark if it reaches the route's start and
  **never a finish mark**. Computed once per candidate set (`compute_shown`, cached by the set), not per
  frame: the fork ahead does not move while the set is unchanged. When a candidate is dropped, the
  stretch grows to the next fork (or to certainty).
- *Duplicates:* if the forward comparison runs to the end of the reference and every other candidate is
  within 30 m (`FINISH_TOL_M`) of its own end, they are the same road to the finish and count as
  certain; the lowest line index is drawn whole. (Real data: 69 % of the points of routes that have a
  same-direction neighbour within 20 m have it within 0.5 m, 94 % within 3 m, so the 4 m tolerance
  catches the shared stretches of different routes.) Where one route is the other's **prefix** (it
  ends where the longer one goes on), the shared part ends at the shorter route's end, without a
  finish mark, until the race ends or the car leaves: nothing tells them apart before that.
- `paint2d::draw_race_lines` draws `Poly::slice(s0, s1)` instead of the line when a span is set, and
  `draw_race_marks(.., start, finish)` only the marks that belong to it.

**Real-install check** (`racesel::tests::real_install_drive_every_route`, `--ignored`): all 170 routes
driven from start to finish through the whole layer, exactly on the line and with a +-3.5 m wobble. Every
route ends on its own line (or an identical duplicate), never on a different one; 92 of 170 are certain
within the first 30 m, 142 drops after a fork happened a median 24 m (p90 48 m) past the fork; 4 routes
(351, 1201, 5191, 8008) stay uncertain to the end (prefix / near-duplicate pairs).

**Cost:** per frame one grid query like before plus a few lookups per candidate; `compute_shown` only
when the set changes. Release, real install, 514 623 updates over all 170 routes: 1.5 us per update on average; the worst single update (a set change on a long route) 2.8 ms; a shared prefix focus build is smaller than the route's (route 5555 whole: 11.2 ms under load, median route 0.4 ms).

`RaceSel` (kept per map; the Dashboard's is `ForzaApp::minimap_race_sel`) holds the state.
`RaceSel::update` returns the selector itself (`picked()` = the lines), because the renderer reads the
in-race focus from it too (below).

### Race lines as roads (D80) and "race road only" (D82)

*Why (the user, 2026-10-09):* first "I dont like the way that the racetrack is drawn on top of the
road. If anything, it should be the 3D road mesh just drawn on top of the other one, but i would rather
prefer it just changing the road, that is the racetrack, to the given color", plus floating highway
pieces in 3D; then, after playing a cross-country race that does not follow the road network: "I just
played a race and it was cross-country, I changed my mind on the roads being drawn, it should also just
draw a 3d road to draw the race line." So the race line itself is drawn **as a road**, along its own
points and heights, not the nav roads recoloured (a graph-matching attempt was dropped: a race does not
have to use the roads). `RaceCfg::route: RouteStyle` = `road` (default) / `line` (the thin line of
before).

- **2D** (`paint2d::draw_race_lines`): the line (or the shown span, D76) as a road: every race road's
  casing (the `Road` type's casing colour and `casing_px`), then every fill in the race colour,
  **opaque**, round ends (none for a closed circuit), width = the roads' width rule x
  `style::RACE_ROAD_WIDTH` (1.5). *Why the road rule, not the track's half-width:* the rule keeps every
  road readable at the HUD's and the Viewer's zooms (a 12 m track is a hairline at 3 km and a slab at
  150 m); 1.5 is a little wider than a highway (1.44), so the race road covers whichever road it runs
  on and that road's casing shows as its edge. Drawn after all roads, so it lies on top; where a side road
  meets it, the race road's casing runs across the side road's end (it is the road on top, like an
  overpass). Every picked line is drawn this way in 2D (also `nearest` / `near` / `all`).
- **3D** (`mesh3d::RoadMesh::race_roads`, `store::race_mesh`, `gl3d::scene::Gl3d::sync_race`): every
  line the race-line mode draws is one small road mesh (D88; D80 had only the in-race focus line),
  built by the same code as the roads (`race_road_layer` puts each line into a `RoadLayer`, the
  layers are merged): 8 m samples,
  the deck, mitred joins, round caps at the shown extent's ends, a closed circuit ends on its first point
  so it closes in one mitre. Heights are the line's own (`RaceLine.y`, the AI's driving line: on the
  bridge, in the tunnel, across the field; `uMode` forced to node heights). Stretches 4 m or more under
  the terrain (`RACE_TUNNEL_DEPTH_M`) go into the tunnel slot, so they are drawn like the road tunnels
  (last, without the depth test, at the tunnel alpha). Drawn after the roads with the road shader and
  `roads::race_table` (every slot in the race colour, `Road` casing, `RACE_ROAD_WIDTH`, no muting), in
  the same two passes (casing, fill), depth-tested against terrain and roads with a depth bias above
  every road fill (`RACE_BIAS` 0.0042 casing, +0.00008 fill; the trails stay above at 0.0044): it sits on
  the road it overlaps without z-fighting, and a road really above it (an overpass) still covers it.
  *What the scene gets (D88):* `RaceSel::race_draw(layers, &RaceCfg) -> Option<Arc<RaceDraw>>` =
  the lines (`RaceRoad`: points, heights, closed; the shown extent of D76 in `current`, every
  picked line in `nearest` / `near`, all 170 in `all`) plus the start / finish `RaceMark`s, **cached
  per drawn set** (key: race data, mode, sorted line indices, the extent counter), so a frame that
  changes nothing returns the same `Arc`. It does not depend on the in-race focus any more
  (`RoadFocus::race` is gone), nor on the look: colour, style, width and alpha are
  `Scene3d::race: Option<Race3d { draw, mesh, cfg: RaceCfg }>` and go straight into the style table, so
  changing them rebuilds nothing. The mesh is built **off the UI thread** by `store::race_mesh(draw,
  terrain)` (thread `map-race-mesh`, same contract as `road_mesh`: the newest finished mesh is served
  while a new one builds, so a new pick shows the old lines for a moment instead of a blank; the
  renderer re-uploads when the `Arc` changes, never in the same callback as another heavy upload).
  Cost: route 5555 (85 km) 14 342 samples, 114 k triangles, 5.3 ms release; mode `all` (170 lines,
  ~1 000 km): see "Race lines in the scene: cost" below.
  *Line style (`route = "line"`) in 3D:* the same mesh with `race_table` in its thin form: a
  ribbon of constant screen width `width_px x s x ppp` (the road shader's fixed-width form of the
  width rule: metres 0 makes the clamp return `min`), alpha `RaceCfg::alpha`, no casing, deck
  thickness 0 (a 4 px line must not have walls). *Why the road shader and not the trail
  ribbons:* one mesh, one plan (tiles are culled per frame), the same depth bias and tunnel pass as
  the race road, and the heights and joins come for free; 170 lines would be ~170 k segments of
  streamed trail vertices every frame.
  *Marks (D88, `gl3d::racemark`):* **posts**, drawn with the trail ribbon program (`draw_race_marks`):
  a vertical ribbon on the line at its own height (`RaceMark::y`, 0 = the terrain), constant screen
  width (5 pt, dark outline 8 pt) and a height of 18 pt *at the car's distance* (it shrinks with
  the perspective like the icons). A sprint start is green (`style::START_DOT`), a circuit start and a
  sprint finish are chequered (white / dark / white cells). Depth-tested with a bias a little above
  the race road's fill (`RACE_BIAS_MARK` 0.0045), **no ghost pass** (a trail is "seen through" a
  hill, a mark is not); a mark whose line is 4 m or more under the terrain is drawn without the depth
  test, like a tunnel. *Why posts, not flat bars across the road:* from the tilted camera a bar on the
  road is a hairline and has to be as wide as the road; a post is a few vertices and reads at any
  angle and zoom. Built per frame (the size follows the zoom): a few hundred marks at most.
  *Occlusion (D88; the user, 2026-10-10: "occlusion should work for the race circuit aswel"):* the race
  lines (both styles) and their marks are hidden behind hills and under decks above them exactly like
  the road ribbons: their open stretches are depth-tested and only tunnel stretches (>= 4 m under the
  terrain, by the AI line's own height) show through, as the road tunnels do. Guarded by
  `gl3d::tests::gl3d_race_road_is_hidden_behind_hills` (a race road behind the big hill is not drawn, the
  same road seen from the other side is; a road along the elevated highway is covered by the deck) and
  `gl3d_race_lines_and_marks_are_hidden_behind_hills` (non-focus lines of the Road style, the thin
  Line style and the posts: 0 pixels behind the hill, drawn in plain view).
- **Over the 3D view** `paint2d` draws **no race line and no mark** (`Parts::OVER_3D` = POIs only; 2D
  and Tilted keep `draw_race_lines`). The call sites (`ui/map_scene.rs`, `hud/minimap.rs`) hand the
  scene the race lines whenever the mode draws any (`race_draw`), and the in-race focus only when
  *Other roads* is not `normal` (it no longer carries the race road). While the 3D scene is not
  `Ready` (the underlay frames) the whole map is 2D, race lines included.
- **"Race road only"** (D82, `OtherRoads::RaceOnly`, serde `"race_only"`): *Why (the user, 2026-10-09):*
  "Here should be a setting, to not draw anything from the normal road mesh and only draw the circuit
  using the 3d renderer". While the focus is on, nothing of the road layer is drawn: 2D skips
  `draw_roads`; 3D skips the road mesh (casings, caps, jump lines, tunnels) when the scene has a race
  road. With `route = "line"` it falls back to `hidden` (`OtherRoads::effective`). Outside a race
  everything is normal. (The Map-tab dropdown entry comes with #211; until then the label reuses
  "Hidden".)

**Race lines in the scene: cost (D88).** Real install, release, mode `all` (170 lines, 1 033 km, 297
marks; `gl3d::tests::gl3d_real_install_all_race_lines`): `race_draw` 0.3 ms (the point copies); the race
mesh 269 ms on the `map-race-mesh` thread (174 821 samples, 1.38 M + 0.06 M triangles near + far set,
19.6 MB of vertices) - which is why it is off the UI thread - then one upload on the GL thread; a
frame draws ~308 k triangles in all (terrain + the plan's visible tiles; ~0.06 ms GPU time on the
EGL test device) at a 3 km zoom and the same at 300 m. A single route / a
few picked lines (`current`, `nearest`, `near`) build in 0.1-6 ms. The mesh is rebuilt only when the
drawn set changes (see `RaceSel::race_draw`), never for a change of colour, width or style.

### Navigation route (phase L, D84)

The route from the car to a clicked destination (`nav::view()`, see [navigation.md](navigation.md)) is
drawn on all three maps in all view modes **as a road of its own, like the race road** (casing +
opaque fill in `nav_route.color`, default fuchsia `#d946ef`, round ends, the roads' width rule x
`style::NAV_ROUTE_WIDTH` 1.5 x `nav_route.width`), plus a destination pin. Details, the thread each
map reads it on, the mesh cost and the config are in [navigation.md](navigation.md) §3; what matters
for the renderer:

- **Layer order:** roads, jump lines, race lines, **route**, POIs, then the markers and the pin.
  "Race road only" (D82) does not hide it; the in-race `PausedRace` does (the runtime sends no line).
- **2D / tilted:** `paint2d::draw_nav_route` through `draw_road_polylines`, the road branch of
  `draw_race_lines` factored out; its jump stretches are dashed lines, not a deck over the gap.
- **3D:** its own mesh (`RoadMesh::nav_route`, the race road's builder), `Scene3d::route` /
  `Route3d`, its own pass after the race lines (`roads::nav_table`, bias above the race road),
  depth-tested like the race road: hills and decks hide it, tunnel stretches (>= 4 m under the
  terrain) go through. Built on the calling thread once per `NavLine::rev` (0.5 ms for a 21 km
  route, release), not off-thread.
- **Pin:** `hud::map_shared::draw_destination`, tip at the terrain's height in 3D.

### In-race focus (D66): other roads muted, POIs hidden

*Why (the user, 2026-10-08):* "that it detects the right race is actually really nice, but ... there
should be an option to basically disable all of the non-relevant roads, like the markings, once the
user is in a race or like at least turn them white or something that's not as distracting."

**When it applies:** mode `current` **and** `race_position != 0` **and** a race line is picked
(`RaceSel::focus_line()`). With no pick (detection unsure) or outside a race **everything is drawn
normally**: a wrong guess must never mute the whole map. `nearest` / `near` / `all` never focus.

**Config** (`MapLayerConfig::race_lines.focus: RaceFocusCfg`, serde-default, same for Dashboard and HUD;
editable in the **Race lines** card of the Map tab's three map pages, section *In a race*, see
[map-tab.md](map-tab.md); the rows are greyed unless the mode is *Current race*, the width / colour /
opacity rows unless *Other roads* is *Muted*):

| Field | Default | Meaning |
|---|---|---|
| `other_roads` | `muted` | `normal` / `muted` / `hidden`: roads off the race's corridor; `race_only` (D82): no road of the road layer at all, only the race road |
| `mute_color` | `#ffffff` | colour of muted roads |
| `mute_alpha` | 0.25 | their alpha |
| `mute_width` | 0.8 | factor on each type's own width (keeps highway > road) |
| `hide_pois` | `true` | hide icons, gate lines and the current chest in a race (start / finish marks stay) |

*Why the defaults (lead's call, user can change them):* the user complained about distraction, so the
default is to act; muted rather than hidden so the surroundings stay readable. Muted roads are solid,
without casing, drawn first (under the relevant roads); a type switched off stays off.

**What is relevant:** a road segment is relevant when more than half of its samples (every 10 m, both
ends included) lie within `|half-width| + 8 m` of the drawn line's centre line (`RaceLine::half`, per
segment; a closed circuit's closing segment included) **on a stretch that runs the same way as the road**
(`|cos| >= 0.94`, about 20 degrees, `ALIGN_COS`). *Why the heading test (the user, 2026-10-09):* "when it
hides all non race track roads, while in a race, it still shows the first node of roads that are connected
to the race circuit. This shouldn't be the case." The first segment of a side road out of a junction lies
inside the corridor, but at an angle; without the test it passed the distance rule. Roads that cross or
join the line from the side (perpendicular, 30 degrees) are therefore not relevant at all, junction included
(the race line covers the junction); a road that runs parallel beside the route (a frontage road 10 m away)
or merges at a shallow angle (15 degrees) still counts as along it: from geometry alone it cannot be told
from the route's own road.
**The test is 3D** (the user, 2026-10-09: short, disconnected highway pieces floated over the race road in 3D):
an overpass crossing above the route lies in its xz corridor, so its few segments there counted while the
rest of the highway was hidden. A road sample therefore also has to be within 6 m (`HEIGHT_TOL_M`) of the
race line's height (`RaceLine.y`, interpolated at the nearest line point) using the road's node heights
(`Chain.y`, the D51 rule the 3D mesh draws), judged only when both are known (road height 0 = unknown, the
3D mesh then uses the terrain, so the sample is judged by the corridor alone). Real install, route 5555:
4 379 -> 4 371 relevant segments (8 overpass / underpass pieces gone over 85 km). The 2D map uses the same
`RoadFocus`, so it draws the same set. (D80 first planned nav-graph route matching here; with the race
drawn as a road of its own, the corridor stays.) Cross-country, trail, tunnel and every other type count alike. A jump line is
relevant when both ends are on the corridor. **The corridor follows what is drawn (D76):** while the route is
uncertain it is the corridor of the shared part only (`RoadFocus::build_pts` on `Poly::slice`), the whole
line once it is certain. The focus cache key includes a counter that changes only when the drawn line or
extent changes, i.e. when the candidate set changes (a handful of times per race), never while the car just
drives along the shared part. `racesel::RoadFocus` cuts every road chain into `Run`s (`a..=b` points, relevant or not), so
`draw_roads` runs two passes (`Pass::Muted`, then `Pass::Relevant`) over runs instead of chains.

**Cost:** computed **once per picked line and road data** (cache in `RaceSel`, key = road `rev` +
`Arc` identities + line), not per frame, on the drawing thread. A tiny grid of the one line (32 m
cells, each segment registered with its widened box) answers the point tests, not the store's 100 m grid
over all lines. Release on the real install (39 242 road segments): route 5555 (85 km, 14 224 points)
7.7 ms, 4 532 segments relevant (11.6 %), 1 635 runs; median route 1311 (5 km) 0.3 ms, 83 relevant
(0.2 %). The recompute happens when the pick changes, a handful of times per race at most.
(`racesel::tests::real_install_focus_cost`, `--ignored`.)

### Tilted view (D65)

`view::Camera { view: MapView, centre, rect, pitch, focal }`. At pitch 0 it is exactly `MapView`'s
mapping (tested). Tilted, it is a **flat perspective of the 2D map** done on the CPU, the equivalent of
the demo's CSS `perspective(P) rotateX(a)` about the car: a plane offset `(x, y)` px from the car
(y down) lands at `(x, y cos a) * P / (P - y sin a)`, and the horizon is `P / tan a` above the car
(defaults: a = 55 deg Dashboard / 40 deg HUD (D71), P = 200 px, car 85 % down the view). The image is the visible part of the plane
in 24 x 24 cells, each projected and cut to the map outline, every vertex's UV taken from the inverse
projection (egui interpolates UVs affinely inside a triangle; a perspective is not affine, hence the
subdivision). Every layer vertex goes through the same `Camera::project`, so roads, race lines and POIs
agree with the image; POI icons stay upright and scale with the perspective. *Why a camera type now:*
phase K's GL 3D scene reuses it ("3D: data and camera" below), and this CPU version doubles as a cross-check for the GL matrices.
Polish (I29b):

- **The perspective distance scales with the view's height**: `perspective_px` is the value for the
  136 px HUD pill, `Camera::focal_for` makes it `200 * h / 136`, so the same settings give the same
  picture on a 600 px Dashboard widget (else the horizon would sit inside a large widget).
  `Camera::from_cfg(&TiltCfg, ...)` is the one constructor both maps use (angle clamped to 5 to 80 deg).
- **The tilted plane ends where the depth scale is `view::FAR_MIN_SCALE`** (0.05, just short of the
  horizon), so the view is fully covered whenever the horizon is outside it. *Why:* a fixed
  3.2-view-height limit (from the demo canvas) left the top fifth of the 55 deg pill empty (user report,
  2026-10-08). The base image is a screen-space grid (rows even in log depth).
- **Far-edge fade**: the image's alpha ramps over depth scale 0.05-0.15 (`style::FAR_FADE_DEPTH`), so it
  only shows when a steep tilt (about 58 deg and more on the HUD) brings the horizon into view, fading
  into whatever is behind: the Dashboard's background, the HUD's plate (or the game). *Why alpha, not
  the demo's overlay gradient:* the demo paints the plate colour over the top 30 %; with no plate (the
  HUD default) that does nothing and the plane ends in a hard line. Fading the image itself works with
  any backing.
- **Far POIs are skipped:** POIs smaller than depth scale `style::POI_FAR_K` (0.3) are not drawn (the
  current chest still is), so the far strip doesn't become a pile of icons.
- **Line-width taper** (`TiltCfg::taper`, default on): road, race-line and trail widths scale with the
  perspective at their screen row (`Camera::depth_scale_at_row`, `1 + dy tan(pitch) / focal`), as the
  demo's CSS transform does by construction. egui lines have one width, so each polyline is cut into
  pieces by depth band (8 bands, `style::TAPER_BANDS`) and each piece gets its band's width: a
  stepped taper, invisible at these sizes. Off = constant widths. Race-line marks shrink with their row too.
- **Markers follow the tilt**: `hud::map_shared::MapCanvas` holds the `Camera` (not a bare `MapView`),
  so trails, teammates, waypoints and the own arrow sit where the layers' projection puts the same
  world point on both maps (pitch 0 is the old mapping, tested). Arrows themselves stay upright; trail
  widths taper. A point behind the eye goes far off-screen along its flat direction.
- Both maps' tilt settings are on the Map tab (Minimap / Dashboard map & Viewer page → View mode).

### 3D: data and camera (phase K, K1)

K1 lays the CPU foundation of the shared 3D renderer (D61); the GL scene itself (`maprender::gl3d`,
K2) and the call sites (K3 HUD, K4 Dashboard + Viewer, K5 settings card) come after. Nothing here
draws yet, and no maps' behaviour changed: the defaults keep today's views (HUD tilted, Dashboard
and Viewer flat). Design: phase K scout (`design.md`, kept by the lead); this section is the
reference for what exists.

**View modes** (`cfg::ViewMode`, derived by `TiltCfg::view_mode()`): `Flat` (`tilt.on` off),
`Tilted` (D65) and `Relief` = "3D" (`tilt.on && tilt.relief.on`; the variant cannot be called `3D`).
`cfg::ReliefCfg` is **nested in `TiltCfg`** (`tilt.relief`, `serde(default)`): old configs load, the
`MapLayerConfig { image, roads, pois, race_lines, tilt }` shape is unchanged, and "Copy to ..." of the
tilt category (`LayerCategory::Tilt`, D68) copies the 3D settings with it. *Why not a separate
category:* the angle, perspective and car position of the tilted view **are** the 3D camera's pitch,
FOV and car position, so the two are one set of settings. Fields and defaults (design questions
answered with the recommendations, to be revisited after the user has seen it): `on` false on all
three maps, `road_height` `Nodes` (`RoadHeight::{Nodes, Terrain}`; serde `"nodes"` / `"terrain"`),
`deck_m` 3.0 (0..20), `exaggeration` 1.0 (0.5..3), `shading` 0.35 (0..1); `ReliefCfg::sane()` clamps
hand-edited values. The sea is flat at y 100 and not configurable.

**Terrain** (`maprender/terrain.rs`, `docs/game-data/fh6-terrain.md`): the filled 8 m `HeightGrid`
(`u16`, 0.1 m steps) in an `Arc<Terrain>`. **Lazy:** `store::terrain()` / `Store::terrain()` returns
`TerrainStatus::{NoInstall, Loading, Ready(Arc<Terrain>), Error(String)}`; the first call starts the
build on its own thread `map-terrain` (cached elevation rasters, 0.2-0.4 s release warm, +~2 s per
raster cold), keyed on the install only (not season or the road-type file: a Save rebuilds the
layers but never the terrain), a failed load is not retried until the install changes. A user who
never uses 3D never pays the 15 MB / 0.3 s, so call it **only while some map is in 3D mode**.
*Why a separate thread from `map-layers`:* a cold terrain build must not delay the 2D layers of the
other map. While `Loading` the maps draw the tilted 2D path (`Camera::from_cfg_relief` returns the
plain tilted camera without a terrain).

**Camera relief** (`view::Relief { terrain, exag, car_y }`, `Camera::relief: Option<Relief>`).
The tilted view is exactly a pinhole camera looking at a flat world; with `a` the pitch, `P` the
focal, `(ox, oy)` the plane offset px of a world point (y down) and `h = (y - car_y) * exag * scale` its
height px above the car's plane:

```text
cam    = ( ox,  oy cos a - h sin a,  P - oy sin a - h cos a )
screen = centre + cam.xy * P / cam.z
```

At `h = 0` this **is** `project_offset` (test `project3_at_height_zero_equals_project_offset`: 2000
random points, yaws, pitches 5-80 deg, eye distances: max difference < 1e-3 px, the prototype
measured 1.5e-5), so Tilted <-> 3D is seamless: the car stays on the same screen point at the same
ground scale, only relief and parallax appear. Geometrically: an eye `P / scale` m (= 2.94 x zoom for any
landscape view) from the car, up by `cos a`, back by `sin a`, looking at it, with the car off-centre
(`centre`, the lens shift). Zoom is a dolly. API (all on `Camera`):

- `project3(x, y, z) -> Option<(Pos2, depth_px)>` the formula; `project(x, z)` is **unchanged for
  a camera without a relief** and with one projects the *terrain surface point* above (x, z), so
  every existing call site (layers, markers, trails; ~30) gets 3D with no edit. `k_at(x, z)` =
  `P / depth` (the general `perspective_at`; icon scale). *Known v1 limit:* egui overlays have no
  depth test, so a POI behind a ridge still shows (occlusion = K6).
- `with_relief(Relief)`, `Relief::new(terrain, exag, car_y)`, and the constructor the call sites use,
  `Camera::from_cfg_relief(tilt, car, yaw, zoom, rect, Option<&Arc<Terrain>>, Option<car_y>)`: relief
  only when `tilt.on && tilt.relief.on` **and** a terrain is given (car height defaults to the
  terrain under the car; pass the telemetry height + ~1 m when following).
- `eye()` (world metres), `eye_clear(margin)`, `with_eye_clearance(margin)`: the **eye-clearance
  rule**. If the eye is within `margin` m (use 3) of the ground, or the line of sight car -> eye is
  blocked by more than 1 m of terrain, the pitch is lowered by bisection (down to `MIN_PITCH` 5 deg)
  until it clears; unchanged when it already clears. Needed for 0.1-0.7 % of road positions
  (measured over 7 500 road samples, 8 headings); the caller should **ease** the pitch over ~0.5 s
  (`MapAnim`) instead of snapping.
- `footprint(margin) -> [min_x, min_z, max_x, max_z]`: the 3D replacement of `world_aabb` for
  culling: the four view corners cast onto the plane at the car's height and onto the planes at the
  lowest / highest terrain found under that first box (so hills and valleys are inside), far-limited
  at depth `P / FAR_MIN_SCALE`. Test: 16 000 random terrain points that land inside the rect are
  inside the box.
- `view_proj_rel(ppp)` / `view_proj(ppp)` / `car_exag()` / `exag()` / `near_far(ppp)`: the GL matrix
  (column-major; NDC spans the camera's `rect` as the GL viewport, `rect.size() * ppp` px; near
  0.05 P, far 60 P). **Draw with the car-relative form:** the shader subtracts `car_exag()` from
  `(x, y * exag, z)` and multiplies by `view_proj_rel`; the folded `view_proj` (translation included)
  loses up to ~0.01 px near the near plane in f32 (a translation column of thousands of px; found by
  the test), which is fine for **culling tile corners** (`mesh3d::Tile::in_frustum`) but not for
  vertices. Both equal `project3` (relative 1e-3 px, folded 0.05 px beyond the near plane, tested
  with exaggeration, a raised car, ppp 1 / 1.5 / 2).
- **Pan / zoom / `unproject` stay on the flat plane at the car's height** (the tilt maths): while
  panning, the grabbed ground point stays under the pointer exactly on flat ground and
  approximately on hills (an exact ray march is K6).

*`Camera` is no longer `Copy`* (decision, K1): the relief holds an `Arc<Terrain>`. *Why not keep `Copy`
and pass the terrain separately:* `cam.project(x, z)` has ~30 call sites that must get terrain heights
without being touched; with the terrain inside the camera they do. Cost: none outside `maprender`
(every use was already by reference or a fresh `Camera::from_cfg`; nothing needed a change), clone it
(one `Arc` bump) where a copy was implied.

**Road mesh** (`maprender/mesh3d.rs`, pure CPU): `RoadMesh::build(&RoadLayer, &Terrain, rev)` makes
the GPU-ready bytes. Every chain is resampled to <= 8 m (all nodes kept; jump lines 2 m), cut at
1 km tile borders, and laid out as 4 vertices per sample `{right, left} x {top, bottom}` (28 B:
`x, z, y_node`, `side/bot/slot`, tangent, distance along) plus two index sets per tile: **near**
(8 m, 8 triangles per segment: top, both walls, underside) and **far** (samples >= 32 m apart, jump
lines all, top surface only: the LOD of design §5.5). Tunnels sit in a second range of each tile
(drawn on top, depth test off). On the real island: **121 267 samples, 485 068 vertices (13.6 MB),
950 616 near triangles (11.4 MB of u32 indices), 54 176 far triangles, 2 440 pieces in 177 tiles**,
built in 186 ms in a debug test (~15 ms release). The shader does the rest (`road_y` documents the
rules): the **height rules of D51** (node heights by default, switchable to the terrain drape;
**cross-country always draped**; **jump lines a taut string** over the terrain, computed here into
`y_node` with `taut_string`: the upper convex hull of the two end heights and the ground between,
port of the editor page's `tautString`; real data: 12 of the 18 jump lines bend > 0.5 m, the largest
13.4 m, as measured by the scout), a 0.6 m lift, the deck (`deck_m / exag` dropped from the bottom
vertices) and the per-vertex width rule. *Why heights live in the shader, not in the mesh:* the
Nodes/Terrain switch, the deck and the exaggeration are live settings; a mesh rebuild per slider
step would stall. *Why `s` is the distance along the whole chain:* dashes run on across tile borders.
Tangents are the bisector of the two segments at each sample, scaled by the mitre factor, so
adjacent pieces meet without a gap and corners keep the full width; chain ends are mitred onto the
next piece or capped (D81, "Road joins and junctions" above; the numbers above are pre-D81, the caps
add 14 644 samples and 66 944 near triangles).
`Tile { key, bbox, y_range, near, far }`: `in_frustum(view_proj, exag, pad)` (conservative, 8
corners x 6 planes) and `dist_to(x, z)` (for choosing the LOD set) are the helpers for the per-frame
tile loop (one `draw_elements` per visible tile, +1 for its tunnels).
**Cache:** `store::road_mesh(&Arc<MapLayers>, &Arc<Terrain>) -> Option<Arc<RoadMesh>>` returns the
newest finished mesh (stale while a rebuild runs: compare `mesh.rev` / `mesh.terrain_rev`), starts
the build on a `map-mesh` thread when the key `(MapLayers::rev, Terrain::rev)` changed, and shares
it between the HUD and the Dashboard/Viewer (each GL context uploads it itself).
**In-race focus (D66):** `RoadMesh::build_rel(&RoadFocus) -> Vec<u8>` is one byte per GPU vertex, 1
= relevant (own style), 0 = other (muted / hidden by `RaceFocusCfg`), from the same `RoadFocus` the
2D path uses (`RaceSel::road_focus`); recompute only when the drawn extent or the road rev changes (~0.2 ms)
and upload with `buffer_sub_data`. A sample is tagged with the segment it starts, and a sample on a
node is relevant only if the segment before it is too, so the flag never leaks from the relevant side
into a side road's first metres (D76 follow-up); the flag flips up to half a quad (<= 4 m) early on the
relevant side once interpolated.
Tests: `view::tests` (h = 0 parity, matrices, eye clearance, footprint, `from_cfg_relief`),
`terrain::tests` (incl. `real_install_terrain`), `mesh3d::tests` (resample/tiles/LOD/winding/rel/frustum
on synthetic data, `real_install_mesh` on the install), `store::tests` (lazy terrain, mesh cache),
`cfg::tests` (relief serde, view mode, the `copy_category` fixture differs in `relief`).

### 3D renderer (phase K, K2)

`maprender/gl3d/` is the GL scene all three maps will share (D61): terrain from the height raster with
the satellite image draped on it, roads as ribbons with decks. K2 delivers the renderer, its API and
the 2D-side API step; **nothing calls it yet** (K3 HUD, K4 Dashboard + Viewer wire it in, K5 builds
the settings card), so no map's behaviour changed. Module map and the call shape are in the
`gl3d/mod.rs` header; this section is the reference for the *why*.

**Public API** (`maprender::gl3d`): `Gl3dHandle` (one per GL context, `Clone`: `status()`,
`wants_underlay()`, `busy()`, `stats()`, `caps()`, `destroy(&glow::Context)`), `Gl3dStatus::{Untried,
Ready, Failed(String)}`, `Scene3d` (everything one frame needs: the relief `Camera`, the newest
`Arc<RoadMesh>`, the egui map texture + calibration, image look, fade `a`, size factor `s`, corner
radius, `ReliefCfg`, `RoadsCfg`, the optional in-race `Focus3d`), `add_scene(&Painter, &Gl3dHandle,
Scene3d)`, `Gl3dOptions` (requirements and test switches), `last_failure()`. Since D77 also
`Scene3d::trails` (`Trail3d` / `TrailSeg`) and `add_marker(&Painter, &Gl3dHandle, MarkerScene)` with
`Marker3d` (the own car, a callback of its own; see "3D: the own car and the trails in the scene"). The 2D-side step in
`paint2d.rs`: `draw_layers_parts(cx, layers, cfg, Parts)` with `Parts::OVER_3D` (POIs only: roads,
since D88 race lines and since phase L the navigation route are the scene's; `draw_layers_or_route` is what the call sites use, it also draws the route when there is no layer data), POIs and the culling boxes go through `Camera::project` /
`k_at` / `footprint` (identical to the plane maths without a relief), `MapCanvas::to_screen`
(`hud/map_shared.rs`) goes through `Camera::project`, so waypoints (and teammates without a height) sit on the
terrain (teammates with a telemetry height: `to_screen_at`, see "Teammates at their real height"). In 3D the egui lines of `draw_layers` keep a constant width (`tapered` returns factor 1 for a
relief camera: its row-based depth scale is a flat-plane formula); the GL roads taper per vertex.

**Architecture and the whys**

- **The scene is rendered into the renderer's own FBO (RGBA8 + 24-bit depth) and composited as a
  quad inside the same `egui_glow::CallbackFn`**, not shown via `register_native_texture`. That needs
  `&mut Painter` (only the overlay has it) and eframe offers no *replace* for a resized texture, and
  the Dashboard's widget code has no `Frame`. The callback needs nothing from the frame loop, so the
  call is identical at all three sites; the composite shader does the HUD pill's rounded mask (an
  SDF) and the fade alpha (premultiplied, egui's own blend). Markers, POIs, compass and border are
  ordinary egui shapes after the callback (egui keeps the layer order).
- **One `Gl3d` per GL context, created lazily inside the callback** (the one place with the right
  context current), state in a `Gl3dHandle` the call site owns. The HUD (overlay thread) and the
  Dashboard + Viewer (UI thread) are different contexts: nothing GL is shared, only the CPU data. The
  FBO is *transient* (render, then composite in the same callback) and grow-only, so one serves any
  number of callbacks a frame. The map texture is **not uploaded by us**: `painter.texture(id)` is
  egui's own, reused (no second 64-256 MB copy). `prepare_map` gives it mipmaps + anisotropy once;
  egui re-sets `MIN_FILTER` on a re-upload, so the filter is read every frame and the chain rebuilt
  when it was reset (self-healing; the HUD's texture already has mips and only gets the anisotropy).
- **Init is spread over frames** (`Gl3dHandle::busy()` = ask for another frame): programs + static
  buffers, then the 15 MB height texture, then the road buffers (each a few ms), so there is no
  40-50 ms hitch on the HUD. **Until `Ready` the call site draws the tilted 2D map underneath**
  (`wants_underlay()`), which is also the whole fallback.
- **Terrain: `R16UI` height texture + geometry clipmap** (`clipmap.rs`). The grid is the K1
  `HeightGrid` verbatim, read with `texelFetch` and a hand bilinear (exact, ES 3.0 has no filterable
  integer textures; the same arithmetic as `HeightGrid::height`, so CPU markers and GPU picture
  agree). 7 levels of 64 x 64 cells (cell = 8 m x 2^level), one static grid VBO + 10 index buffers
  (full grid + the 9 ring variants of the hole offset), 7 draws and ~43 k triangles at any zoom. The
  level centres snap to multiples of 2^(level+1) px so the hole offset is -1/0/+1 cells and rings tile
  exactly (unit-tested for 7 car positions, no gap or overlap); odd edge vertices take the mean of
  their even neighbours (no cracks, no skirts; this is the alpha = 1 case of the geomorph below). The coarsest level (+-16 km) fades over its outer
  eighth: the Viewer at 8 km zoom reaches its edge and a hard line looked like a wall.
  Heights outside the raster are clamped (flat sea at y 100), the satellite wraps by egui's
  texture option (mirror on) or is discarded (`uMirror`). Hill shading (Lambert, `ReliefCfg::shading`),
  brightness / saturation / opacity of the image are applied exactly in the fragment shader.
- **Geomorphed clipmap (hills stop popping).** *User, 2026-10-09: "hills and the height map keep adapting
  for some reason ... figure out why it shifts the hills and their structures."* Cause (the only one):
  level `l` re-centres every 16 * 2^l m while driving (vertices are world-locked, bases are multiples of
  the stride), and the ring strip that switches went from the fine to the coarse height *and* shading
  normal in one frame; the coarse levels point-sample the 8 m raster and the normal is a +-stride central
  difference, so far ridge silhouettes and the horizon jumped. Not the cause: the height filtering, the
  car height bobbing, reloads, depth. Fix, in `TERRAIN_VS`: every vertex of a level blends from its own
  height to the coarser level's value over the level's outer cells, `alpha = clamp((d - 24) / 7, 0, 1)`
  with `d` = Chebyshev distance in level cells from the continuous car raster position (`uCarPx`, the
  `car_px` `scene.rs` already computes; the coarsest level has no coarser one and is not blended). The coarser value is the mean of the two even neighbours
  for an odd axis, and for odd-odd the mean of the **anti-diagonal** pair (it must be the diagonal the
  index buffer cuts the cell along, `(b, c)`), the same height for even-even. The same alpha blends the
  normal stencil from +-stride to +-2*stride. *Why 24..31:* a level's outer boundary is always >= 31
  cells from the car (the centre snap is <= 1 cell), so alpha is exactly 1 where two rings meet and both
  sides are identical, while the blend starts well outside the finer level's hole. Measured on the real
  island (hilliest straight road stretch, 3 m steps, a frame with this step's levels vs the previous
  step's under the same camera, terrain only): the worst frame changed 0.35 / 0.07 / 0.41 / 0.28 % of the
  pixels (HUD 500 m / HUD 150 m / Dashboard 800 m / Dashboard 1500 m; mean |diff| 0.099 / 0.049 / 0.114 /
  0.086 of 255) before and 0.003 / 0.002 / 0.004 / 0.002 % (0.010-0.014) after. Cost: GPU +~0.003 ms (the
  3D is <= 0.05 ms), plain mix / clamp / `&`, so ES 3.0 is fine. Side effect: the outer cells of every
  level are a little coarser than before (the static picture differs by ~1/255 on average, up to 3 % of
  pixels in mountain scenes, only in rings of hills). Regression tests (ignored, EGL):
  `gl3d_terrain_levels_do_not_pop` (synthetic) and `gl3d_real_install_terrain_levels_do_not_pop`
  assert the worst-frame share stays below 0.01 % / 0.05 %; the `#[cfg(test)]` hook `clipmap::LOD_CAR`
  makes `scene.rs` place the levels for another car position to render the "previous frame".
- **Roads** (`roads.rs`, shaders): static GPU ribbons from K1's `RoadMesh`; heights, deck, width,
  colour, dashes and focus are all uniforms / per-vertex shader work, so no setting re-uploads
  anything. Node heights vs terrain drape is one uniform, cross-country always draped, jump lines
  carry their taut string. The width is the 2D rule evaluated per vertex depth
  (`clamp(px/m * metres, min, max) * type factor`; the casing is a pass of its own since D81: every
  casing at fill + `casing_px`, then every fill over them, see "Road joins and junctions"), so it
  tapers by construction. Dashes (trail, jump) are in **metres** along the chain, converted from the 2D
  pixel pattern at the car's scale (so they foreshorten with the road). **Tunnels** are a second range
  per tile, drawn last with the depth test off (underground, as the editor page). In-race focus
  (D66): a 1-byte-per-vertex attribute rebuilt (`RoadMesh::build_rel`) only when the `Arc<RoadFocus>`
  changes; muted roads take the mute colour / alpha / width factor without casing or dashes, hidden
  ones get alpha 0 (never a moved vertex: it would drag the triangle it shares with a visible sample
  across the screen).
- **Depth bias toward the eye** (0.2 % of the camera depth + 0.02 % per draw rank, `BIAS_*`):
  the coarse clipmap levels (cells ~ distance / 32) sit above or below the exact bilinear surface the
  roads follow, so unbiased ribbons were buried on slopes at distance; the per-rank step orders
  overlapping types (highway over road over trail) without z-fighting (since D81 per pass: the casing
  pass at 0.2 % + 0.005 % per rank, the fill pass at 0.25 % + 0.02 % per rank). A *relative* bias keeps working
  from the HUD's 1 km eye distance to the Viewer's 25 km (depth resolution is relative too).
- **LOD** (design 5.5, `roads::plan`): per 1 km tile, from the screen scale at the tile's nearest
  point: below `FAR_PPM` = 0.1 px/m the 32 m top-only index set, else the 8 m set with the deck; tiles
  outside the frustum or beyond the far fade are skipped (`Tile::in_frustum`). The HUD at the stopped
  zoom drops from the design's 585 k triangles to ~99 k.
- **Winding:** culling back faces of the road mesh needs `FRONT_FACE = CW` (K1's quads are
  counter-clockwise in a right-handed reading; the world is left-handed). Chosen by looking and
  pinned by `gl3d_culling_matches_no_culling` (the wrong face removes every road top: 836 -> 0
  pixels). The terrain is not culled (depth testing is enough).
- **State hygiene / Windows:** egui sets scissor and viewport before the callback and re-establishes
  its own state after, *except* the framebuffer binding. `Gl3d::render` reads `FRAMEBUFFER_BINDING`,
  `VIEWPORT` and `SCISSOR_*` first and restores them before the composite; the binding goes back to
  what it was (`None` only when it was 0), never a blind 0, because the Windows overlay renders into
  an offscreen FBO of its own (`wgl.rs`) and `Painter::intermediate_fbo()` is always `None`. Our
  textures use units 1-3 (egui only uses 0), unit 0 is active again on exit, `UNPACK_ALIGNMENT` is
  restored after the 2-byte height upload. No readback, no `glFinish` in production.
- **Dialects / requirements:** `#version 330 core` on desktop GL >= 3.3, `#version 300 es` on ES >=
  3.0 (also fine in the Windows compatibility context); explicit `layout(location)`, integer vertex
  attributes, no float textures, no instancing. `probe` demands the version, a 4096 px texture and a
  vertex texture unit; the shaders compiling, the FBO being complete and a clean `get_error()` in the
  first frames are checked where they happen. Optional: anisotropic filtering, `GL_TIME_ELAPSED`
  (desktop only; ES uses the CPU time).
- **Soft fallback + slow-GPU guard.** Any failure ends in `Gl3dStatus::Failed(reason)` (also
  `last_failure()`: the settings UI is on another thread than the HUD's context), the GL objects are
  freed, `eprintln!` says why, and the maps keep drawing 2D. A software GL passes every feature test
  and then costs 6-40 ms a frame, so the **guard** keeps an EMA of the frame cost (timer query, else
  the callback's CPU time) and fails with "3D is too slow on this GPU" above 8 ms for 2 s (frames
  after an upload and a pause > 1 s do not count; `Gl3dOptions::guard = None` disables it). On
  llvmpipe that means 3D is refused for real scenes, as intended.
- **Debug:** `FORZA_MAP_3D_DEBUG=1` prints one line per second (status, size, ppp, triangles, draws,
  tiles near / far, CPU and GPU ms, EMA) and the probe result once, so the user's machine can be
  checked without a profiler.
- **Node heights, orphans (K1 note):** nav orphans carry `y = 0` (`data::build_roads`), 100 m below
  the sea; `mesh3d::known_y` treats a height of exactly 0 or a non-finite one as unknown and uses
  the terrain there (a node-height road no longer dips to 0 at such a node; test
  `orphan_nodes_without_a_height_take_the_terrain_not_zero`).

**Defaults used** (design 9.3): sea flat at y 100, POIs / markers over the 3D in egui
without occlusion (the race lines are in the scene since D88) (a POI behind a ridge still shows: K6), exaggeration 1.0, shading 0.35, deck 3 m.

**Measured** before D81 (two road passes since, about twice the road triangles and draws, GPU
+0.004..0.012 ms: see Performance) (RX 7900 XTX, Mesa 26.2.4, release, `GL_TIME_ELAPSED` around terrain + roads, median of
20 frames; real island data; the design's numbers in brackets):

| Scene | triangles | draws | GPU ms | CPU ms (render) |
|---|---|---|---|---|
| HUD 208 x 136 driving 500 m, city (x3) | 122 k | 29 | 0.014 (0.015) | 0.41 |
| HUD bridge 150 m (x3) | 138 k | 14 | 0.013 | 0.46 |
| HUD mountain 500 m (x3) | 61 k | 13 | 0.013 (0.025) | 0.29 |
| HUD stopped 3 km | 99 k | 211 | 0.023 (0.059, 585 k tri) | 0.72 |
| Dashboard 600 x 400, 1.5 km, city | 294 k | 176 | 0.034 (0.051) | 0.45 |
| Viewer 1280 x 720, 8 km, island | 88 k | 180 | 0.028 (0.129, 962 k tri) | 0.58 |
| Viewer 3 km mountain | 93 k | 24 | 0.031 (0.038) | 1.0 |

The LOD sets make the heavy scenes 3-5x cheaper than the design's unculled 8 m set. ES 3.0 and
llvmpipe draw the same pictures (ES has no timer query). On llvmpipe (6 cores, release) the real scenes cost 9.5-14 ms of CPU for the HUD at 3x and 19-25 ms for Dashboard / Viewer (the design measured 8 ms for the HUD at 150 k triangles, 40 ms at 585 k), which the slow-GPU guard rejects, as intended. First-use cost: programs ~ms, heights
~16 ms, roads ~10 ms, one step per frame.

**Tests** (`gl3d/tests.rs`, headless EGL through `overlay::gl::Headless::new_with(Flavour, device)`;
GL tests are `#[ignore]`, run `GL3D_PNG_DIR=dir cargo test gl3d -- --ignored --test-threads=1
--nocapture`, GLES 3.0 needs `MESA_GLES_VERSION_OVERRIDE=3.0` in the environment of its own
process, `GL3D_DEVICE=<n>` runs the real-scene test on another EGL device): `gl3d_default_gl`,
`gl3d_gles30`, `gl3d_llvmpipe` (HUD at ppp 1 / 1.5, Dashboard, roads of every type by colour, markers
over the 3D, rounded mask, `get_error() == 0`, target FBO restored, PNGs),
`gl3d_callback_keeps_egui_state_and_respects_the_clip_rect` (egui meshes before / after, scissor,
the fade alpha), `gl3d_fallback_status_and_the_2d_map` (too-old context, broken shader, too-slow
guard: status text, and the picture is exactly the 2D map), `gl3d_culling_matches_no_culling`,
`gl3d_flat_world_equals_the_tilted_2d_map` (over a flat world the draped image equals the tilted 2D
base to 0.7/255 and a road lies on `Camera::project` to 0.01 px), `gl3d_in_race_focus_mutes_or_hides_the_other_roads`,
`gl3d_real_install_scenes` (real island, skipped without an install). Pure tests run in every `cargo
test`: clipmap tiling, the draw plan, the style table, the guard, mesh orphans.

**What cannot be agent-tested:** the Windows overlay (WGL, legacy compatibility context, offscreen FBO
readback) and a real eframe window (the Dashboard path is covered by the same `egui_glow::Painter`
code in the harness, but not by a window framebuffer 0 or a compositor `pixels_per_point`); an
integrated GPU, and the user's GPU while FH6 runs. The FBO-restore rule is the key defence on
Windows; K3 / K4 keep 3D opt-in there.

### 3D on the eframe side: Dashboard map and Viewer (phase K, K4)

`ui/map_scene.rs` follows the renderer's five-step call shape for both maps (they share `map_scene::draw`):

1. `relief_camera`: only when the map's View mode is 3D (and, on Windows, `map_3d_windows` is on) it asks
   `store::terrain()` (so the terrain loads only while a 3D map is on screen) and, once `Ready`, builds
   `Camera::from_cfg_relief`. `car_y` = telemetry `position_y + 1` while the view follows the car and a
   race is on, else `None` (the terrain height under the view centre: a panned view).
2. While `Map3d::wants_underlay()` (the scene is not `Ready` yet, or failed for good) the **tilted 2D map**
   is drawn with the relief-less camera: image, roads, race lines, POIs, *and the markers*. *Why the
   markers too:* they project with `cam.project`, which follows the terrain only for the relief camera;
   mixing the two would put the arrow off its road.
3. `gl3d::add_scene` with `corner_radius 0`, `a = s = 1`, the Viewer's / Dashboard's own `RoadsCfg`,
   `ReliefCfg`, `ImageLook`; the mesh from `store::road_mesh` only when roads are on; the in-race focus
   exactly when the 2D path applies it, plus `race_draw` + `store::race_mesh` for the race lines (D88). Always called while 3D is wanted, also during the underlay
   frames (the GL objects are created inside the callback).
4. Over it `draw_layers_parts(.., Parts::OVER_3D)` (POIs; the race lines are in the scene, D88), then the markers and compass.
5. `Gl3dHandle::busy()` -> `request_repaint()` (the init is staged over a few frames).

**Ownership.** `ForzaApp::map3d: Map3d` holds the *one* `Gl3dHandle` of the window's GL context. The
Dashboard map and the viewer share it: they are never on screen together and the renderer's FBO is
transient. `on_exit(Some(gl))` calls `Map3d::destroy(gl)` first (idempotent), eframe destroys its painter
after. **Retry:** the failure is sticky per handle; at the start of `update` (context current)
`RetryGate` fires on the frame a 3D map appears again (the settings page, another tab or Flat/Tilted
took it off screen in between) while the handle is failed: `destroy` + `gl3d::clear_failure_if` so the
next callback probes afresh (once per appearance; a "too slow" verdict costs two more slow seconds). The
map texture needs no change: `app.rs` already loads it with `MirroredRepeat`, and `gl3d` adds the mipmaps
itself on first use. The eframe window needs no depth or stencil buffer (the renderer has its own FBO).

**Pan, zoom and clicks in 3D.** In 3D, pan and wheel zoom anchor on the terrain surface under the pointer, not the car-height plane. `map_scene::draw` leaves the drawn 3D look in the egui context, keyed by the map's `Ui`, and `ManualView::interact` solves for the view centre that keeps the grabbed or zoomed terrain point under the pointer. A panned view's camera height eases (about 0.2 s) towards the terrain under the centre rather than snapping to it. *Why:* the camera height shifts the whole picture, so a pivot that follows the terrain exaggerates a pan uphill by several times, cancels it downhill, and makes the view jump on the first drag frame. `pick()` now ray-marches the terrain, so waypoint clicks on slopes are exact.

**Status line.** The View mode card shows, while 3D is picked: "Loading terrain…" (store `Loading`), "3D not
available: <reason>" (`gl3d::last_failure()`: an old GL, a shader that does not compile, "3D is too slow on
this GPU", or the store's no-install / read error), nothing when fine (`maprender::ui::status_3d`).

**Verified live** (headless sway, eframe on llvmpipe, `FORZA_MAP_3D_DEBUG=1`): Dashboard and viewer in 3D
with roads, race line and POIs; drag and wheel; no GL error; the slow-GPU guard tripping on llvmpipe,
the tilted fallback, the status line, and the retry on coming back to the map. Not verified: a real GPU, a
compositor `pixels_per_point` other than 1, Windows.

### 3D: the own car and the trails in the scene (D77, D78)

The user: "The arrow and the following trail should also be rendered in 3d, while the 3d mode is
enabled", "drawn according to coordinates, and the road height its driving on. i just noticed that a
tunnel made it 'fly'", and "a really simple 3d sedan model ... plus a normal arrow 3d model, and let
the user decide what he prefers". Before, the own arrow and the trails were flat egui shapes over the
GL picture, and `MapCanvas::to_screen` projects onto the **terrain surface** (`Camera::project`), so in
a tunnel they ran over the hill above it.

**What changed** (`maprender/gl3d/marker.rs`, `scene.rs`, `shaders.rs`; call sites `hud/minimap.rs`,
`ui/map_scene.rs`):

- **Position = telemetry x, y, z.** The car marker stands at the packet's position and height (the
  model's ground is `GROUND_BELOW_M` = 0.45 m under it: Forza reports roughly the centre of mass), the
  same height the 3D camera follows (`position_y + 1`). Without a live position (paused, no race on)
  it stands on the terrain. The trail points carry their height: `minimap::Trail` is now a deque of
  `TrailPt { x, y, z, t }` and `trail_push` takes the y (`ForzaApp::update_minimap_trails`, the HUD's
  `CoopLayer`); teammates' trails record their packets' heights too.
- **Two models built in code, no assets** (`marker::arrow_model`, `sedan_model`), flat-shaded, lit
  so the tops are exactly the car colour (white solo, the co-op colour in a session) and the sides
  darker:
  - **Arrow** (default, 14 triangles): the flat arrow's chevron (5 m x 3.8 m, a shallow notch at the
    back), extruded 0.5 m, with a low four-facet gem on top so the light shows it is 3D.
  - **Sedan** (196 triangles): lower body, dark glass greenhouse (raked windscreen, rear window), a
    roof slab in the body colour, four octagonal wheels, head- and tail-lights; from the usual view
    from above-behind it reads as hood / windscreen / roof / rear window / trunk with red tail-lights.
  Both get a dark ~1 pt outline (the flat arrow's black stroke): the model pushed out along its
  smoothed normals, drawn first without depth, the model over it.
- **Sizing rule:** real size (sedan 4.6 m, arrow 5 m) when that is big enough, else scaled up to a
  minimum on-screen length at the car: **20 pt** for the sedan, **16 pt** for the arrow (x the HUD's
  size factor `s`): `k = max(1, min_pt * s / (len_m * pt_per_m_at_car))` (`marker::model_scale`;
  `pt_per_m_at_car` = `Camera::view.scale` x the perspective at the marker). Heights are divided by the
  exaggeration so the model is not stretched with the hills. *Why a minimum:* at HUD zoom (500 m) a
  real car is ~2 px; the flat arrow was 14 pt and the car must stay as readable (it is ~14 px tall at
  the HUD's 1x, seen at 40 deg). Zoomed in on the Viewer it is real size.
- **Heading:** yaw only (telemetry yaw: forward = `(sin yaw, cos yaw)`); pitch and roll are left out
  (scaled up 5-10x at HUD zoom a tilted car would look odd, and yaw is what the arrow showed).
- **Tunnel visibility (the "it flies" bug):** the marker is **its own paint callback**
  (`gl3d::add_marker(painter, handle, MarkerScene)`), queued where the flat arrow was drawn - after
  the race lines and POIs, one callback for the own car and all teammates (D89) - rendered into the renderer's FBO with a depth buffer of its
  own and composited like the scene. So it is always whole and on top, also deep in a tunnel or
  under a bridge (like the tunnels, which the scene draws without the depth test), and correctly
  self-occluded. *Why not inside the scene pass:* a callback composites where it is queued; the POIs,
  race lines and the HUD's darkening tint are egui shapes queued after the scene, so a car inside it
  ended up under POI icons and dimmed (seen in the HUD harness, `m2_3d_coop`). *Why not
  depth-tested against the terrain:* the own car is the one thing that must never disappear; the
  camera follows it, so a hill hiding it would only ever be a hill between the eye and the car.
- **Trails** are ribbons in the scene pass (`draw_trails`, after the roads), 2 pt x `s` wide at the
  car with the flat trail's taper (0.4..1.5 x by depth), its colour and fade (`map_shared::trail_3d`
  turns a `Trail` into `Trail3d` segments with the `TrailFade` alpha). Each vertex carries both ends
  of its segment and is pushed sideways on screen in the vertex shader. Depth-tested twice: the
  visible parts at full strength, then the parts **behind terrain** (through a tunnel, behind a hill)
  with the test inverted at 45 % (`GHOST`): a trail into a tunnel stays visible, fainter, "seen
  through" the hill, like the tunnel itself. They stay under the egui race lines / POIs (a deviation
  from 2D, where trails were over them).
- **Flat in 2D, Tilted and the fallback.** The flat arrow and trails are drawn whenever the scene is
  not drawing (`wants_underlay`: Flat / Tilted, the frames before `Ready`, a failed context), so the
  failed-3D picture is still exactly the tilted 2D map.
- **Teammates are the same 3D car as the own car (D89, builds on D87):** a teammate's body is the
  GL arrow / sedan - whichever the map's *Car marker* picks (`ReliefCfg::marker`, no setting of its
  own) - at the teammate's telemetry position, **height** and yaw, tinted with their co-op colour
  (grey while paused, like the flat arrow), outlined, in a tunnel down at the road like the own car.
  `map_shared::remote_markers_3d` turns the `Remote`s that are inside the map's bounds (the same
  `within` test that decides arrow or edge pointer) into `Marker3d`s; they go into
  `MarkerScene::mates`, so **the own car and every teammate are ONE callback**: `render_markers`
  clears the FBO once, then per marker 2 draws (outline hull, model) with a depth clear between them
  (later = on top: teammates first, the own car last), and one composite for all. **Cost per extra
  teammate: 2 draw calls, ~7 uniform calls and one depth clear; no FBO clear, no composite, no
  callback** (the harness asserts `draws` grows by exactly 4 for 2 teammates). The name label and the
  edge pointer stay egui (`draw_remotes_in(.., flat_body = false)`: no arrow polygon, the label lifted
  a little more to clear the model), drawn **after** the marker callback so a name is not under a
  car. A paused teammate uses the height of its last known spot (`CoopSeen::y`, the HUD's
  `last_pos`); a `None` height stands on the terrain. Flat / Tilted, and 3D while the scene is not
  `Ready` (the underlay), keep the flat arrows (`flat_body = true`). The name label and the edge
  pointer are still placed with D87's `MapCanvas::to_screen_at` (`Camera::project3` at
  `y - GROUND_BELOW_M`), so they sit over the model, not on the hill. Teammates' trails were already in the
  scene at their recorded heights. *Why (D89, the user, 2026-10-10):* "The 3d car model should also
  be used for co-op players." *Why D87 did not do it then, and what changed:* a GL marker used to be
  a callback of its own (3 draws + a composite + an FBO clear) per car, too much per teammate; the
  marker pass now takes a list, so a teammate is two draws. *Why the same pass, not the scene
  pass:* same reason as the own car (the POIs are egui shapes queued after the scene). *Why a
  teammate also gets a depth clear and a hull of its own:* it must be whole and outlined like the own
  car (a tunnel!), and the own car on top where they overlap.
- **Waypoints stay on the terrain surface.** The co-op waypoint is one (x, z) per player on the wire
  (`coop.rs`) and a click in 3D picks the terrain under the pointer, so the terrain is where it
  was placed; there is no height to use, and the wire format is not changed for it.
- **GL resources:** two more programs (`marker`, `trail`) compiled with the others; the two model
  VBOs uploaded at the first marker, the trail VBO streamed per frame (`STREAM_DRAW`, 6 vertices per
  segment, ≤ 400 points per player); all freed by `Gl3d::destroy`. GLSL 330 core / 300 es as before.
- **Config (D78):** `ReliefCfg::marker: MarkerStyle { Arrow (default), Sedan }` (`serde(default)`,
  snake_case `"arrow"` / `"sedan"`), the **Car marker** row in the View mode card's 3D section, per map;
  "Copy to…" on that card copies it (it travels with `TiltCfg`). *Why Arrow by default:* it keeps
  today's look until the user picks the car.

Tests: `marker::tests` (model sizes and triangle counts, the transform: position / yaw / scale /
exaggeration, the sizing rule, trail vertices carry the heights and the fade), the GL suite's
`markers` section (every flavour: both markers on a hill, on the bridge and **in the tunnel** with a
trail going in, HUD 1x / 3x and Viewer; the marker is there, whole, outlined; the tunnel trail
shows; the sedan is 14 px tall at HUD 1x), and the HUD harness states `hills_trail`, `hills_sedan`,
`bridge_sedan`, `tunnel`, `tunnel_sedan` (a tunnel through the big hill was added to its world).
D89: the GL suite's `mates` section (both kinds: two teammates + the own car, `draws` = the own car's
+ exactly 4, each teammate's colour appears at its projected position, the own car on top of a
teammate at the same spot; a teammate **in the tunnel** is at the tunnel point and not at the hill
surface above it, Viewer + HUD 1x / 3x), `map_shared`'s `remote_markers_3d` unit test (inside the
test of D87's tunnel teammate: height, yaw, colour, paused grey, no height = terrain, none for flat /
tilted, none off the map, no egui arrow with `flat_body = false` but the name and the pointer kept)
and the HUD harness states `coop_sedan`, `coop_tunnel_sedan`, `coop_tunnel` (teammate pixels checked).

### Configuration

`AppConfig::minimap_layers` (Dashboard; in `MINISETTINGS_KEYS`) and `OverlayConfig::map_layers` (HUD;
`OverlayConfig::effective()` copies the Dashboard's when "Use Dashboard map settings" is on). **All
of it has a settings UI on the Map tab** (D63, `maprender/ui.rs::layers_ui`, one function for all
the maps: pages *Minimap* and *Dashboard map & Viewer*; see [map-tab.md](map-tab.md), where "Copy
to …" ports a card between them, D68); the JSON stays editable.
Every field has `serde(default)`, colours are `"#rrggbb"`, POI categories are a list of ids.

| | Dashboard | HUD |
|---|---|---|
| Satellite image | on, 100 % opacity / brightness / saturation | on, opacity 100 %, brightness and saturation 50 % |
| Roads, per type | on, "by type" colours | same |
| POIs | on, hidden above a 10 km view, everywhere: barn finds, car meets, festival sites, houses, aftermarket spots + boards, speed traps, speed zones, trailblazers, drift zones, danger signs, current-season treasure chest; every other kind off but selectable | on, hidden above a 3 km view, only within 1 km of the car: festival sites, houses, speed traps, speed zones, trailblazers, drift zones, danger signs |
| Race lines | current | current |
| Tilt | off | on (40 deg, P 200 at 136 px, car 85 %, taper on) |
| Navigation route (`nav_route`, phase L) | on, `#d946ef`, width 1.0 | same |
| Existing keys (D62, D71) | radius 1 500 m driving / 4 500 m stopped, north-up (also when stopped), no compass | 500 m driving (3 000 m stopped), heading-up, no minimap plate (`map_plate_opacity` 0) |

The existing Dashboard keys (`minimap_zoom_*_m`, `minimap_north_up`, `minimap_show_compass`) changed
**defaults only**: fresh installs get them from the embedded `assets/default-config.json` and
`AppConfig::default()`; a saved config keeps its own values.

Defaults = the user's own settings of 2026-10-08 (D71; they replaced the demo-export values of D62: POI list, zoom radii, HUD image opacity and tilt angle).

### Performance

Release, one thread, real data, car on a busy part of the island (`cargo test --release bench_ --
--ignored --nocapture`; shape building + egui tessellation, median of 40 frames): Dashboard 900 x 600
at 5 km 0.5 + 1.3 ms (1 448 chains, 19 k vertices after thinning), at 1.5 km 0.1 + 0.3 ms; 420 x 420
at 5 km 0.4 + 0.6 ms; tilted 900 x 600 at 5 km 0.8 + 0.6 ms; HUD-sized 208 x 136 tilted at 300 m
0.1 + 0.1 ms. With the game's icons and gate lines (I29b, `maprender::paint2d` bench): HUD-sized tilted
at 300 m 0.20 + 0.13 ms (144 chains, 3 k vertices, 20 POIs, 7 gates), at 700 m 0.56 + 0.30 ms (90
POIs); tilted 900 x 600 at 5 km 1.3 + 0.6 ms with the taper. Debug builds are about 8x slower. Loading: `GameData::load` 0.44 s in a cold test run,
a roads rebuild 4 ms.

**D81 (road joins), the same bench before -> after on one machine** (build + tessellate, ms): Dashboard
900 x 600 at 5 km 0.73 + 1.22 -> 0.76 + 1.08 (roads alone 0.73 + 1.16), at 1.5 km roads alone 0.23 +
0.37; 420 x 420 at 5 km 0.64 + 0.79 -> 0.76 + 1.06; tilted 900 x 600 at 5 km 1.68 + 0.92 -> 1.67 + 0.69;
HUD 208 x 136 tilted at 300 m 0.12 + 0.08 -> 0.13 + 0.07, at 700 m 0.38 + 0.16 -> 0.37 + 0.17. *The
first cut drew every cap and wedge*: ~4 000 tiny polygons at 5 km doubled it (1.58 + 2.02 ms), so
caps, wedges and overpass outlines of lines narrower than 3 px are skipped (`DECO_MIN_R`; unseen at
that size). The road layer rebuild 5 -> 12 ms (the join table, loader thread). 3D: the mesh build 16 ->
20 ms release (real island), the real-scene GPU time +0.004..0.012 ms (twice the road draws and
triangles: HUD city 122 k -> 219 k, Dashboard city 294 k -> 608 k), render CPU unchanged (~0.3 ms).

## Solo trail

Outside a co-op session the map draws **your own breadcrumb trail in white** (the own arrow's
outside-session colour, same rule), with the same fade settings as the co-op trails (Dashboard: Map →
Co-Op "Tracer fade", `coop_trail_fade_secs` / `coop_trail_fade_m`; HUD: its trail fade). It is the
`"local"` entry of the same trail buffers (`ForzaApp::minimap_trails`, the HUD's `CoopLayer::trails`),
recorded with `trail_push` while driving (race on, not paused).

- **Session start / end:** the own trail's points are kept; only its colour follows the current
  state (co-op colour in a session, white outside). Teammates' trails, last-known spots and waypoints
  are dropped when the session ends, as before. *Why:* simplest, and a trail that suddenly vanished
  (or a gap) on joining a session would read as a glitch; its points still age out by the fade.
- **Toggles:** the Dashboard has no trail switch (it never had one for co-op either), so the solo trail
  is always on there. The HUD's **Show trails** (`coop_trails`, `true` under "Use Dashboard co-op
  settings") gates the own trail in and out of a session alike: off = no solo trail either.
- *Why:* the user asked for the trails "in solo too, but only in white".

## Co-Op integration

When in a [[coop]] session, the map additionally draws:

- **Breadcrumb trails** — each player's recent path (yours too, which is also drawn
  solo, see above), in their identity colour, fading out by whichever comes first: age
  (**Fade after (time)**) or distance behind the player's current position
  (**Fade after (distance)**).
- **Remote players** — coloured heading arrows with name labels when on-screen;
  off-screen teammates are clamped to the map edge as a small marker with the
  distance to them. A paused teammate is shown grey at their last known position
  with a pause glyph instead of their arrow.
- **Shared waypoints** — left-click the map to drop a pulsing diamond waypoint
  (in your colour) that every player in the session sees, with each player's
  distance to it shown; right-click clears it. Waypoints are only active outside
  Edit Mode (Edit Mode reserves clicks for drag/resize).
- **On-map player list** (**Show player list on map**) — a fixed-width panel in the
  top-right listing name, and optionally distance/speed/gear/car-class columns per
  player, so the layout doesn't reflow as values change.

Click/drag on the map is only sensed outside Dashboard Edit Mode, so grid
drag/resize gestures take priority while editing.

## Pan, zoom and the Map tab viewer (D67, D72)

The Dashboard map widget and the Map tab's viewer draw one scene (`ui/map_scene.rs`). Both can be
panned (drag) and zoomed (wheel, around the cursor once panned); the Dashboard map only outside
layout-edit mode. The manual view is temporary: it resets to following the car and the configured
zoom once the player drives off again (>= 3 m/s for 0.6 s after having been stopped; no telemetry
keeps it). Option `minimap_allow_pan_zoom` (default on). Details: [map-tab.md](map-tab.md).
