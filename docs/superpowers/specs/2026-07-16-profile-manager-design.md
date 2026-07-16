# Profile Manager — Design

Date: 2026-07-16 · Branch: `feature/profile-manager`

## Goal

A **Profile Manager** in the Settings tab: create, duplicate, load (switch),
rename, delete, and selectively export/import named settings profiles. A profile
is a full snapshot of `AppConfig`. Switching profiles auto-saves the outgoing one
first (no unsaved-changes state, no confirm dialogs).

This replaces and generalises the old mini-settings **Config** tab (bundled-preset
loader + clipboard export/paste import), which is removed.

## Data model & storage

- One profile = a full `AppConfig` snapshot serialized to
  `app_data_dir()/profiles/<name>.json`.
- `config.json` stays the **live config** (everything still autosaves to it) and
  gains one field: `active_profile: String` (serde default `"Default"`).
- **Save is continuous**: `AppConfig::save()` also mirrors the live config into
  `profiles/<active_profile>.json`. This guarantees the outgoing profile is always
  already saved before any switch — no explicit Save button.
- **Migration / seeding**: on `load()`, if `profiles/` has no file for the active
  profile, seed it from the current live config. On a fresh install the active
  profile is `"Default"`. Existing configs keep their settings — the first launch
  just writes them into `profiles/Default.json`.
- Profile files are read through the same merge path as `config.json` (missing
  newer keys fill from code defaults), so an old profile never resets the config.

## Operations (each ends in `save()`)

- **Switch/Load**: `save()` (flush current) → set `active_profile = target` →
  overlay `profiles/<target>.json` onto live config → `save()`.
- **New**: snapshot current live settings → apply the Windows override (below) →
  write `profiles/<name>.json` → switch to it. Name defaults to `"Profile N"`.
- **Duplicate**: copy the active profile's snapshot under `"<name> copy"` → switch.
- **Rename**: rename the file + update `active_profile`.
- **Delete**: remove the file; if it was active, switch to the first remaining
  profile. Disabled when only one profile remains (always ≥1). Confirm click.
- **Export**: active profile JSON filtered to the checked groups → clipboard.
- **Import**: paste JSON (or pick a built-in preset) → choose target (New profile
  or overwrite an existing one) → check groups → overlay only those groups' keys
  onto the target; unselected keys in the target are preserved.

### Windows-only default for New

On `#[cfg(windows)]`, a **newly created** profile (New only — not duplicate/import)
sets `hotkeys.gate_mode = WindowFocus` ("Game window focused") and
`hotkeys.input_focus_gate = true` ("Only send inputs when game focused").

## Selective import/export — key-group registry

Fields are grouped into tab-shaped units for the selection tree (curated, not raw
config structure — the flat ~100-key `AppConfig` JSON has no user-meaningful tree).
Extends the existing `LAYOUT_KEYS` / `MINISETTINGS_KEYS` pattern into one ordered
registry: `&[(section, group_name, &[keys])]`.

- **Dashboard** → *Layout* (`LAYOUT_KEYS`) / *Mini-settings* (`MINISETTINGS_KEYS`)
- **Settings** → *Network* / *Display* / *Co-Op* / *Hotkeys* / *Input*
- **Tuning** → *Backfire* / *Automatic Gearbox* / *Power Curve* / *Engine Swaps*
- Excluded from export: `active_profile` and any pure-runtime keys (explicit
  exclude-list).

**Completeness test** (guards the hardcoding): assert every key of a serialized
`AppConfig` is in exactly one group or the exclude-list. Adding an ungrouped field
fails the test. Gives the safety of the structural approach with readable UX.

Export = filter config JSON to the checked groups' keys. Import = overlay the
checked groups' keys from the pasted JSON onto the target (reuses
`apply_preset_overlay`).

## UI

New **`PROFILES`** category card, **first in the left column** of Settings (before
Network).

- **Active profile**: combo listing all profiles; selecting one switches.
- Button row: **New**, **Duplicate**, **Rename**, **Delete** (delete confirms).
- **Export** subsection: the group-tree of checkboxes + *Copy to clipboard*.
- **Import** subsection: paste-box (+ built-in-preset source for Ale/Ritze), a
  **target** selector (New profile name / overwrite existing), the group-tree, and
  *Import*. A status line shows the last action.

The group-tree is one reusable helper used by both Export and Import.

## Removed

- Mini-settings **Config** sub-tab (`DashboardSubTab::Config`) and its inline
  render. Its state fields (`config_export_minisettings`, `config_import_*`,
  `config_io_status`, `pending_preset`) move into the Profiles UI or are dropped.
- Bundled presets (`PRESET_NAMES`/`PRESET_DATA`) are kept as data, re-exposed as
  built-in import sources in the Profiles Import UI.

## Files touched

- `src/config.rs` — `active_profile` field, profiles dir + CRUD, key-group
  registry + completeness test, selective export/import, migration/seed.
- `src/app.rs` — remove Config sub-tab; wire nothing else here that belongs in
  settings.rs.
- `src/ui/settings.rs` — new Profiles card + import/export tree.
- `src/i18n.rs` — new strings.
- Docs: new `docs/features/profiles.md`, update `presets.md`, `settings.md`,
  `dashboard.md`, `README.md` index, `CHANGELOG.md`.

## Non-goals / YAGNI

- No native file dialog (reuse clipboard/paste).
- No unsaved-changes tracking or revert (continuous save makes it moot).
- No per-profile machine-field carve-out (full snapshot; personal tool).
