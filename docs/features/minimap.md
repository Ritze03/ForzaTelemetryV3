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

The four bundled season maps (`assets/maps/{spring,summer,autumn,winter}.jpg`, 8192²,
embedded via `include_bytes!` in `src/minimap.rs`) are the hi-res seasonal FH6 maps
published by Reddit user **Le0_X8**:
<https://www.reddit.com/r/ForzaHorizon/comments/1td6qzb/8096x_hires_seasonal_maps_of_fh6_from_the_early/>

The credit is shown in the app: **Setup** tab → **Repository / Credits** category
(`repo_card` in `src/ui/settings.rs`), as the link "Le0_X8 — seasonal map images" under
"Credits".

**Why it matters:** if the images are replaced, re-cropped, or used to derive other data
(e.g. road extraction), keep that credit (or update it to the new source) — the maps are
someone else's work.

## Shared code (`src/minimap.rs`)

The season logic, the image loading/cache and the map maths live in `src/minimap.rs`,
shared by this widget and the in-game HUD overlay's minimap ([[overlay]]). Nothing in it
takes `&ForzaApp`, so the overlay thread can call it.

- **Cache:** `load_map_color_image(season, quality)` decodes the 8192² JPEG once, writes
  `map_cache/<season>_q<quality>.bin`, and reads that file afterwards. The write is
  **atomic** (unique temp file, then rename). *Why:* the Dashboard loader and the overlay's
  `hud-map` thread can build or read the same file at once, and a reader must never see a
  partial file.
- **Maths:** `MapCalibration` (world ↔ UV in original-image pixels), `MapView` (heading-up
  offsets, UV per mesh vertex, compass direction, arrow angle), `target_yaw` /
  `ease_yaw` / `ease_zoom`, and the stopped rule (`STOPPED_KMH` 5, `STOPPED_SECS` 1.5).
  The Dashboard camera in `app.rs` uses these too.
- **The overlay's copy:** `overlay_map_image(season)` is the **q50 (4096²)** cache file,
  uploaded with `OVERLAY_MAP_TEXTURE_OPTIONS`: linear filtering with **trilinear mipmaps**
  (egui_glow builds the chain on upload) and **ClampToEdge**. *Why mipmaps:* the HUD map is
  heavily minified and rotates, which shimmers without them. The Dashboard map has no
  mipmaps (it aliases; out of scope so far) and its texture wraps with MirroredRepeat
  (visible when **Mirror map at edges** lets UVs past the edge); the HUD map doesn't mirror.
  Switch `OVERLAY_MAP_TEXTURE_OPTIONS.wrap_mode` if it ever should. The q50 file is shared with a Dashboard set
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

## Co-Op integration

When in a [[coop]] session, the map additionally draws:

- **Breadcrumb trails** — each player's (including your own) recent path, drawn
  in their identity colour, fading out by whichever comes first: age
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
