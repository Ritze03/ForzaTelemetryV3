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
- **Overlay thread** (Linux, only while `overlay.enabled`) — `overlay/wayland.rs:run`: its own
  Wayland connection, calloop loop, layer-shell surface, glutin EGL context, `egui::Context`
  and `egui_glow` painter. Draws the HUD from the listener's `HudSnapshot` mailbox, one frame
  per packet. Owned by `overlay::OverlayHandle`, held in `app.rs:OverlayRuntime`. By default
  (auto) it falls back to `overlay/x11.rs:run` when layer-shell is unavailable (an
  override-redirect X11 window via XWayland, for GNOME — tested on GNOME 50.4); `FORZA_OVERLAY_BACKEND=wayland|x11`
  forces one. **Why its
  own thread:** like the listener, it must keep working while the game covers the main
  window. Helpers: `overlay-start` (runs the blocking `OverlayHandle::spawn`, up to 5 s),
  `overlay-drop` (drop = shutdown + join, kept off the UI thread), `hud-map` (loads the
  season map for the HUD minimap). See [[overlay]].
- **Short-lived background threads** — the Dashboard's seasonal minimap image decode
  (`app.rs:map_load_thread` → `minimap.rs:load_map_color_image`, results returned over its
  own `mpsc` channel of `MapLoadMessage`), and Co-Op's WebSocket relay + cloudflared tunnel
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
| `hotkeys.rs` | `HotkeyListener` — background global key capture (Linux evdev read / Windows `GetAsyncKeyState`), matches configured combos → mpsc channel, whose `Receiver` `new()` hands to the listener thread. See [[hotkeys]]. |
| `focus.rs` | `FocusDetector` — "is the game the focused window?" poll thread (Hyprland/X11/GNOME/Custom/Windows); reused by the hotkey gate, the input gate and the overlay's focus-only option. Also runs the overlay's **monitor detection** (`monitor_tick`, `query_monitor`, `parse_hyprland_monitor`), only while the game is focused. See [[hotkeys]], [[overlay]]. |
| `minimap.rs` | Season detection (`current_season`), the on-disk map cache (`load_map_color_image`, atomic write), the overlay's 4096² mipmapped copy (`overlay_map_image`), and the world↔UV / heading-up maths (`MapCalibration`, `MapView`, easing). Shared by the Dashboard map and the HUD minimap. See [[minimap]]. |
| `coop.rs` (+ `coop/{nostr,rtc,mesh}.rs`) | `CoopState` — WebSocket relay over a cloudflared quick tunnel, or a Trystero-style P2P WebRTC mesh signalled over Nostr relays (`start_trystero`); roster, remote players. `CoopReader` is the cross-thread handle (the listener sends through it, the overlay reads teammates through it). See [[coop]]. |
| `engines.rs` | `engines.csv` loader (`EngineRecord`) for the Engine Swaps table. |

### `src/overlay/` (in-game HUD runtime; Linux only, except `snapshot.rs`) — see [[overlay]]

| File | What it does |
| --- | --- |
| `mod.rs` | `OverlayHandle` (spawn / send / waker / slot / `is_dead`; drop = shutdown + join), `OverlayCmd`, `DisabledReason` + the pure `capability` / `capability_x11` probes, `Backend` (auto by default; `FORZA_OVERLAY_BACKEND` override) + `auto_error` (which reason to show when both fail), the `FORZA_OVERLAY_TEST` dev pattern (`spawn_dev_test`). |
| `wayland.rs` | The overlay thread: calloop loop, sctk layer surface on `Layer::Overlay` (empty input region), surface create/destroy following `snapshot.visible`, output selection, frame-callback pacing (`next_wake`). |
| `x11.rs` | X11/XWayland fallback backend (tested on GNOME 50.4) (auto when layer-shell is missing, or `FORZA_OVERLAY_BACKEND=x11`): override-redirect 32-bit ARGB window per RandR monitor (name → primary → first), empty XShape input region, ping/timer pacing, destroyed on hide. |
| `gl.rs` | glutin EGL: `Gl` (display + context, current surfaceless between surfaces) and `Headless` (for the PNG harness); the teardown order. |
| `render.rs` | `Renderer`: the overlay's own `egui::Context` + `egui_glow::Painter`, one frame per call, the map texture, co-op teammates; the dev test pattern. |
| `snapshot.rs` | `HudSnapshot`, `SnapshotSlot`, `HudSink`, `hud_clock()`. Platform-neutral, since the listener compiles everywhere. |

### `src/hud/` (pure HUD draw code, driven by `overlay/render.rs`) — see [[overlay]]

| File | What it does |
| --- | --- |
| `mod.rs` | `Hud` (global fade, count-ups, speed hold, map easing) and `Hud::draw`; `modules()` decides which slots show; HUD colours (`col`). |
| `cluster.rs` | Drive cluster: D1a Pill and D3a′ Halo; gear label with the drive-mode letter. |
| `minimap.rs` | M2′ minimap, `MapLoader` (`hud-map` thread), co-op teammate markers. |
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
| `install.rs` | Steam detection of the FH6 install (+ `FH6_INSTALL_DIR` override). |
| `strtable.rs` | `.str` string-table parser + `strhash`. |
| `cars.rs` | `CarDb::load(lang)` / `lookup(ordinal)`: CarOrdinal → make + model, JSON-cached in `app_data_dir()`. Blocks the caller, so load it on a background thread (the Debug tab does). Docs: `docs/game-data/fh6-cars-names-icons.md`. |

### `src/listeners/` (event-driven, fire inside `drain_packets`)

| File | What it does |
| --- | --- |
| `mod.rs` | Declares the five listener modules plus `worker`, `hud` and `lap_trace`. |
| `worker.rs` | The **listener thread**: owns Backfire + DSG, the per-car calibration map, the detected redline, its own pps, the global hotkeys (incl. the Hide HUD toggle, `hud_hidden`), the co-op send, and the HUD snapshot publishing; runs off the UDP channel so key output survives a hidden window. Mailboxes (`ListenerView` / `ToListener` / `PacketQueue`) + `Command` channel + `ListenerHandle` (`set_hud_sink`). |
| `hud.rs` | HUD data on the listener thread: `HudTracker` (packet → `HudSnapshot`), the race/drift `ModeClassifier`, the drift gain `DriftWindow`, `visible_target`. See [[overlay]]. |
| `notify.rs` | D26 `Notifier`: queue of HUD messages + `watch` (diffs gearbox/backfire/calibration state each loop pass). See [[overlay]]. |
| `lap_trace.rs` | Best-lap trace keyed by distance → the HUD's live lap delta. |
| `backfire.rs` | Synthetic anti-lag / throttle-blip; echo-window bookkeeping. See [[backfire]]. |
| `dsg.rs` | DSG-style auto-shifter with per-car calibration. See [[gearbox]]. |
| `perf_test.rs` | Configurable accel/decel timers. |
| `power_capture.rs` | Captures RPM vs power/torque/boost during full-throttle runs. See [[power-curve]]. |
| `sprint_timer.rs` | 0→100…400→500 km/h sprint splits. |

### `src/ui/` (one module per tab; each exposes `show(ui, app)`)

| File | What it does |
| --- | --- |
| `mod.rs` | Declares the compiled tab modules. |
| `dashboard.rs` | The draggable/resizable widget grid (largest UI file). See [[dashboard]]. |
| `overlay_tab.rs` | Overlay tab: the HUD overlay's settings page (General, Monitor Detection, drag-and-drop 3×3 Layout, per-module cards). See [[overlay]]. |
| `backfire.rs` | Backfire tab controls (`show_backfire`). |
| `gearbox.rs` | Automatic Gearbox tab (`show_gearbox`). |
| `power_curve.rs` | Power Curve tab (live RPM vs power/torque, boost). |
| `engine_swaps.rs` | Engine Swaps reference table from `engines.csv`. |
| `coop.rs` | Co-Op tab: transport selector, Cloudflare host/join, Trystero room ID. |
| `settings.rs` | Settings tab, labelled **Setup** (profiles, hotkeys, network, display, co-op port, Window Detection). See [[settings]]. |
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
- **Minimap maths / season image (both maps)** → `minimap.rs`. See [[minimap]].
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
