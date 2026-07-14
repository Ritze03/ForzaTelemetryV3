# Changelog Maintenance

**Mandatory working rule for agents, linked from the repo-root `CLAUDE.md`.**

`CHANGELOG.md` in the **project root** is the user-facing "What's New" — shown in-app
via the top-right button on the tab bar (`src/ui/changelog.rs`). It is for users to see
what changed, so write entries in plain user-facing language, not implementation detail.

- **Maintain it as you work.** Whenever you make a user-facing change, add a bullet under
  the right heading in the top `## [version]` section. Format is strict because the viewer
  parses it: `## [x.y.z] – YYYY-MM-DD` version headers, `### Added` / `### Fixed` /
  `### Removed` / `### Info` category subheads, and `- **Short title**: one-line detail`
  bullets. Keep to that shape.
- **You decide when to bump the version.** Roll the accumulated entries into a fresh
  `## [x.y.z] – YYYY-MM-DD` section at a natural stopping point (a batch of features, a fix
  milestone) and bump `version` in `Cargo.toml` to match. Purely internal refactors don't
  need a changelog entry or a version bump.
