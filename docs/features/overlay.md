# In-game HUD overlay

A compact, click-through HUD drawn **over the running game** on Linux/Wayland: RPM, gear and
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
- **Doesn't:** Windows/macOS. When no backend works the overlay reports a *disabled* reason
  instead of failing, shown in the Overlay tab's status line. On non-Linux builds the tab says
  the overlay is Linux only (Wayland or X11).
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

Options (Drive Cluster card): style; **Show engine RPM instead of KM/H label** (D13: the
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

208×136 pill-framed, heading-up season map with a compass (optional, D12) and the **same
markers as the Dashboard map**: the own arrow, co-op teammates, trails and shared waypoints,
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
  Zoom eases between **Zoom when driving** / **Zoom when stopped** (defaults 1500 / 3000,
  independent of the Dashboard map's unless reused); stopped = under 5 km/h for 1.5 s.

**Options (Mini-Settings → Overlay tab).** The HUD minimap has the Dashboard map's options.
*Why:* the user wants everything the Dashboard map has on the HUD too. They live in
`OverlayConfig` (`overlay.*`, not the top-level `minimap_*` keys), in the cog-wheel
**Mini-Settings**, tab **Overlay** (`src/app.rs`, "Minimap" and "Co-Op" sections; the Overlay
tab's Minimap card edits compass, zooms and teammates too, greyed while reused).

| Field | Default | Meaning |
|---|---|---|
| `map_use_dashboard` | off | **Use Dashboard map settings**: all the map fields below follow the Dashboard map and their controls are hidden. |
| `map_north_up` | off | Lock north-up: map fixed, the car arrow turns (`MapView::arrow_angle`). Off = heading-up, arrow fixed apex-up. |
| `map_north_up_when_stopped` | off | Heading-up only: ease to north once stopped. |
| `map_smooth_rotation` | on | Ease rotation; off snaps (ease-to-north still eases). |
| `map_use_movement_dir` | off | Heading-up only: rotate to the velocity direction. |
| `map_look_stick` | off | Rotate the map by the right stick: look relative to the car in north-up and heading-up alike (look-around, see [[minimap]]; the reference heading honours `map_use_movement_dir`). |
| `map_mirror_edges` | on | **Mirror map at edges**: past the image edge the map continues mirrored; off = the plate shows outside the image. |
| `compass`, `zoom_driving_m`, `zoom_stopped_m` | on / 1500 / 3000 | Compass and the two zoom radii. |
| `coop_use_dashboard` | off | **Use Dashboard co-op settings**: the co-op fields below follow the Dashboard and their controls are hidden. |
| `coop_teammates` | on | Draw teammates (and their trails). |
| `coop_trails` | on | Trails behind each player, own included (the own one solo too, white). |
| `coop_trail_fade_secs`, `coop_trail_fade_m` | 10 s / 500 m | Trail fade (Dashboard: Co-Op tab "Tracer fade"). |
| `coop_waypoints` | on | Shared waypoints. |

**The two "use Dashboard settings" checkboxes.** One tick mirrors the Dashboard, separately
for the map view and for co-op. *Why:* the user wanted a single box that reuses all the
Dashboard map settings instead of setting everything twice, individually for the map part and
for the co-op part (so e.g. the map can mirror the Dashboard while the HUD's trails stay its
own). Both default off, which keeps the HUD's own values. **One resolver:**
`OverlayConfig::effective(&AppConfig)` returns the config with the ticked group(s) replaced by
the Dashboard's values (`minimap_*`, `coop_trail_fade_*`; the Dashboard has no
teammate/waypoint/trail switches, so those become `true`). The listener builds the snapshot's
`cfg` from it (`listeners/worker.rs`), so the renderer reads one config and never branches on
the flags. The HUD's own fields are kept untouched underneath, so unticking restores them.

Defaults equal the HUD's behaviour before these were settable except `map_mirror_edges` (on =
the Dashboard's default; the old HUD smeared the edge pixel instead, a side effect of the
clamped texture) and the marker look (above). `map_use_movement_dir` defaults off, unlike
the Dashboard's `minimap_use_movement_dir`. The target yaw is `map_target_yaw` in
`hud/minimap.rs`.

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

- The map is drawn as a triangle-fan mesh with per-vertex UVs from
  `MapView::uv_at_offset` (affine, so UV interpolation is exact), 0.5 px under the frame
  because meshes aren't feathered.
- The image is the overlay's own **4096² (q50) copy** with trilinear mipmaps, loaded by the
  `hud-map` helper thread (`MapLoader`) and uploaded on the overlay thread; the CPU copy is
  dropped right after upload. See [[minimap]] for the shared loader. Until it arrives the
  map draws over a plain backing. The season is re-checked every 60 s.
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
| `Calibration started` | `notif_calibration` | Clear RPM calibration hotkey / controller action, the tab's Clear RPM calibration button, a new car that starts uncalibrated |
| `Calibration done: N rpm` | `notif_calibration` | the gearbox engaging (`DsgListener::engaged` false to true: first manual upshift or a restored profile) |

Not included: Hide HUD (the HUD, notifications with it, is hidden by that very key), dashboard
edit mode and Mini-Settings (app-window only).

**Transport.** Everything is detected on the **listener thread**: it already receives the
hotkeys and gets the UI's config every frame, so `Notifier::watch` just diffs
`dsg_enabled` / `backfire_enabled` / effective mode / `engaged` once per loop pass. *Why a
diff and not a hook per source:* UI-side toggles, the G key, profile loads and the automatic
race switch all produce one identical message exactly once, and there is no UI to listener
queue. Calibration *start* can't be diffed (a reset of an uncalibrated box changes nothing),
so the reset sites (hotkey, controller action, tab command) push it explicitly. The `Notifier` queue (max 8, pruned after 4 s)
is copied into `HudSnapshot::notifications` (`Vec<Notification { id, text, kind, created }>`,
`created` on `hud_clock`); a new entry forces a publish even without a packet. Text is
translated when created. The first pass only records a baseline, so starting the app says
nothing. Nothing is queued while the overlay is off.

**Look.** A 38 px plate pill (Drive cluster plate colour and font), a status dot (green on /
done, red off, blue info) and the text. Alive for `TTL_SECS` = 2.5 s, 0.15 s fade-in, 0.45 s
fade-out; the newest 5 are drawn; the overlay's frame timer runs while any is alive. They
follow the HUD's global fade, so a hidden or paused HUD shows none.

**Position** (Overlay tab, Notifications card): a 3×3 anchor picker, `notif_cell`, default
top-centre; the edge margin is the Layout card's. **Stacking** (`hud::notify::stack_layout`,
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
- **Hide HUD hotkey** (default **H**, global scope, rebindable in either the Overlay tab or
  Setup → Hotkey; it is the one binding `hotkeys.bindings[HideHud]`). It toggles
  `hud_hidden` on the **listener thread** and is **ignored while the overlay is disabled**;
  enabling the overlay resets it to shown. The Overlay tab shows a warning hint while hidden.
  - *Why listener runtime state, not config:* a config field would be reverted by the UI's
    per-frame config push (the UI isn't even drawn while the game covers it), and a persisted
    hide would look broken after a restart.
  - Note: H may also be an in-game binding; rebind or clear it (Backspace) if it clashes.
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

## Layout (D19, D22, D26, D28)

Three placeable modules — **Minimap**, **Drive cluster**, **Race / Drift** (one slot shared by
R1′ and X1′) — each in one cell of a **3×3 screen grid**; there's no free placement.
Cells are relative to the screen edges, so a layout survives a resolution change. Defaults:
map bottom-left, cluster bottom-centre, race/drift top-left.

Modules sharing a cell **stack vertically**, in the order Map → cluster → race/drift, from
the screen edge inward (`hud/layout.rs`):

- top row: top-down from the top margin (map highest);
- bottom row: bottom-up from the bottom margin (map lowest);
- middle row: the whole stack centred on the screen's vertical middle, map lowest, growing
  upward (the tab mockup's `column-reverse; justify-content:center`).

Horizontal alignment follows the column. **Edge margin** (`margin_px`, default 44, 0–200)
and **module spacing** (`gap_px`, default 12, 0–60) are user settings in 1080p design px,
scaled with the surface height and the HUD scale like the modules themselves (*Why:* a layout
then keeps its proportions across resolutions and scale). The defaults are the former consts
`hud::layout::MARGIN` / `GAP`, so old configs look the same. The 44 px margin shifts the
defaults a few px from the mockup's hand-placed positions (accepted).

## The Overlay tab (`src/ui/overlay_tab.rs`)

A plain settings page, registered after Dashboard (icon `OVERLAY`). **No live preview** and no
"show on monitor": the layout is a one-time setup (D24). It only edits `config.overlay` (and
the shared Hide HUD binding); `ForzaApp::sync_overlay` and the listener's per-frame config
push apply every change to the running HUD, so there's no Apply button.

- **Columns:** three at a page width ≥ 1100 px (General + Monitor Detection | Layout + Race
  Block + Drift Counter | Drive Cluster + Minimap), otherwise two with **Layout first**.
  *Why 1100:* each of three columns is then ≥ 355 px, the narrowest the two-half control rows
  still read at. *Why Layout first:* it's the one card you can't find by scrolling past
  settings.
- **General:** Enable overlay, the status line (off / starting / running / disabled reason /
  stopped), the Hide HUD binding row, scale (50–200 %), plate opacity (scales plate, halo
  disc and cap), fade, and **Only when game window is focused** (tooltip points to Setup → Window Detection). There's no "hide when
  paused" option: that's automatic.
- **Layout:** one shared 3×3 grid; drag a module chip onto a cell, or select one and click a
  cell / use the arrow keys; **Reset layout**. The selection drops after a cell-click move,
  on a press outside the grid, on Esc and on a tab switch (`clear_layout_selection`), so
  stray arrow keys can't move a chip while you're elsewhere; arrow-key moves keep it so you
  can keep stepping. Under the grid: **Edge margin** and **Module spacing** sliders (px at
  1080p); Reset layout resets them too. Chip stacking in the grid reuses
  `hud::layout::layout`, always with the default margin/gap (the grid shows cell and stacking
  order, not spacing; a 0 gap would break its scale trick). The drag-and-drop is hand-rolled because egui's `dnd_drop_zone`
  sizes to its content.
- **Notifications:** Enabled box + the anchor picker (see Notifications).
- **Drive Cluster / Minimap / Race Block / Drift Counter:** a module on/off toggle plus the
  options listed under Widgets. No style thumbnails (D24).
- **Rebinding:** the Hide HUD button arms the shared `ForzaApp::capture_rebind` (Esc cancels,
  Backspace clears to "Not set"); see [[hotkeys]].

Overlay settings travel with profiles/presets as one `overlay` key (KeyGroup **Overlay → HUD
Overlay**, appended last so existing group indices don't shift). The Hide HUD binding lives in
`hotkeys`.

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
- **hud-map** — `hud::minimap::MapLoader` loads the season map (2.3 s cold, ~35 ms warm) off
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
  hold, drift chip, both drift styles) plus 1080p composites through the real `Renderer` on a
  headless EGL device into `target/hud_png/`, at pinned time. It lives in `src/hud/png.rs` but
  is compiled as `overlay::render::png` via `#[path]`. *Why:* a binary crate can't expose its
  modules to `examples/`, and the harness needs the private `Renderer` and `gl::Headless`.
- Unit tests cover the classifier, drift window, lap trace, visibility target, layout
  stacking, fade/place/count-up curves, gear labels, the frame-pacing helper and the
  capability probe.

## Deliberately left out

Per-widget colour themes, custom fonts, per-widget opacity, a Race/Drift mode selector
(detection is automatic, D25), a VRR measurement spike (D17). *Why:* nobody asked, each is
easy to add later.
