# Co-Op — shared telemetry (Cloudflare tunnel or Trystero P2P)

Players share live telemetry and see each other on the Dashboard minimap. No login,
no port-forwarding. Two transports, chosen with the `[Trystero | Cloudflare]` pill control at
the top of the Session card (`theme::segmented`, locked while a session is live; config
`coop_transport`, default Cloudflare):

- **Cloudflare** — one player **Hosts**, others **Join** with a short word-code (below).
- **Trystero** — everyone joins the same **Room ID**; direct peer-to-peer mesh (see
  [Trystero transport](#trystero-transport)).

## How it works

- The host runs a local WebSocket server (`ws://localhost:<coop_port>`, default 7071)
  and launches a **cloudflared quick tunnel** pointing at it. The tunnel gives a public
  `https://<slug>.trycloudflare.com` URL; the app shows only the `<slug>` word-code
  (e.g. `payment-amount-sample-ver`).
- Guests type that word-code and connect over `wss://<slug>.trycloudflare.com`.
- Each player's raw 324-byte FH6 packet is relayed to everyone in **binary** WebSocket
  frames (prefixed with a 16-byte sender UUID) — minimal bandwidth. Names/colours travel
  as small JSON control messages.
- **Who sends:** the listener thread (`listeners/worker.rs`), on every received packet, via
  a `CoopReader` handle (`CoopState::reader()`) passed to `worker::spawn`. While paused, the
  game zeroes car class/PI, so `coop::outgoing` substitutes the last race-on values.
  **Why not the UI thread:** egui stops calling `update` while the game window covers the
  app — i.e. the whole time you're driving — so a send from `drain_packets` meant peers saw
  us frozen. The shared `Inner` is never replaced, so the one handle stays valid.
- The host is authoritative: it assigns every player a random UUID and owns the roster,
  so **duplicate names are fine** — identity is the UUID.

## Using it

1. Open the **Co-Op** tab. Set your **Player name** and **Player color** (hue).
2. **Host Session** → wait for "Tunnel ready" → share the word-code (Copy button, or
   select it and copy by hand).
3. Others paste/type the code and press **Join**.
4. On the Dashboard minimap, remote players appear as coloured arrows with their name;
   your own arrow uses your colour (no name). Off-screen teammates show as a coloured
   marker clamped to the map edge with the distance to them. Each player also leaves a
   fading breadcrumb trail in their colour.
5. Optional dashboard widgets (drag them in via Edit Mode): **Co-Op Players** — a live
   speed-bar leaderboard of everyone in the session. The status bar also shows your role
   and the player count from any tab — yellow (`WARN`) while the session is connecting
   (Trystero: only while the Co-Op page shows "Negotiating…", i.e. a peer link is
   mid-handshake and the player count is still <=1; "Connecting to relays…", "Reconnecting…",
   "Waiting for players…" and "N player(s)" are all green. Cloudflare: host tunnel not ready /
   client socket not open or reconnecting), green otherwise (`CoopState::is_connecting`,
   a `connecting` flag in `Inner`). With *Status bar: show text labels* off it shrinks to the
   players icon + count, and the hover tooltip spells out role · count · status.
   *Why:* colour carries the state so icon-only mode stays readable. For Trystero, yellow
   means the co-op page is actually negotiating, not the background relay/handshake churn.
   *Limitation:* a Cloudflare **host** has no handshake phase for a newly joining client, so
   its indicator is green as soon as the tunnel is up (or the LAN-only fallback is).
6. **Waypoints**: left-click the minimap to drop a shared waypoint everyone sees (a
   diamond in your colour, showing each player's distance to it) — handy for "meet here".
   Right-click clears it.

## Trystero transport

Pick **Trystero**, type a **Room ID** (or press **Generate**: lowercase Crockford base32, 8 groups of 4 = 32 chars / 160 bits,
e.g. `k7f2-9qzm-x4pd-...`; older 12-char IDs still work, `normalize_room` is unchanged) and press **Join Room**; everyone using the same ID ends up in one
session. While connected the card shows "Room" + ID with Copy. The ID is persisted as
`coop_room`; it is normalised (`normalize_room`: lowercase, all whitespace stripped) in
`start_trystero`, so the UI and auto-connect agree on the room (it is hashed into the topics and
key, so any difference is a different room). *Why 32 chars:* generated IDs should be >= 32 chars so shared public rooms practically never collide (the ID is also the encryption secret). Enter in the Room ID field joins. The field fills the card width (no char limit, hint shows a full-length example); the shown room code in the session card drops to 12 px for IDs over 24 chars so all 39 chars (incl. dashes) fit beside Copy. Hint shown: "Anyone with this ID can join. Treat it like a password."
**Auto-connect on startup** (`coop_autoconnect`, Trystero only) rejoins the last room at launch
(`ForzaApp::new` in `app.rs`, right after `CoopState::new`). **Why Trystero only:** Cloudflare
slugs are random per host session, so auto-rejoin would nearly always fail.
`coop_transport`, `coop_room`, `coop_autoconnect` are in `COOP_KEYS` (profile export).

Code: `src/coop.rs` (`start_trystero`, `generate_room_id`), `src/coop/{nostr,rtc,mesh}.rs`.

- **Model follows [Trystero](https://github.com/dmotz/trystero)** (JS lib): peers meet through
  a shared room ID on public **Nostr relays**, then connect directly over **WebRTC data
  channels**. **Not wire-compatible** with JS Trystero clients. Why: its data-plane framing is a
  moving target and co-op is app-to-app; topics, event kinds and event format follow Trystero so
  interop stays possible later. appId `ForzaTelemetryV3`.
- **Signaling (`nostr.rs`)**: topic = SHA-1 digest rendered byte-wise base36 of
  `Trystero@ForzaTelemetryV3@<room>` (root) and that + `@<selfId>` (per peer); event kind =
  (sum of UTF-16 units % 10000) + 20000 (ephemeral range, relays don't store). The REQ uses
  `since = now - 30 s` (`SINCE_SLACK_SECS`). Why: ephemeral events aren't stored, so the slack only
  forgives clock skew that would otherwise hide a peer's first announces. Events are NIP-01,
  BIP-340 schnorr-signed (`k256`) with a per-session key. Peers announce `{peerId, nonce}` to the
  root topic at 0.2/0.5/1.3/5.3 s, then every ~30 s; offers/answers go to the target's topic.
  SDP is AES-GCM encrypted with key SHA-256(`:ForzaTelemetryV3:<room>`) (12-byte IV) — the room
  ID is effectively the secret, and this hides IPs from relay operators. Glare (both offer): the
  lower selfId keeps its offer. De-dup by (peerId, offerId). Non-trickle ICE.
- **Relays**: 6 hard-coded (nos.lol, nostr.mad-social.net, nostr.purpura.cloud,
  nostr.stakey.net, relay.mappingbitcoin.com, offchain.pub). Why hard-coded: both peers must
  share the list. One std thread per relay; connects via `connect_relay` (5 s TCP connect timeout
  per address, 2 s write timeout), run on a helper thread polled against the stop flag because DNS
  can't be bounded. Why: blackholed relays kept threads alive long after Leave (the Cloudflare
  `connect_ws` has no timeouts). Rate-limited -> backoff;
  blocked/restricted/auth-required/pow -> relay retired.
- **WebRTC (`rtc.rs`)**: webrtc-rs 0.21 with `runtime-smol` + `crypto-ring`, on one dedicated
  thread (smol `LocalExecutor`). Why no tokio: the app is std-threads; keeps the build light and
  needs no C toolchain. STUN: Google (stun, stun1) and Cloudflare. ICE gathering capped at 8 s;
  offer/answer timeout 57 s; channel must open within 30 s (`CONNECT_TTL`) else the link fails as
  NAT; 12 s grace on Disconnected; max 24 peers. **Telemetry drop policy:** the channel is
  reliable+ordered, so the send buffer is capped at 16 KiB (`SEND_BUFFER_LIMIT`), Binary frames go
  via `try_send` and are dropped on `ErrSendBufferFull`, and the per-peer out queue holds 64 frames
  (`OUT_QUEUE`) -> worst-case staleness ~1 s. Why: a 1 MiB buffer meant ~50 s of stale backlog on a
  bad link. A second unordered/no-retransmit channel for telemetry would be the better long-term
  design (deferred: needs a second queue type and channel pairing).
- **Topology: full mesh**, not a host star. Why: Trystero is a mesh; no host election/failover.
  Each peer sends its own telemetry to every peer (~20 KB/s per link). All Trystero peers use
  `Role::Client` (no new Role variant). Why: the UI matches exhaustively on `Role`, and a mesh has
  no host, so "Joined" is truthful. `inner.clients` is keyed by the remote's Nostr peer id; a new
  additive `Control::Peer {id,name,hue}` sent on channel open binds the player UUID to the channel
  — binary frames are attributed to the bound id and the 16-byte prefix is ignored (anti-spoof).
  Binding is **latest channel wins**: a `Peer` claiming an id already bound to another channel
  evicts the old binding (its queue is dropped, stale waypoint cleared; roster/jitter buffer
  kept). Own-id and malformed-id claims are still refused. Why: `Peer` is sent once per channel,
  and a rejoining player can appear on a new channel before we noticed the old link die, so
  refusing would leave them invisible. Tradeoff: a room member could take over another player's
  id — acceptable, room members already hold the room secret. Each Trystero join also generates
  a **fresh player UUID** (`begin_trystero`). Why: rejoining with the old UUID but a new Nostr
  selfId could be rejected by peers that hadn't noticed the old link die. Old Cloudflare peers
  ignore the message. Roster = self + connected peers.
- **Status/errors**: "Connecting to relays…", "Waiting for players…", "Negotiating…",
  "{n} player(s)", "Reconnecting…", "Stopped"; errors "No relay reachable" and "Couldn't reach a
  player directly (NAT). Try the Cloudflare option." The NAT error shows only while no peer is
  Live and is cleared when a link opens; glare-killed links (stale generation) never flag it. An
  answerer-side `CONNECT_TTL` timeout still counts as NAT (webrtc 0.21 can't distinguish "the
  offerer left").
- Threads are detached with a per-session stop flag; `stop()` never blocks the UI.
- **Limitation**: two peers both behind symmetric NAT/CGNAT can't connect (no free TURN);
  use Cloudflare as the fallback. **Untested against live relays / real NAT so far** — only unit,
  in-process loopback and fake-relay tests.
- **Toolchain**: `rust-toolchain.toml` pins 1.96.1 and Cargo.toml has `rust-version = "1.91"`.
  Why: webrtc 0.21 -> rtc-mdns needs Rust >= 1.91.

## On the in-game HUD overlay

With **Show co-op teammates** on (Overlay tab → Minimap, default on), teammates also appear on
the HUD overlay's minimap ([[overlay]]) as arrows in their identity colour with their name,
while they're inside the map pill. Paused teammates are skipped there (their packet sits at
the world origin; the last-known spot is UI-side state). No trails, waypoints or edge markers
on the HUD.

- The overlay thread reads them through a `CoopReader` (`OverlayOptions::coop`), not through
  the HUD snapshot and not through the UI thread.
- **Why the overlay advances the jitter buffers itself:** `CoopReader::remote_players` calls
  the same `tick` the UI runs each frame before reading. The UI's `coop.tick()` stops while
  the game covers the window, which is exactly when the HUD is on screen, so without this the
  teammates would freeze. The advance is time-based (`now − buffer_ms`), so two callers
  (UI and overlay) are harmless.

## Options

(Cloudflare transport unless noted.)

- **Packet Buffer Size (ms)** — jitter buffer that delays remote players slightly for smoother pacing.
  0 = lowest latency; raise it if other cars stutter.
- **Host port** (`coop_port`, default 7071) — local port the cloudflared tunnel points at.
  A **Cloudflare** card directly under the Session card, shown only while the Cloudflare
  transport is selected; change it only if it clashes with another app (the hint is a
  tooltip). *Why here (was Setup → Co-Op):* it only concerns the Cloudflare transport
  (`start_host(coop_port, …)` is the tunnel's local WebSocket server; Trystero never uses it).

## Bandwidth

Each player streams one raw 340-byte frame (16-byte sender UUID + 324-byte packet)
at the game's ~60 Hz, so **~20 KB/s upload per player**. Measured on a 2-player
loopback session: ~40 KB/s total (both directions). The host relays, so its usage
scales with player count, but it stays modest — a 4-player host is on the order of
~180 KB/s up. Comfortable even on slow connections.

## cloudflared

The app uses `cloudflared` from its data dir (`app_data_dir()/cloudflared`), downloading
it via curl/wget if missing. If neither the binary nor a downloader is available, the host
still runs on the LAN (`ws://<host-ip>:<port>`) — paste that as the join code instead.

## Testing without the game

`python3 tools/sim.py --port <listen_port> --scenario circle` emits synthetic packets.
Run two app instances with `FORZA_DATA_DIR` pointing at separate data dirs and two sims on
different ports/phases to exercise co-op locally.
