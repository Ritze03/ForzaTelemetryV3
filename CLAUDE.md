# CLAUDE.md

Guidance for Claude Code in this repository. Kept lean on purpose — the detail lives in
`docs/`, and this file is the entry map.

## Project overview

**ForzaTelemetryV3** — a Rust desktop app that receives Forza Horizon 6 UDP telemetry and
shows it in a live dashboard.

- **Rust** / **Cargo**; GUI is **egui** (immediate-mode) via **eframe**.
- Single dark "Graphite" theme in `src/theme.rs` — reference chrome colours by role token
  (`ACCENT`, `PANEL`, `TEXT_DIM`…), never hard-code hex at call sites. Semantic data colours
  (tyre temps, input bars) live at their call sites. Styling rules: `docs/ui/STYLING-GUIDE.md`.
- All user-facing strings go through `tr("...")` in `src/i18n.rs` (English source → German).
  Add the English key + German value there; duplicate keys warn at compile time.

## Docs: read before you touch, update after you change

When a task touches an area, **read its doc first** (start with the navigation map). When you
change something the docs describe, **update the doc in the same commit** — including the *why*
behind a design choice. Full discipline in the always-loaded
`claude-instructions/documentation.md` (below).

Reference docs — plain links, read the one relevant to your task on demand:

- **`docs/README.md`** — the docs table of contents; what each subfolder holds.
- **`docs/architecture/overview.md`** — codebase navigation map (threading, data flow, full
  module map, "where to look for X"). **Start here to navigate the code.**
- Packet / protocol wire format — `docs/protocol/forza-fh6-packet-format.md`
- Per-tab feature behaviour — `docs/features/`
- Domain notes & project scope — `docs/meta/project-notes.md`

## Always in context (force-loaded, mandatory)

These are the only files CLAUDE.md pulls in with `@`. **`@` is reserved for this
must-always-know set** — the `claude-instructions/` rules plus the terminology glossary.
Every other doc above is a plain link: an optional, on-demand read.

- **Terminology** — @docs/meta/TERMINOLOGY.md — project vocabulary; use these meanings, ask
  before acting on an undefined term, and keep it current.
- **Documentation discipline** — @docs/claude-instructions/documentation.md — read before you
  touch, update after you change, and record design rationale (the *why*).
- **Sub-agent orchestration** — @docs/claude-instructions/sub-agent-orchestration.md — when/how
  to fan out, partition edits, worktree isolation, model/effort, the 4-agent cap. Read before
  dispatching any sub-agent work.
- **Changelog — required** — every user-facing change **must** get a bullet in the top
  `## [version]` section of the repo-root `CHANGELOG.md`, in the same commit. Format &
  versioning: @docs/claude-instructions/changelog.md.
