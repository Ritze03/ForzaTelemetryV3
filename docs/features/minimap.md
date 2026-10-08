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

Option **Rotate with right stick** (`minimap_look_stick`, on by default; Mini-Settings -> Dashboard -> Map; the
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
`draw_remotes` (teammate arrows, edge pointers, names, paused grey), `draw_trail` (+ `TrailFade`),
`draw_waypoint`, and `draw_compass` (in `hud/minimap.rs`). Each takes a `MapCanvas` (painter,
`MapView`, centre, bounds rect, size factor `s`, fade alpha `a`): the Dashboard passes `s = 1`,
`a = 1`, the HUD its design scale and show/hide fade. Trail recording (`Trail`, `trail_push`) is
shared in `src/minimap.rs`. *Why:* the user wants the HUD map to match the Dashboard's, and one
implementation means a tweak to an arrow, label or trail lands on both. The buffers differ: the
Dashboard's is `ForzaApp::minimap_trails` (UI thread), the HUD's is `CoopLayer` (overlay thread).

## Shared renderer & layers (`src/maprender/`)

Phase J. The Dashboard map and the HUD minimap draw the base image, roads by type, jump lines, race
lines and points of interest through one renderer, `src/maprender/`: both call the very same
`draw_base` / `draw_layers` (the HUD since I29b, with the rounded pill clip, see [[overlay]] "Minimap
M2′"). **Why (D61):** the user wants the two maps to look alike, and one drawing path means they
can't drift. `hud::map_shared` already shares the *markers*; `maprender` shares the *world*. A 3D mode
(phase K) will be one more renderer shared the same way.

| Module | What it holds |
|---|---|
| `cfg.rs` | `MapLayerConfig` (image look, roads + per-type style, POIs, race lines incl. the in-race `focus`, tilt) and its serde; `::dashboard()` (= `Default`) and `::hud()`. |
| `data.rs` | `MapLayers { rev, roads, pois, races, icons, race_class, note }` (`Arc`s), `build_roads`, `GameData::load` (nav + POIs + race lines + danger signs + icons), `PoiLayer` (250 m cell grid, the chests apart for the weekly pick), `RaceLayer` (100 m segment grid). |
| `icontex.rs` | `IconTex`: uploads the store's icon pixels as a texture **per egui context** and builds that context's `IconAtlas`. |
| `store.rs` | The process-wide loader / cache: `layers()`, `refresh_now()`, thread `map-layers`. |
| `view.rs` | `Camera` (flat or tilted; `from_cfg`, `focal_for`, `depth_scale_at_row`), `world_aabb`, `thin`, `clip_convex`, `clip_polyline_convex`, `clip_segment_convex`, `fan`. |
| `style.rs` | Road draw order, the zoom-dependent width rule, dash patterns, the POI category table. |
| `racesel.rs` | `RaceSel`: which race lines to draw, incl. the "current race" guess, and the in-race focus (`RoadFocus`: which roads lie along the picked line). |
| `ui.rs` | The settings UI (D63): `layers_ui` (the Image / Tilted view / Race lines / Roads / Points of interest cards, used by both Overlay-tab map tabs), `view_rows` + `ViewCfg` (zoom / orientation options of either config), `status_ui` (store status + `MapLayers::note`). |
| `paint2d.rs` | `draw_base` (image mesh, far-edge fade) and `draw_layers` (roads, jumps, race lines, gate lines, POIs, the current chest) onto an egui `Painter`; `IconAtlas`, `CornerClip`. |

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
  test pins them). *Why not `mapedit::data::road_graph`:* it rounds to 0.1 m, loses the polyline
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

Order on the Dashboard: base image, then **`draw_layers`** (roads by type, jump lines, race lines with
start / finish marks, POIs), then trails, teammates, own arrow, waypoints, compass.

- **CPU `Painter`, no baking.** One `Shape::line` per visible chain, a casing pass then a fill pass per
  type, chains culled by their bbox against the view's world box (`Camera::world_aabb`), vertices
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

### Race lines and the "current race" guess

Modes: `off`, `current` (default), `nearest`, `near` (within `radius_m`, default 1 500 m), `all`
(40 000-vertex budget). 4 px, circuit `#38bdf8`, sprint `#fb7185`, alpha .85, with start / finish marks
(green dot + chequered flag for sprints, chequered flag for circuits).

The telemetry has **no race id**, so "current" is a **best-effort guess**, not verified against live
races: while `race_position != 0` (the HUD's own "in a race" rule) the line is the one whose centre
line passes within `|half-width| + 4 m` of the car with a driving direction that agrees with the car's
heading (dot product > 0). Several can match (many races share roads), so the closest wins, and the
previous pick is **kept while it still matches** (no flicker between overlapping lines) and for a
moment when the car is off every line. The lookup is the 100 m segment grid built once in the store.
`RaceSel` (kept per map; the Dashboard's is `ForzaApp::minimap_race_sel`) holds the pick.
`RaceSel::update` returns the selector itself (`picked()` = the lines), because the renderer reads the
in-race focus from it too (below).

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
| `other_roads` | `muted` | `normal` / `muted` / `hidden`: roads off the race's corridor |
| `mute_color` | `#ffffff` | colour of muted roads |
| `mute_alpha` | 0.25 | their alpha |
| `mute_width` | 0.8 | factor on each type's own width (keeps highway > road) |
| `hide_pois` | `true` | hide icons, gate lines and the current chest in a race (start / finish marks stay) |

*Why the defaults (lead's call, user can change them):* the user complained about distraction, so the
default is to act; muted rather than hidden so the surroundings stay readable. Muted roads are solid,
without casing, drawn first (under the relevant roads); a type switched off stays off.

**What is relevant:** a road segment is relevant when more than half of its samples (every 10 m, both
ends included) lie within `|half-width| + 8 m` of the picked line's centre line (`RaceLine::half`, per
segment; a closed circuit's closing segment included). Roads that merely cross the line are therefore
muted except for their short pieces at the crossing (which the race line covers anyway); cross-country,
trail, tunnel and every other type count alike. A jump line is relevant when both ends are on the
corridor. `racesel::RoadFocus` cuts every road chain into `Run`s (`a..=b` points, relevant or not), so
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
phase K's GL 3D scene will reuse it, and this CPU version doubles as a cross-check for the GL matrices.
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
- Both maps' tilt settings are on the Map tab (Minimap / Dashboard map / Viewer page → Tilted view).

### Configuration

`AppConfig::minimap_layers` (Dashboard; in `MINISETTINGS_KEYS`) and `OverlayConfig::map_layers` (HUD;
`OverlayConfig::effective()` copies the Dashboard's when "Use Dashboard map settings" is on). **All
of it has a settings UI on the Map tab** (D63, `maprender/ui.rs::layers_ui`, one function for all
three maps: pages *Minimap*, *Dashboard map* and *Viewer*; see [map-tab.md](map-tab.md), where "Copy
to …" ports a card between them, D68); the JSON stays editable.
Every field has `serde(default)`, colours are `"#rrggbb"`, POI categories are a list of ids.

| | Dashboard | HUD |
|---|---|---|
| Satellite image | on, 100 % opacity / brightness / saturation | on, opacity 100 %, brightness and saturation 50 % |
| Roads, per type | on, "by type" colours | same |
| POIs | on, hidden above a 10 km view, everywhere: barn finds, car meets, festival sites, houses, aftermarket spots + boards, speed traps, speed zones, trailblazers, drift zones, danger signs, current-season treasure chest; every other kind off but selectable | on, hidden above a 3 km view, only within 1 km of the car: festival sites, houses, speed traps, speed zones, trailblazers, drift zones, danger signs |
| Race lines | current | current |
| Tilt | off | on (40 deg, P 200 at 136 px, car 85 %, taper on) |
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
