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

The **listener thread** (`src/listeners/worker.rs`, see [[overview]]) owns the matching
`Receiver` and blocks on `recv_timeout(200 ms)`. Per packet it detects a car change (DSG
calibration restore/flush), tracks the dynamic (measured) max RPM, runs
`BackfireListener::update` then `DsgListener::update` — the two listeners that drive the
game — and then hands the packet to the UI through a **capped mailbox**
(`Arc<Mutex<VecDeque<ForzaPacket>>>`, `worker::UI_BACKLOG_CAP` = 200, oldest dropped).
*Why the extra hop:* a minimized or fully occluded window on GNOME/Wayland gets no frame callbacks, so
winit stops calling `eframe::App::update` and anything inside it stops with it; key output
must not depend on redraws. `ForzaApp` keeps the UDP channel's `Sender`, so a port change
(`src/app.rs:ForzaApp::restart_receiver`) only swaps the UDP thread — the listener thread
and its state are untouched.

Every `eframe::App::update` frame calls `self.drain_packets()`, which takes the whole
mailbox in one lock (`ListenerHandle::take_packets`, a `std::mem::take`) and then processes
it **unlocked**. A window hidden for minutes hands back at most 200 packets — replaying
minutes of telemetry through the sprint timer, trace buffer and Co-Op relay is worse than
skipping it, and a plain `mpsc` channel here would also grow without bound (~71 MB/hour)
while nobody drained it. A `sync_channel` was rejected because `try_send` drops the
*newest* packets, which are the only ones the UI wants. For each packet it:

1. Detects a car change (`pkt.car_ordinal` changed) and resets per-car UI state
   (sprint timer, power capture, perf test, session maxima). The gearbox's own per-car
   reset happened on the listener thread, which saw the packet first.
2. Updates session-wide derived state directly on `ForzaApp` — session maxima
   (max power/torque/boost/speed), G-force and suspension-travel stats, wheel-radius
   estimate, speed history/delta, and the ~25 Hz speed/RPM trace buffer.
3. Calls each **UI-side** listener's `update(&pkt, …)` (see table below).
4. Relays the packet to Co-Op (`self.coop.push_local`), then calls
   `self.telemetry.update(pkt)` (`src/telemetry.rs`), which stores `latest`, flips
   `is_connected`, and recomputes `packets_per_sec` once per second of wall-clock
   elapsed.

`self.last_packet_time` drives a 2-second-since-last-packet disconnect check right after
the drain loop.

## The listener pattern

`src/listeners/mod.rs` is just `pub mod` declarations — there is no shared trait or
registry. Each listener is a plain struct driven by an explicit `<listener>.update(&pkt, …)`
call written by hand. The three read-only ones are fields on `ForzaApp`, constructed in
`ForzaApp::new` and called from `drain_packets`; the two that drive the game are owned by
the listener thread and called from `listeners/worker.rs:run`. A listener takes whatever
slice of the packet, `AppConfig`, and shared services (`InputSender`, derived values like
`dynamic_max_rpm`) it needs as arguments, and mutates its own internal state plus (for the
two that act) sends synthetic input.

Because the UI can't reach the two listeners on the other thread, each publishes a
**display-only copy** — `dsg.rs:DsgView` and `backfire.rs:BackfireView`, built by their
`view()` methods and shipped in `worker.rs:ListenerView`. The UI reads `app.dsg` /
`app.backfire`, which hold the same field names, so widget code is unchanged. The pedal
overlay's `sim_step` was made a **free function over just the calibration table**
(`dsg.rs:sim_step`), exposed on both `DsgView` and (test-only) `DsgListener`: it is pure
and display-only, so running it on the UI's copy keeps drawing code from ever mutating —
or even touching — the live listener.

**To add a new listener:**
1. Create `src/listeners/<name>.rs` with a struct holding whatever state it needs across
   packets, a `new()`/`Default`, and an `update(&mut self, pkt: &ForzaPacket, …)` method.
   Gate on `pkt.is_race_on == 0` (return early) — telemetry is only live while actually
   driving.
2. Add `pub mod <name>;` to `src/listeners/mod.rs`.
3. Add a field to `ForzaApp` and initialize it in `ForzaApp::new` (`src/app.rs`).
4. Call `<name>.update(...)` in whatever order relative to the others matters (see the
   backfire-echo-suppression note below): from `drain_packets` (`src/app.rs`) for a
   read-only listener, or from `listeners/worker.rs:run` if it drives synthetic input —
   anything driving the game must not depend on the frame loop.
5. If it needs config, add fields to `AppConfig` (`src/config.rs`) and, if they should
   travel with presets, list them in `MINISETTINGS_KEYS`.

There's no dynamic dispatch or event bus by design — every listener call is a visible,
ordered line in `drain_packets` / `worker.rs:run`, which is what lets later listeners
deliberately react to earlier ones (e.g. DSG suppressing itself during Backfire's
synthetic-input echo).

## The five listeners

| Listener | File | Triggers on | Does |
|---|---|---|---|
| `SprintTimer` | `src/listeners/sprint_timer.rs` | Speed crossing 0/100/200/300/400 km/h thresholds (armed below, timed above) | Records the five FH6 speed-split times (`zero_to_hundred` … `four_to_five`) as `Option<f32>`, using packet timestamps (`ts_diff_secs`, wrapping-safe) rather than wall clock. |
| `PowerCapture` | `src/listeners/power_capture.rs` | `pkt.accel >= 245` (full throttle) while moving (`pkt.speed >= 0.1`) | Buckets `current_engine_rpm` by `step_rpm` and keeps the max power/torque/boost seen per bucket (`upsert_max`), building the live power/torque/boost-vs-RPM curves. Cleared on car change or when brake+handbrake are both held at 100%. See [[power-curve]]. |
| `PerfTest` (`AccelTest`/`DecelTest`) | `src/listeners/perf_test.rs` | Speed entering a configured start→end window (accel: rising through it; decel: falling, with a "dynamic" auto-arm mode) | Times the run with `Instant`, tracks progress (0..1) and instantaneous G, and aborts a decel run on re-acceleration or overshoot. |
| `BackfireListener` | `src/listeners/backfire.rs` | Off-throttle + no-brake + RPM inside a (fixed or dynamic-%-of-redline) window + RPM has moved far enough since the last pop | Emits a synthetic `W` press (see below) to trigger the game's anti-lag/backfire effect; ignores its own echoed-back accel spike via `InputSender::synthetic_active`. See [[backfire]]. |
| `DsgListener` | `src/listeners/dsg.rs` | Continuously calibrates per-gear redline speed from clean, on-throttle samples; once engaged, compares current RPM/gear against the shift-point/cruise-target math | Emits synthetic `E`/`Q` presses to shift up/down, tracked through a `ShiftPhase::{Idle, Shifting}` state machine that waits for the expected gear (or times out and resyncs). See [[gearbox]]. |

Only `BackfireListener` and `DsgListener` drive `InputSender`; the other three are
pure read/derive listeners with no output side effect. That split is exactly the split
across threads. `worker.rs:run` calls Backfire before DSG and threads a
`suppress_gearbox_accel` flag (from `InputSender::synthetic_active`) into DSG's `update`
when `dsg_ignore_backfire_accel` is on, so DSG can ignore the throttle spike Backfire's own
key-press causes to echo back in telemetry. On that thread the window is read with **zero
grace**, because the packet is processed the instant it arrives. The UI reads the same
window through `ForzaApp::backfire_echo_active()`, which *does* add
`backfire::echo_grace(cfg)` — an active FPS limit can delay drawing by up to one frame past
the raw window.

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

**No uinput access (Linux).** The worker builds the virtual device (`Forza Telemetry Input`,
KEY_W/E/Q) in a retry loop and publishes a three-state readiness (`AtomicU8`: pending / ready /
failed; `InputSender::uinput_ready() -> Option<bool>`, `None` = pending). On failure it logs
**once** (`uinput: could not create virtual device: …`), discards queued commands, waits 2 s
(or until `InputSender::recheck()`, sent as `Cmd::Retry` from Setup → Re-check) and tries again;
it exits only when every sender is dropped. While the state isn't *ready* the sender methods drop
their key at once, so nothing queues up and the UI thread can't block on the full channel.
`new()` waits up to 500 ms for the first answer. The Input Permissions "Key input" light and the
permission modal read this readiness (`input::uinput_ok`), not just `open("/dev/uinput")`.
*Why:* the worker used to try once and end, so key sending stayed dead after the user fixed the
permission, while an open-only check could show green with sending dead. See the "What the lights
mean" note in [[hotkeys]].

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

## Blocking IO on the listener thread

The listener thread sits between a packet and the synthetic keypress it may produce, so
blocking work on it directly delays key output. What remains:

- `config::save_car_calibrations` — on car change, on a calibration reset and at shutdown.
  Rare and bounded, so it stays inline.
- **Not** the per-shift CSV row: `dsg.rs:write_shift_log` formats the row and hands it to
  `dsg.rs:shift_log_writer`, a lazily-started thread behind a bounded `sync_channel(64)`
  that does the open/append. Fire-and-forget — a full or dead writer drops the row, because
  logging must never disrupt shifting.
