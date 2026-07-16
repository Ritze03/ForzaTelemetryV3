# Profile Manager

Named, switchable snapshots of your entire configuration, managed from the **PROFILES**
card (first in the Settings left column). Create, duplicate, rename, delete, switch, and
selectively export/import profiles.

## Model — why continuous save, no Save button

A profile is a full `AppConfig` snapshot at `app_data_dir()/profiles/<name>.json`.
`config.json` stays the live config (everything still autosaves to it) and carries one
extra field, `active_profile`.

`AppConfig::save()` **mirrors** the live config into `profiles/<active_profile>.json` on
every change. That's the whole design: because the active profile is always already
up to date on disk, switching profiles can never lose the outgoing one — so there is no
"unsaved changes" state, no confirm dialog, and no explicit Save button.

**Why this way:** the app already autosaved aggressively. Rather than bolt on
dirty-tracking + a revert/confirm flow (the alternative considered during design), we
lean on the existing autosave and just add a second write. The user's stated rule —
"when switching profiles, save the previous one before loading the new one" — falls out
for free. See `docs/superpowers/specs/2026-07-16-profile-manager-design.md`.

## Operations (`config.rs`, each ends in a `save()`)

- **Switch** (`switch_profile`) — `save()` flushes the current profile, then
  `load_profile` overlays the target snapshot and re-asserts `active_profile`.
- **New** (`new_profile`) — snapshots the *current* live settings as the starting point,
  then switches to the new profile. **On Windows only**, a new profile also defaults to
  `gate_mode = WindowFocus` ("Game window focused") and `input_focus_gate = true` ("Only
  send inputs when game focused"). This override is New-only — duplicate and import don't
  apply it.
- **Duplicate** (`duplicate_profile`) — copies the active snapshot to "<name> copy".
- **Rename** (`rename_active_profile`) — renames the active profile's file.
- **Delete** (`delete_profile`) — removes the file; if it was active, loads the first
  remaining profile **without** flushing first (the deleted profile is gone — flushing
  would resurrect its file). Disabled in the UI when only one profile remains.

Names are filesystem-sanitised (`sanitize_profile_name`) and de-duplicated with a
`" (2)"` suffix (`unique_profile_name`).

## Seeding / migration

On `AppConfig::load()`, if the active profile has no file yet (fresh install, or an
existing `config.json` upgrading to this version), one is written from the current live
config. So `profiles/` is never empty and the active profile always has a backing file.
The default active profile name is `"Default"`.

## UI layout (`settings.rs`)

Two cards, both in the left Settings column:

- **PROFILES** (`profiles_card`) — an `Active profile` dropdown (compact mirror), a
  fixed-height **scrollable list** (`profile_row`: full-width click target, active row
  washed + right-aligned check), and a four-button row New / Duplicate / Rename / Delete.
  New & Rename reveal an inline name field; Delete an inline confirm.
- **EXPORT / IMPORT** (`export_import_card`) — a bordered card whose header is a
  two-segment tab bar (`io_segment`, accent-filled when active) swapping between the
  export tree and the import form. The shared green status line sits at the bottom.

Both cards deliberately sit next to `HOTKEY` / `INPUT` in the left column; the right
column holds `REPOSITORY / CREDITS`, `DISPLAY`, `NETWORK`, `CO-OP`.

## Selective export / import

Both use the `KEY_GROUPS` registry and a checkbox group-tree (`group_tree` in
`settings.rs`, drawn with `theme::styled_checkbox_enabled`) — see [[presets]] for the
registry, the completeness test, and the `export_selected` / `import_selected` /
`groups_present` functions.

- **Export** tab — tick groups → *Copy to clipboard* (JSON of just those keys).
- **Import** tab — paste JSON (or pick a bundled preset as a built-in source), choose a
  **target** (a new profile, or overwrite an existing one), tick which groups to apply.
  Only the selected groups' keys overwrite the target; everything else is preserved.
  Groups absent from the pasted JSON are disabled (greyed) in the tree.

## UI state (`app.rs`)

Transient (not persisted): `profile_dialog` (inline New/Rename/Delete-confirm),
`profile_io_tab` (Export/Import), `profile_name_buf`, `profile_io_status`,
`profile_export_sel` / `profile_import_sel` (bool masks aligned to `KEY_GROUPS`),
`profile_import_present`, `profile_import_buf`, and the import-target fields
(`profile_import_new`, `profile_import_new_name`, `profile_import_overwrite`).
