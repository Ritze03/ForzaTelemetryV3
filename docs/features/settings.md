# Settings — network, units, display

The **Settings** tab holds the app-wide options that aren't tied to a single
dashboard widget. Rendered from `src/ui/settings.rs` as bordered category cards
laid out per the [styling guide](../ui/STYLING-GUIDE.md) (label-left / control-right
rows); values persist in `config.json` (see [[state-and-config]]). Per-widget tuning
lives in the cog **Mini-Settings** popup instead — see [[presets]].

Cards: **Profiles**, **Network**, **Co-Op**, **Display** (left column); **Hotkey**,
**Input**, **Repository** (right column). Hotkeys and window/input detection get their
own doc — see [[hotkeys]]; the Profiles card gets its own doc too — see [[profiles]].

## Profiles

First card in the left column. Switch between named full-config snapshots, and create /
duplicate / rename / delete them, plus selectively export/import settings by group. Save
is continuous (the live config mirrors the active profile on every change), so there's no
Save button and switching always persists the outgoing profile. Full detail in
[[profiles]].

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

## Hotkey & Input

The **Hotkey** and **Input** cards configure rebindable shortcuts and window-focus
detection — documented in full in [[hotkeys]].

## Save

Settings save automatically (on change and on exit) — there is no Save button.
