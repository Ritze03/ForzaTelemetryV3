# Architecture Overview

The first doc to read to understand and navigate ForzaTelemetryV3. It maps the
threads, the packet-to-pixel data flow, the per-frame loop, and every source file.
Deep dives live in sibling docs — this one links out rather than duplicating them:
[[networking]], [[state-and-config]], [[ui-architecture]], and per-feature docs
like [[coop]], [[minimap]], [[gearbox]], [[power-curve]].

## Big picture

ForzaTelemetryV3 is a single-window **egui/eframe** desktop app (immediate mode:
the whole UI is rebuilt from state every frame). It listens for **one-way FH6 UDP
telemetry** — a fixed 324-byte packet at the game's frame rate — parses each packet,
folds it into state held on one big struct (`app.rs:ForzaApp`), lets event-driven
listeners react (backfire, auto-gearbox, timers, power capture), and redraws a tabbed
dashboard. `ForzaApp` *is* the application: it owns config, telemetry, listeners, and
all derived/session state, and it implements `eframe::App`.

## Threading model

Three long-lived threads plus a few short-lived helpers. Packets cross UDP → listener → UI
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
  *forwarded* packets. Everything else — stats, Co-Op, the three read-only listeners,
  widgets — still happens here, single-threaded.
- **Short-lived background threads** — the seasonal minimap image decode
  (`app.rs:map_load_thread`, results returned over its own `mpsc` channel of
  `MapLoadMessage`), and Co-Op's WebSocket relay + cloudflared tunnel
  (`coop.rs`, see [[coop]]). Synthetic keypress emission (`input.rs:InputSender`) also
  runs on its own worker thread, as does the focus poll (`focus.rs`).

### Listener ↔ UI: two mailboxes

Each direction has its own `Arc<Mutex<…>>`; neither lock is ever held across a blocking
call, disk IO or drawing, and neither side ever holds both.

| Direction | Payload | Writer | Reader |
| --- | --- | --- | --- |
| listener → UI | `worker.rs:ListenerView` (`DsgView`, `BackfireView`, `dynamic_max_rpm`, the two enable flags + `toggle_gen`) | listener `lock`s once per loop and overwrites it from its local copy | UI `try_lock`s once a frame in `app.rs:sync_listener_view`, clones into `app.dsg` / `app.backfire` / `app.dynamic_max_rpm`, drops the guard |
| UI → listener | `worker.rs:ToListener` (`AppConfig` + `our_focused` / `wants_text`, which only egui knows) | UI `try_lock`s at the end of every frame (`ListenerHandle::push`) | listener `try_lock`s once per loop and takes it |
| UI → listener (one-shot) | `worker.rs:Command` — `ClearRpmCalibration`, `ClearGearMap`, `Shutdown` | UI, over an `mpsc` channel (never blocks, never dropped) | listener drains it once per loop |

**Why this shape** (the user's own pattern): each side keeps a *local* copy and only opens
the shared one to write it or copy it out, so the listener loop is never affected by the UI
and the UI never blocks. A `try_lock` miss on the UI side just means it draws last frame's
values — acceptable because nothing in the UI is critical, while the loop that drives the
game must never stall.

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
      ├─ publishes ListenerView into the listener→UI mailbox
      ▼  mpsc::Sender<ForzaPacket>::send  (forward)
──────────────────── channel ────────────────────
      ▼  mpsc::Receiver::try_recv   [main thread]
app.rs:ForzaApp::drain_packets()   (called first each frame)
      │  keeps only the newest 200 queued packets, then for each:
      ├─ car-change reset (per-car session state, session maxima)
      ├─ session maxima + cached car identity (power/torque/boost/speed,
      │    wheel-radius estimate)  [only when is_race_on != 0]
      ├─ stats: gforce_stats.update / suspension_stats.update / speed-delta /
      │    trace_history (Speed Trace, active-time axis)
      ├─ UI-side listeners fire (see below)
      ├─ coop.push_local(&pkt)          → relay to peers (coop.rs)
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

**Backlog cap.** The forwarded channel keeps filling while the window is hidden, so
`drain_packets` throws away all but the newest 200 queued packets (`UI_BACKLOG_CAP`)
before processing any. *Why:* replaying minutes of stale telemetry through the sprint
timer, the trace buffer and the Co-Op relay on restore is worse than skipping it. The
listener thread itself never falls behind — it drains continuously and processes each
packet as it arrives, so it can never replay stale input.

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
3. `drain_packets()` — ingest everything queued since last frame (see above).
4. `coop.tick()` — advance Co-Op jitter buffers; `update_minimap_trails()`.
5. Poll the minimap image channel; handle season change; throttle/smooth the minimap
   camera (position cache, eased yaw, zoom).
6. Hotkeys — only the app-focused actions (Ctrl+S mini-settings, Ctrl+E dashboard edit),
   matched from config against egui input; F11 fullscreen (Windows) stays hardcoded. The
   global actions (G gearbox, B backfire, F reset-RPM) and the synthetic-input focus gate
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

`on_exit` saves config, then `listener.shutdown()` tells the listener thread to flush the
(opt-in) per-car DSG calibrations and joins it. The thread also flushes if the channel
disconnects, so a drop without `on_exit` can't lose calibration data.

## Module map

### `src/`

| File | What it does |
| --- | --- |
| `main.rs` | Entry point: `eframe::run_native`, viewport size, constructs `ForzaApp`. |
| `app.rs` | `ForzaApp` (all app + session state), the `eframe::App` update loop, `drain_packets`, `sync_listener_view`, tab bar, status bar, mini-settings popup, minimap camera logic, season detection. The hub everything hangs off. |
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
| `focus.rs` | `FocusDetector` — "is the game the focused window?" poll thread (Hyprland/X11/Custom/Windows); reused by the hotkey gate and the input gate. See [[hotkeys]]. |
| `coop.rs` | `CoopState` — WebSocket relay over a cloudflared quick tunnel; roster, remote players. See [[coop]]. |
| `engines.rs` | `engines.csv` loader (`EngineRecord`) for the Engine Swaps table. |

### `src/listeners/` (event-driven, fire inside `drain_packets`)

| File | What it does |
| --- | --- |
| `mod.rs` | Re-exports the five listener modules plus `worker`. |
| `worker.rs` | The **listener thread**: owns Backfire + DSG, the per-car calibration map, the detected redline, its own pps, and the global hotkeys; runs off the UDP channel so key output survives a hidden window. Mailboxes (`ListenerView` / `ToListener`) + `Command` channel + `ListenerHandle`. |
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
| `backfire.rs` | Backfire tab controls (`show_backfire`). |
| `gearbox.rs` | Automatic Gearbox tab (`show_gearbox`). |
| `power_curve.rs` | Power Curve tab (live RPM vs power/torque, boost). |
| `engine_swaps.rs` | Engine Swaps reference table from `engines.csv`. |
| `coop.rs` | Co-Op host/join tab. |
| `settings.rs` | Settings tab (port, units, language, FPS, presets, connection). |
| `changelog.rs` | "What's New" viewer — parses root `CHANGELOG.md`, category filters. |
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
