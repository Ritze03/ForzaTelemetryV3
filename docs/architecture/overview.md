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

Two long-lived threads plus a few short-lived helpers. State crosses the UDP→UI
boundary through an **`mpsc` channel**, not shared memory.

- **UDP receive thread** — spawned in `network.rs:start_receiver`. Binds
  `0.0.0.0:<port>`, sets a 200 ms read timeout, and loops: `socket.recv` →
  `packet.rs:ForzaPacket::from_bytes` → `sender.send(pkt)`. It owns nothing the UI
  touches; it only pushes parsed packets down the channel. A `network.rs:NetworkHandle`
  holds an `Arc<AtomicBool>` stop flag; when the handle is dropped (`Drop`), the flag
  flips and the thread exits its loop. Changing the port drops the old handle and starts
  a fresh thread + channel (`app.rs:ForzaApp::restart_receiver`).
- **egui/eframe render thread (main)** — owns `ForzaApp` including the `Receiver`
  end of the channel. Everything UI, all listeners, and all state mutation happen here,
  single-threaded, so no locking is needed around app state.
- **Short-lived background threads** — the seasonal minimap image decode
  (`app.rs:map_load_thread`, results returned over its own `mpsc` channel of
  `MapLoadMessage`), and Co-Op's WebSocket relay + cloudflared tunnel
  (`coop.rs`, see [[coop]]). Synthetic keypress emission (`input.rs:InputSender`) also
  runs on its own worker thread.

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
      ▼  mpsc::Receiver::try_recv   [main thread]
app.rs:ForzaApp::drain_packets()   (called first each frame)
      │  for each packet (capped at 200/frame):
      ├─ car-change reset (per-car session state, DSG calibration)
      ├─ session maxima + cached car identity (power/torque/boost/speed,
      │    dynamic redline, wheel-radius estimate)  [only when is_race_on != 0]
      ├─ stats: gforce_stats.update / suspension_stats.update / speed-delta /
      │    trace_history (Speed Trace, active-time axis)
      ├─ listeners fire (see below)
      ├─ coop.push_local(&pkt)          → relay to peers (coop.rs)
      └─ telemetry.update(pkt)          → stores latest + packet-rate (telemetry.rs)
      ▼
egui::CentralPanel dispatch → crate::ui::<tab>::show(ui, self)
      │  widgets read app.telemetry.latest + derived state and paint
      ▼
frame drawn; repaint rescheduled (FPS limiter)
```

**Listeners** (each `update(&pkt, …)` inside `drain_packets`, wired via
`listeners/mod.rs`):
- `sprint_timer.rs:SprintTimer::update` — 0→100…400→500 km/h splits.
- `power_capture.rs:PowerCapture::update` — captures RPM/power/torque/boost during
  full-throttle pulls (gated by `backfire_echo_active()` so fake blips don't seed it).
- `perf_test.rs:PerfTest::update` — configurable accel/decel timers.
- `backfire.rs:BackfireListener::update` — decides when to inject a synthetic throttle
  blip, driving the game through `input.rs:InputSender`.
- `dsg.rs:DsgListener::update` — DSG-style auto-shifter; also drives `InputSender`.

`telemetry.rs:TelemetryState` holds `latest: Option<ForzaPacket>`, `is_connected`, and
`packets_per_sec` (recomputed each 1 s window). Connection is marked **down** at the tail
of `drain_packets` if no packet arrived for 2 s. `ForzaPacket::is_paused()` (all position
+ orientation fields zero) distinguishes an actively-driving packet from a paused-game one,
which several stats and the Co-Op relay respect.

## Frame loop

`app.rs:<ForzaApp as eframe::App>::update` runs once per repaint and does, in order:

1. `i18n::set_language(config.language)` — pick the active language for `tr(...)`.
2. `drain_packets()` — ingest everything queued since last frame (see above).
3. `coop.tick()` — advance Co-Op jitter buffers; `update_minimap_trails()`.
4. Poll the minimap image channel; handle season change; throttle/smooth the minimap
   camera (position cache, eased yaw, zoom).
5. Global hotkeys — F10 (map orientation), F11 (fullscreen, Windows), Ctrl+S
   (mini-settings popup), Ctrl+E (dashboard edit mode).
6. Chrome panels — top **tab bar** (`TopBottomPanel::top`, three styles via
   `tab_button`/`page_pill`), bottom **status bar** (connection, pps, Co-Op,
   cog), and the floating **mini-settings window** (`page_settings_*`, driven by
   `PageSettingsTab` / `DashboardSubTab`).
7. **Central panel dispatch** — `match self.current_tab { … }` calls the one
   `crate::ui::<tab>::show(ui, self)` for the active `Tab`.
8. **FPS limiter** — if `config.fps_limit_enabled`,
   `ctx.request_repaint_after(1.0 / fps_limit)`; otherwise `ctx.request_repaint()` to run
   flat-out. This governs render cadence and is independent of the packet rate — the UDP
   thread keeps filling the channel regardless.

`on_exit` saves config and (opt-in) per-car DSG calibrations.

## Module map

### `src/`

| File | What it does |
| --- | --- |
| `main.rs` | Entry point: `eframe::run_native`, viewport size, constructs `ForzaApp`. |
| `app.rs` | `ForzaApp` (all app + session state), the `eframe::App` update loop, `drain_packets`, tab bar, status bar, mini-settings popup, minimap camera logic, season detection. The hub everything hangs off. |
| `network.rs` | UDP receive thread + `NetworkHandle` (stop flag, `Drop`-based shutdown). See [[networking]]. |
| `packet.rs` | 324-byte FH6 packet: `ForzaPacket` struct, `from_bytes`/`to_bytes`, helpers (`is_paused`, `power_ps`, `car_class_str`, …). See [[forza-fh6-packet-format]]. |
| `telemetry.rs` | `TelemetryState`: latest packet, connection flag, packets-per-second. |
| `config.rs` | `AppConfig` (serialised to `config.json`), enums, `MINISETTINGS_KEYS`/`LAYOUT_KEYS`, presets (`apply_preset`/`export_preset`/`import_preset`), `default_widget_layout`, `app_data_dir`. See [[state-and-config]]. |
| `theme.rs` | "Graphite" theme: role colour tokens + egui style, `card`/`slider_row`/`checkbox_row` helpers. See [[ui-architecture]]. |
| `i18n.rs` | `tr(...)` translation (English keys → German), `set_language`. |
| `icons.rs` | Nerd-Font icon codepoint constants. |
| `iconcache.rs` | `IconCenterCache` — ink-centres icon glyphs in a fixed box. |
| `labels.rs` | Car class / drivetrain label images + PI-stamping renderer. |
| `input.rs` | `InputSender` — synthetic keypresses (drives backfire/gearbox into the game) on a worker thread; the shared "synthetic echo" window. |
| `coop.rs` | `CoopState` — WebSocket relay over a cloudflared quick tunnel; roster, remote players. See [[coop]]. |
| `engines.rs` | `engines.csv` loader (`EngineRecord`) for the Engine Swaps table. |

### `src/listeners/` (event-driven, fire inside `drain_packets`)

| File | What it does |
| --- | --- |
| `mod.rs` | Re-exports the five listener modules. |
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
- **Tune / add a listener** → the relevant `listeners/<name>.rs`; it's called from
  `app.rs:drain_packets`. Listeners that drive the game go through `input.rs:InputSender`.
- **Change packet parsing / a new field** → `packet.rs` (struct + `from_bytes`/`to_bytes`)
  against [[forza-fh6-packet-format]].
- **Networking / port / connection** → `network.rs` (receive thread) and
  `telemetry.rs` (connection + rate). See [[networking]].
- **Theme colours / control layout** → `theme.rs` and [[ui-architecture]] /
  the styling guide.
- **Frame timing / FPS** → the FPS limiter at the end of `app.rs:update`.
