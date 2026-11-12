# Changelog Maintenance

**Mandatory working rule for agents, linked (force-loaded) from the repo-root `CLAUDE.md`.**

`CHANGELOG.md` in the **project root** is the user-facing "What's New" — shown in-app
via the top-right button on the tab bar (`src/ui/changelog.rs`). It is for users to see
what changed, so write entries in plain language a user with no technical knowledge
understands.

## The shape

```
## YYYY-MM-DD

### Added
- **Short name**: one short sentence of what got better for the user.
```

- **`## YYYY-MM-DD`**: one block per calendar day, newest on top. The project is
  **date-based**: new blocks carry no version number (see
  [`documentation-version-policy.md`](documentation-version-policy.md)).
- **`### Added` / `### Fixed` / `### Removed` / `### Info`**: the only four categories.
  Omit a category with no entries.
- **Bullets**: `- **Name**: sentence.`, hard-wrapped at about 90 columns, continuation
  lines indented two spaces.

## Rules

- **Maintain it as you work.** Every commit that changes what users get adds a bullet in
  the **same commit**, under **today's** block. If today's block exists, add to it;
  otherwise create it at the top.
- **Never rewrite past blocks.** Old `## [x.y.z] – YYYY-MM-DD` blocks keep their
  version and date as they are.
- **Plain language, what and never how.** Say what got better for the user. No file
  names, function names, flags, libraries, protocols, config keys, commit ids or
  implementation details. One short sentence per bullet; drop anything a non-technical
  user would not notice. Example: a fix for a variable-refresh-rate bug becomes
  `- **Variable refresh rate**: fixed an issue with variable refresh rate.`
- **Skip internal-only changes**: docs, refactors, tests, CI. A day with only such work
  adds no block.
- **Anonymize**: no hostnames, usernames, emails, real names, absolute home paths or
  private project names.
