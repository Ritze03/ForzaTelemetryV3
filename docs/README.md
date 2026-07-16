# ForzaTelemetryV3 — Documentation

Docs for the FH6 UDP telemetry dashboard. Start with **[Architecture Overview](architecture/overview.md)**
to navigate the codebase, or jump to a feature below.

Docs cross-link with `[[wikilinks]]` (Obsidian) and relative paths (GitHub). The
authoritative project rules live in the repo-root `CLAUDE.md`; this folder is the
reference behind it.

## architecture/ — how the code fits together

The navigation reference — read these to move around the source efficiently.

- [Overview](architecture/overview.md) — big picture, threading model, data flow (UDP → parse → state → UI), the frame loop, a full module map, and a "where to look for X" cheat-sheet. **Start here.**
- [Networking & Listeners](architecture/networking.md) — the UDP receive path, the hand-wired listener pattern, the 5 event listeners, and the synthetic-keypress output.
- [State & Configuration](architecture/state-and-config.md) — where runtime state lives, the `config.json` load→mutate→save lifecycle, the serde migration gotcha, and `LAYOUT_KEYS` vs `MINISETTINGS_KEYS`.
- [UI Architecture](architecture/ui-architecture.md) — tab structure & dispatch, the Graphite theme + helpers, the mini-settings popup, and the i18n flow.

## features/ — what the app does

- [Dashboard](features/dashboard.md) — the draggable widget grid, Edit Mode, and every available widget.
- [Minimap](features/minimap.md) — seasonal map rendering, north-up/heading-up, trails, waypoints.
- [Co-Op](features/coop.md) — shared telemetry over a cloudflared tunnel; remote players on the map.
- [Backfire](features/backfire.md) — synthetic anti-lag / throttle-blip.
- [Automatic Gearbox](features/gearbox.md) — DSG-style auto-shifter with per-car calibration.
- [Power Curve](features/power-curve.md) — live RPM vs power/torque, captured on full-throttle runs.
- [Engine Swaps](features/engine-swaps.md) — display-only reference table from `engines.csv`.
- [Presets & Mini-Settings](features/presets.md) — the config-overlay mechanism and bundled presets.
- [Settings](features/settings.md) — network, units, display (the Settings tab).

## protocol/ — the wire format

- [FH6 Data Out Packet Format](protocol/forza-fh6-packet-format.md) — the 324-byte little-endian struct, field by field.

## ui/ — how to build consistent UI

- [Styling Guide](ui/STYLING-GUIDE.md) — cards/categories, control rows, reserved spinner widths, the `theme::` helpers.

## claude-instructions/ — mandatory rules for agents

- [Working with the Docs](claude-instructions/documentation.md) — read before you touch, update after you change, record the *why*, and the `@` force-load convention. Linked from the repo-root `CLAUDE.md`.
- [Sub-Agent Orchestration](claude-instructions/sub-agent-orchestration.md) — when to fan out sub-agents, how to partition edits, worktree isolation, model/effort selection, the 4-agent cap. Linked from the repo-root `CLAUDE.md`.
- [Changelog Maintenance](claude-instructions/changelog.md) — how to keep the user-facing `CHANGELOG.md` current and when to bump the version.

## meta/

- [Terminology](meta/TERMINOLOGY.md) — project vocabulary. Keep it current as terms appear.
- [Project Notes](meta/project-notes.md) — cross-cutting domain facts (shift indicator, presets rule, FPS limiter) and what's deliberately not in V3.
- [What's New](meta/whats-new.md) — the `unattended-testing` branch feature summary. (User-facing release notes are in the repo-root `CHANGELOG.md`.)
