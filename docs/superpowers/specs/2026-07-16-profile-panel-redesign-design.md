# Profile Panel Redesign + Styled-Checkbox Tree

Date: 2026-07-16 · Branch: `feature/profile-manager`

Redesign the Settings tab's Profile Manager UI to the approved mockup (option 3a),
reorder the Settings categories, and make the export/import tree use the app's custom
styled checkbox.

## Settings layout & order

Two columns of cards:

- **Left:** `PROFILES` → `EXPORT / IMPORT` → `HOTKEY` → `INPUT`
- **Right:** `REPOSITORY / CREDITS` → `DISPLAY` → `NETWORK` → `CO-OP`

The Export/Import card lives directly under PROFILES in the left column — it's part of
the profile feature, not a standalone category (hence absent from the category list).
`Repository` is renamed `Repository / Credits` and leads the right column.

## PROFILES card

- Row: `Active profile` label + dropdown (compact mirror of the current profile).
- **Scrollable profile list**, fixed ~110px height, one row per profile. Active row has a
  subtle bg wash + a right-aligned check; clicking a row switches to it. Scrolls when
  profiles exceed the height.
- Four equal-width buttons: `New` / `Duplicate` / `Rename` / `Delete`. New & Rename reveal
  an inline name field; Delete an inline confirm (unchanged behaviour).

## EXPORT / IMPORT card

Bordered card whose header is a **2-segment tab bar** (`EXPORT` | `IMPORT`, active segment
accent-filled) that swaps the body — replacing the two collapsibles.

- **Export:** helper text → scrollable checkbox tree (~130px) → full-width
  `Copy to clipboard`.
- **Import:** paste textarea + `Built-in` dropdown → target radios
  (`New profile` / `Overwrite [dropdown]`) → same tree → full-width `Import`.
- **Status line** below the card; green on success.

New transient app state: `profile_io_tab: ProfileIoTab { Export, Import }`.

## Styled checkbox for the tree

`theme::styled_checkbox` currently can't render a disabled row, which the import tree
needs (groups absent from the pasted JSON must be greyed and non-interactive). Add an
`enabled` capability by threading a flag through `checkbox_ui` and exposing
`styled_checkbox_enabled(ui, checked, label, enabled)`; `styled_checkbox` stays a
thin `enabled = true` wrapper. Disabled = dim outline + dim label, clicks ignored.

The tree (`group_tree` in `settings.rs`) then uses the styled box for both parent and
child rows (children indented), removing the two raw `egui::Checkbox::new` calls — the
only non-styled checkboxes in the codebase.

**Why no codebase-wide checkbox sweep:** a survey found every other checkbox already
uses `styled_checkbox` / `checkbox_row`. The only raw ones are those two tree cells.

## Docs

Expand the **Checkbox** section of `docs/ui/STYLING-GUIDE.md` to document the visual
style (18px accent box, white check, hover wash, whole-row click target) and the new
disabled/tree variant. Update `docs/features/profiles.md` and `CHANGELOG.md`.

## Non-goals

- No native file dialog (clipboard/paste unchanged).
- No new checkbox widget library; reuse `checkbox_ui`.
- No changes to profile backend semantics (CRUD, mirrored save) — pure UI + one theme fn.
