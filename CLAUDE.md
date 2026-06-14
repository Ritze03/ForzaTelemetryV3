# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project overview

**ForzaTelemetryV3** — a Rust desktop application that receives Forza Horizon 6 UDP telemetry and displays it in a live dashboard.

- Language: **Rust**, build tool: **Cargo**
- GUI: **egui** (immediate-mode)
- Previous versions: V7.1 targeted FH4, V2.1 targeted FH5 (compiled JARs only in `old_versions/`, no source)

## Protocol

FH6 broadcasts a fixed **324-byte UDP packet** at the game's frame rate to a configured IP/port. Traffic is one-way (game sends, never receives). Data is only sent while actively driving — not in menus, pauses, or replays.

**Configure in-game**: SETTINGS > HUD AND GAMEPLAY > Data Out (toggle, IP, port).

**Port constraint**: avoid 5200–5300 (game uses this range for its own outgoing socket). FH6 supports localhost (127.0.0.1) natively.

FH6 has three fields not present in Forza Motorsport: `CarGroup`, `SmashableVelDiff`, `SmashableMass` — inserted after `NumCylinders`, before `PositionX`.

Full packet struct: @docs/forza-fh6-packet-format.md

## UI layout

Tab-based layout (mirrors V2.1 structure), with subtabs where needed. Must look polished and remain responsive — include an FPS limiter.

Tabs to implement:
- **Dashboard** — live RPM/speed/gear/inputs, shift indicator, tire data, suspension
- **Acceleration** — 0–100, 0–200 km/h timers (auto-triggered); configurable range test with progress bar and G-rate
- **Deceleration** — brake-to-stop timing; dynamic mode + manual mode with configurable start/end speed
- **Power Curve** — live RPM vs Power/Torque chart (captured during full-throttle runs); boost bar chart; engine swap info
- **Setup** — listening port, temperature units, theme, shift indicator thresholds, FPS limit, connection status, packet rate

## Features to carry forward

### Core architecture
- **Named value store** — all packet fields accessible by name (like V2's `ValueManager`)
- **Session max tracker** — tracks per-field maximums across the session
- **Event dispatch** — per-packet callbacks + car-change event when `CarOrdinal` changes
- **Per-car persistence** — shift indicator thresholds and custom name stored by `CarOrdinal`

### Shift indicator
Based on **measured** max RPM (not `EngineMaxRpm` from the packet, which is a display limit). Calibrated automatically per car. Default thresholds: 91% (low warning) / 99% (shift) of measured max.

### Performance measurements
- 0–100 and 100–200 km/h timers (auto-triggered on speed crossing)
- Brake-to-stop timer (triggered by decelerating speed + brake input)
- Configurable acceleration/deceleration test (start/end speed, live timer, progress bar)

### Power curve
Live-captured during full-throttle runs. RPM vs Power + Torque series. Boost bar chart (boost per RPM step). Configurable RPM step size. Engine swap table loaded from `engines.csv` — display-only reference (columns: engine_label, source_vehicle, engine_name, horsepower), no automation.

### Display data
- Speed (km/h / mph toggle), RPM, gear, inputs (accel/brake/clutch/handbrake/steer)
- Tire temps, tire slip, suspension travel, G-forces
- Lap times (current / last / best), race position, fuel level
- Car info: class, PI, drivetrain, cylinders

### App features
- Always-on-top / overlay mode
- Temperature unit toggle (°C / °F)
- Multiple themes
- RAM usage display
- Packet rate (pps / effective fps) and connection status
- FPS limiter (configurable render rate independent of packet rate)

## What is NOT in V3
- Data relay to another IP/port
- Loopback companion app or .bat scripts (FH6 supports localhost natively)
- Android APK web server
- Screenshot upload to Imgur
