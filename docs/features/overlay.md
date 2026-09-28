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
- **Doesn't:** GNOME (Mutter has no layer-shell), X11 sessions, Windows. The overlay reports
  a *disabled* reason instead of failing, and the Overlay tab's status line shows it (e.g.
  "Your compositor doesn't support wlr-layer-shell (e.g. GNOME)…"). On non-Linux builds the
  tab says the overlay is Linux (Wayland) only.
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

- **D1a Pill** — 184×46 pill with a 16-segment level rev bar.
- **D3a′ Halo** — 112×112 disc with a thick 24-sector ring on a 260° arc (idle → max). Each
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
  Without it (new car, after Reset RPM Calibration / Clear RPM calibration): redline =
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

208×136 pill-framed, heading-up season map with the car arrow at the centre, a compass
(optional, D12) and **co-op teammates** (optional): a 12×14 arrow in the teammate's identity
colour plus their name, only while inside the pill. Paused teammates are skipped (their
packet sits at the world origin). There is no scale bar. Zoom eases between **Zoom when
driving** / **Zoom when stopped** (100–5000 m, defaults 1500 / 3000, independent of the
Dashboard map's); stopped = under 5 km/h for 1.5 s, the same rule as the Dashboard. It stays
heading-up when stopped.

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
  is_race_on == 0 || (!electric && current_engine_rpm <= 0) || yaw == pitch == roll == 0.0
  electric = num_cylinders == 0
  ```

  The same fact drives `paused_since`, `HudSnapshot::paused`, the drift/race classifier's
  pause handling and the drift window stop. HUD only; the gearbox and backfire keep their
  own `is_race_on` rules.
  - *Why 0 rpm:* FH6 reads 0 rpm in menus / pause (user-confirmed).
  - *Why the EV exemption:* an electric car reads 0 rpm at a standstill, so the rpm rule
    would hide the HUD whenever an EV stops. `num_cylinders == 0` is the app's existing EV
    test (the Dashboard Engine widget's "Electric" caption).
  - *Why zero orientation:* loading screens send yaw/pitch/roll all exactly 0
    (user-observed), while the pause menu keeps the car's real rotation. A driven car is
    never exactly 0 on all three axes, so an exact compare has no false positives; it also
    covers an EV on a loading screen, which the rpm rule no longer does.
- **No packets for 2 s** → hidden (same 2 s as the rest of the app's "connected").
- **Focus-only** (optional, `overlay.focus_only`): the checkbox lives in **Setup → Window
  Detection** ("Only when game window is focused"), not on the Overlay tab (D5), because it
  uses that card's detection method. Turning the overlay on enables the focus detector (an
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
  disc and cap), fade, and a pointer to Setup → Window Detection. There's no "hide when
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
  `egui_glow::Painter` (`overlay/render.rs`). One per enabled overlay.
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
coop.rs CoopReader ───────── remote_players() each HUD frame ──► M2′ teammates
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
- **Co-op teammates** are read by the overlay itself through a `CoopReader`
  (`OverlayOptions::coop`), not carried in the snapshot. `CoopReader::remote_players`
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
  couldn't connect, no layer-shell, EGL failed. The probe order is a pure, unit-tested
  function (`overlay::capability`); `WAYLAND_DISPLAY` is checked before connecting because
  `connect_to_env` would otherwise fall back to `wayland-0` from an X11 session.

### Dev and test hooks

- `FORZA_OVERLAY_TEST=1` shows a static test pattern (pill, outlined text, ring) on
  `FORZA_OVERLAY_OUTPUT` (default: first output); `=2` also wakes it at ~60 Hz with a frame
  counter, to check game frametimes under a redrawing overlay. While set, the app doesn't
  start the real HUD (the dev hook wins, so two overlays never fight).
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
