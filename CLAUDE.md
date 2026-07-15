# CLAUDE.md

Guidance for Claude Code in this repository. Kept lean on purpose — the detail lives in
`docs/`, and this file is the entry map. Follow the links below rather than grepping blind.

## Project overview

**ForzaTelemetryV3** — a Rust desktop app that receives Forza Horizon 6 UDP telemetry and
shows it in a live dashboard.

- **Rust** / **Cargo**; GUI is **egui** (immediate-mode) via **eframe**.
- Single dark "Graphite" theme in `src/theme.rs` — reference chrome colours by role token
  (`ACCENT`, `PANEL`, `TEXT_DIM`…), never hard-code hex at call sites. Semantic data colours
  (tyre temps, input bars) live at their call sites. Styling rules: @docs/ui/STYLING-GUIDE.md.
- All user-facing strings go through `tr("...")` in `src/i18n.rs` (English source → German).
  Add the English key + German value there; duplicate keys warn at compile time.

## Where to look (read before grepping)

- **@docs/README.md** — the docs table of contents: what each subfolder (`architecture/`,
  `features/`, `protocol/`, `ui/`, `meta/`, `claude-instructions/`) holds, with links to every page.
- **@docs/architecture/overview.md** — the codebase navigation map: threading model, data
  flow (UDP → parse → state → UI), full module map, and a "where to look for X" cheat-sheet.
  **Start here to navigate the code.**
- Packet / protocol wire format — @docs/protocol/forza-fh6-packet-format.md
- Terminology (use these meanings, ask before acting on an undefined term, keep it current) —
  @docs/meta/TERMINOLOGY.md
- Per-tab feature behaviour — `docs/features/` (listed in the TOC)
- Domain notes & project scope (shift indicator, presets rule, what's deliberately not in
  V3) — @docs/meta/project-notes.md

## Mandatory working rules

- **Sub-agent orchestration** — when/how to fan out, partition edits, worktree isolation,
  model/effort selection, the 4-agent cap: @docs/claude-instructions/sub-agent-orchestration.md.
  Read it before dispatching any sub-agent work.
- **Changelog — required.** Every user-facing change **must** get a bullet in the top
  `## [version]` section of the repo-root `CHANGELOG.md`, added in the same commit as the
  change. Don't skip it. Format and when-to-bump-the-version rules:
  @docs/claude-instructions/changelog.md.
