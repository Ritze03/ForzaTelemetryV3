# What's New — `unattended-testing` branch

A summary of everything added on this branch. Built and screenshot-verified end
to end (co-op tested over both LAN and a real trycloudflare tunnel, with 2 and 3
players; recording verified by capturing a run and replaying it with the live
feed off). No game required — a synthetic packet sender (`tools/sim.py`) drives it.

## 1. Co-Op — shared telemetry (highest priority)

See **docs/coop.md** for full usage. In short:

- **Host** starts a local WebSocket relay and a cloudflared quick tunnel; shares
  a short **word-code** (e.g. `blue-fox-rapid-owl`) — copy button + selectable,
  plus a **LAN URL** for lower-latency same-network play.
- **Join** by typing the word-code (or a `ws://host:port` for LAN).
- Set your **name** and **colour** (hue). Duplicate names are fine — the host
  assigns each player a UUID.
- Raw 324-byte packets are relayed (low bandwidth); a **jitter buffer** (ms)
  smooths pacing.
- On the **Dashboard minimap**: every player shows as a coloured arrow with their
  name (yours is colour-only), a fading **breadcrumb trail**, and **edge markers
  with distance** for teammates who are off-screen. Left-click drops a **shared
  waypoint** ("meet here") everyone sees; right-click clears it.
- Robustness: connect-retry for tunnel propagation, **auto-reconnect** on
  mid-session drops, bounded send queues, LAN fallback, port setting in Settings.
- A **status-bar indicator** shows your role + player count from any tab.

## 2. Ritz "Graphite" theme (high priority)

Adopted the look from the Ritz launcher: a role-based colour system in
`src/theme.rs`, applied once at startup. UPPERCASE accent **section labels**
across the Dashboard and Settings, darker header/footer chrome, and
primary/danger/secondary button styles.

## 3. Dashboard data-viz (medium priority)

New draggable widgets (add them via **Edit Mode** on the Dashboard; toggle in the
Modules sub-tab):

- **Co-Op Players** — live speed-bar leaderboard with distance to each teammate.
- **Speed Trace** — rolling speed + RPM sparkline (~30 s).
- **Boost** — turbo/supercharger gauge with a session-peak tick.
- **Session Stats** — top speed, peak power/torque/boost, peak G, max RPM.

Plus the G-force meter gained a fading **traction-circle trail**.

## 4. Telemetry recording & replay

In **Settings → Recording**:

- **Record** live telemetry to `recordings/*.ftr` (status-bar REC indicator).
- **Replay** a file — it streams back over UDP so the whole dashboard plays as if
  live (with a **Loop** option).
- **Export CSV** — 24 fields per packet, for analysis in a spreadsheet / pandas.

## Testing without the game

`python3 tools/sim.py --port <listen_port> --scenario circle|figure8|accel|idle`
emits valid FH6 packets at 60 Hz. Point the app at a throwaway data dir with the
`FORZA_DATA_DIR` env var so your real config is never touched.
