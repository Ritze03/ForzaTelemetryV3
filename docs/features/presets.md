# Presets & Mini-Settings

A **preset** is not a separate format — it's a partial `AppConfig` serialized as JSON.
Whatever keys are present in the JSON overwrite the live config; every key it omits is
left exactly as it was. This means new config fields never need a preset-format bump:
they just don't travel until someone adds them to the relevant key list.

The overlay mechanism here is the foundation the **Profile Manager** builds on — see
[[profiles]] for the user-facing feature (named full-config snapshots + selective
export/import). This page documents the underlying overlay + key lists.

## Where it lives

- `src/config.rs` — `AppConfig`, the overlay logic, the key lists below, and the
  `KEY_GROUPS` registry used by selective export/import.
- `src/ui/settings.rs` — the **Profiles** card hosts the export/import UI (a checkbox
  group-tree + clipboard/paste). The old mini-settings **Config** sub-tab is gone.
- `assets/configs/*.json` — the bundled presets, baked into the binary via `include_str!`.

## The two key lists

- **`LAYOUT_KEYS`** — dashboard layout: `grid_cols`, `grid_rows`, `dashboard_widgets`,
  `dashboard_edit_mode`, `dashboard_show_grid`, `dashboard_show_outlines`,
  `disabled_modules`. **Always** part of a preset — there's no toggle to exclude these.
- **`MINISETTINGS_KEYS`** — every other per-widget tuning knob exposed in the cog popup
  (tire style, shift %, boost display, minimap calibration, G-Force widget toggles,
  Co-Op list columns, power-curve settings, etc.). This list is **hand-maintained** — a
  new mini-setting that should travel with a preset must be added here explicitly, or it
  silently won't export/import.
- Anything not in either list (e.g. `listen_port`, `coop_name`, DSG/backfire tuning) is
  local machine/session state and never exports.

## Applying a preset — `apply_preset_overlay`

1. The current `AppConfig` is serialized to a `serde_json::Value` (the "base").
2. The preset JSON's keys are merged on top, overwriting matching keys in the base.
3. The merged value is deserialized back into `AppConfig`. If that fails (e.g. an
   invalid enum variant), the whole operation is a no-op — the live config is untouched.
4. `inject_missing_widget_kinds` runs afterward so a preset from an older version — one
   missing a widget kind added since — gets that widget parked below the grid instead of
   disappearing.
5. `migrate_tire_display_style` rewrites the removed `"Separate"`/`"Combined"`
   `tire_display_style` values onto the surviving `"Tires"` variant first, so old presets
   still deserialize.

## Selective export / import — `KEY_GROUPS`

The Profile Manager exports/imports a chosen **subset** of the config, organised into
tab-shaped groups for the selection tree. `KEY_GROUPS` (`config.rs`) is an ordered list
of `KeyGroup { section, name, keys }`:

- **Dashboard** → *Layout* (`LAYOUT_KEYS`) / *Mini-settings* (`MINISETTINGS_KEYS`)
- **Settings** → *Network* / *Display* / *Hotkeys & Input* / *Co-Op*
- **Tuning** → *Backfire* / *Automatic Gearbox* / *Acceleration Tests*

Every serialized `AppConfig` key is in **exactly one** group or the `EXPORT_EXCLUDE`
list (`active_profile`). The `key_groups_partition_all_keys` test enforces this — add a
new field without categorising it and the test fails, so nothing silently drops out of
export. This replaces the old include-mini-settings checkbox with per-group ticks.

- **Export** (`export_selected(cfg, &sel)`) — `sel` is a bool mask aligned to
  `KEY_GROUPS`; only the selected groups' keys are serialized to clipboard JSON.
- **Import** (`import_selected(target, json, &sel)`) — overlays only the selected
  groups' keys onto `target`, so unselected settings in the target are preserved. Returns
  `false` (nothing applied) on non-object JSON. `groups_present(json)` reports which
  groups the pasted JSON actually contains, used to pre-check and disable the tree rows.
- Both reuse `apply_preset_overlay`, so hand-edited or swapped-between-users JSON behaves
  identically to a bundled preset.

## Bundled presets

`PRESET_NAMES` / `PRESET_DATA` in `config.rs` list the two presets shipped in the binary:

- **"Ale (halb)"** → `assets/configs/ale.json`
- **"Ritze (ganz)"** → `assets/configs/ritze.json`

Exposed as **built-in sources** in the Profiles → Import combo: picking one fills the
paste box, then it flows through the same selective-import path. Each bundled JSON is a
full layout + mini-settings dump (e.g. Ritze's uses a 32×19 grid), not a partial
hand-written file — they're just regular exports checked into the repo.

## Gotchas

- Adding a **new mini-setting** that should be preset-portable: add the `AppConfig`
  field *and* its key string to `MINISETTINGS_KEYS`. Forgetting the second step means the
  setting silently doesn't travel — no compile error.
- Adding **any** new `AppConfig` field: it must go into one `KEY_GROUPS` group (usually
  the settings/tuning group it belongs to) or into `EXPORT_EXCLUDE`. The
  `key_groups_partition_all_keys` test fails until you do — this is the guard that a new
  field is a conscious "exports or not" decision, not an oversight.
- **Removing/renaming** a config enum value or field needs a migration in
  `AppConfig::load` (see the `Theme::Light` → `Dark` and `compact_tabs` → `top_bar_style`
  fix-ups in `config.rs`) plus updates to the bundled presets, or old configs/presets fail
  to parse and silently no-op instead of applying.
- See also [[coop]] for `coop_*` fields that are part of `MINISETTINGS_KEYS` (name/hue
  are deliberately excluded — session identity, not a preset value).
