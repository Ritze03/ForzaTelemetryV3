# Settings — network, units, display, recording

The **Settings** tab holds the app-wide options that aren't tied to a single
dashboard widget. Rendered from `src/ui/settings.rs`; values persist in
`config.json` (see [[state-and-config]]). Per-widget tuning lives in the cog
**Mini-Settings** popup instead — see [[presets]].

## Network

- **Listen port** — the UDP port the app binds to receive FH6 telemetry. Type a
  new port and press **Apply** to rebind the receiver. Configure the game to send
  to this port under **SETTINGS > HUD AND GAMEPLAY > Data Out**.
- Avoid ports **5200–5300** — the game binds its own outgoing socket there.

## Co-Op

- **Host port** — the local port the cloudflared tunnel points at when you host.
  Change only if it clashes with another app. See [[coop]].

## Display

- **Language** — English or German (all strings go through `tr(...)`; see
  [[ui-architecture]]).
- **Speed unit**, **Tire temp unit**, **Boost / pressure unit** — pick the units
  used across every readout. The boost/pressure toggle also drives the
  [[power-curve]] boost axis and other boost readouts.
- **FPS limit** — cap the render rate independently of the packet rate (the
  limiter uses `request_repaint_after`, so it renders at most this often even
  though telemetry still arrives at ~60 Hz).
- **Always on top** — keep the window above other windows.

## Recording

- **Record / Stop Recording** — capture live telemetry to a recording file; the
  status bar shows a REC indicator while active.
- **Export CSV** — export a saved recording to CSV for analysis.
- **Delete** — remove a recording.

There is **no in-app playback** — recordings are for offline analysis. Full
detail in [[recording]].

## Save

Settings save when you press **Save Settings** and are also auto-saved on exit.
