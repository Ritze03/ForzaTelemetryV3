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

*Why from the install (D1):* the app used to bundle four 8192² JPEGs (~109 MB, the binary was
145 MB; now ~31 MB). The map is Playground Games' IP, so the app must not ship it; it reads
the user's own copy instead (licensing rule, see `docs/game-data/fh6-game-files.md`). A bonus:
decoding the tiles is ~60 ms (one thread per tile row, `std::thread::scope`) against ~2.3 s
for the JPEG decode. *Why L2 directly for 50 %:* resizing level 3 down to 4096² costs ~1.7 s
(longer than the whole level-3 decode) and level 2 is the same imagery.

*Why the credit for the bundled images is gone:* the credits link (Setup → Repository /
Credits) thanking the Reddit user whose seasonal map images were bundled was kept only while
those images were used, and left with them (plan decision D49).

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
