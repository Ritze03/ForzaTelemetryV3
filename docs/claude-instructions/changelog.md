# Changelog Maintenance

**Mandatory working rule for agents, linked from the repo-root `CLAUDE.md`.**

`CHANGELOG.md` in the **project root** is the user-facing "What's New" — shown in-app
via the top-right button on the tab bar (`src/ui/changelog.rs`). It is for users to see
what changed, so write entries in plain user-facing language, not implementation detail.

- **Maintain it as you work.** Whenever you make a user-facing change, add a bullet under
  the right heading in **today's** dated block. Format is strict because the viewer parses
  it: `## [x.y.z] – YYYY-MM-DD` version+date headers, `### Added` / `### Fixed` /
  `### Removed` / `### Info` category subheads, and `- **Short title**: one-line detail`
  bullets. Keep to that shape.

- **Group entries by date — one block per day.** Each day's work gets its **own**
  `## [x.y.z] – YYYY-MM-DD` block (with its own version — see the bump rule below), so the
  changelog shows what landed when. When adding a bullet:
  - If **today's** block already exists at the top, add your bullet under the right `###`
    category inside it.
  - Otherwise, create a **new** block at the **top** (newest date first — the viewer renders
    blocks top-to-bottom), choose its version per the bump rule below, and put the bullet
    there.
  Do **not** change an existing block's date or version; past days keep theirs. The viewer
  shows each block as a separate `version · date` group.

- **Every dated block is its own version bump — you size it.** Each day's
  `## [x.y.z] – <date>` block gets a **new** version relative to the block below it (the
  previous day), and **how big the bump is reflects how big that day's user-facing changes
  were**. Judge the magnitude honestly — read the day's actual changes, don't inflate:
  - **Patch** (`x.y.Z+1`) — a small day: a handful of fixes, polish, a minor tweak or a
    small feature surfaced. *(e.g. 2026-07-15 → `0.1.1`: a couple of widget options + one
    alignment fix.)*
  - **Minor** (`x.Y+1.0`) — a substantial day: meaningful new features or a notable rework.
    *(e.g. 2026-07-16 → `0.2.0`: the Profile Manager, global hotkeys, status-bar indicators,
    a settings reorg — a landmark day.)*
  - **Major** (`X+1.0.0`) — a true milestone or breaking overhaul. In `0.x` hold off unless
    it's genuinely a 1.0-scale landmark; most days are patch or minor.

  Keep `version` in `Cargo.toml` in sync with the **newest (top)** block. Older blocks keep
  the version they were assigned — never re-bump a past day. Purely internal refactors with
  no user-facing effect still get **no** changelog entry and **no** bump (a day of only
  refactors adds no new block).
