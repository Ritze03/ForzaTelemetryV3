# Settings — network, units, display

The **Settings** tab (`Tab::Settings`, labelled **Setup** in the tab bar) holds the app-wide
options that aren't tied to a single dashboard widget. Rendered from `src/ui/settings.rs` as
bordered category cards laid out per the [styling guide](../ui/STYLING-GUIDE.md)
(label-left / control-right rows); values persist in `config.json` (see
[[state-and-config]]). Per-widget tuning lives in the cog **Mini-Settings** popup instead —
see [[presets]]. The in-game HUD overlay has its own **Overlay** tab — see [[overlay]].

Cards: **Profiles**, **Hotkey** (left column); **Repository / Credits**, **Display**,
**Network**, **Co-Op**, **Window Detection** (right column). Hotkeys and window detection get
their own doc — see [[hotkeys]]; the Profiles card gets its own doc too — see [[profiles]].

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
- **Always on top** — a checkbox and config field (`always_on_top`), but **currently not
  read anywhere**, so it has no effect.

## Hotkey & Window Detection

The **Hotkey** card lists every binding in two groups (Global / In-app), including **Hide
HUD** (default H, the same binding the Overlay tab edits). Click a button and press a key;
**Esc** cancels, **Backspace** clears it to a faint **Not set**. Details in [[hotkeys]].

The **Window Detection** card (formerly **Input**; its duplicate "Window Detection"
sub-heading is gone) holds the focus detection that several features share:

- **Active if** (hotkey gate: Telemetry live / Game window focused); the detection method
  and game window title (with **Detect**), shown when window focus gates something (Active
  if = Game window focused, the input gate, or the overlay's focus-only option); and the
  **Focus check rate**.
- A status dot with three states: green *Game window focused*, **amber** *Game window not
  focused* (a normal waiting state, not an error), red *Focus detection failed*. It shows
  while the detector runs (including whenever the overlay is enabled). The colours are the
  theme's `GOOD` / `WARN` / `DANGER` tokens.
- **Overlay → Only when game window is focused** (`overlay.focus_only`): hides the in-game
  HUD while another window is focused. It lives here rather than on the Overlay tab because
  it uses this card's detection method (D5); the Overlay tab points here.
- **Send Input → Only send inputs when game focused** (the synthetic-input gate).

Full detail in [[hotkeys]].

## Save

Settings save automatically (on change and on exit) — there is no Save button.
