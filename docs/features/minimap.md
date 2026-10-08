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

Phase J. The Dashboard map draws the base image, roads by type, jump lines, race lines and points of
interest through one renderer, `src/maprender/`; the HUD minimap will call the very same functions
(I29b; until then `hud/minimap.rs` keeps its own image code and only imports `clip_convex` / `fan`
from here). **Why (D61):** the user wants the two maps to look alike, and one drawing path means they
can't drift. `hud::map_shared` already shares the *markers*; `maprender` shares the *world*. A 3D mode
(phase K) will be one more renderer shared the same way.

| Module | What it holds |
|---|---|
| `cfg.rs` | `MapLayerConfig` (image look, roads + per-type style, POIs, race lines, tilt) and its serde; `::dashboard()` (= `Default`) and `::hud()`. |
| `data.rs` | `MapLayers { rev, roads, pois, races, note }` (three `Arc`s), `build_roads`, `GameData::load` (nav + POIs + race lines), `PoiLayer` (250 m cell grid), `RaceLayer` (100 m segment grid). |
| `store.rs` | The process-wide loader / cache: `layers()`, `refresh_now()`, thread `map-layers`. |
| `view.rs` | `Camera` (flat or tilted), `world_aabb`, `thin`, `clip_convex`, `clip_polyline_convex`, `fan`. |
| `style.rs` | Road draw order, the zoom-dependent width rule, dash patterns, the POI category table. |
| `racesel.rs` | `RaceSel`: which race lines to draw, incl. the "current race" guess. |
| `paint2d.rs` | `draw_base` (image mesh) and `draw_layers` (roads, jumps, race lines, POIs) onto an egui `Painter`. |

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
- **POIs**: the reader's `Poi`s, each mapped to a *category* (`style::POI_CATS`, the demo's category
  ids: `barn_find`, `car_meet`, `speed_zone` ...). Items with `|x|` or `|z|` beyond 12 km are dropped:
  11 parking areas sit at z ~ 18 km, off the map. A 250 m cell grid culls by the view box.
- **Race lines**: the reader's `RaceLine`s (170, decimated to 5 m, ~171 k points) plus a 100 m
  segment grid over all segments.

### Store lifecycle

`maprender::layers()` returns `Layers { status, data }` (`NoInstall | Loading | Ready | Error`, and
`Option<Arc<MapLayers>>`). A map calls it only while one of its layers is on, so a user who never
enables them never pays the load (~70 ms release, mostly nav + POIs + race lines).

Two cache levels: the install-derived `GameData` is keyed on the install's `media` path (looked up at
most every 2 s, since Steam detection hits the filesystem); the `RoadLayer` is keyed on
`(media, mtime + len of the road-type override file)`, the same key `MapData::poll` uses for the Setup
card, checked at most once a second. A change spawns the `map-layers` thread, which re-reads
`RoadTypes::current(override_path(), &nav)` and rebuilds only the roads (~4 ms release); POIs and race
lines are shared `Arc`s, and the old data is served until the new arrives. So an editor **Save** or
**Reset** shows up on the Dashboard within about a second, independently of the editor server and of
the egui frame loop; `app.rs:poll_map_editor` calls `maprender::refresh_now()` on `MapEvent::Saved` to
remove that second. A failed load is remembered with its key and **not retried** until the key changes
(no retry loop); a panic in the loader becomes `Error`. Without an install the status is `NoInstall`
and the map is the image alone, as before.

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
  jump 0.875. At the Dashboard's default 5 km radius (0.042 px/m on a 420 px map) the base is the 1 px
  minimum; at 300 m it is 7 px.
- **Dashes**: dashed types (trail, jump) use the demo's `[6,4]` pattern scaled by `max(1, 0.6 w)`, but
  only above 0.02 px/m (below that they are sub-pixel and draw solid). The casing is always solid.
- **POIs**: a marker per enabled category within the view, nothing above `max_zoom_m` (3 km; **the
  Dashboard's default 5 km radius therefore shows no POIs until you zoom in**), optionally only within
  `radius_m` of the car. Back to front by screen y. **Icons are a hook (D64)**: `IconAtlas { texture,
  rects }` is a texture id plus a UV rect per category; without one, a coloured circle / diamond /
  square / ring is drawn (the demo's colours). The game's icon decoder (a separate task) fills the
  atlas; the Dashboard passes `None` today. **Gate lines of speed zones / trailblazers / drift zones
  are not drawn yet:** `Poi` carries only the gate's midpoint (the reader drops the left / right
  marker), so `gates` is a config bit waiting for that data. Danger signs and the current-season
  treasure chest have their category ids and config bits (`danger_sign`, `treasure_chest_current`) but
  no `PoiKind` until the reader adds them.
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

### Tilted view (D65)

`view::Camera { view: MapView, centre, rect, pitch, focal }`. At pitch 0 it is exactly `MapView`'s
mapping (tested). Tilted, it is a **flat perspective of the 2D map** done on the CPU, the equivalent of
the demo's CSS `perspective(P) rotateX(a)` about the car: a plane offset `(x, y)` px from the car
(y down) lands at `(x, y cos a) * P / (P - y sin a)`, and the horizon is `P / tan a` above the car
(defaults: a = 55 deg, P = 200 px, car 85 % down the view). The image is the visible part of the plane
in 24 x 24 cells, each projected and cut to the map outline, every vertex's UV taken from the inverse
projection (egui interpolates UVs affinely inside a triangle; a perspective is not affine, hence the
subdivision). Every layer vertex goes through the same `Camera::project`, so roads, race lines and POIs
agree with the image; POI icons stay upright and scale with the perspective. *Why a camera type now:*
phase K's GL 3D scene will reuse it, and this CPU version doubles as a cross-check for the GL matrices.
Limits today: the Dashboard has the option in config JSON only (`minimap_layers.tilt.on`); trails,
teammates and waypoints (`hud::map_shared`) still use the flat mapping, so with tilt on they are off
(I29b gives `MapCanvas` the camera); line widths are constant in screen px, they do not taper with
distance as the demo's CSS transform does; the far edge is not faded into the backing (the demo fades
the top 30 %). The perspective distance is in px, so a value tuned for the 136 px HUD pill (200) puts
the horizon inside a large Dashboard widget; scale it with the view (the benchmark uses `200 * h / 136`).

### Configuration

`AppConfig::minimap_layers` (Dashboard; in `MINISETTINGS_KEYS`) and `OverlayConfig::map_layers` (HUD;
`OverlayConfig::effective()` copies the Dashboard's when "Use Dashboard map settings" is on). **There
is no settings UI yet** (D63: the next task puts all map settings on the Overlay tab); edit the JSON.
Every field has `serde(default)`, colours are `"#rrggbb"`, POI categories are a list of ids.

| | Dashboard | HUD (I29b wires it) |
|---|---|---|
| Satellite image | on, 100 % opacity / brightness / saturation | on, 50 % / 50 % / 50 % |
| Roads, per type | on, "by type" colours | same |
| POIs | on: barn finds, car meets, fast travel, festival sites, houses, aftermarket spots + boards, Horizon jobs + stories, XP boards, speed traps, speed zones, trailblazers, drift zones, danger signs, current-season treasure chest; every other kind off but selectable | same |
| Race lines | current | current |
| Tilt | off | on (55 deg, P 200, car 85 %) |
| Existing keys (D62) | radius 5 000 m driving and stopped, north-up, no compass | HUD: 300 m, heading-up (I29b) |

The existing Dashboard keys (`minimap_zoom_*_m`, `minimap_north_up`, `minimap_show_compass`) changed
**defaults only**: fresh installs get them from the embedded `assets/default-config.json` and
`AppConfig::default()`; a saved config keeps its own values.

### Performance

Release, one thread, real data, car on a busy part of the island (`cargo test --release bench_ --
--ignored --nocapture`; shape building + egui tessellation, median of 40 frames): Dashboard 900 x 600
at 5 km 0.5 + 1.3 ms (1 448 chains, 19 k vertices after thinning), at 1.5 km 0.1 + 0.3 ms; 420 x 420
at 5 km 0.4 + 0.6 ms; tilted 900 x 600 at 5 km 0.8 + 0.6 ms; HUD-sized 208 x 136 tilted at 300 m
0.1 + 0.1 ms. Debug builds are about 8x slower. Loading: `GameData::load` 0.44 s in a cold test run,
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
