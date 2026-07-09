# Co-Op — shared telemetry over cloudflared quick tunnels

Players share live telemetry and see each other on the Dashboard minimap. No login,
no port-forwarding: one player **Hosts**, others **Join** with a short word-code.

## How it works

- The host runs a local WebSocket server (`ws://localhost:<coop_port>`, default 7071)
  and launches a **cloudflared quick tunnel** pointing at it. The tunnel gives a public
  `https://<slug>.trycloudflare.com` URL; the app shows only the `<slug>` word-code
  (e.g. `payment-amount-sample-ver`).
- Guests type that word-code and connect over `wss://<slug>.trycloudflare.com`.
- Each player's raw 324-byte FH6 packet is relayed to everyone in **binary** WebSocket
  frames (prefixed with a 16-byte sender UUID) — minimal bandwidth. Names/colours travel
  as small JSON control messages.
- The host is authoritative: it assigns every player a random UUID and owns the roster,
  so **duplicate names are fine** — identity is the UUID.

## Using it

1. Open the **Co-Op** tab. Set your **Name** and **Colour** (hue).
2. **Host Session** → wait for "Tunnel ready" → share the word-code (Copy button, or
   select it and copy by hand).
3. Others paste/type the code and press **Join**.
4. On the Dashboard minimap, remote players appear as coloured arrows with their name;
   your own arrow uses your colour (no name). Off-screen teammates show as a coloured
   marker clamped to the map edge with the distance to them. Each player also leaves a
   fading breadcrumb trail in their colour.
5. Optional dashboard widgets (drag them in via Edit Mode): **Co-Op Players** — a live
   speed-bar leaderboard of everyone in the session. The status bar also shows your role
   and the player count from any tab.
6. **Waypoints**: left-click the minimap to drop a shared waypoint everyone sees (a
   diamond in your colour, showing each player's distance to it) — handy for "meet here".
   Right-click clears it.

## Options

- **Buffer (ms)** — jitter buffer that delays remote players slightly for smoother pacing.
  0 = lowest latency; raise it if other cars stutter.
- **coop_port** — local port the tunnel points at (config only).

## cloudflared

The app uses `cloudflared` from its data dir (`app_data_dir()/cloudflared`), downloading
it via curl/wget if missing. If neither the binary nor a downloader is available, the host
still runs on the LAN (`ws://<host-ip>:<port>`) — paste that as the join code instead.

## Testing without the game

`python3 tools/sim.py --port <listen_port> --scenario circle` emits synthetic packets.
Run two app instances with `FORZA_DATA_DIR` pointing at separate data dirs and two sims on
different ports/phases to exercise co-op locally.
