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
- **Duplicate** (`duplicate_profile_as`) — copies the active snapshot under a chosen name
  (the dialog seeds "<name> copy") and switches to it.
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

One **PROFILES** card (`profiles_card`) in the left column, holding everything:

- A fixed-height **scrollable list** (`profile_row`: full-width click target, active row
  washed + right-aligned check; clicking a row switches to it — this replaces the old
  active-profile dropdown), a four-button row New / Duplicate / Rename / Delete, and an
  **Export / Import** button row. New / Duplicate / Rename / Delete open the small
  `profile_dialog_modal` (text field / confirm; Enter confirms, Esc/backdrop cancels).
  Export / Import open the large `profile_io_modal` (below). The green status line sits at
  the bottom of the card. Both modals render at the end of `settings::show`, floating over
  the whole tab.

Left column: `PROFILES`, `HOTKEY`. Right column: `REPOSITORY / CREDITS`, `DISPLAY`,
`NETWORK`, `CO-OP`, `INPUT`.

## Export / Import modal (`profile_io_modal`)

A large centered window over a dim backdrop, so the card stays compact. Section headers
use `theme::section_label` (the category-blue uppercase style).

- **Export** — one split: *What to export* (outlined group tree) on the left, a live JSON
  **Preview** on the right.
- **Import** — a fixed-height **top row** of two columns: **Source** (`import_source_col`:
  a Paste JSON / bundled-preset dropdown + a fixed-height paste box, or a caption for a
  preset) and **Destination** (`import_dest_col`: a fixed-height profile **list** —
  `profile_row` reused — whose first row is a blue **`+` New profile**, plus an
  always-present name field disabled unless *New profile* is selected, so nothing jumps).
  Below that, a **split**: *What to import* (group tree) on the left, the live **Preview**
  on the right.
- **Preview** — `json_preview` (read-only) shows exactly what will be written, filtered by
  the ticked groups (`config::export_selected` for export, `config::filter_selected` over
  the source for import).
- **Bottom** — a big accent **Export** / **Import** button (`theme::primary_button`) plus
  **Cancel**. Esc or a backdrop-click also cancels.

The outlined tree/preview boxes (`tree_box` / `json_preview`) use a rounded frame with
enough top/bottom inner margin that scrolled content clears the rounded corners.

The tree uses the `KEY_GROUPS` registry and `theme::styled_checkbox_enabled` (see
[[presets]] for the registry, the completeness test, and `export_selected` /
`import_selected` / `filter_selected` / `groups_present`). Only the selected groups' keys
overwrite the destination; everything else is preserved. Groups absent from the source are
greyed out; `recompute_import_present` refreshes that when the source changes.

## UI state (`app.rs`)

Transient (not persisted): `profile_dialog`
(`ProfileDialog`: None/New/Duplicate/Rename/ConfirmDelete/Export/Import),
`profile_dialog_focus` (focus the dialog text field next frame), `profile_name_buf`,
`profile_io_status`, `profile_export_sel` / `profile_import_sel` (bool masks aligned to
`KEY_GROUPS`), `profile_import_present`, `profile_import_buf`, `profile_import_builtin`
(source: `Some(preset)` / `None` = paste), and the import-target fields (`profile_import_new`,
`profile_import_new_name`, `profile_import_overwrite`).
