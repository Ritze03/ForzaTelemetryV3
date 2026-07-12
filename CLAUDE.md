# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Project overview

**ForzaTelemetryV3** — a Rust desktop app that receives Forza Horizon 6 UDP telemetry and shows it in a live dashboard.

- **Rust** / **Cargo**; GUI is **egui** (immediate-mode) via **eframe**.
- Single dark "Graphite" theme in `src/theme.rs` — reference chrome colours by role token (`ACCENT`, `PANEL`, `TEXT_DIM`…), never hard-code hex at call sites. Semantic data colours (tyre temps, input bars) live at their call sites.
- All user-facing strings go through `tr("...")` in `src/i18n.rs` (English source → German). Add the English key + German value there; duplicate keys warn at compile time.
- Predecessors (FH4/FH5) are compiled JARs only under `old_versions/` — no source.

## Protocol

FH6 broadcasts a fixed **324-byte little-endian UDP packet** at the game's frame rate to a configured IP/port. One-way (the game only sends). Data flows **only while actively driving** — not in menus, pauses, or replays.

- Configure in-game: **SETTINGS > HUD AND GAMEPLAY > Data Out** (toggle, IP, port).
- **Avoid ports 5200–5300** — the game binds its own outgoing socket there. Localhost (127.0.0.1) works natively.
- FH6-only fields vs Forza Motorsport: `CarGroup`, `SmashableVelDiff`, `SmashableMass` (after `NumCylinders`, before `PositionX`).
- Full struct: @docs/forza-fh6-packet-format.md

## Code map

- `src/packet.rs` — 324-byte packet parse/encode.
- `src/network.rs`, `src/listeners/` — UDP receive + event-driven features: `backfire`, `dsg` (auto gearbox), `perf_test` (accel/decel timers), `power_capture`, `sprint_timer`.
- `src/telemetry.rs` — connection state + packet rate. Session maxima and derived values are held on `ForzaApp`.
- `src/input.rs` — synthetic keypress output (backfire / gearbox drive the game via keys).
- `src/config.rs` — `AppConfig` (serialised to `config.json`), enums, and presets: `apply_preset` / `export_preset` / `import_preset` overlay a subset of keys (`LAYOUT_KEYS` + `MINISETTINGS_KEYS`) onto the live config.
- `src/app.rs` — `ForzaApp`, the top tab bar, and the status-bar **cog "mini-settings" popup** (`page_settings_*`, `DashboardSubTab`) that tunes each dashboard widget.
- `src/ui/` — one module per tab: `dashboard`, `backfire`, `gearbox`, `power_curve`, `engine_swaps`, `coop`, `settings`.
- `src/coop.rs` — Co-Op WebSocket relay over a cloudflared quick tunnel.
- `src/recorder.rs` — packet recording to CSV. `src/engines.rs` — `engines.csv` loader. `src/labels.rs`, `src/icons.rs` — chrome.
- `assets/configs/*.json` — bundled dashboard presets (Ale, Ritze).

Note: `src/ui/acceleration.rs` and `deceleration.rs` are orphaned (not in `ui/mod.rs`, not compiled).

## Tabs (`Tab` in `app.rs`)

- **Dashboard** — a resizable grid of draggable widgets (RPM, speed, gear, inputs, tyres, suspension, G-force, engine, car, race/sprint, boost, power/boost graphs, minimap, co-op players, speed trace, position). Edit mode toggles drag/resize; each widget has options under the cog popup's dashboard sub-tabs.
- **Backfire** — synthetic anti-lag / throttle-blip effect.
- **Automatic Gearbox** — DSG-style auto-shifter with Street/Sport/Race tuning and per-car calibration.
- **Power Curve** — live RPM vs Power/Torque, boost bar chart; captured during full-throttle runs.
- **Engine Swaps** — display-only reference table from `engines.csv` (no automation).
- **Co-Op** — host/join a session via cloudflared tunnel; remote players drawn on the minimap.
- **Settings** — listen port, units, language, FPS limit, presets, connection status, packet rate.

## Domain notes

- **Shift indicator** uses *measured* max RPM (not `EngineMaxRpm`, which is a display limit); calibrated per car. Defaults: 91% low warning / 99% shift.
- **Presets / mini-settings**: a preset is a subset of `AppConfig`. A new mini-setting that should travel with export/presets must be added to `MINISETTINGS_KEYS` in `config.rs`.
- **FPS limiter** renders independently of packet rate.
- MiniMap and Co-Op have deeper design notes in the auto-memory index.

## Not in V3

- Data relay to another IP/port; loopback companion app / .bat scripts (localhost works natively); Android APK web server; Imgur screenshot upload.
