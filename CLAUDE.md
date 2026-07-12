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

## Terminology

Project vocabulary is defined in @docs/TERMINOLOGY.md. Use those meanings, ask before acting on an undefined non-standard term, and keep that file updated as terms are introduced.

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

## Sub-Agent Orchestration

This project uses parallel sub-agents by default to avoid unnecessary waiting.
Follow these rules for every new request:

### Core Rule
If a new user request arrives while a task is already running
(or if a request contains multiple independent subtasks):
- Immediately start a separate sub-agent via the Task tool for each
  independent subtask, instead of working sequentially.
- Do NOT wait on your own initiative until the current task is finished
  before accepting the new one — first check whether a dependency exists.

### When to Dispatch in Parallel (all conditions must be met)
- The new task does not need the result of the currently running task.
- There is no shared state (e.g. the same file being edited).
- The task is clearly scoped and self-contained.

→ In this case: spawn a new sub-agent in parallel (Task tool), keep the
main thread free for further input.

### When to Work Sequentially / Wait (any one condition applies)
- The new task depends on the result of the currently running task.
- Both tasks touch the same files or the same code section
  (merge conflict risk).
- The scope of the new task is unclear — clarify or briefly analyze
  before delegating.

→ In this case: give a short heads-up ("waiting on X before starting Y")
and only start / attach afterward.

### Background Dispatch
- Research, analysis, and read-only tasks (no file changes) should
  generally be started as background agents so the main thread doesn't block.
- File-modifying tasks (edit/write) should only be run in parallel if they
  touch different, clearly separated directories/files.

### Isolation for Parallel File Changes
- Sub-agents that modify code while running in parallel with others use
  `isolation: worktree` so agents don't overwrite each other.
- Each agent commits its changes to its own worktree branch.

### Reporting
- Each sub-agent returns a short, concise summary at the end
  (no raw logs, no intermediate steps).
- When multiple sub-agents run in parallel, consolidate their results
  into a single overview in the main thread at the end.
- Briefly state what each spawned sub-agent is responsible for
  (e.g. "Sub-agent A: Auth research" / "Sub-agent B: DB layer refactor").

### Model and Effort Selection for Sub-Agents (dynamic)

The main agent decides independently which model and which effort level
are appropriate for a task. The following values are guidelines, not fixed
rules — deviate from them whenever the task calls for it.

**Available models:** Sonnet, Opus, Ultracode
**Available effort levels:** Low, Medium, High, XHigh

#### Guidelines for Estimation
- **Small, simple adjustments** (e.g. value changes, simple formatting,
  trivial config edits) → Sonnet, Medium
- **Tasks involving logic** (e.g. new features, business logic,
  non-trivial refactoring) → Opus, High/XHigh
- **Bug fixing** → ALWAYS Opus, at least High, XHigh for complex/unclear bugs
- **Very complex or critical tasks** (e.g. architecture decisions,
  security-relevant code, hard-to-trace bugs) → Ultracode, XHigh if needed

This mapping is guidance only. The main agent should assess the actual
complexity of each subtask itself and choose model/effort accordingly —
even if that means deviating from the guidelines (e.g. Opus for a
seemingly simple but sensitive task).

#### User Override
- If I (the user) explicitly name a model or effort level
  (e.g. "use Sonnet for this" or "with low effort"), that ALWAYS takes
  precedence over the automatic estimation — switch immediately.
- Without explicit input from me, the main agent decides independently
  based on the guidelines above.

### Limits
- Sub-agents cannot spawn further sub-agents themselves (no nested
  delegation). For multi-step workflows, let the main thread coordinate
  instead of nesting deeply.
- Maximum of 4 parallel sub-agents at once, to keep token cost and
  rate limits in check.
