# Architecture Overview

The first doc to read to understand and navigate ForzaTelemetryV3. It maps the
threads, the packet-to-pixel data flow, the per-frame loop, and every source file.
Deep dives live in sibling docs — this one links out rather than duplicating them:
[[networking]], [[state-and-config]], [[ui-architecture]], and per-feature docs
like [[coop]], [[minimap]], [[gearbox]], [[power-curve]], [[overlay]].

## Big picture

ForzaTelemetryV3 is a single-window **egui/eframe** desktop app (immediate mode:
the whole UI is rebuilt from state every frame). It listens for **one-way FH6 UDP
telemetry** — a fixed 324-byte packet at the game's frame rate — parses each packet,
folds it into state held on one big struct (`app.rs:ForzaApp`), lets event-driven
listeners react (backfire, auto-gearbox, timers, power capture), and redraws a tabbed
dashboard. `ForzaApp` *is* the application: it owns config, telemetry, listeners, and
all derived/session state, and it implements `eframe::App`. On Linux (Wayland, or X11/XWayland) an optional
**in-game HUD overlay** draws over the game from its own thread (see [[overlay]]).

## Threading model

Three long-lived threads (four with the HUD overlay on) plus a few short-lived helpers. Packets cross UDP → listener → UI
through two **`mpsc` channels**; the listener↔UI *state* exchange is the mailbox pair
described below.

- **UDP receive thread** — spawned in `network.rs:start_receiver`. Binds
  `0.0.0.0:<port>`, sets a 200 ms read timeout, and loops: `socket.recv` →
  `packet.rs:ForzaPacket::from_bytes` → `sender.send(pkt)`. It owns nothing the UI
  touches; it only pushes parsed packets down the channel. A `network.rs:NetworkHandle`
  holds an `Arc<AtomicBool>` stop flag; when the handle is dropped (`Drop`), the flag
  flips and the thread exits its loop. Changing the port starts a fresh thread on the
  **same** channel and drops the old handle (`app.rs:ForzaApp::restart_receiver`) — the
  listener thread keeps its receiver, and therefore its calibration and shift state.
- **Listener thread** — `listeners/worker.rs:spawn`. Owns the packet `Receiver`, the
  `BackfireListener`, the `DsgListener`, the per-car calibration map, the detected redline
  (`dynamic_max_rpm`), its own packets-per-second measurement, a clone of `InputSender`,
  and the global-hotkey event receiver. Blocks on `recv_timeout(200 ms)` — never holding a
  lock while it waits — runs Backfire then DSG on every packet, then forwards the packet to
  the UI. **Why:** GNOME/Wayland stops delivering frame callbacks for a minimized or fully
  occluded window and winit gates `RedrawRequested` on that callback
  (`winit wayland/event_loop/mod.rs:486`), so `eframe::App::update` simply stops being
  called. Anything key-output-driving that lives in the frame loop dies with it, so these
  two features (and the `G`/`B`/reset hotkeys that toggle them, and the synthetic-input
  focus gate) must not depend on redraws.
- **egui/eframe render thread (main)** — owns `ForzaApp`, including the receiver of
  *forwarded* packets. Everything else — stats, the three read-only listeners, Co-Op's
  incoming side (jitter buffers, roster, minimap), widgets — still happens here,
  single-threaded. Co-Op's *outgoing* relay runs on the listener thread (see [[coop]]).
- **Overlay thread** (Linux and, experimentally, Windows via `overlay/win32.rs:run`, only while `overlay.enabled`) — `overlay/wayland.rs:run`: its own
  Wayland connection, calloop loop, layer-shell surface, glutin EGL context, `egui::Context`
  and `egui_glow` painter. Draws the HUD from the listener's `HudSnapshot` mailbox, one frame
  per packet. Owned by `overlay::OverlayHandle`, held in `app.rs:OverlayRuntime`. By default
  (auto) it falls back to `overlay/x11.rs:run` when layer-shell is unavailable (an
  override-redirect X11 window via XWayland, for GNOME — tested on GNOME 50.4); `FORZA_OVERLAY_BACKEND=wayland|x11`
  forces one. **Why its
  own thread:** like the listener, it must keep working while the game covers the main
  window. Helpers: `overlay-start` (runs the blocking `OverlayHandle::spawn`, up to 5 s),
  `overlay-drop` (drop = shutdown + join, kept off the UI thread), `hud-map` (loads the
  season map for the HUD minimap, from the FH6 install's tiles). See [[overlay]].
- **Map editor server threads** (only after the map editor was opened, `mapedit/server.rs`, I26b; stopped by dropping `ForzaApp::map_editor`) — `mapedit-accept` (blocking `accept` on `127.0.0.1:<sticky port>`), one short-lived `mapedit-conn` thread per HTTP connection, and `mapedit-build` (builds `mapedit::data::EditorData` once, 0.5-3 s release, then exits). They never touch `ForzaApp`: events (`MapEvent::{Ready, Saved, Error}`) go over an `mpsc` channel polled once a frame by `app.rs:poll_map_editor` (which opens the browser on `Ready`), plus `ctx.request_repaint()`. **Why Save updates its state inside the server thread:** the egui frame loop stops while the window is hidden (see the listener thread above), so a Save made in the browser must not wait for a frame to take effect; the UI only learns of it later. Drop = stop flag + a self-connect to wake `accept` (like `NetworkHandle`, which polls with a read timeout instead). See `docs/game-data/fh6-map-tooling.md` ("Local server").
- **`map-terrain` / `map-mesh` threads** (`maprender/store.rs`, K1; spawned on demand by `maprender::store::terrain()` / `road_mesh()`, which the maps call only while in 3D mode; exit when done) — build the 3D height grid (cached elevation rasters, 0.2-0.4 s warm, +2 s per raster cold) and the GPU road mesh (~15 ms). Separate from `map-layers` so a cold terrain build never delays the 2D layers.
- **GL contexts and the 3D map scene** (`maprender/gl3d/`, K2) — no thread of its own: the 3D scene is drawn by an `egui_glow` paint callback **on whichever thread paints**, in that thread's own GL context. There are two contexts, so two independent `Gl3d`s (own shaders, 15 MB height texture, 25 MB road buffers, scene FBO): the overlay thread's EGL / WGL context (owned by `overlay::render::Renderer`, which will destroy its `Gl3dHandle` before `painter.destroy()`) and eframe's on the UI thread (a `Gl3dHandle` on `ForzaApp`, destroyed in `on_exit`). Nothing GL is shared between them; the CPU data (`Arc<Terrain>`, `Arc<RoadMesh>` from the `map-terrain` / `map-mesh` threads) is. The settings UI (UI thread) cannot see the HUD context's state, so a failure is also published process-wide (`gl3d::last_failure()`).
- **`map-layers` thread** (`maprender/store.rs`, spawned on demand by the first `maprender::layers()` call, exits when done) — loads the Dashboard / HUD map layer data (nav, POIs incl. danger signs, race lines, road chains, the game's POI icons; the icons and danger signs on two scoped threads) and rebuilds the roads when the road-type override file changes. Process-global store, polled by whichever map draws (the HUD from the overlay thread, `overlay/render.rs`); *why* it is not a field of the app or the HUD snapshot: the UI frame loop stops while the game covers the window, which is when the HUD is used. See [[minimap]] ("Shared renderer & layers").
- **Short-lived background threads** — the Dashboard's seasonal minimap image load
  (`app.rs:map_load_thread` → `minimap.rs:load_map_color_image` → `gamedata/tiles.rs`, which
  decodes the install's map tiles with one scoped thread per tile row; results returned over
  its own `mpsc` channel of `MapLoadMessage`), and Co-Op's WebSocket relay + cloudflared tunnel
  (`coop.rs`, see [[coop]]). Synthetic keypress emission (`input.rs:InputSender`) also
  runs on its own worker thread, as does the focus poll (`focus.rs`), which also runs the
  overlay's monitor detection and sends `OverlayCmd::SetOutput` to the overlay thread.

### Listener ↔ UI: two mailboxes

Each direction has its own `Arc<Mutex<…>>`; neither lock is ever held across a blocking
call, disk IO or drawing, and neither side ever holds both.

| Direction | Payload | Writer | Reader |
| --- | --- | --- | --- |
| listener → UI | `worker.rs:ListenerView` (`DsgView`, `BackfireView`, `dynamic_max_rpm`, the two enable flags + `toggle_gen`) | listener `lock`s once per loop and overwrites it from its local copy | UI `try_lock`s once a frame in `app.rs:sync_listener_view`, clones into `app.dsg` / `app.backfire` / `app.dynamic_max_rpm`, drops the guard |
| UI → listener | `worker.rs:ToListener` (`AppConfig` + `our_focused` / `wants_text`, which only egui knows) | UI `try_lock`s at the end of every frame (`ListenerHandle::push`) | listener `try_lock`s once per loop and takes it |
| listener → UI packets | `worker.rs:PacketQueue` = `Arc<Mutex<VecDeque<ForzaPacket>>>`, capped at `worker::UI_BACKLOG_CAP` (200) | listener locks, `push_back`, `pop_front` when over cap, unlocks | UI locks, `std::mem::take`s the deque, unlocks, *then* iterates (`ListenerHandle::take_packets`) |
| UI → listener (one-shot) | `worker.rs:Command` — `ClearRpmCalibration`, `ClearGearMap`, `SetHudSink`, `Shutdown` | UI, over an `mpsc` channel (never blocks, never dropped) | listener drains it once per loop |
| listener → overlay | `overlay/snapshot.rs:HudSnapshot` in a `SnapshotSlot` (`Arc<Mutex<Option<…>>>`, latest wins) plus a calloop wake ping, via `HudSink` | listener `lock`s and overwrites on every packet and on any visibility-target change | overlay thread `try_lock`s and clones on each wake, keeping its previous copy on a miss |

*Why a capped deque rather than a channel for the packets:* an `mpsc` channel is unbounded,
so it would grow by ~71 MB/hour while the window is hidden and nobody drains it; a
`sync_channel` would instead drop the **newest** packets, which are the only ones that
matter. The capped deque drops the oldest and bounds memory at 200 packets.

**Why this shape** (the user's own pattern): each side keeps a *local* copy and only opens
the shared one to write it or copy it out, so the listener loop is never affected by the UI
and the UI never blocks. A `try_lock` miss on the UI side just means it draws last frame's
values — acceptable because nothing in the UI is critical, while the loop that drives the
game must never stall.

**Focus facts expire.** `our_focused` / `wants_text` (from `ctx.input(|i| i.focused)` and
`ctx.wants_keyboard_input()`) only change when the UI is drawn, so the listener ignores them
after `worker::FOCUS_FACTS_TTL` (1 s) and treats the window as neither focused nor typing —
which is what a hidden window is. *Why it matters:* frozen at `(focused, not typing)` the
global-hotkey gate would be stuck **open**, so a bare `G`/`B`/`F` typed in any other app
would toggle the gearbox or wipe the calibration; frozen at `(focused, typing)` it would be
stuck **shut** for as long as the window stayed hidden. 1 s is ~5× the slowest legitimate
frame (`update` always re-arms a repaint, and the FPS-limit slider floors at 5 fps).

**A dead thread is visible.** `ListenerHandle::is_dead()` (`JoinHandle::is_finished()`)
tells the UI the listener panicked; the status-bar Backfire/Gearbox indicators then read
*Stopped (error)* in `theme::DANGER` instead of a frozen "Active". There is deliberately no
restart mechanism — the point is not to lie about the feature being alive.

**The one two-writer field** is the `dsg_enabled` / `backfire_enabled` pair, which both a
global hotkey (listener) and a checkbox (UI) can flip. The listener bumps `toggle_gen` on
every hotkey toggle; the UI adopts both flags whenever it sees a new generation and echoes
the generation back as `ack_gen`. A config push whose `ack_gen` is behind is treated as
stale for those two fields only — otherwise the first push after a hotkey press would undo
the key the user just hit while the window was hidden.

The receive thread is the only place raw bytes become a `ForzaPacket`; the main thread
never blocks on the socket (it uses non-blocking `try_recv`).

## Data flow

Packet → parsed → state → listeners → widgets, per real symbol:

```
game (UDP :port)
      │  324 bytes, little-endian, ~60 Hz
      ▼
network.rs:start_receiver          [UDP thread]
      │  ForzaPacket::from_bytes()  (packet.rs)
      ▼  mpsc::Sender<ForzaPacket>::send
──────────────────── channel ────────────────────
      ▼  mpsc::Receiver::recv_timeout  [listener thread]
listeners/worker.rs:run()          (runs whether or not we're being drawn)
      ├─ car-change reset + per-car calibration restore/flush (DSG)
      ├─ dynamic redline (highest RPM while making power)
      ├─ backfire.update(&pkt, …)   → synthetic W  (input.rs:InputSender)
      ├─ dsg.update(&pkt, …)        → synthetic E/Q
      ├─ coop.push_local(&coop::outgoing(..)) → relay to peers (coop.rs; paused class/PI
      │    carried over). Here because the UI loop stops while the game covers the window
      ├─ hud.on_packet → hud.snapshot → HudSink::publish + wake   (listeners/hud.rs; only
      │    while the overlay is attached) ──► overlay thread draws one HUD frame
      ├─ publishes ListenerView into the listener→UI mailbox
      ▼  push_back into the capped packet mailbox (oldest dropped)
─────────── Arc<Mutex<VecDeque<ForzaPacket>>> ───────────
      ▼  ListenerHandle::take_packets()  [main thread]
app.rs:ForzaApp::drain_packets()   (called first each frame)
      │  for each packet taken (at most 200):
      ├─ car-change reset (per-car session state, session maxima)
      ├─ session maxima + cached car identity (power/torque/boost/speed,
      │    wheel-radius estimate)  [only when is_race_on != 0]
      ├─ stats: gforce_stats.update / suspension_stats.update / speed-delta /
      │    trace_history (Speed Trace, active-time axis)
      ├─ UI-side listeners fire (see below)
      └─ telemetry.update(pkt)          → stores latest + packet-rate (telemetry.rs)
      ▼
egui::CentralPanel dispatch → crate::ui::<tab>::show(ui, self)
      │  widgets read app.telemetry.latest + derived state and paint
      ▼
frame drawn; repaint rescheduled (FPS limiter)
```

**Listeners** (wired via `listeners/mod.rs`). The two that drive the game run on the
listener thread; the three read-only ones still run inside `drain_packets`:
- `backfire.rs:BackfireListener::update` — decides when to inject a synthetic throttle
  blip, driving the game through `input.rs:InputSender`. *(listener thread)*
- `dsg.rs:DsgListener::update` — DSG-style auto-shifter; also drives `InputSender`.
  *(listener thread)*
- `sprint_timer.rs:SprintTimer::update` — 0→100…400→500 km/h splits.
- `power_capture.rs:PowerCapture::update` — captures RPM/power/torque/boost during
  full-throttle pulls (gated by `backfire_echo_active()` so fake blips don't seed it).
- `perf_test.rs:PerfTest::update` — configurable accel/decel timers.

**Backlog cap.** The packet mailbox keeps filling while the window is hidden, so the
listener drops the **oldest** once it holds `worker::UI_BACKLOG_CAP` (200). *Why:* replaying
minutes of stale telemetry through the sprint timer and the trace buffer on restore is worse than skipping it, and an uncapped queue would also grow without bound. The
listener thread itself never falls behind — it drains continuously and processes each packet
as it arrives, so it can never replay stale input.

`telemetry.rs:TelemetryState` holds `latest: Option<ForzaPacket>`, `is_connected`, and
`packets_per_sec` (recomputed each 1 s window). Connection is marked **down** at the tail
of `drain_packets` if no packet arrived for 2 s. `ForzaPacket::is_paused()` (all position
+ orientation fields zero) distinguishes an actively-driving packet from a paused-game one,
which several stats and the Co-Op relay respect.

## Frame loop

`app.rs:<ForzaApp as eframe::App>::update` runs once per repaint and does, in order:

1. `i18n::set_language(config.language)` — pick the active language for `tr(...)`.
2. `sync_listener_view()` — `try_lock` the listener→UI mailbox and copy it into
   `self.dsg` / `self.backfire` / `self.dynamic_max_rpm`; adopt the Backfire/Gearbox enable
   flags if a hotkey toggled them. On a lock miss, last frame's copy stands.
   Then `sync_overlay()` — push the focus/monitor-detection params and start/stop/collect the
   HUD overlay thread to follow `overlay.enabled` (see [[overlay]]).
3. `drain_packets()` — `take_packets()` from the listener's capped mailbox in one lock, then
   process them unlocked (see above).
4. `coop.tick()` — advance Co-Op jitter buffers; `update_minimap_trails()`.
5. Poll the minimap image channel; handle season change; throttle/smooth the minimap
   camera (position cache, eased yaw, zoom).
6. Hotkeys — only the app-focused actions (Ctrl+S mini-settings, Ctrl+E dashboard edit),
   matched from config against egui input; F11 fullscreen (Windows) stays hardcoded. First,
   `capture_rebind` takes the key while a rebind button is armed (Esc / Backspace / bind),
   so it never also fires a hotkey. The
   global actions (G gearbox, B backfire, F reset-RPM, H hide HUD) and the synthetic-input focus gate
   live on the listener thread instead — they have to work while this loop isn't running.
   Rebindable in Settings → Hotkeys. See [[hotkeys]].
7. Chrome panels — top **tab bar** (`TopBottomPanel::top`, three styles via
   `tab_button`/`page_pill`), bottom **status bar** (connection, pps, Co-Op,
   cog), and the floating **mini-settings window** (`page_settings_*`, driven by
   `PageSettingsTab` / `DashboardSubTab`).
8. **Central panel dispatch** — `match self.current_tab { … }` calls the one
   `crate::ui::<tab>::show(ui, self)` for the active `Tab`.
9. `listener.push(...)` — hand the listener thread this frame's `AppConfig` plus
   `ctx.input(|i| i.focused)` / `ctx.wants_keyboard_input()` (the global-hotkey gate needs
   both and neither exists off the UI thread). Pushed unconditionally: one `AppConfig`
   clone per frame, exactly what `drain_packets` used to do for its own `fun_cfg` clone,
   and repeating it means a push the thread was busy for simply lands next frame.
10. **FPS limiter** — if `config.fps_limit_enabled`,
    `ctx.request_repaint_after(1.0 / fps_limit)`; otherwise `ctx.request_repaint()` to run
    flat-out. This governs render cadence and is independent of the packet rate — the UDP
    and listener threads keep working regardless.

`on_exit` reads the listener view **blocking** (`view_now`, so a quit from a minimized
window can't save a stale Backfire/Gearbox toggle), saves config, then `listener.shutdown()`
tells the listener thread to flush the (opt-in) per-car DSG calibrations and joins it. The
thread also flushes when the packet channel disconnects, which covers an orderly drop
without `on_exit` — but not a process kill, which can end the thread before it notices.
Calibrations are also written on every car change, so at most the current car's progress
since that point is at risk.

**Blocking IO on the listener thread** is down to one thing: `save_car_calibrations` on car
change, on a calibration reset and at shutdown. Those are rare and bounded. The per-shift
`dsg_shift_log.csv` append was moved onto its own fire-and-forget writer thread
(`dsg.rs:shift_log_writer`) precisely because it sat between a packet and the keypress it
might produce.

## Module map

### `src/`

| File | What it does |
| --- | --- |
| `main.rs` | Entry point: `eframe::run_native`, viewport size, constructs `ForzaApp`. |
| `app.rs` | `ForzaApp` (all app + session state), the `eframe::App` update loop, `drain_packets`, `sync_listener_view`, tab bar, status bar, mini-settings popup, minimap camera logic and season-change handling, the HUD overlay's start/stop (`sync_overlay`, `OverlayStatus`) and `capture_rebind`. The hub everything hangs off. |
| `network.rs` | UDP receive thread + `NetworkHandle` (stop flag, `Drop`-based shutdown). See [[networking]]. |
| `packet.rs` | 324-byte FH6 packet: `ForzaPacket` struct, `from_bytes`/`to_bytes`, helpers (`is_paused`, `power_ps`, `car_class_str`, …). See [[forza-fh6-packet-format]]. |
| `telemetry.rs` | `TelemetryState`: latest packet, connection flag, packets-per-second. |
| `config.rs` | `AppConfig` (serialised to `config.json`), enums, `MINISETTINGS_KEYS`/`LAYOUT_KEYS`, presets (`apply_preset`/`export_preset`/`import_preset`), `default_widget_layout`, `app_data_dir`. See [[state-and-config]]. |
| `theme.rs` | "Graphite" theme: role colour tokens + egui style, `card`/`slider_row`/`checkbox_row` helpers. See [[ui-architecture]]. |
| `i18n.rs` | `tr(...)` translation (English keys → German), `set_language`. |
| `icons.rs` | Nerd-Font icon codepoint constants. |
| `iconcache.rs` | `IconCenterCache` — ink-centres icon glyphs in a fixed box. |
| `labels.rs` | Car class / drivetrain label images + PI-stamping renderer. |
| `input.rs` | `InputSender` — synthetic keypresses (drives backfire/gearbox into the game) on a worker thread; the shared "synthetic echo" window; optional focus gate (suppress emission when the game isn't focused). |
| `keymap.rs` | `HotKey`/`Mods`/`HotkeyBinding` — serde-stable key identity with egui/evdev/VK mapping tables. See [[hotkeys]]. |
| `hotkeys.rs` | `HotkeyListener` — background global key capture (Linux evdev read / Windows `GetAsyncKeyState`), matches configured combos → mpsc channel, whose `Receiver` `new()` hands to the listener thread; also the sysfs device inventory (physical vs virtual keyboards) behind the Setup hotkeys light. See [[hotkeys]]. |
| `gamepad.rs` | `Gamepad` — controller backend (Linux evdev on the physical pad with 2 s hot-plug rescan / Windows XInput poll). Pure `Processor` (deadzones, hysteresis, rising edges) → `PadControl` presses → `HotkeyAction`s sent on the hotkey channel (`HotkeyListener::action_sender`), plus the shared right-stick vector `right_stick()`. See [[gamepad]]. |
| `focus.rs` | `FocusDetector` — "is the game the focused window?" poll thread (Hyprland/X11/GNOME/Custom/Windows); reused by the hotkey gate, the input gate and the overlay's focus-only option. Also runs the overlay's **monitor detection** (`monitor_tick`, `query_monitor`, `parse_hyprland_monitor`), only while the game is focused. See [[hotkeys]], [[overlay]]. |
| `minimap.rs` | Season detection (`current_season`), the on-disk map cache (`load_map_color_image` builds it from the install's tiles; atomic write), the overlay's 4096² mipmapped copy (`overlay_map_image`), and the world↔UV / heading-up maths (`MapCalibration`, `MapView`, easing). Shared by the Dashboard map and the HUD minimap. See [[minimap]]. |
| `coop.rs` (+ `coop/{nostr,rtc,mesh}.rs`) | `CoopState` — WebSocket relay over a cloudflared quick tunnel, or a Trystero-style P2P WebRTC mesh signalled over Nostr relays (`start_trystero`); roster, remote players. `CoopReader` is the cross-thread handle (the listener sends through it, the overlay reads teammates through it). See [[coop]]. |
| `engines.rs` | `engines.csv` loader (`EngineRecord`) for the Engine Swaps table. |

### `src/overlay/` (in-game HUD runtime; Linux + Windows, `snapshot.rs` everywhere) — see [[overlay]]

| File | What it does |
| --- | --- |
| `mod.rs` | `OverlayHandle` (spawn / send / waker / slot / `is_dead`; drop = shutdown + join), `OverlayCmd`, `DisabledReason` + the pure `capability` / `capability_x11` probes, `Backend` (auto by default; `FORZA_OVERLAY_BACKEND` override) + `auto_error` (which reason to show when both fail), the `FORZA_OVERLAY_TEST` dev pattern (`spawn_dev_test`). |
| `wayland.rs` | The overlay thread: calloop loop, sctk layer surface on `Layer::Overlay` (empty input region), surface create/destroy following `snapshot.visible`, output selection, frame-callback pacing (`next_wake`). |
| `x11.rs` | X11/XWayland fallback backend (tested on GNOME 50.4) (auto when layer-shell is missing, or `FORZA_OVERLAY_BACKEND=x11`): override-redirect 32-bit ARGB window per RandR monitor (name → primary → first), empty XShape input region, ping/timer pacing, destroyed on hide. |
| `gl.rs` | glutin EGL: `Gl` (display + context, current surfaceless between surfaces) and `Headless` (test-only, for the PNG harness and the 3D renderer tests; `Headless::new_with(Flavour::{Default, Gles3}, device)` picks desktop GL or an explicit ES 3.0 context and an EGL device, e.g. llvmpipe; module is `pub(crate)` for that); the teardown order. |
| `win32.rs` | Windows backend (experimental, blind): `OverlayHandle` / `Waker` / `OverlaySender` on an event + command queue, the message-loop thread, the click-through layered window + DIB section, `UpdateLayeredWindow` present, monitor enumeration. |
| `wgl.rs` | Windows only: WGL context on a hidden helper window, offscreen FBO, BGRA readback into the DIB. |
| `monitors.rs` | Pure Windows monitor choice (`pick`), unit-tested on Linux. |
| `pacing.rs` | `next_wake` / `wait_ms`: the D17 animation-timer rule shared by Wayland and Windows. |
| `render.rs` | `Renderer`: the overlay's own `egui::Context` + `egui_glow::Painter`, one frame per call, the map texture, co-op teammates; the dev test pattern. |
| `snapshot.rs` | `HudSnapshot`, `SnapshotSlot`, `HudSink`, `hud_clock()`. Platform-neutral, since the listener compiles everywhere. |

### `src/hud/` (pure HUD draw code, driven by `overlay/render.rs`) — see [[overlay]]

| File | What it does |
| --- | --- |
| `mod.rs` | `Hud` (global fade, count-ups, speed hold, map easing) and `Hud::draw`; `modules()` decides which slots show; HUD colours (`col`). |
| `cluster.rs` | Drive cluster: D1a Pill and D3a′ Halo; gear label with the drive-mode letter. |
| `minimap.rs` | M2′ minimap: runs `maprender` (`draw_base` + `draw_layers` with the rounded-pill `CornerClip`) under the markers; `MapAnim` (eased view + the layer data / icons / `RaceSel` set via `Hud::set_layers` / `set_icons`), `MapLoader` (`hud-map` thread), `CoopLayer` (teammates, trails, waypoints). |
| `map_shared.rs` | Marker drawing shared with the Dashboard map: own arrow, teammates, trails, waypoints (`MapCanvas`, which holds the `maprender::Camera`, tilt included; `to_screen` goes through `Camera::project`, so markers sit on the terrain in the 3D view). |
| `race.rs` | R1′ race block; the position cap shared with the drift counter. |
| `drift.rs` | X1′ drift counter, Position + Gain and Total styles. |
| `notify.rs` | D26 notification pills: pure `stack_layout`, fade curve, `draw`. |
| `layout.rs` | The 3×3 slot layout and stacking (`MARGIN` 44, `GAP` 12). |
| `anim.rs` | Time-based curves: fade, pulse, place change, lap hold, chip, count-up. |
| `prims.rs` / `fonts.rs` | Drawing primitives in design px (fixed digit cells, outlined text); the baked Big Shoulders fonts. |
| `png.rs` / `tests.rs` | Offscreen PNG harness (compiled as `overlay::render::png`) and unit tests. |

### `src/gamedata/` (runtime reads of the user's own FH6 install; nothing shipped) — see [[fh6-cars-names-icons]]

| File | What it does |
| --- | --- |
| `install.rs` | Steam detection of the FH6 install (+ `FH6_INSTALL_DIR` override, and `set_user_dir`: the Setup folder, mirrored from `config.fh6_install_dir` by `app.rs` so the map-loader threads can find the install). |
| `tiles.rs` | Map tiles from `Map_Brio_<Season>.zip`: swatchbin parse, hand-written BC1 decode, `load_mosaic(media, season, level)` (parallel per tile row), `MapLoadError`. Used by `minimap.rs`. Docs: `docs/features/minimap.md`, `docs/game-data/fh6-game-files.md` §2. |
| `strtable.rs` | `.str` string-table parser + `strhash`. |
| `lz4.rs` / `pgzp.rs` / `burg.rs` | Map-editor data layer (I25, used by `terrain.rs` (map-editor elevation, `src/mapedit/`); `pgzp` also by `poi.rs` danger signs (`load_danger_signs`, via the map-layers loader)): raw LZ4 block decoder; PGZP v101 reader for `GeoChunk0.minizip` (index only, entries seek-read, `names` filtered); `burG` container + `terrain_mesh`. Docs: `docs/game-data/fh6-terrain.md`. |
| `terrain.rs` | `Elevation` 8 m raster (2752², `i16` decimetres): `build` / `load_or_build` (multi-threaded, progress callback, cached in `<app_data_dir>/map_editor/cache/`), bilinear `height(x, z)`. Heavy: background thread only. |
| `nav.rs` | `Nav::load(media)`: `Brio_00.nav` road graph (polylines split at > 60 m, stable node ids, SHA-1 of the file, `edges()`). |
| `roadtypes.rs` | `fh6-road-types` v1/v2 model: `parse` / `to_json_string` (byte-identical one-entry-per-line writer), embedded project file (`assets/map/fh6-road-types.json`), `raw()`, `current()` = project **replaced wholesale** by the user's override (`override_path()`), `project_updated_since_save`. Docs: `docs/game-data/fh6-map-tooling.md`. |
| `cars.rs` | `CarDb::load(lang)` / `lookup(ordinal)`: CarOrdinal → make + model, JSON-cached in `app_data_dir()`. Blocks the caller, so load it on a background thread (the Debug tab does). Docs: `docs/game-data/fh6-cars-names-icons.md`. |
| `poi.rs` | I28: `Pois::load(media)` reads the exact POI sources (race / landmark / creature / story trigger zones, `route0.nt` and the other locator files, the `GameObjs.xml` pair) with a forward `str::find` scan, no XML crate (~18 ms release); `PoiKind` (38 kinds), `Poi { kind, x, z, y, name, n, gate }` (`gate` = the line across the road of traps / zones, drawn by `maprender`), `Region` outlines. `Pois::load_danger_signs(media)` (I28b, ~0.2 s, GeoChunk0, called on the `map-layers` thread and merged into the POIs), `Pois::current_treasure_chest(week)` + `week_index_now()` (the inferred weekly chest, unverified). Consumed by `maprender::data`. Docs: `docs/game-data/fh6-game-files.md`. |
| `bc7.rs` / `icons.rs` | I28b: BC7 block decoder (modes 0-7, partial-region decode) and `PoiIcons::load(media)`: the ~45 map icons of `Horizon_Map.zip` (swatchbins + cells of `ForteMapIconSheet`) decoded, scaled to 64 px and packed into one 512 x 384 RGBA atlas with UVs per `PoiKind` / `RaceClass` / mascot region (~22 ms release). Loaded on the `map-layers` thread (`maprender::data::GameData::load`); both maps upload the atlas themselves (`maprender::icontex`). Nothing is shipped: read from the user's install. Docs: `docs/game-data/fh6-cars-names-icons.md`. |
| `racelines.rs` | I28: `load_all(media, step_m)` reads the 170 `Route<N>.owt` race lines (+ start / finish from the `.nav` RVAN block), trims and decimates them to `RaceLine { route, circuit, pts, y, half, length_m, closed, bbox, … }` (~18 ms release); `race_pins`. Consumed by `maprender::data`. |

### `src/maprender/` (the shared map renderer, phase J, D61; the Dashboard map and the HUD minimap) — see [[minimap]]

| File | What it does |
| --- | --- |
| `mod.rs` | Module docs with the *why*s, re-exports, `MapTex` (uploaded season map + its original size). |
| `cfg.rs` | `MapLayerConfig` (image look, roads + per-type styles, POIs, race lines, tilt incl. the nested 3D `ReliefCfg` / `RoadHeight` / `ViewMode`, K1), serde with `"#rrggbb"` colours; `::dashboard()` / `::hud()` defaults (D62). Held by `AppConfig::minimap_layers` and `OverlayConfig::map_layers`. |
| `data.rs` | `MapLayers { rev, roads, pois, races, icons, race_class, note }` of `Arc`s; `build_roads` (chains per type), `PoiLayer` (cell grid, off-map cull, the chests for `current_chest`), `RaceLayer` (segment grid), `GameData::load` (+ danger signs and `PoiIcons` on scoped threads), `race_class_of`. |
| `icontex.rs` | `IconTex`: uploads the icon pixels as a texture in one egui context and builds its `IconAtlas`; one per context (Dashboard: `ForzaApp::minimap_icons`, HUD: `Renderer::icons`). |
| `store.rs` | The global loader / cache: `layers()`, `refresh_now()`, status `NoInstall / Loading / Ready / Error`; thread `map-layers`; keyed on install + override file mtime/len. Phase K (K1): lazy `terrain()` (`TerrainStatus`, thread `map-terrain`, install-keyed, only when a map is in 3D) and `road_mesh(layers, terrain)` (thread `map-mesh`, keyed on `(MapLayers::rev, Terrain::rev)`). |
| `terrain.rs` | K1: `HeightGrid` (filled 8 m `u16` grid, bilinear `height`), `Terrain { grid, rev }`, `build_height_grid` / `Terrain::load`, and the hole fill / sea skirt shared with `mapedit/data.rs` (`fill_holes`, `nearest_land`, `apply_skirt`; the 3D map fills sea with y 100, the editor with 40). Docs: `docs/game-data/fh6-terrain.md`. |
| `mesh3d.rs` | K1: `RoadMesh::build` (roads resampled to 8 m, tiles, GPU vertex bytes + near/far index sets, taut-string jump lines, `road_y` height rules), `build_rel` (in-race focus flags), `Tile::in_frustum` / `dist_to`; nav orphans (`y = 0`, "no height") take the terrain (`known_y`, K2). Pure CPU; the GL renderer (`gl3d`, K2) uploads it. See [[minimap]] ("3D: data and camera"). |
| `gl3d/` | K2: the shared GL 3D renderer (`#[cfg(linux or windows)]`, glow + egui_glow). `mod.rs` the public API (`Gl3dHandle`, `Gl3dStatus`, `Scene3d`, `add_scene`, `Gl3dOptions`, the slow-GPU guard, the `FORZA_MAP_3D_DEBUG` line); `scene.rs` `Gl3d` (the GL objects of one context: scene FBO + depth, render pass, composite, state save / restore); `clipmap.rs` (7-level terrain clipmap, `R16UI` height texture); `roads.rs` (road buffers, per-frame draw plan with the near / far LOD sets, style table); `shaders.rs` (GLSL for desktop 3.3 core and ES 3.0); `probe.rs` (requirements); `tests.rs` (headless EGL: default GL, GLES 3.0, llvmpipe; `#[ignore]`). One `Gl3d` **per GL context** (HUD: `overlay::render::Renderer`; Dashboard + Viewer: `ForzaApp`), created lazily inside the paint callback. See [[minimap]] ("3D renderer"). |
| `view.rs` | `Camera` (flat = `MapView`, tilted = flat perspective, and with a `Relief` the 3D camera: `project3`, `k_at`, `eye` + eye clearance, `footprint`, `view_proj_rel`; no longer `Copy`; `from_cfg`, `from_cfg_relief`, `focal_for` scales the perspective with the view height, `depth_scale_at_row`), `world_aabb`, `thin`, `clip_convex`, `clip_polyline_convex`, `clip_segment_convex`, `fan`. |
| `style.rs` | Road draw order, width rule, dash patterns, the POI category table (`POI_CATS`). |
| `racesel.rs` | `RaceSel`: which race lines to draw (nearest / near / the best-effort current race). |
| `paint2d.rs` | `draw_base` (image mesh incl. the subdivided tilted one and its far-edge fade) and `draw_layers` (roads with the tilt taper, jump lines, race lines, gate lines, POIs with the game's icons, the current treasure chest) onto an egui `Painter`; `draw_layers_parts` + `Parts` (K2: draw a subset, the 3D views keep race lines and POIs here and leave the roads to `gl3d`; POIs project through `Camera::project` / `k_at` so they follow the terrain); `IconAtlas`, `CornerClip` (the HUD pill). |
| `ui.rs` | Settings cards for both maps (`layers_ui`, `view_rows` / `ViewCfg`, `status_ui`); used by the Map tab's Minimap and Dashboard map & Viewer pages. |

### `src/mapedit/` (the FH6 map editor inside the app, I26; D50) — see `docs/game-data/fh6-map-tooling.md`

| File | What it does |
| --- | --- |
| `mod.rs` | Module docs + re-exports (`MapServer`, `MapEvent`, `MapServerState`, `StartFrom`). |
| `data.rs` | I26a generators: every data file the editor / 3D pages load (`data/*.js`, `preview3d/*`, tile + texture JPEGs) from the install's nav, elevation and tiles, byte-compatible with the Python tools; `EditorData::{build, resolve, set_road_types}` (builds the text files once, imagery on demand with a disk cache under `<app_data_dir>/map_editor/cache/`), `write_atomic`. |
| `server.rs` | I26b local web server: hand-rolled `TcpListener` HTTP, loopback + token path prefix + Host/Origin checks, embedded editor pages (`assets/editor/`) served from memory, `POST save` (validate -> stamp `based_on` -> atomic write of the override -> new current road types), sticky port (`map_editor/port`). `MapServer::start(ctx, media, StartFrom, events)`; the app side is `app.rs:{start_map_editor, stop_map_editor, map_editor_state, map_editor_url, map_editor_current, poll_map_editor}` and `map_editor_last`; the Map tab → Map data page (`ui/map_data.rs:map_data_card`) is the caller, see `docs/features/map-editor.md`. |

### `src/listeners/` (event-driven, fire inside `drain_packets`)

| File | What it does |
| --- | --- |
| `mod.rs` | Declares the five listener modules plus `worker`, `hud` and `lap_trace`. |
| `worker.rs` | The **listener thread**: owns Backfire + DSG, the per-car calibration map, the detected redline, its own pps, the global hotkeys (incl. the Hide HUD toggle, `hud_hidden`), the co-op send, and the HUD snapshot publishing; runs off the UDP channel so key output survives a hidden window. Mailboxes (`ListenerView` / `ToListener` / `PacketQueue`) + `Command` channel + `ListenerHandle` (`set_hud_sink`). |
| `hud.rs` | HUD data on the listener thread: `HudTracker` (packet → `HudSnapshot`), the race/drift `ModeClassifier`, the drift gain `DriftWindow`, `visible_target`. See [[overlay]]. |
| `notify.rs` | D26 `Notifier`: queue of HUD messages + `watch` (diffs gearbox/backfire/calibration state each loop pass). See [[overlay]]. |
| `lap_trace.rs` | Best-lap trace keyed by distance → the HUD's live lap delta. |
| `backfire.rs` | Synthetic anti-lag / throttle-blip; echo-window bookkeeping. See [[backfire]]. |
| `calib.rs` | Calibration check structs (`MaxRpmChecks`, `EngageChecks`, `GearMapChecks`) shared by the real calibration logic in `dsg.rs` and the Debug tab's calibration panel, so the panel shows exactly what gates the logic. |
| `dsg.rs` | DSG-style auto-shifter with per-car calibration. See [[gearbox]]. |
| `perf_test.rs` | Configurable accel/decel timers. |
| `power_capture.rs` | Captures RPM vs power/torque/boost during full-throttle runs. See [[power-curve]]. |
| `sprint_timer.rs` | 0→100…400→500 km/h sprint splits. |

### `src/ui/` (one module per tab; each exposes `show(ui, app)`)

| File | What it does |
| --- | --- |
| `mod.rs` | Declares the compiled tab modules. |
| `dashboard.rs` | The draggable/resizable widget grid (largest UI file). See [[dashboard]]. |
| `overlay_tab.rs` | Overlay tab: the HUD overlay's settings page (General, Monitor Detection, drag-and-drop 3×3 Layout, per-module cards). Also hosts the module selector (`page_selector_with`) and `module_card` the Map tab reuses. See [[overlay]]. |
| `map_tab.rs` | Map tab (D67): the full-size viewer (drag / wheel / follow) and the settings mode with the Minimap · Dashboard map & Viewer · Map data pages. See [[map-tab]]. |
| `map_scene.rs` | The map scene the Dashboard Map widget and the viewer both draw (`draw`, `texture_or_status`), and their temporary pan / zoom state with the reset-when-driving rule (`ManualView`, `DriveGate`, D72). |
| `map_data.rs` | The Map data page (road-type map editor card), moved from `settings.rs`. See [[map-editor]]. |
| `backfire.rs` | Backfire tab controls (`show_backfire`). |
| `gearbox.rs` | Automatic Gearbox tab (`show_gearbox`). |
| `power_curve.rs` | Power Curve tab (live RPM vs power/torque, boost). |
| `engine_swaps.rs` | Engine Swaps reference table from `engines.csv`. |
| `coop.rs` | Co-Op tab: transport selector, Cloudflare host/join, Trystero room ID. |
| `settings.rs` | Settings tab, labelled **Setup** (profiles, hotkeys, network, display, co-op port, Game Install, Window Detection). See [[settings]], [[map-editor]]. |
| `changelog.rs` | "What's New" viewer — parses root `CHANGELOG.md`, category filters. |
| `debug_tab.rs` | Debug tab (just left of Setup): every field of `telemetry.latest` as a raw name → value grid, parsed from `{:#?}` so it can't drift; Copy button. See [[debug]]. |
| `acceleration.rs` | **ORPHANED** — not in `ui/mod.rs`, not compiled. |
| `deceleration.rs` | **ORPHANED** — not in `ui/mod.rs`, not compiled. |

## Where to look for X

- **Add / change a dashboard widget** → `ui/dashboard.rs` (render), plus
  `config.rs:WidgetKind` + `default_widget_layout` (register it) and a mini-settings
  sub-tab in `app.rs` (`DashboardSubTab` + its match arm). See [[dashboard]].
- **Add a config field / mini-setting** → `config.rs:AppConfig` (field + default);
  if it should travel with presets/export, add it to `MINISETTINGS_KEYS`; wire its
  control in the relevant `app.rs` mini-settings arm. See [[state-and-config]].
- **Add a translation** → add the English key + German value in `i18n.rs`; wrap the
  user-facing string in `tr("...")` at the call site.
- **Add a top-level tab** → add a variant to `app.rs:Tab`, a `ui/<tab>.rs` module with
  `show(...)` (declared in `ui/mod.rs`), a tab-bar entry, and a `CentralPanel` match arm.
- **Tune / add a listener** → the relevant `listeners/<name>.rs`. A read-only listener is
  called from `app.rs:drain_packets`; one that drives the game (through
  `input.rs:InputSender`) belongs on the listener thread, in `listeners/worker.rs:run`.
- **Show new gearbox/backfire state in the UI** → add the field to `DsgView`/`BackfireView`
  (`listeners/dsg.rs`, `listeners/backfire.rs`) and to the matching `view()`; the UI reads
  `app.dsg` / `app.backfire`, never the live listener.
- **Hotkeys / a new bindable action** → `config.rs:HotkeyAction` (+ `scope`); dispatch for
  app-focused actions in `app.rs:run_app_hotkey`, for global ones in
  `listeners/worker.rs:run`; backend in `hotkeys.rs`, gate in `focus.rs`. Keys map via
  `keymap.rs`. See [[hotkeys]].
- **Change packet parsing / a new field** → `packet.rs` (struct + `from_bytes`/`to_bytes`)
  against [[forza-fh6-packet-format]].
- **Networking / port / connection** → `network.rs` (receive thread) and
  `telemetry.rs` (connection + rate). See [[networking]].
- **Theme colours / control layout** → `theme.rs` and [[ui-architecture]] /
  the styling guide.
- **Frame timing / FPS** → the FPS limiter at the end of `app.rs:update`.
- **Map tab (viewer, pan / zoom, settings pages)** → `ui/map_tab.rs`; the map scene both full maps draw and the shared pan / zoom state (`ManualView`) → `ui/map_scene.rs`. See `docs/features/map-tab.md`.
- **Map editor UI (Map tab → Map data page, start modes, reset / rebuild, Contribute)** → `ui/map_data.rs:map_data_card` / `map_data_view`; `CONTRIBUTING.md`. See `docs/features/map-editor.md`.
- **Map editor (server, Save, generated data files)** → `mapedit/server.rs` / `mapedit/data.rs`; app wiring in `app.rs:start_map_editor`; the editor pages in `assets/editor/`. See `docs/game-data/fh6-map-tooling.md`.
- **Minimap maths / season image (both maps)** → `minimap.rs`. See [[minimap]].
- **3D map data, camera and road mesh (phase K)** → `maprender/terrain.rs`, `view.rs` (`Camera` relief), `mesh3d.rs`, `store.rs` (`terrain()`, `road_mesh()`); settings in `cfg.rs` (`tilt.relief`). The GL renderer is `maprender/gl3d/` (K2; the call sites are K3 HUD / K4 Dashboard + Viewer). See [[minimap]] ("3D: data and camera", "3D renderer").
- **Map layers (roads, POIs, race lines, tilt) on the Dashboard / HUD map** → `maprender/` (`paint2d.rs` draws, `style.rs` looks, `cfg.rs` settings and defaults, `store.rs` data lifecycle); the call sites are `ui/dashboard.rs:show_minimap_widget` and `hud/minimap.rs:draw` (data handed over by `overlay/render.rs`). See [[minimap]], [[overlay]].
- **Map settings UI (layer / view cards for the Minimap and Dashboard map & Viewer pages)** → `ui/map_tab.rs` + `maprender/ui.rs`. See [[map-tab]].
- **HUD overlay: a widget's look** → `hud/<widget>.rs` (+ `hud::col` colours, `hud/anim.rs`
  timings); check it with the PNG harness `cargo test render_spec_states -- --ignored`.
  See [[overlay]].
- **HUD overlay: new data on the HUD** → a field on `overlay/snapshot.rs:HudSnapshot`, filled
  in `listeners/hud.rs:HudTracker::snapshot` (listener thread), drawn in `hud/`.
- **HUD overlay: a setting** → `config.rs:OverlayConfig` (+ its default) and a control in
  `ui/overlay_tab.rs`; it reaches the HUD through the config push, no extra wiring.
- **HUD overlay: when it shows/hides** → `listeners/hud.rs:visible_target` (the target) and
  `overlay/wayland.rs:follow_snapshot` / `maybe_render` (the surface lifecycle).
- **HUD overlay: race vs drift detection** → `listeners/hud.rs:ModeClassifier`.
- **HUD overlay: start/stop, status, which monitor** → `app.rs:sync_overlay` /
  `sync_overlay_thread` / `attach_overlay`; `focus.rs:monitor_tick`.
