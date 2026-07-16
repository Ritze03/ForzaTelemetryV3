# Networking & Listeners

How a UDP packet from Forza Horizon 6 becomes dashboard state, and how the app talks
back to the game via synthetic keypresses. See [[forza-fh6-packet-format]] for the wire
format, [[overview]] for where `ForzaApp` fits, and [[state-and-config]] for `AppConfig`.

## Receive path: socket → parse → dispatch

`network::start_receiver` (`src/network.rs:18`) spawns a dedicated thread that owns the
`UdpSocket`, bound to `0.0.0.0:<port>` with a 200 ms read timeout so it can poll a
`stop_flag` and exit cleanly. Every datagram is handed to `ForzaPacket::from_bytes`
(`src/packet.rs:122`), which sanity-checks the length (`>= 232` bytes) and parses the
fixed little-endian layout field-by-field via small `ri32!`/`ru32!`/`rf32!`/`ru16!`
macros over a `Cursor`. A successful parse is pushed through an `mpsc::Sender<ForzaPacket>`
into the app; a short/garbled datagram is silently dropped. `NetworkHandle`
(`src/network.rs:8`) is just the `stop_flag` handle — dropping it (e.g. on port change)
signals the thread to stop.

`ForzaApp` owns the matching `Receiver` and creates the pair in `restart_receiver`
(`src/app.rs:794`), used both at startup and whenever the listen port changes.

Every `eframe::App::update` frame (`src/app.rs:1167`) calls `self.drain_packets()`
(`src/app.rs:824`), which drains up to 200 queued packets per frame with
`self.receiver.try_recv()` — bounding worst-case per-frame work if the UI briefly falls
behind the packet rate. For each packet it:

1. Detects a car change (`pkt.car_ordinal` changed) and resets per-car listener state
   (sprint timer, power capture, perf test, DSG calibration, session maxima).
2. Updates session-wide derived state directly on `ForzaApp` — session maxima
   (max power/torque/boost/speed), the dynamic (measured) max RPM, G-force and
   suspension-travel stats, wheel-radius estimate, speed history/delta, and the
   ~25 Hz speed/RPM trace buffer.
3. Calls each listener's `update(&pkt, …)` (see table below).
4. Relays the packet to Co-Op (`self.coop.push_local`), then calls
   `self.telemetry.update(pkt)` (`src/telemetry.rs`), which stores `latest`, flips
   `is_connected`, and recomputes `packets_per_sec` once per second of wall-clock
   elapsed.

`self.last_packet_time` drives a 2-second-since-last-packet disconnect check right after
the drain loop.

## The listener pattern

`src/listeners/mod.rs` is just five `pub mod` declarations — there is no shared trait or
registry. Each listener is a plain struct owned as a field on `ForzaApp`
(`src/app.rs:511-517`), constructed once in `ForzaApp::new`, and driven by an explicit
`self.<listener>.update(&pkt, …)` call written by hand inside `drain_packets`. A listener
takes whatever slice of the packet, `AppConfig`, and shared services (`InputSender`,
derived values like `dynamic_max_rpm`) it needs as arguments, and mutates its own
internal state plus (for the two that act) sends synthetic input.

**To add a new listener:**
1. Create `src/listeners/<name>.rs` with a struct holding whatever state it needs across
   packets, a `new()`/`Default`, and an `update(&mut self, pkt: &ForzaPacket, …)` method.
   Gate on `pkt.is_race_on == 0` (return early) — telemetry is only live while actually
   driving.
2. Add `pub mod <name>;` to `src/listeners/mod.rs`.
3. Add a field to `ForzaApp` and initialize it in `ForzaApp::new` (`src/app.rs`).
4. Call `self.<name>.update(...)` from `drain_packets` (`src/app.rs:824`), in whatever
   order relative to the others matters (see the backfire-echo-suppression note below).
5. If it needs config, add fields to `AppConfig` (`src/config.rs`) and, if they should
   travel with presets, list them in `MINISETTINGS_KEYS`.

There's no dynamic dispatch or event bus by design — every listener call is a visible,
ordered line in `drain_packets`, which is what lets later listeners deliberately react to
earlier ones (e.g. DSG suppressing itself during Backfire's synthetic-input echo).

## The five listeners

| Listener | File | Triggers on | Does |
|---|---|---|---|
| `SprintTimer` | `src/listeners/sprint_timer.rs` | Speed crossing 0/100/200/300/400 km/h thresholds (armed below, timed above) | Records the five FH6 speed-split times (`zero_to_hundred` … `four_to_five`) as `Option<f32>`, using packet timestamps (`ts_diff_secs`, wrapping-safe) rather than wall clock. |
| `PowerCapture` | `src/listeners/power_capture.rs` | `pkt.accel >= 245` (full throttle) while moving (`pkt.speed >= 0.1`) | Buckets `current_engine_rpm` by `step_rpm` and keeps the max power/torque/boost seen per bucket (`upsert_max`), building the live power/torque/boost-vs-RPM curves. Cleared on car change or when brake+handbrake are both held at 100%. See [[power-curve]]. |
| `PerfTest` (`AccelTest`/`DecelTest`) | `src/listeners/perf_test.rs` | Speed entering a configured start→end window (accel: rising through it; decel: falling, with a "dynamic" auto-arm mode) | Times the run with `Instant`, tracks progress (0..1) and instantaneous G, and aborts a decel run on re-acceleration or overshoot. |
| `BackfireListener` | `src/listeners/backfire.rs` | Off-throttle + no-brake + RPM inside a (fixed or dynamic-%-of-redline) window + RPM has moved far enough since the last pop | Emits a synthetic `W` press (see below) to trigger the game's anti-lag/backfire effect; ignores its own echoed-back accel spike via `InputSender::synthetic_active`. See [[backfire]]. |
| `DsgListener` | `src/listeners/dsg.rs` | Continuously calibrates per-gear redline speed from clean, on-throttle samples; once engaged, compares current RPM/gear against the shift-point/cruise-target math | Emits synthetic `E`/`Q` presses to shift up/down, tracked through a `ShiftPhase::{Idle, Shifting}` state machine that waits for the expected gear (or times out and resyncs). See [[gearbox]]. |

Only `BackfireListener` and `DsgListener` drive `InputSender`; the other three are
pure read/derive listeners with no output side effect. `drain_packets` calls Backfire
before DSG and threads a `suppress_gearbox_accel` flag (from `backfire_echo_active()`)
into DSG's `update` when `dsg_ignore_backfire_accel` is on, so DSG can ignore the
throttle spike Backfire's own key-press causes to echo back in telemetry.

## Synthetic-input path

`src/input.rs` defines `InputSender`, a per-platform virtual keyboard: `evdev` +
`uinput` on Linux, `enigo` on Windows, a no-op stub elsewhere. Construction spawns a
worker thread owning the virtual device; all senders talk to it over an
`mpsc::sync_channel<Cmd>` so the calling (UI/packet-processing) thread never blocks on
key timing. Commands:

- `Cmd::Key { key, hold_ms, gap_ms, echo_ms }` — press, hold `hold_ms`, release, then
  wait `gap_ms` (so back-to-back queued presses, e.g. a batched multi-gear DSG
  kickdown, land as distinct key events instead of coalescing).
- `Cmd::Hold { key, max_hold_ms, echo_ms }` — press and hold until a later `Release`,
  auto-releasing at `max_hold_ms` as a stuck-key safety if packets stop arriving (used
  by Backfire's packet-based hold mode, released at the top of the next packet).
- `Cmd::Release` — release whatever `Hold` is outstanding.

Four sender methods wrap these: `press` (untracked, fire-and-forget), `press_tracked`/
`hold_tracked` (tracked — see below), and `release`.

**Echo tracking.** Because the synthetic key press makes the game report a fake `accel`
value back in the very telemetry the app is reading, `EchoWindow` (`src/input.rs:11`) —
a shared `Arc<Mutex<Option<Instant>>>` deadline — lets a listener ask "might my own
press still be echoing?" via `InputSender::synthetic_active(grace)`. The window is
opened by the **worker thread at actual key-emission time** (not at enqueue time), so
queue backlog from another press ahead of it can't erode the window, and a dead worker
(no `/dev/uinput` access) never opens a phantom suppression window. `grace` accounts for
an active FPS limiter delaying packet processing by up to one frame interval past the
raw window (`backfire::echo_grace`, `src/listeners/backfire.rs:18`) — every consumer of
the echo window must use that same grace calculation.

Backfire uses `press_tracked`/`hold_tracked` (its press must be filtered out of the
DSG throttle read and out of Power Capture's full-throttle detection). DSG uses plain
untracked `press` for its `E`/`Q` shift presses — its own shifts aren't telemetry that
other listeners need to filter out.
