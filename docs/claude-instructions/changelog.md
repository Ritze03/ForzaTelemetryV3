# Changelog Maintenance

**Mandatory working rule for agents, linked from the repo-root `CLAUDE.md`.**

`CHANGELOG.md` in the **project root** is the user-facing "What's New" — shown in-app
via the top-right button on the tab bar (`src/ui/changelog.rs`). It is for users to see
what changed, so write entries in plain user-facing language, not implementation detail.

- **Maintain it as you work.** Whenever you make a user-facing change, add a bullet under
  the right heading in **today's** dated block for the current version. Format is strict
  because the viewer parses it: `## [x.y.z] – YYYY-MM-DD` version+date headers, `### Added` /
  `### Fixed` / `### Removed` / `### Info` category subheads, and
  `- **Short title**: one-line detail` bullets. Keep to that shape.

- **Group entries by date.** A single version accumulates over several days, and each day's
  work gets its **own** `## [x.y.z] – YYYY-MM-DD` block so the changelog shows what landed
  when. When adding a bullet:
  - If a block with **today's date** and the current version already exists at the top,
    add your bullet under the right `###` category inside it.
  - Otherwise, create a **new** `## [x.y.z] – <today>` block at the **top** of the section
    (newest date first — the viewer renders blocks top-to-bottom) and put the bullet there.
  Do **not** bump the date on an existing block; yesterday's entries keep yesterday's date.
  The viewer shows each block as a separate `version · date` group.

- **You decide when to bump the version.** The same `x.y.z` repeats across its daily blocks
  until you choose to release. At a natural stopping point (a batch of features, a fix
  milestone), bump `version` in `Cargo.toml` and start using the new `x.y.z` in the next
  dated block you create — older blocks keep their old version number, preserving the
  history. Purely internal refactors don't need a changelog entry or a version bump.
