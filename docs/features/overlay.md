# In-game HUD overlay

A compact, click-through HUD drawn **over the running game** on Linux (Wayland, X11) and, experimentally, Windows: RPM, gear and
speed, a minimap, and race position/lap (or a drift counter). It replaces FH6's stock HUD
with telemetry you pick. Configured in the **Overlay** tab. The user calls it the
"WSL overlay" / "WLR overlay" (see [[TERMINOLOGY]]): *WSL* here means **wlr-layer-shell**, not
Windows Subsystem for Linux.

Code: `src/overlay/` (runtime: Wayland, EGL, frame pacing, the snapshot type), `src/hud/`
(pure draw code), `src/listeners/hud.rs` + `lap_trace.rs` (the data), `src/ui/overlay_tab.rs`
(settings), `src/minimap.rs` (shared map maths), `OverlayConfig` in `src/config.rs`. The
original plan with every decision (D1–D28) is in `.claude/teamlead/plan/wsl-overlay.md`.

## Platform support

- **Works:** Wayland compositors with `wlr-layer-shell`. Tested on Hyprland; should work on
  others with it, such as Sway and KDE Plasma (untested). It is
  a surface on the `overlay` layer, which sits above fullscreen games.
- **Fallback: X11 / XWayland backend** (`overlay/x11.rs`) for GNOME (Mutter has
  no layer-shell) and native X11 sessions. **Picked automatically, no setup:** a plain
  `cargo run` tries layer-shell first and falls back to X11 (the layer-shell reason is logged:
  `overlay: layer-shell unavailable (<reason>); trying X11`). **Tested on GNOME 50.4** (Nobara 44 /
  Fedora 44 base, GNOME Wayland + XWayland, AMD RX 9070 XT, Mesa 26.2.3, two 1080p monitors
  DP-2/DP-3) — works. Still experimental in the remaining gaps: fractional scaling is untested,
  older Mutter may name outputs `XWAYLAND0…` (see below), and native X11 sessions are untested.
- **Experimental, untested: Windows** (own backend, see [Windows](#windows)).
- **Doesn't:** macOS. When no backend works the overlay reports a *disabled* reason
  instead of failing, shown in the Overlay tab's status line. On other OSes the tab greys out
  Enable overlay and says "The in-game overlay needs Linux (Wayland or X11) or Windows."
  (`overlay_tab::OVERLAY_OS` gates this and the status lines).
- **Backend selection** (`overlay::Backend::from_env`, env var `FORZA_OVERLAY_BACKEND`, a
  developer override only):
  - unset / empty / unknown / `auto` (case-insensitive) = **auto**: layer-shell, then X11. On
    Hyprland the first try succeeds, so it behaves exactly like `wayland`;
  - `wayland`: layer-shell only (then GNOME shows the `NoLayerShell` reason);
  - `x11`: the X11 backend only (needs `DISPLAY`; on GNOME Wayland that's XWayland).

  The dev test pattern (`FORZA_OVERLAY_TEST=1`) spawns through the same selection, so it works
  on GNOME without the variable too.
  - *Why auto by default:* the user wants a plain `cargo run` to just work (or visibly not)
    on any desktop, with no environment variables.
  - *Which error when both fail* (`overlay::auto_error`): the X11 one, since X11 is what could
    have worked there — unless layer-shell *was* present and failed later (connect/EGL), which
    means a layer-shell desktop whose own setup broke, so its error wins; and when neither
    `WAYLAND_DISPLAY` nor `DISPLAY` is set, `NoWayland`, whose text names both variables.
  - **How:** an **override-redirect** window with a **32-bit ARGB visual** (and its own
    colormap; border 0, background None), sized to one XRandR monitor, made click-through by
    an **empty XShape input region** (`shape_rectangles(SET, INPUT, …, [])`; the bounding
    shape stays full so it still draws). Never takes focus (override-redirect windows aren't
    managed). Mapped on show, **destroyed on hide**, like the layer surface.
  - *Why override-redirect + XShape:* Mutter has no layer-shell, and a normal window can't sit
    above a fullscreen game. Mutter keeps override-redirect X windows in its topmost stacking
    layer (above normal and fullscreen windows), and XWayland is always there on GNOME, so an
    OR window is the one surface a GNOME client can put over a fullscreen game. The empty
    *input* shape (not the bounding shape) is X11's equivalent of the empty `wl_region`.
  - EGL runs on the xcb connection (`EGL_EXT_platform_xcb`, Mesa ≥ 21) with the same 8-bit
    RGBA / no-MSAA config filter, narrowed to configs whose native visual is a depth-32
    TrueColor visual (Mesa also offers alpha configs on the 24-bit visual, whose alpha the
    compositor would ignore). `x11rb` is the same version/features winit already builds, so
    no new crate. ponytail: older NVIDIA drivers (before ~560) only support the Xlib platform.
  - **Output names:** the target (Fixed monitor / monitor detection / `FORZA_OVERLAY_OUTPUT`)
    is matched against RandR 1.5 monitor names; no match → the primary monitor → the first.
    On GNOME 50 XWayland reports the real connector names (`DP-2`…), but older Mutter/XWayland
    name them `XWAYLAND0…`, which never match. The backend logs
    `overlay[x11]: outputs: NAME WxH+X+Y (primary), …` at start (and on each RandR change)
    and `overlay[x11]: showing on NAME (no output named "X")` when it falls back. RandR
    screen/output/CRTC change events re-read the list and recreate the window if its monitor
    changed.
  - **Pacing:** no frame callbacks on X11, so every ping draws one frame (packets are ~60 Hz)
    and the same animation timer drives fades without packets; swap interval 0. The window
    also redraws on Expose/MapNotify, because a frame swapped before XWayland/the compositor
    adopted the window can be lost (the static test pattern would stay blank).
  - Known gaps (ponytail): no scale handling (1.0; GNOME without xwayland-native-scaling
    reports 1:1 px), and in a *native* X11 session a game raised later could cover the
    window (no re-raise). The Focus-only option needs a GNOME focus method (Window Calls
    extension); monitor detection's Hyprland default fails on GNOME, so use Fixed monitor.
- *Why layer-shell:* an ordinary window can't sit above a fullscreen game on Hyprland; a
  layer-shell overlay surface can (D1). No crate renders egui into layer-shell (winit has
  no layer-shell support), so the runtime is hand-built on sctk + glutin + egui_glow (all
  already in the dependency tree, no new crates).
- Always **click-through**: an empty input region and no keyboard interactivity, so clicks
  and keys go to the game. The HUD therefore can't be dragged in place; the layout is set
  in the tab.
- Scale factor is fixed at 1.0 (all target monitors are 1080p @1); fractional/integer
  scaling is a known ponytail in `overlay/render.rs` and `wayland.rs`.

## Windows

Experimental (D31): written **blind**. Nobody could run Windows while it was built; it compiles
(`cargo check --target x86_64-pc-windows-gnu`, and links) and its test pattern was smoke-tested
under Wine/Hyprland (transparent per-pixel alpha, text, ring), nothing more. Code: `overlay/win32.rs`
(thread, window, handle), `overlay/wgl.rs` (GL), `overlay/monitors.rs` (pure monitor choice),
`overlay/pacing.rs` (shared D17 pacing). The HUD drawing (`src/hud/`), `Renderer`, the snapshot
pipeline, focus-only, pause hiding and the Hide HUD hotkey are the same code as on Linux; the
Windows module is just another surface backend with the same `OverlayHandle` API (`app.rs`'s
`#[cfg(any(linux, windows))]` code drives either).

**Render path.** `Renderer` (egui + `egui_glow`) draws into an **offscreen FBO** on a private
**WGL context** (a legacy `wglCreateContext` on a hidden 1x1 helper window; drivers return their
newest compatibility profile, enough for egui_glow). Each frame the FBO is read back with
`glReadPixels(GL_BGRA)` straight into a **DIB section** (`CreateDIBSection`, positive height, so
row order matches GL's bottom-up) and presented with `UpdateLayeredWindow(ULW_ALPHA, AC_SRC_ALPHA)`.
egui_glow already blends premultiplied (and keeps destination alpha correct), so the bytes are
exactly the premultiplied BGRA the call wants: no CPU conversion.
- *Why not an OpenGL window surface:* per-pixel alpha through DWM needs a layered window
  (`UpdateLayeredWindow`), and a GL swap chain can't feed one; the alternative is a DXGI
  composition swap chain with `WS_EX_NOREDIRECTIONBITMAP`, which is a lot of new GPU code. This
  path reuses `Renderer` unchanged and adds only ~300 lines of WGL.
- *Cost:* one blocking readback + copy per frame (about 8 MB at 1080p, 33 MB at 4K) at ~60 Hz.
  ponytail: PBO double-buffering or a dirty rectangle if it ever shows up in a profile.
- *Why not `windows-sys` DWM/D3D or `glutin` WGL:* no new crates or features beyond
  `Win32_Graphics_Gdi/OpenGL`, `Win32_System_LibraryLoader`, `Win32_UI_HiDpi`, `Win32_Security`
  on the existing `windows-sys 0.59`, plus a direct `egui_glow` dependency (already built by
  eframe's glow backend on Windows).
- GL needs a real GPU driver (OpenGL 3+); in a VM without one (GDI Generic, GL 1.1) the overlay
  reports "Couldn't set up OpenGL (WGL) for the overlay: OpenGL 1.1 is too old ...".

**Window.** `WS_POPUP` with `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE |
WS_EX_TOOLWINDOW`, covering exactly one monitor's rectangle:
- `LAYERED`: per-pixel alpha. `TRANSPARENT` (plus `WM_NCHITTEST` returning `HTTRANSPARENT`):
  clicks fall through to the game. `NOACTIVATE` (plus `WM_MOUSEACTIVATE` returning
  `MA_NOACTIVATE`, shown with `SWP_NOACTIVATE`): it never takes focus, so the game keeps keyboard
  and pad input and Window Detection still sees the game as the foreground window. `TOPMOST`,
  **re-asserted once a second** since a borderless game may re-raise itself. `TOOLWINDOW`: no
  taskbar button, not in Alt+Tab.
- Created when the HUD becomes visible, shown after its first frame is presented (no empty
  flash), **destroyed when the fade-out ends**, like the Linux surfaces. *Why:* a topmost window over
  a borderless game can cost it the independent-flip/direct-scanout fast path; a hidden HUD must
  have no window at all. The GL context, painter and map texture stay alive.

**Thread and loop.** One `overlay` thread (spawned and waited for like Linux: `OverlayHandle::spawn`
blocks up to 5 s, run from `overlay-start`). `MsgWaitForMultipleObjects` on **one auto-reset
event** plus the window's messages. The event is the listener's `Waker` ping *and* the doorbell
for `OverlayCmd`s (a mutex-guarded queue, `OverlaySender`), so no calloop on Windows. Frames: one
per ping (coalescing); the animation fallback timer (`pacing::next_wake`, also used by Wayland)
is the wait timeout; a 1 s housekeeping tick re-asserts topmost and re-checks the monitor.
Windows' default timer granularity (~15.6 ms) makes the 16 ms animation step coarse when packets
have stopped; ponytail: `timeBeginPeriod(1)` if it looks choppy. Three failed frames in a row
stop the thread (the tab shows "stopped"), like Linux's EGL failures.

**Snapshot reuse.** Identical: `HudSink` writes the `SnapshotSlot`, `Waker::wake` sets the event,
the thread `try_lock`s the slot. Visibility (`HudSnapshot::visible`) is followed exactly as in
`wayland.rs`.

**Monitor.** `EnumDisplayMonitors` + `GetMonitorInfoW`; the overlay thread sets **per-monitor-v2
DPI awareness for itself** (`SetThreadDpiAwarenessContext`), whatever the process default is, so
rectangles are physical pixels and the bitmap is never stretched (a blurry or mis-sized HUD
otherwise). The HUD's own scale is `surface_height / 1080` already, so 1440p/4K need nothing more.
Which monitor (`monitors::pick`, pure, unit-tested): the target name (Overlay tab -> Monitor
Detection -> **Fixed monitor**, or `FORZA_OVERLAY_OUTPUT`) matched case-insensitively against the GDI
device name (`\\.\DISPLAY2`), the name without the prefix (`DISPLAY2`), or a 1-based number
(`2`, the enumeration order, which normally equals Windows' Display settings numbering but isn't
guaranteed); no match, empty or a Linux-style name (`DP-1`) -> the **primary monitor**. The
Hyprland/Custom detection methods don't exist on Windows. Instead `focus::query_monitor` has a
`#[cfg(windows)]` branch returning `overlay::foreground_monitor_name()` (the monitor of the
foreground window, `\\.\DISPLAYn`), run by the focus thread only while the game window is focused,
exactly like Hyprland on Linux. *Why:* it reuses the existing `MonitorMethod::Hyprland` variant (the
"built-in" detector; serde name unchanged, so the default config and profiles shared across OSes
just work, no new enum value or migration) rather than adding an `ActiveWindow` variant. On Windows
the Method combo shows **Active window (built in)** and **Fixed monitor** only; Custom command is
hidden (no `sh`), and a config that has `Custom` is normalised to the built-in method when the tab
is drawn. The Fixed field's Detect button fills in the focused monitor the same way.
**UI (Windows):** the Enable overlay checkbox's tooltip carries the "Borderless or Windowed only,
exclusive fullscreen can't be overlaid" note (a tooltip, not under-option text).
A changed monitor layout recreates the window within a second.

**Limits.**
- **Exclusive fullscreen can't be overlaid.** Only a *borderless/windowed* FH6 works; in exclusive
  fullscreen the game owns the display and no window can be drawn above it. Set the game to
  borderless (FH6's default "Fullscreen" is usually borderless-fullscreen already).
- Anti-cheat/overlay-hostile software, capture software hooks and HDR output are all unverified.
- `FORZA_OVERLAY_BACKEND` does nothing on Windows.

**Testing it on Windows** (for whoever has a machine or VM with a GPU driver):
1. Build: `cargo build --release` (or use a build artifact). Run it with the console variable
   `set FORZA_OVERLAY_TEST=1` first (PowerShell: `$env:FORZA_OVERLAY_TEST=1`): a pill, outlined text
   and a ring appear bottom-centre of the primary monitor without the game. `=2` redraws at
   60 Hz with a counter. `FORZA_OVERLAY_OUTPUT=2` picks monitor 2. Check: it is crisp (no blur) at
   your display scaling, clicks pass through, it's absent from the taskbar and Alt+Tab, typing in
   another window keeps working, and it stays on top of a borderless fullscreen window.
2. Without the variable: Overlay tab -> **Enable overlay** (note: the checkbox is greyed out until
   the tab drops its Linux-only gate), start Forza (borderless), enable Data Out; the HUD should
   show while driving and hide in menus/pause.
3. On a failure the status line shows the reason (`Couldn't set up OpenGL (WGL)...` = driver/GL;
   `Couldn't create the overlay window:` = Win32). Stderr has `overlay: ...` lines (run from a
   console; release builds have no console, debug builds do).
4. Things to look at: colours/transparency (premultiplied alpha), the HUD sitting on the right
   monitor, flicker or a one-frame white/black flash on show, CPU use at 1440p/4K (readback),
   and the Hide HUD hotkey / focus-only behaviour.
5. **The 3D minimap is opt-in on Windows** (K3, K4): the Map tab's View mode *3D* (labelled "3D
   (experimental)" there) is treated as *Tilted* until **Allow 3D on Windows** is ticked on the View
   mode card (`OverlayConfig::map_3d_windows`; `hud::minimap::wants_3d`: `!cfg!(windows) ||
   map_3d_windows`). *Why:* the 3D scene renders into its own FBO inside the paint callback and
   restores the framebuffer binding it read first; the Windows overlay draws into its *own* offscreen
   FBO (`wgl.rs`) and reads it back, a combination nobody could run. For a tester: tick the box, set
   View mode to 3D and check the pill shows terrain with roads, no flicker, and that Hide/Show and a
   switch back to Tilted leave no stale picture; `FORZA_MAP_3D_DEBUG=1` prints a status line per
   second to stderr. The same flag also gates the Dashboard map and the Map-tab viewer
   ([map-tab.md](map-tab.md)).

## Widgets

All drawn in 1920×1080 design px and scaled by `surface_height / 1080 × scale`. The names
(D1a, D3a′, M2′, R1′, X1′) come from the design mockup's round-4 spec sheet
(`.claude/teamlead/plan/wsl-overlay-assets/overlay-mockup-v2.html`). HUD colours are the
mockup's values in `hud::col`, **not** `theme.rs` tokens: they're game-HUD semantics and never
follow the app theme.

### Drive cluster (`hud/cluster.rs`)

RPM, gear and speed in one super-compact widget (D9), in two styles:

Picked in a **Style** dropdown on the Overlay tab, labelled **Pill** / **Halo** (the D1a / D3a′
codes are the mockup's spec names, not shown in the UI). *Why a dropdown:* the user asked
for it; it also avoids the cryptic spec codes in the label.

- **Pill** (D1a) — 184×46 pill with a 16-segment level rev bar.
- **Halo** (D3a′) — 112×112 disc with a thick 24-sector ring on a 260° arc (idle → max). Each
  sector gets thicker with the rpm it stands for (inner radius 47 → 40, outer 52); redline
  is shown by colour only (D14).

Options (Drive Cluster card): style; **Show engine RPM instead of KM/H label** (on by default; D13: the
small unit text is replaced by live rpm digits, the speed value stays); **Update speed only
every 0.5 s** (`speed_hold`); shift flash; gear-change pulse; **Redline at** (`redline_frac`,
default 0.85); **Shift cue before calibration** (`shift_frac`, default 0.93, D21).

- **Shift cue and redline come from the gearbox's RPM calibration** (`listeners/hud.rs:cue_rpms`,
  fed from `worker.rs`). The calibrated value is `dynamic_max_rpm`: the highest rpm seen while
  the engine makes power without handbrake or tyre slip, per car (i.e. the real rev limiter).
  It is counted once `DsgListener::engaged` (first manual upshift, or a restored per-car
  profile), the same moment the DSG starts trusting it. With it: **shift cue =
  `dynamic_max_rpm × dsg_shift_rpm_pct`** (Gearbox → Shift RPM, default 98 %, the DSG's own
  full-throttle "redline upshift" point) and **redline = `redline_frac × dynamic_max_rpm`**.
  Without it (new car, after Clear RPM calibration): redline =
  `redline_frac × engine_max_rpm`, cue = `shift_frac × engine_max_rpm`. *Why:* the user asked
  for the gearbox's reliable max-rpm calibration to drive the cue. It runs whether the
  automatic gearbox is on or off (the max-rpm tracking and the `engaged` detection sit before
  the DSG's `dsg_enabled` gate), so manual drivers get it too. *Why keep `shift_frac` as a
  fallback instead of `dsg_shift_rpm_pct × engine_max_rpm`:* `engine_max_rpm` is the tacho's
  end, usually above the real limiter, so 98 % of it may never be reached and there'd be no
  cue at all before calibration.

- **Drive-mode letter.** While the Automatic Gearbox is switched on, a forward gear is
  prefixed with its drive mode: **D** Street, **S** Sport, **R** Race (e.g. "D4"), drawn a
  little smaller than the gear digit. The mode is `dsg_effective_mode(race_position != 0)`,
  i.e. the one in effect (Race when auto-switched in a race), computed in
  `HudTracker::snapshot` → `HudSnapshot::auto_gear`. *Why "switched on", not
  `DsgListener::engaged`:* engaged only means "has seen the first manual upshift", which the
  user can't see; the switch (G hotkey) is what they toggle and expect confirmed.
  **Reverse stays a plain "R" and neutral "N"**, never prefixed. *Why:* a prefixed reverse
  would read like Race mode.
- **Speed hold.** With `speed_hold` the speed number keeps its value for
  `hud::SPEED_HOLD_SECS` (0.5 s), then takes the live one; gear, rev bar/ring and the rpm
  label stay live. Off by default. *Why:* the user wanted a calmer number. It doesn't keep
  the HUD animating: packets redraw at ~60 Hz anyway, so a due refresh waits for the next one.

### Minimap M2′ (`hud/minimap.rs`)

208×136 pill-framed by default (size, shape and outline are options, [Minimap frame](#minimap-frame-d75)),
heading-up season map with a compass (optional, D12), the **same layers as
the Dashboard map** (roads, race lines, POIs; below) and the **same markers**: the own arrow, co-op teammates, trails and shared waypoints,
all drawn by `hud/map_shared.rs`, the code the Dashboard map widget calls too
([[minimap]] "Shared drawing"). *Why:* the user wanted the HUD map to look like the Dashboard
map ("the same arrow styling with the same co-op colour and the trail behind the player"), and
one implementation can't drift. This replaced the mockup's white car marker and chevron
teammate arrows (the spec sheet's M2′ look), which were the HUD's own.

- **Own arrow:** the Dashboard's triangle (14×~14, black outline), in the player's **co-op
  colour** (Co-Op → Your Identity, `AppConfig::coop_hue`, carried as `HudSnapshot::coop_hue`)
  while a co-op session runs, white otherwise (exactly the Dashboard's rule).
- **Teammates:** arrow in their colour with the name above (labels nudged apart), a pointer
  clamped to the edge with the distance when off the map, grey + pause glyph at the last known
  spot when paused. (The first HUD version skipped off-map and paused teammates.)
- **Trails:** each player's breadcrumb trail in their colour, fading by age or distance behind
  them, whichever first (`TrailFade`; `minimap::trail_push` records them). Teammates' trails only
  during a co-op session; **your own trail is drawn solo too, in white** (like the arrow), kept
  across a session starting or ending (only its colour changes), and switched off with **Show
  trails** (`coop_trails`) in and out of a session alike. Same as the Dashboard, see [[minimap]]
  "Solo trail" (the Dashboard's trail fade lives on the map's Co-Op sub-tab).
- **Shared waypoints:** the pulsing diamond with the distance (set by clicking the Dashboard map;
  the HUD cannot be clicked, it only shows them).
- **Trail transport:** the overlay thread records trails itself (`CoopLayer` in
  `hud/minimap.rs`, updated by `overlay/render.rs`) from the snapshot's packet (own) and
  `CoopReader::remote_players` (teammates); nothing travels in the snapshot except
  `coop_hue`. *Why not reuse `ForzaApp::minimap_trails`:* the UI thread fills that and its
  loop stops while the game covers the window, i.e. exactly when the HUD is in use. The
  recording rules are the shared `minimap::trail_push`, so both buffers behave alike.
  Zoom eases between **Zoom when driving** / **Zoom when stopped** (defaults 300 / 3000,
  independent of the Dashboard map's unless reused); stopped = under 5 km/h for 1.5 s.
- **Layers (I29b, D61/D62):** `minimap::draw` runs the shared renderer (`maprender`, see
  [[minimap]] "Shared renderer & layers") over a **camera** (`Camera::from_cfg(map_layers.tilt, ...)`:
  flat, or **tilted by default**, 40 deg, perspective 200 px at the pill's height, the car 85 % down):
  `draw_base` (the dimmed satellite, 50 % opacity / brightness / saturation, with the far edge fading
  out) → `draw_layers` (roads by type, jump lines, race lines, gate lines, POIs with the game's icons,
  the current treasure chest) → markers → compass → frame. The markers use the same camera
  (`MapCanvas`), so trails, teammates and waypoints sit on the tilted roads. **Corner clip:** roads,
  race lines and gate lines are cut to the pill outline (`CornerClip { poly: the 0.5 px inset rounded
  outline, safe: the pill shrunk by its radius }`, Cyrus-Beck only for segments outside `safe`; jump
  lines too since D75, they used to poke out), icons
  straddling a corner are clipped to it with their UVs, small markers near a corner are dropped; the
  3 px frame hides the rest. *Why a clip and not a scissor:* a rect scissor lets a road poke out of
  the rounded corner into the transparent surround. **No plate** (`map_plate_opacity` 0): the game
  shows through the dimmed image; a plate, if set, is drawn first and the far edge fades into it.
  Widths are in design px and scaled by the HUD's `s` (the zoom rule runs on `px/m / s`).
- **How the layer data reaches the HUD:** `overlay/render.rs` `Renderer::frame_at` calls
  `maprender::layers()` itself, **only while the minimap is on and a layer toggle is on**
  (`MapLayerConfig::wants_layers`), and hands the result to `Hud::set_layers` /
  `Hud::set_icons`, which store it in `MapAnim` (`minimap::draw`'s signature is fixed: the PNG harness
  and tests call it). *Why the overlay thread polls the store and nothing is forwarded:* the UI frame
  loop stops while the game covers the window, which is when the HUD is used; verified live with the
  Dashboard window hidden (the HUD map kept moving) and with no Dashboard map at all (the HUD alone
  starts the `map-layers` load). One `RaceSel` per map lives in `MapAnim`; "in a race" is the HUD's own
  rule, `race_position != 0`. **Icons:** `IconTex` (`maprender::icontex`) uploads the store's icon
  pixels into the overlay's own `egui::Context` (the Dashboard uploads its own copy; a GL object is
  never shared between the two contexts, only the decoded pixels), 512 x 384 RGBA with mipmaps,
  0.75 MB; dropped again when no layer is on.

#### Minimap in 3D

Phase K (K3, plan D61/D65/D66). The Map tab's **View mode** card (`Flat / Tilted / 3D`, stored as
`map_layers.tilt.{on, relief.on}`, see [map-tab.md](map-tab.md)) can put the HUD minimap in **3D**: the
satellite image draped on the game's height raster, the roads as ribbons at their nav heights with
decks, drawn by the shared GL renderer (`maprender::gl3d`, reference: [[minimap]] "3D renderer").
Default stays **Tilted**; 3D is opt-in.

- **Draw order (`hud/minimap.rs::draw`, 3D branch).** plate (as before) → the 3D scene as one egui
  paint callback over the pill (`gl3d::add_scene`, `corner_radius = Frame::radius * s`, 22 by default, the composite shader does
  the rounded mask and the HUD fade) → the **tint** (`MAP_TINT`, drawn *after* the scene because a
  callback is opaque and a tint under it would be invisible; it also dims the GL roads slightly) →
  race lines and POIs (`draw_layers_parts(.., Parts::OVER_3D)`: egui, projected through the relief
  camera so they sit on the terrain, **no occlusion by hills**, v1) → markers (`MapCanvas`, same
  camera) → compass → border. The relief camera is the tilted one plus heights (`Camera::from_cfg_relief`,
  D65): the car arrow sits at the same screen point as in Tilted; the car's height is the telemetry
  `position_y + 1 m` while driving, else the terrain under the car (`minimap::car_height`).
  The in-race focus (D66) is passed along: while the HUD's `RaceSel` has a focus line the scene
  gets a `Focus3d` and mutes / hides the other roads exactly as the 2D map does.
- **Where the 3D state lives.** `Renderer` (`overlay/render.rs`) owns a `Gl3dHandle` (no GL object
  until the first scene callback) and, each frame the Minimap is in 3D, polls `store::terrain()`
  (lazy: the first call starts the load, so a user who never selects 3D pays nothing) and
  `store::road_mesh(layers, terrain)`, and hands `Hud::set_scene3d(Some(Scene3dIn { gl3d, terrain, mesh }))`
  to `MapAnim` (like `set_layers` / `set_icons`: `minimap::draw`'s signature is fixed, the PNG
  harness and tests call it). `Hud::set_scene3d(None)` = the 2D map. `minimap::wants_3d(cfg)` is the
  one switch (`minimap_on`, view mode 3D, not Windows).
- **Staged init, frame pacing.** The renderer spreads its init over three callbacks (programs and
  static buffers, the 15 MB height raster, the road buffers), a few ms each, so the first 3D frame is
  not a 40 ms hitch in the game. `Renderer::frame_at` returns "animating" (so the pacing in
  `overlay/pacing.rs` keeps scheduling frames at ~60 Hz, never per monitor frame) while
  `Gl3dHandle::busy()` (read **after** the paint, i.e. this frame's), while the terrain is still
  loading, and while the road mesh is missing or stale (the `map-mesh` thread). *Why poll with
  frames:* nothing else would wake the overlay for them when no packet flows. A hidden HUD never
  counts: `waiting3d` needs `snap.visible` and `Hud` resets its state (the scene included) once the
  fade-out finished, so a half-initialised scene cannot pin the frame loop and keep the surface up.
- **Fallback.** Until the scene is `Ready`, and for good when it `Failed` (GL too old, shader
  compile, incomplete FBO, a GL error in the first frames, or the slow-GPU guard: > 8 ms for 2 s),
  `Gl3dHandle::wants_underlay()` is true and the minimap draws today's **tilted 2D map** (the
  relief-less camera, `Parts::ALL`, markers through the 2D camera). The harness asserts the failed
  3D HUD equals the Tilted picture pixel for pixel. The reason is `gl3d::last_failure()` (process
  global, the settings status line reads it; the HUD's context is unreachable from the UI thread).
  *Why a terrain that is still loading, missing or errored also gives the tilted map:*
  `Camera::from_cfg` ignores the relief, and `Renderer` sets no scene then.
- **Lifecycle and drop order.** Leaving 3D (view mode changed, minimap off) calls
  `Gl3dHandle::destroy` with the context current (terrain, road buffers, scene FBO freed; a no-op
  when nothing was created), which also resets a sticky `Failed`, so toggling the mode is the retry.
  `Renderer::drop` destroys it **before** `painter.destroy()` (see
  [Lifecycle and drop order](#lifecycle-and-drop-order)): its GL objects were made through the
  painter's context, which must still be current.
- **Windows: opt-in.** The Windows overlay path (WGL context, offscreen FBO, readback) cannot be
  tested here, and the 3D callback's FBO save/restore is the part most likely to differ, so on
  Windows `wants_3d` needs `OverlayConfig::map_3d_windows` (default false; one flag for the HUD,
  the Dashboard map and the viewer, ticked with **Allow 3D on Windows** on the View mode card, which
  reads "3D (experimental)" on Windows); without it *3D* behaves as *Tilted* there and the card's
  status line says so. *Why one flag:* the untested part is the Windows GL path as a whole, not a
  single map, and one tick is easier for a tester than three. Untested pieces on Windows: the FBO
  restore (`FRAMEBUFFER_BINDING` is read first and put back, which is what the offscreen path
  needs), the compat-profile `#version 330 core` compile, the 8 ms guard on integrated GPUs.
- **Verification** (Linux, headless EGL): `cargo test render_3d_states -- --ignored --nocapture`
  writes `target/hud_png/m2_3d_*` (synthetic terrain: hills, a bridge, race focus, horizon view,
  exaggeration, no roads / no image, co-op, fade 0.5, the failed-renderer fallback),
  `composite_3d_1080p` through the real `Renderer::frame_at` and, with an install,
  `m2_real_*_3d_*` (real terrain, roads, satellite) and a frame-time line. Checks: the pill's
  rounded corners stay clear, the map paints the pill, no GL error, the car arrow above the scene,
  relief / roads / image each change the picture, the fade scales it, the staged init settles,
  leaving 3D frees the renderer, a loading terrain asks for frames without touching GL, a hidden HUD
  never asks. CPU-side tests: `hud/tests.rs` `scene3d::*` (one callback, 2D underlay while not
  Ready, hidden reset, `wants_3d`).
- **Not agent-testable:** smoothness while FH6 saturates the GPU (the user's check), Windows, and an
  integrated GPU. `FORZA_MAP_3D_DEBUG=1` prints the renderer's status line once a second to stderr.

#### Minimap frame (D75)

The module's **frame** (shape, size, outline, background) is set on the **Overlay tab → Minimap** page
(`overlay_tab::minimap_card`), not the Map tab: it is the HUD module's look, like the Drive cluster's
style, while the Map tab keeps what the map *shows* (layers, view, zoom; D67). The card has a
"Map layers…" button that jumps to the Map tab's Minimap page. *Why (D75):* the user asked, "In the Overlay
tab, let me change the minimap size (width and height) + edit the outline/background of it. A second
style that is a circle would also be neat."

- **Config** (`OverlayConfig`, all `serde(default)`, defaults = the M2′ pill exactly, so a saved config
  and every default PNG are unchanged): `map_shape` (`RoundedRect` | `Circle`), `map_width` /
  `map_height` (208 / 136 design px, sliders 120-600), `map_corner_radius` (22, rect only, clamped to
  half the shorter side), `map_border_width` (3, 0 = no outline, 0-20), `map_border_color` (RGB
  `12,17,27`) + `map_border_opacity` (0.88), `map_plate_color` (RGB `9,13,21`) + the existing
  `map_plate_opacity` (0). *Why colour + opacity pairs instead of one RGBA:* the plate already was one,
  the egui colour picker round-trips an alpha channel lossily through premultiplied `Color32`, and
  the two controls read the same way. The default frame colour is `rgba(12,17,27,.88)`.
  `OverlayConfig::reset_map_frame` (the card's **Reset frame**) restores exactly these, leaving the
  map layers alone. `map_plate_opacity` is still also on the Map tab's Image card (one field, two
  controls).
- **`hud::minimap::Frame::of(cfg)`** resolves and sanitises it (NaN, absurd sizes 20-2000, a radius
  past half the side, a border past a quarter of it) so a hand-edited file can't reach the renderer;
  `minimap::size(cfg)` is the module's layout footprint. **Circle:** the diameter is `map_width`
  (the height control is hidden, `Frame` is square). *Why the width alone and not `min(w, h)`:* one
  size control, and a height that silently does nothing would confuse; switching shape keeps the
  width the user set.
- **How a circle is drawn.** One code path with the rounded rect, radius = half the size:
  the outline polygon (`prims::rounded_points_n`, 64 segments per quarter for a circle, the usual 12
  otherwise, so default pixels are bit-identical) feeds the plate, the tint, `draw_base`, the vector
  `CornerClip` (`safe` = a square inscribed at 0.68 of the diameter) and the border
  (`prims::rounded_border`, now with a segment count). **Markers** (`map_shared`) keep the painter's
  clip rect (the border-inset bounding box) and, for a circle, additionally cut trails to the circle
  (`clip_segment_circle`) and pin off-map teammates and waypoints to the circle, not the box
  (`draw_*_in(.., round)`; the plain `draw_*` the Dashboard calls are unchanged). The **compass**
  sits where the pill's does, `(border + 15, border + 15)` = `(18, 18)` by default, pulled in along
  the diagonal to stay inside a big corner radius, and on the up-left diagonal of a circle
  (`Frame::compass`; `draw_compass_at`, `draw_compass` keeps the Dashboard's signature).
  **3D:** the composite shader's mask is already a rounded-box SDF, so radius = size / 2 *is* a
  circle: no change to `maprender::gl3d`.
- **Layout footprint.** `hud::modules` passes `minimap::size(cfg)` instead of a constant; `layout.rs`
  already works on sizes, so a bigger map pushes its stack neighbours by exactly its height and keeps
  its margin anchoring (right column: right margin minus width; centre column: centred). The Layout
  card's chips do not scale with the module size (they only show cell and order).
- **PNG states** (`m2_frame_*`, `m2_3d_frame_*`): circle tilted / flat, a 300 x 200 rect (radius 40,
  6 px blue outline, blue plate showing through a half-transparent image), 2D and 3D. The harness checks
  that every pixel more than 1.5 px outside the shape is the backdrop (the map is clipped exactly), that
  the outline ring has its colour all round, and the co-op-coloured own arrow.

**Options (Mini-Settings → Overlay tab).** The HUD minimap has the Dashboard map's options.
*Why:* the user wants everything the Dashboard map has on the HUD too. They live in
`OverlayConfig` (`overlay.*`, not the top-level `minimap_*` keys), in the cog-wheel
**Mini-Settings**, tab **Overlay** (`src/app.rs`, "Minimap" and "Co-Op" sections; the Map
tab's Minimap page edits the same options and all the layer settings, see [map-tab.md](map-tab.md)).

| Field | Default | Meaning |
|---|---|---|
| `map_use_dashboard` | off | **Use Dashboard map settings**: all the map fields below follow the Dashboard map and their controls are hidden. |
| `map_north_up` | off | Lock north-up: map fixed, the car arrow turns (`MapView::arrow_angle`). Off = heading-up, arrow fixed apex-up. |
| `map_north_up_when_stopped` | off | Heading-up only: ease to north once stopped. |
| `map_smooth_rotation` | on | Ease rotation; off snaps (ease-to-north still eases). |
| `map_use_movement_dir` | on | Heading-up only: rotate to the velocity direction. |
| `map_look_stick` | on | Rotate the map by the right stick: look relative to the car in north-up and heading-up alike (look-around, see [[minimap]]; the reference heading honours `map_use_movement_dir`). |
| `map_mirror_edges` | on | **Mirror map at edges**: past the image edge the map continues mirrored; off = the plate shows outside the image. |
| `map_plate_opacity` | 0 | The minimap's own plate behind the dimmed image (D62: none, the game shows through); the far edge of a tilted map fades into it. Independent of the global `plate_opacity` (the other modules). Overlay tab → Minimap ("Background opacity") and Map tab → Minimap → Image. |
| `map_shape` | `RoundedRect` | **Shape** (D75): rounded rectangle or circle (diameter = `map_width`). Overlay tab → Minimap. |
| `map_width`, `map_height` | 208, 136 | Frame size in 1080p design px (sliders 120-600), scaled like the other modules; the layout footprint. |
| `map_corner_radius` | 22 | Rounded rect only; clamped to half the shorter side. |
| `map_border_width`, `map_border_color`, `map_border_opacity` | 3 px, `[12,17,27]`, 0.88 | The outline, drawn inside the frame; width 0 = none. |
| `map_plate_color` | `[9,13,21]` | The plate's colour (its opacity is `map_plate_opacity`). |
| `map_layers` | `MapLayerConfig::hud()` | What the map draws besides the markers: `image` (on, 50 / 50 / 50 %), `roads` (on, by type, per-type colour / width / dash / casing), `pois` (on, 32 px, categories, `max_zoom_m` 3000, `near_only` + `radius_m`, `gates`), `race_lines` (current race only, 4 px, marks), `tilt` (on, `angle_deg` 55, `perspective_px` 200 at the pill's 136 px, `car_y` 0.85, `taper` on). Every field `serde(default)`; edited on the Map tab's Minimap page (D63, moved D67). Follows the Dashboard's `minimap_layers` under `map_use_dashboard`. |
| `compass`, `zoom_driving_m`, `zoom_stopped_m` | on / 300 / 3000 | Compass and the two zoom radii. |
| `coop_use_dashboard` | on | **Use Dashboard co-op settings**: the co-op fields below follow the Dashboard and their controls are hidden. |
| `coop_teammates` | on | Draw teammates (and their trails). |
| `coop_trails` | on | Trails behind each player, own included (the own one solo too, white). |
| `coop_trail_fade_secs`, `coop_trail_fade_m` | 10 s / 500 m | Trail fade (Dashboard: Co-Op tab "Tracer fade"). |
| `coop_waypoints` | on | Shared waypoints. |

**The two "use Dashboard settings" checkboxes.** One tick mirrors the Dashboard, separately
for the map view and for co-op. *Why:* the user wanted a single box that reuses all the
Dashboard map settings instead of setting everything twice, individually for the map part and
for the co-op part (so e.g. the map can mirror the Dashboard while the HUD's trails stay its
own). `map_use_dashboard` defaults off (the HUD's own map values) and `coop_use_dashboard` on (co-op follows the Dashboard). **One resolver:**
`OverlayConfig::effective(&AppConfig)` returns the config with the ticked group(s) replaced by
the Dashboard's values (`minimap_*`, `coop_trail_fade_*`; the Dashboard has no
teammate/waypoint/trail switches, so those become `true`). The listener builds the snapshot's
`cfg` from it (`listeners/worker.rs`), so the renderer reads one config and never branches on
the flags. The HUD's own fields are kept untouched underneath, so unticking restores them.

Defaults are the user's own tuned setup (see **Tuned defaults** below), not the HUD's behaviour
before these were settable. `map_mirror_edges` is on (the Dashboard's default; the old HUD
smeared the edge pixel instead, a side effect of the clamped texture). `map_use_movement_dir`
and `map_look_stick` are on. The target yaw is `map_target_yaw` in `hud/minimap.rs`.

**Mirror at edges.** The overlay texture now wraps with `MirroredRepeat` (always; was
`ClampToEdge`), so the fan mesh's UVs past 0..1 show the reflected map. With mirroring off the
pill polygon is clipped to the (rotated) image quad (`clip_convex`, Sutherland-Hodgman) and the
plate shows outside. *Why a clip instead of two texture modes:* switching the wrap mode would
mean re-uploading the 64 MiB image (the CPU copy is dropped after upload); clipping is a few
vertices per frame.

**Not ported:** *Render FPS limit* (the overlay draws on its own wake/frame pacing), *Image
quality / Reload / Rebuild* (the HUD uses the fixed q50 cache file; the two maps share the q50
file only when the Dashboard is at 50 %), *Advanced calibration* (the HUD already reads the
Dashboard's calibration; one source of truth), the F10 north-up hotkey, and the **on-map
player list** and its columns (a 208×136 pill has no room for a table; the names are on the
arrows already).

- The map image is `maprender::draw_base`: a triangle fan over the pill outline with per-vertex UVs
  from `Camera::unproject` + the calibration (affine when flat, so UV interpolation is exact; a
  24 x 24 subdivided plane when tilted), 0.5 px under the frame because meshes aren't feathered.
- The image is the overlay's own **4096² (q50) copy** with trilinear mipmaps, loaded by the
  `hud-map` helper thread (`MapLoader`) and uploaded on the overlay thread; the CPU copy is
  dropped right after upload. See [[minimap]] for the shared loader. Until it arrives the
  map draws over a plain backing. The season is re-checked every 60 s; a failed load (no FH6
  install to read the tiles from) resets the loader so that check retries it.
- *Why its own copy with mipmaps:* the 8192² map minified onto a small rotating pill
  shimmers without mips (the Dashboard map has none and aliases). ~85 MiB of VRAM.
- *Why no RAM cache:* the image lives on disk (`map_cache/<season>_q50.bin`) and in VRAM
  only, so no 64 MiB CPU copy lingers for the app's lifetime.

### Race block R1′ (`hud/race.rs`)

196×46: a position cap (no "P") and the current lap time with "LAP n" (`lap_number + 1`:
FH6 sends it 0-based, D21).

- **Hidden in free roam** (`race_position == 0`); the slot then counts as empty so stacked
  modules close the gap. *Why:* there's no race to show (the stock HUD shows nothing there
  either). Known risk: Rivals/time-attack might also report 0 — unverified.
- **Lap hold:** for 4 s after a lap completes it shows "LAST LAP" with `last_lap`, green if
  that lap set the best.
- **Place-change colour** (D15): green layer on the cap for a place gained, red for a place
  lost; in over 0.15 s, hold to 2.4 s, out by 3.0 s.
- **Lap delta chip** (optional): a small right-aligned pill, green when ahead of your best
  lap, red when behind, hidden during the lap hold. Our own design (the spec sheet has
  none). The delta comes from `listeners/lap_trace.rs` (below).

### Drift counter X1′ (`hud/drift.rs`)

Same size as R1′ and takes R1′'s slot while drifting is detected (D25/D27). Two styles
(`DriftStyle`):

- **Position + Gain** (default) — R1′'s position cap (same place-change colours) on the
  left; on the right the last interval's "+N", counting up from 0 when the window closes and
  holding until the next. A scoring dot left of the "+N" is amber for 1 s after the score
  last rose (`DRIFT_ACTIVE_SECS`); the window's progress bar runs underneath. There's no
  "DRIFT" label in this style (the user asked for it to go).
- **Total** — the spec sheet's X1′: "DRIFT" and the live dot in the cap (the "+N" chip
  replaces them while it shows), the event's total score on the right (counting up).

*Why Position + Gain is the default:* FH6's own drift score UI can't be hidden, so a second
total on screen is redundant (the user's words). Total stays as an option.

Options: style, **Gain chip interval** (1–10 s, default 5), **Progress bar** for the running
window. The slot is empty in drift mode when the drift counter is off, or when score and
best are both 0 (free roam after a drift event: `current_lap` drops to 0 and the classifier
stays in Drift, since flat windows are no evidence).

## Notifications (D26, `hud/notify.rs`, `listeners/notify.rs`)

Short pill messages on the HUD when a setting changes. *Why:* the user wants in-game
feedback when a hotkey flips something (the game covers the app window, so there is nowhere
else to see it).

**Events** (each has its own toggle):

| Message | Toggle (`OverlayConfig`) | Source |
|---|---|---|
| `Gearbox: ON / OFF` | `notif_gearbox_toggle` | G hotkey or the Gearbox tab |
| `Gearbox mode: <mode>` | `notif_gearbox_mode` | mode picker, or the automatic switch to Race in a race and back (the *effective* mode, `dsg_effective_mode`); silent while the gearbox is off |
| `Backfire: ON / OFF` | `notif_backfire` | Backfire hotkey or tab |
| `Calibration started` | `notif_calibration` | the level rule below (driving an uncalibrated car, once per episode), and at once on Clear RPM calibration **and** Clear gear map (hotkey / controller action / tab button) |
| `Shift at redline` (yellow dot, `NotifKind::Hint`) | `notif_calibration` | once per calibration cycle: the box is not calibrated (`!engaged`), gear 1's gear-map entry has data (`DsgListener::gear_redline_speeds[1] > 0`) and, after a reset, a gear-1 sample newer than the reset (`DsgListener::gear1_seq`); tells the user data was collected and now is the time to rev out and shift. Re-arms on every new cycle (see below) |
| `Calibration done: N rpm` | `notif_calibration` | the gearbox engaging (`DsgListener::engaged` false to true: first manual upshift or a restored profile) |

**Calibration started rule** (`Notifier::watch`, level not edge). Announced when the game is
running and not paused (`driving`: `!hud_paused(pkt, experimental_pause_detection)`, which
includes race-on, and a non-zero car ordinal; `worker.rs:driving_car`) and the car is not
calibrated (`!engaged`), **once per episode** (`Notifier::started_for` = the car it was told
for). A new episode starts on: a car change (different ordinal), the box becoming calibrated
(`engaged`). A reset (Clear RPM calibration or Clear gear map) is not part of the level rule:
`Notifier::calibration_reset` pushes the message **immediately** whatever the driving / paused
state and marks the current car's episode as told (`started_for`), so resuming the same car
does not repeat it. *Why (the old delay):* the reset used to defer to the next drive while
paused, Clear gear map said nothing, and after Clear RPM calibration the kept gear map made
`Shift at redline` fire in the same `watch` pass and overwrite `Calibration started` in the
shared pill, so it was never seen. The worker also polls the socket for 1 ms instead of the
200 ms idle wait while a message is waiting to be published, so it reaches the HUD without a
packet. So: the first car after starting the app announces once you leave the pause
menu (or at once if the app starts mid-drive); pausing and resuming the same car does not
repeat it; a car restored from a saved profile (already `engaged`) says nothing; the app
restarting says it again. *Why a level:* the old rule only fired on a car change (not the
first car) and on explicit resets, so an uncalibrated car you simply started driving got no
message. *Why not tied to the gearbox switch:* calibration (`dynamic_max_rpm`, `engaged`) runs
with the box off too and feeds the HUD's shift cue, the user's wording has no gearbox clause,
and the car-change message was never gated on it; the `notif_calibration` toggle is the
off-switch. Like the shift hint it is not announced retroactively when a toggle is switched
on mid-episode.

**Calibration sequence:** `Calibration started` (reset / level rule) then, after driving, `Shift at redline` (level
check in `Notifier::watch`) then `Calibration done` (`engaged` false to true). *Why gear 1's
entry is the signal:* calibration needs the first pull to redline, which in a normal pull is
gear 1; any sample needs >60 % of the detected redline, so the first non-zero entry is the first
moment there is something to shift on. *Why a level, not an edge, plus a sample counter:* Clear RPM
calibration keeps the gear map, so gear 1 already has data right after it; a bare level would fire at
once and hide `Calibration started`. `DsgListener::gear1_seq` counts gear-1 samples ever taken (never
reset); every `CalibrationStarted` push records the current value (`Notifier::hint_floor`) and the hint
needs `gear1_seq` to advance past it, i.e. the user actually drove (gear 1 pulled past 60 % of the
redline) after the reset. `Notifier::shift_hinted` makes it fire once; it clears when the box engages, when
gear 1's entry empties (car change, Clear gear map) and on every `Event::CalibrationStarted` push. The
first pass (baseline) counts as already told. It shares the `notif_calibration` toggle and is silent
when the toggle is off (and is not announced retroactively when switched on mid-cycle). Clear gear map
while the box is still calibrated gives only `Calibration started`: no uncalibrated stretch, so no hint
or Done.

**Hidden HUD.** Pills only draw while the HUD is visible (paused, game unfocused, Hide HUD), so a
reset done in the pause menu would expire unseen after 2.5 s. `Notifier::hold_unseen` (called every
worker pass with the snapshot's visibility) therefore keeps restarting the life of a **Calibration**
pill pushed while hidden until the HUD is visible, at most 30 s (`HOLD_MAX`) after the push; from the
first visible pass the normal 2.5 s TTL runs. *Why only Calibration and not a general "start TTL when
first shown":* the other pills answer a hotkey pressed in the running game (HUD up); a reset in the pause
menu is the case this fixes, and a delayed "Gearbox: ON" would be stale news. *Why a 30 s cap:* so Hide HUD
for ten minutes does not pop up an old message.

**Groups and replacement.** Every notification has a `NotifGroup` (`overlay/snapshot.rs`):
`Calibration` (Started, Shift at redline, Done), `Gearbox` (ON/OFF), `GearboxMode`,
`Backfire`. Pushing one while a pill of the same group is still showing (younger than
`TTL_SECS`) **replaces that pill in place** (`Notifier::push`): same `id`, so the stack order
and every other pill stay put; new text and dot colour; `created` restarts the 2.5 s life.
So Started, Shift, Done in quick succession is one pill that changes text, and ON then OFF is
one pill saying OFF. Different groups still stack. *Why:* rapid same-type messages (a few
presses of the toggle key, the calibration cycle) piled up as redundant pills; only the
latest state matters. *Why Gearbox and Gearbox mode are two groups:* "ON/OFF" and "mode:
Race" are different facts, and a mode change should not wipe the ON/OFF the user just saw.
An older pill that has already expired (but is still in the 4 s queue) is not reused; the
new one gets its own slot. *Look of a replacement:* just a text swap, no cross-fade, kept
smooth by `Notification::born`: the fade-in start, which the replacement keeps (or backdates by the
pill's current opacity if it was mid fade-out), so the pill neither blinks out nor re-fades
from zero. `alpha(since_born, since_update)` = fade-in from `born`, fade-out from `created`.
The HUD orders the stack by `id` (slot), not by `created`, since a replacement keeps its slot.

Not included: Hide HUD (the HUD, notifications with it, is hidden by that very key), dashboard
edit mode and Mini-Settings (app-window only).

**Transport.** Everything is detected on the **listener thread**: it already receives the
hotkeys and gets the UI's config every frame, so `Notifier::watch` just diffs
`dsg_enabled` / `backfire_enabled` / effective mode / `engaged` once per loop pass. *Why a
diff and not a hook per source:* UI-side toggles, the G key, profile loads and the automatic
race switch all produce one identical message exactly once, and there is no UI to listener
queue. Calibration *start* after a reset can't be diffed (a reset of an uncalibrated box
changes nothing), so the reset sites (hotkey, controller action, tab command: Clear RPM calibration and
Clear gear map) push it explicitly via `Notifier::calibration_reset`; the uncalibrated-car case is the level rule above. The `Notifier` queue (max 8, pruned after 4 s)
is copied into `HudSnapshot::notifications` (`Vec<Notification { id, group, text, kind,
created, born }>`, times on `hud_clock`); a new entry forces a publish even without a packet. Text is
translated when created. The first pass only records a baseline, so starting the app says
nothing. Nothing is queued while the overlay is off.

**Look.** A 38 px plate pill (Drive cluster plate colour and font), a status dot (green on /
done, red off, blue info, yellow hint: `col::AMBER`, the HUD's amber) and the text. Alive for `TTL_SECS` = 2.5 s, 0.15 s fade-in, 0.45 s
fade-out (both restart per replacement, see above); the newest 5 are drawn; the overlay's frame timer runs while any is alive. They
follow the HUD's global fade, so a hidden or paused HUD shows none.

**Position** (Overlay tab, Notifications card): a 3×3 anchor picker, `notif_cell`, default
centre; the edge margin is the Layout card's. **Stacking** (`hud::notify::stack_layout`,
pure and unit-tested per anchor; always one vertical column, never side by side):

- top row (incl. top-centre): grows down from the margin, newest on top, older below;
- bottom row: grows up from the margin, newest at the bottom, older above;
- left-centre / right-centre: top to bottom, newest on top, the block centred on the
  vertical middle;
- dead centre: starts at the vertical middle, grows downward, newest on top.

The master switch and per-event toggles are in **Mini-Settings → Overlay → Notifications**
(self-explanatory, no tooltips); the master switch is also the card's Enabled box on the
Overlay tab.

## Race vs drift detection (`listeners/hud.rs`)

FH6 has no drift field. In drift events it puts the **live drift score in `current_lap`**
and the **event high score in `best_lap`**, the fields that hold times in a race (D27; see
[[forza-fh6-packet-format]]). The mode is detected from how `current_lap` moves:

- A lap timer rises at the packet clock's rate: Δ`current_lap` ≈ Δ`timestamp_ms` / 1000.
  A score is flat between drifts, then rises far faster or jumps.
- `ModeClassifier` judges **0.5 s windows** by rate. Within **1 ± 0.15** → race evidence;
  any other rise → drift evidence. **A flat window is no evidence either way**: a score sits
  still between drifts, but so does a frozen timer (countdown, results screen), and free
  roam sends 0. Flat windows neither extend nor break a streak.
- **Hysteresis:** 3 drift windows in a row (1.5 s) to enter Drift, 2 race windows (1 s) to
  go back. *Why asymmetric:* a false Drift swaps the race block away mid-race, while a
  running timer is unambiguous.
- A gap over 0.25 s or a drop over 0.05 restarts the window (hitch, pause, lap wrap, score
  reset); a paused packet keeps the mode.
- The "+N" gain: `DriftWindow` takes the score difference over each back-to-back interval;
  a score drop restarts the window instead of producing a negative chip.
- **Status:** confirmed working well by the user in-game (2026-09-28).

## Lap delta (`listeners/lap_trace.rs`)

Records `(distance into lap, lap time)` per packet (~58 KB per 2-minute lap, capped at
10 min) and promotes a finished lap to "best" if it was recorded from its start and was
faster. Delta = current time − best time at the same distance, interpolated. Rewinds keep
the delta but stop that lap becoming best; a restart, lap skip, car change or drift mode
drops the best. *Why:* the old best may be for another track, and **no delta beats a wrong
delta**.

## Visibility

The listener computes a **target** (`HudSnapshot::visible`, `listeners/hud.rs:visible_target`):

```
visible = enabled && !hud_hidden && (!focus_only || game_focused)
          && a packet within 2 s && not paused for ≥ 0.3 s
```

- **Pause:** hides once the HUD's pause fact has lasted 0.3 s (rides out one-packet blips,
  still clears the screen promptly on the pause menu). Shows immediately again. The fact
  (`listeners/hud.rs:hud_paused`):

  ```
  is_race_on == 0 || engine_max_rpm <= 0 || yaw == pitch == roll == 0.0
  ```

  Plus, when **Setup → Network → "Experimental pause detection"** is on (`experimental_pause_detection`,
  default on, D29), `garage_paused(pkt)` (also in `hud.rs`, pure, returns which sub-check
  matched for the Debug tab): `|yaw|, |pitch|, |roll| < 0.01` **and** `hand_brake == 255` **and**
  all linear velocity components `< 0.01`. Angular velocity is **not** checked. `hud_paused(pkt, experimental)` takes
  the flag as a parameter; the HUD tracker (`on_packet` / `snapshot`) and the gearbox's drift
  classifier (`dsg.rs`) pass `AppConfig::experimental_pause_detection`.
  - *Why:* user report: the HUD stayed visible in the garage, because garage packets keep
    `is_race_on = 1` and a normal max rpm. The captured garage packet is level (yaw -0.008),
    handbrake 255, every velocity/acceleration/wheel speed 0, rpm = idle.
  - *Why no angular-velocity check:* angular velocity jumps when you accelerate while standing
    still and when grabbing/rotating the car in the garage, so requiring it ~0 wrongly blocked
    the garage match.
  - *Why the linear-velocity guard and no extra debounce:* the guard stops a car held by the
    handbrake while level and rolling from matching; a one-packet match is already ridden
    out by the HUD's 0.3 s pause delay, so `garage_paused` is stateless.
  - *Limitation:* yaw is about 0 only when the garage view keeps the car's default
    orientation; a rotated garage view is **missed** (the car then looks like a normal
    parked one). A car parked on a slope with the handbrake on isn't matched (pitch > 0.01).
  The same fact drives `paused_since`, `HudSnapshot::paused`, the drift/race classifier's
  pause handling and the drift window stop. HUD only; the gearbox and backfire keep their
  own `is_race_on` rules.
  - *Why max rpm 0, not current rpm:* FH6 sends 0 max rpm while paused or in menus, while the
    current rpm is legitimately 0 for an EV at a standstill (user-confirmed).
  - *Why zero orientation:* loading screens send yaw/pitch/roll all exactly 0
    (user-observed), while the pause menu keeps the car's real rotation. A driven car is
    never exactly 0 on all three axes, so an exact compare has no false positives.
- **No packets for 2 s** → hidden (same 2 s as the rest of the app's "connected").
- **Focus-only** (optional, `overlay.focus_only`): the checkbox lives in the Overlay tab's
  **General** card ("Only when game window is focused"); its tooltip says it uses the Window
  Detection method set in Setup. *Why here (supersedes the old D5 choice to keep it in
  Setup):* the user looked for it on the Overlay tab and didn't find it in Setup. Turning the overlay on enables the focus detector (an
  idle detector fails open, i.e. reports "focused").
- **Hide HUD hotkey** (default **J**, global scope, rebindable in either the Overlay tab or
  Setup → Hotkey; it is the one binding `hotkeys.bindings[HideHud]`). It toggles
  `hud_hidden` on the **listener thread** and is **ignored while the overlay is disabled**;
  enabling the overlay resets it to shown. The Overlay tab shows a warning hint while hidden.
  - *Why listener runtime state, not config:* a config field would be reverted by the UI's
    per-frame config push (the UI isn't even drawn while the game covers it), and a persisted
    hide would look broken after a restart.
  - Note: the key may also be an in-game binding; rebind or clear it (Backspace) if it clashes.
- **Fade:** our own 0.16 s linear fade on show/hide (`hud::anim::FADE_SECS`, per the
  mockup CSS); **Fade on show / hide** off = hard cut. Nothing compositor-specific (no
  Hyprland layer animations or blur, D17). Hyprland's own quick map fade was judged
  acceptable; a `no_anim` layer rule on namespace `forza-telemetry-hud` removes it.
- **A hidden HUD has no mapped surface.** When the fade-out finishes, the layer surface is
  **destroyed**, not left mapped and transparent. *Why:* while an overlay-layer surface is
  mapped on an output, Hyprland disables direct scanout and tearing there, and the user
  observed that it effectively turns VRR off (the game still displays fine). The EGL context,
  painter, fonts and map texture stay alive (surfaceless), so re-showing uploads nothing.
- *Why visibility is decided on the listener thread:* the UI frame loop stops while the game
  covers the window, i.e. the whole time you drive; the listener keeps running.

## Monitor detection (D18)

The overlay is one full-output surface on one monitor, chosen by the **Monitor Detection**
card:

- **Hyprland** (built in, default): `hyprctl activeworkspace`, parsing `… on monitor NAME:`
  from the first line (`focus::parse_hyprland_monitor`).
- **Custom command**: prints a monitor name (first non-blank line).
- **Fixed monitor**: a typed output name; empty = the first output. Its **Detect** button
  fills in the monitor Hyprland reports as focused (the one the app window is on when you
  click). A text field, not a list, since there's no output-list API on the UI side.

**Chained to focus detection:** Hyprland/Custom are only queried while the Window Detection
method confirms FH6 is the focused window, since only then is the focused monitor the
game's. Monitor detection uses the **real** focus answer, not the fail-open one. When the
answer changes, the overlay recreates its surface on the new output (a layer surface can't
move). A failed query keeps the last known output. Runs on the focus poll thread
(`focus.rs:monitor_tick`), never the UI thread. *Why detect, not assume:* the user's words,
"It shouldn't just assume a monitor".

Status line under the card: green "Game on DP-1" / "HUD pinned to DP-1" (Fixed), **amber**
"Game window not focused, keeping DP-1" (third status-dot state), red on a failed query.

## Tuned defaults (the user's own setup)

Every `OverlayConfig` field defaults to the **user's live config**, not the original mockup
values, so a fresh install (or an old config lacking a key) starts as the user runs it.
*Why:* the user asked for their current settings to be the defaults wherever nothing is
configured yet, for the whole Overlay tab. Where it differs from the former default:

| Field | Default |
|---|---|
| `enabled` (master switch) | **on** (was off) |
| `focus_only` | on (was off) |
| `race_cell` (race / drift slot) | bottom-right (was top-left) |
| `margin_px` | 4 (was 44) |
| `rpm_label` | on (was off) |
| `zoom_driving_m` | 300 (was 500, 1500 before; D62: the HUD map's radius with the layers) |
| `map_plate_opacity` | 0 (new: no plate behind the dimmed map) |
| `map_use_movement_dir`, `map_look_stick` | on (were off) |
| `coop_use_dashboard` | on (was off) |
| `notif_gearbox_mode` | off (was on) |
| `notif_cell` | centre (was top-centre) |

All other fields (scale, plate opacity, fade, module toggles, Pill style, shift cues,
`zoom_stopped_m`, `map_north_up*`, drift counter, other notification toggles, ...) already
matched. **Not** taken from the user: the monitor settings (`monitor_method`, `monitor_cmd`,
`monitor_fixed` stay Hyprland / empty / empty: per-machine). The same pass changed the Hide HUD
default key to J and gave Clear gear map the default key F ([[hotkeys]]), and set the
controller default binding Toggle Gearbox = L3 ([[gamepad]]). The defaults live in
`OverlayConfig::default()` (`src/config.rs`), which is what serde's struct-level
`#[serde(default)]` fills missing keys from, so an older config gets them per field; a config
that already stores a value keeps it.

## Layout (D19, D22, D26, D28)

Three placeable modules — **Minimap**, **Drive cluster**, **Race / Drift** (one slot shared by
R1′ and X1′) — each in one cell of a **3×3 screen grid**; there's no free placement.
Cells are relative to the screen edges, so a layout survives a resolution change. Defaults:
map bottom-left, cluster bottom-centre, race/drift bottom-right.

Modules sharing a cell **stack vertically**, in the order Map → cluster → race/drift, from
the screen edge inward (`hud/layout.rs`):

- top row: top-down from the top margin (map highest);
- bottom row: bottom-up from the bottom margin (map lowest);
- middle row: the whole stack centred on the screen's vertical middle, map lowest, growing
  upward (the tab mockup's `column-reverse; justify-content:center`).

Horizontal alignment follows the column. **Edge margin** (`margin_px`, default 4, 0–200)
and **module spacing** (`gap_px`, default 12, 0–60) are user settings in 1080p design px,
scaled with the surface height and the HUD scale like the modules themselves (*Why:* a layout
then keeps its proportions across resolutions and scale). The gap default is the const
`hud::layout::GAP`; the margin default is `OverlayConfig::DEFAULT_MARGIN_PX` (4), which is
*not* the const `hud::layout::MARGIN` (44, the HUD's layout fallback): the defaults are the
user's tuned values (**Tuned defaults**), and the Layout card's Reset restores them.

## The Overlay tab (`src/ui/overlay_tab.rs`)

A settings page, registered after Dashboard (icon `OVERLAY`). **No live preview** and no
"show on monitor": the layout is a one-time setup (D24). It edits `config.overlay` (and the
shared Hide HUD binding);
`ForzaApp::sync_overlay` and the listener's per-frame config push apply every change to the
running HUD, so there's no Apply button.

**Module selector (D63).** At the top, a segmented control (`theme::segmented`, the Co-Op
tab's transport control, in a `WELL` frame) picks which module's settings the page shows.
*Why:* the user asked for "a multi-selector ... used as tabs so the user can configure the
individual modules on one page". (D63 also put all the map settings here; **D67 moved them to
the [Map tab](map-tab.md)**: "Instead of having the settings for the minimap and the dashboard
map in the overlay, we should ... create a new [tab] that is only used for configuring those
two maps." What stays here is the HUD itself, including the Minimap module's on/off, its
cell in the Layout card and, since D75, its frame.)
Tabs (`config::OverlayPage`):

| Tab | Cards |
|---|---|
| **General** | General, Monitor Detection, Layout (three columns from 1100 px, otherwise two with Layout first) |
| **Minimap** | Minimap frame (D75: shape, size, corner radius, outline, background, Reset frame, "Map layers…") |
| **Drive cluster** | Drive Cluster |
| **Race / Drift** | Race Block, Drift Counter |
| **Notifications** | Notifications |

- *Why one tab per module:* the grouping follows the HUD modules of the Layout grid (Minimap,
  Drive cluster, Race / Drift), plus Notifications. The Minimap page returned in D75 as the module's
  *frame* only (the map layers stay on the Map tab). An old saved `overlay_page` of `dashboard_map`
  (that page moved) loads as General (`serde(alias)` on `OverlayPage::General`); `minimap` now
  selects the new Minimap page.
- **Remembered across restarts** in the top-level config key `overlay_page` (an `OverlayPage`
  enum). It is in `config::EXPORT_EXCLUDE`: where the user last looked is not a setting, so it
  must not travel in presets or profile exports, and keeping it out of the `overlay` object
  keeps `overlay` purely the HUD's settings. `AppConfig::load_profile` keeps the current value
  across a profile switch for the same reason. Switching tab drops the layout grid's chip
  selection.
- **One row or two:** the selector breaks into two rows of two when the longest label
  (German "Benachrichtigungen") doesn't fit its segment, so labels never run into each other
  at the window minimum (`page_selector_with`, generic over the page enum; the Map tab's
  settings use it too).

### Map settings

Moved to the **Map tab** (D67): the Minimap and Dashboard map pages and the `layers_ui` cards
(Image, Roads, Points of interest, Race lines, View mode) are described in
[map-tab.md](map-tab.md). The HUD minimap's own options (`overlay.map_*`, `map_layers`,
`map_use_dashboard`, `map_plate_opacity`) are unchanged and still stored in the `overlay` key.

### Other cards

- **General:** Enable overlay, the status line (off / starting / running / disabled reason /
  stopped), the Hide HUD binding row, scale (50-200 %), plate opacity (scales plate, halo
  disc and cap), fade, and **Only when game window is focused** (tooltip points to Setup → Window Detection). There's no "hide when
  paused" option: that's automatic.
- **Layout:** one shared 3×3 grid; drag a module chip onto a cell, or select one and click a
  cell / use the arrow keys; **Reset layout**. The selection drops after a cell-click move,
  on a press outside the grid, on Esc and on a tab or module-tab switch (`clear_layout_selection`), so
  stray arrow keys can't move a chip while you're elsewhere; arrow-key moves keep it so you
  can keep stepping. Under the grid: **Edge margin** and **Module spacing** sliders (px at
  1080p); Reset layout resets them too. Chip stacking in the grid reuses
  `hud::layout::layout`, always with the default margin/gap (the grid shows cell and stacking
  order, not spacing; a 0 gap would break its scale trick). A module name wider than its chip
  (narrow window) is cut with "…". The drag-and-drop is hand-rolled because egui's `dnd_drop_zone`
  sizes to its content.
- **Notifications:** Enabled box + the anchor picker (see Notifications).
- **Drive Cluster / Race Block / Drift Counter:** a module on/off toggle plus the
  options listed under Widgets. No style thumbnails (D24).
- **Rebinding:** the Hide HUD button arms the shared `ForzaApp::capture_rebind` (Esc cancels,
  Backspace clears to "Not set"); see [[hotkeys]].

Overlay settings travel with profiles/presets as one `overlay` key (KeyGroup **Overlay → HUD
Overlay**, appended last so existing group indices don't shift). The Hide HUD binding lives in
`hotkeys`; the Dashboard map's settings are in the **Mini-settings** group (`minimap_layers`,
`minimap_*`).

Tests (`ui::test_render`, see the styling guide): every tab renders inside its panes at 700,
1000, 1100 and 1235 px and the selector at the window minimum in English and German. (The map
pages' tests moved with them to `ui/map_tab.rs`.)

## Architecture

### Threads

- **overlay** (`overlay/wayland.rs:run`) — its own Wayland connection, a **calloop** event
  loop, sctk layer shell, glutin EGL (`overlay/gl.rs`), its own `egui::Context` and an
  `egui_glow::Painter` (`overlay/render.rs`). One per enabled overlay. With the opt-in X11
  backend the thread body is `overlay/x11.rs:run` instead: same channels and contract, an
  xcb connection (x11rb) whose fd is a calloop source, same `Renderer` and `Gl`.
  - *Why in-process, not a separate binary:* `network.rs` binds the FH6 UDP port without
    `SO_REUSEPORT` and nothing forwards packets, so a second process couldn't receive them.
  - *Why its own thread:* the eframe frame loop stops while the game covers the window; the
    overlay must keep drawing exactly then.
- **overlay-start** — `OverlayHandle::spawn` blocks for up to 5 s (connect, roundtrip, EGL
  init), so `ForzaApp::start_overlay` runs it on this helper and `sync_overlay_thread` polls
  the result each frame. Failures aren't retried until the next off → on, so a missing
  layer-shell costs one probe, not one per frame.
- **overlay-drop** — dropping an `OverlayHandle` sends Shutdown and joins, which waits for the
  current frame; `drop_overlay_async` does it here so the UI never stalls.
- **hud-map** — `hud::minimap::MapLoader` loads the season map (from the install's level-2 tiles, ~60 ms cold, ~35 ms warm) off
  the render thread; the upload happens on the overlay thread.
- Monitor detection runs on the existing **focus** poll thread; the snapshot is built on the
  existing **listener** thread.

### Data flow

```
listener thread (worker.rs)                         overlay thread (wayland.rs)
  HudTracker::on_packet(pkt)  ─┐
  HudTracker::snapshot(...)    ├─ HudSink::publish ─► SnapshotSlot (Arc<Mutex<Option<HudSnapshot>>>)
                               └─ wake() ── calloop ping ──► follow_snapshot → maybe_render
coop.rs CoopReader ───────── remote_players() each HUD frame ──► M2′ teammates, trails
focus thread ── OverlayCmd::SetOutput(name) over a calloop channel ──► recreate surface
```

- `HudSnapshot` (`overlay/snapshot.rs`) carries the packet plus derived values: visibility
  target, redline/shift rpm, race/drift mode, lap delta, drift info, event timestamps (place
  change, lap completed, gear change, pause), `Arc<OverlayConfig>`, speed unit, the
  drive-mode letter and the map calibration. It's platform-neutral, since the listener
  compiles on Windows too.
- **Latest wins:** the listener `lock`s and overwrites; the overlay `try_lock`s and clones,
  keeping its previous copy on a miss (the same pattern as the listener ↔ UI mailboxes).
- The listener publishes on **every packet**, and on any change of the visibility target
  without one (pause timeout, packets stopping, focus, Hide HUD; checked each 200 ms idle
  poll). Attaching a sink forces one publish.
- All event times are seconds on `hud_clock()`, one process-wide monotonic clock shared by
  the listener (stamps) and the overlay (animation "now"), so tests and the PNG harness can
  pin time.
- **Co-op teammates, trails and waypoints** are read by the overlay itself through a
  `CoopReader` (`OverlayOptions::coop`; `in_session`, `remote_players`, `waypoints`), not carried
  in the snapshot, and fed into `CoopLayer` each frame. `CoopReader::remote_players`
  **advances the jitter buffers itself** before reading, because the UI's `coop.tick()`
  stops while the game covers the window (the advance is time-based, so two callers are
  harmless). See [[coop]].

### Frame pacing (D17)

- **One frame per packet** (~60 Hz), woken by the listener's ping; no redraw-rate setting.
  Each frame registers a frame callback before the swap, and a new frame waits for it
  (swap interval 0).
- A timer runs **only while something animates** and packets have stopped (no ping for
  40 ms): then frames step at 16 ms until the animation settles (e.g. the hide fade on
  pause). Frames are never scheduled at the monitor's rate. *Why:* DP-1 is 280 Hz VRR, and
  extra commits there cause judder.
- Animations are time-based (`hud/anim.rs`), not egui's `animate_*`.
- **Fixed-width digit cells:** numbers are laid out in cells as wide as the widest digit
  (egui has no kerning and Big Shoulders digits aren't tabular). *Why:* no jitter as values
  change. Consequence: numbers with a "1" are wider than in the mockup (accepted).
- **Fonts:** static Big Shoulders Display 800/900 TTFs in `assets/fonts/` (OFL), baked
  offline from the variable font. *Why:* epaint can't select a weight and the variable
  default is Thin. Font sizes are never animated (that would grow the glyph atlas).
- Circle strokes are pulled in half their width because epaint strokes outside the radius
  while the mockup's SVG strokes are centred.

### Lifecycle and drop order

- `ForzaApp::sync_overlay` (each frame) pushes focus/monitor params, then
  `sync_overlay_thread` starts the thread on enable, stops it on disable, collects the spawn
  result, and notices a dead thread (`OverlayStatus::Stopped`). `attach_overlay` hooks the
  listener's `HudSink` and the focus thread's output sink; `detach_overlay` unhooks both
  **before** dropping, so nothing targets a dying overlay.
- The overlay thread starts hidden. On a visible snapshot it creates the layer surface; the
  first configure creates the EGL window surface. On hidden, it keeps drawing the fade-out,
  then destroys the surface.
- **Drop order** matters and is structural: EGL window surface (and its `wl_egl_window`)
  before the `wl_surface`; the painter (`Renderer`) before `Gl` (it frees GL objects and
  needs the context current, surfaceless is fine); `Gl` terminates EGL before the Wayland
  connection goes. See the comments on `Live`, `Overlay` and `Gl`.
- **The POI icon texture** (`Renderer::icons`, an `IconTex`) is an `egui` texture in the overlay's
  own context (no GL object of its own: egui_glow frees it with the painter), uploaded the first
  frame a layer is on and the store has icons, replaced when the store hands out new pixels (an
  install change) and dropped when the layers go off. It lives in `Renderer`, so the drop order above
  covers it.
- **The 3D map renderer** (`Renderer::gl3d`, a `Gl3dHandle`; programs, height texture, road buffers,
  scene FBO) is destroyed in `Renderer::drop` **before** `painter.destroy()`, context current, and
  earlier whenever the Minimap leaves 3D. See [Minimap in 3D](#minimap-in-3d).
- **EGL failures:** a failed swap/surface creation rebuilds the surface; 3 in a row (lost
  context, GPU reset) stop the thread, which the tab reports as stopped.
- **Disabled reasons** (`overlay::DisabledReason`, translated): no `WAYLAND_DISPLAY`,
  couldn't connect, no layer-shell, EGL failed; for the X11 backend no `DISPLAY` and
  connect/extension failure (SHAPE, RANDR 1.5). The probe orders are pure, unit-tested
  functions (`overlay::capability`, `overlay::capability_x11`); `WAYLAND_DISPLAY` is checked
  before connecting because `connect_to_env` would otherwise fall back to `wayland-0` from an
  X11 session.
- **X11 teardown** follows the same rule: EGL surface → X window (`Live` field order,
  `destroy_window`), painter → `Gl` (`eglTerminate`), and `Gl::_native` holds an `Arc` of the
  xcb connection so it closes last.

### Dev and test hooks

- `FORZA_OVERLAY_TEST=1` shows a static test pattern (pill, outlined text, ring) on
  `FORZA_OVERLAY_OUTPUT` (default: first output); `=2` also wakes it at ~60 Hz with a frame
  counter, to check game frametimes under a redrawing overlay. While set, the app doesn't
  start the real HUD (the dev hook wins, so two overlays never fight).
- **X11 smoke test:** `cargo test x11_backend_smoke -- --ignored --nocapture` opens the X11
  backend on `$DISPLAY` for ~1 s with the test pattern and checks, from a second connection,
  that the window is override-redirect, viewable, depth 32 with an empty input shape, and
  gone after Hide. Ignored because it opens a window.
- **Offscreen PNG harness:** `cargo test render_spec_states -- --ignored --nocapture` renders
  every widget in its spec states (cruise / redline / shift / pulse, place gained / lost, lap
  hold, drift chip, both drift styles), the minimap with layers (`m2_layers_*`: flat, tilted, with and
  without icons, in a race, north-up, wide, co-op; synthetic roads, race ring, POIs and icons over a
  dark backdrop; checks that the rounded corners stay clear and the layers drew), the frame options
  (`m2_frame_*`, [Minimap frame](#minimap-frame-d75)) and, when an FH6
  install is found, the same with its real data and icons (`m2_real_*`, a visual check only), plus
  1080p composites through the real `Renderer` on a
  headless EGL device into `target/hud_png/`, at pinned time. It lives in `src/hud/png.rs` but
  is compiled as `overlay::render::png` via `#[path]`. *Why:* a binary crate can't expose its
  modules to `examples/`, and the harness needs the private `Renderer` and `gl::Headless`.
- **3D PNG states:** `cargo test render_3d_states -- --ignored --nocapture` (see [Minimap in 3D](#minimap-in-3d)),
  its own headless context with the real gl3d path; `m2_3d_*` (incl. `m2_3d_frame_circle` and
  `m2_3d_frame_rect_300x200`), `composite_3d_1080p`, `m2_real_*_3d_*`.
- Unit tests cover the classifier, drift window, lap trace, visibility target, layout
  stacking, fade/place/count-up curves, gear labels, the frame-pacing helper and the
  capability probe.

## Deliberately left out

Per-widget colour themes, custom fonts, per-widget opacity, a Race/Drift mode selector
(detection is automatic, D25), a VRR measurement spike (D17). *Why:* nobody asked, each is
easy to add later.
