# Presets & Mini-Settings

A **preset** is not a separate format — it's a partial `AppConfig` serialized as JSON.
Whatever keys are present in the JSON overwrite the live config; every key it omits is
left exactly as it was. This means new config fields never need a preset-format bump:
they just don't travel until someone adds them to the relevant key list.

## Where it lives

- `src/config.rs` — `AppConfig`, the overlay logic, and the two key lists below.
- `src/app.rs` — the status-bar cog's **mini-settings popup**, `DashboardSubTab::Config`
  tab ("Config" in the sub-tab row alongside General/Modules/Rpm/etc.), which hosts the
  Load Preset / Export / Import UI.
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

## Export / Import (Config sub-tab)

- **Export** (`export_preset(cfg, include_minisettings)`) — always includes
  `LAYOUT_KEYS`; the **"Include mini-settings"** checkbox additionally pulls in
  `MINISETTINGS_KEYS`. Result is copied to the clipboard as pretty-printed JSON.
- **Import** (`import_preset(cfg, json, include_minisettings)`) — parses the pasted JSON;
  if "Include mini-settings" is unchecked, `MINISETTINGS_KEYS` are stripped from the
  overlay before it's applied, so only the layout takes effect. Returns `false` (nothing
  applied) on invalid JSON.
- Both directions share the same overlay logic as bundled presets, so hand-edited or
  swapped-between-users JSON behaves identically to `apply_preset`.

## Bundled presets

`PRESET_NAMES` / `PRESET_DATA` in `config.rs` list the two presets shipped in the binary:

- **"Ale (halb)"** → `assets/configs/ale.json`
- **"Ritze (ganz)"** → `assets/configs/ritze.json`

Picked from the **Load Preset** dropdown in the Config sub-tab and applied immediately
(then the config is saved to disk). Each bundled JSON is a full layout + mini-settings
dump (e.g. Ritze's uses a 32×19 grid), not a partial hand-written file — they're just
regular exports checked into the repo.

## Gotchas

- Adding a **new mini-setting** that should be preset-portable: add the `AppConfig`
  field *and* its key string to `MINISETTINGS_KEYS`. Forgetting the second step means the
  setting silently doesn't travel — no compile error, no runtime warning.
- **Removing/renaming** a config enum value or field needs a migration in
  `AppConfig::load` (see the `Theme::Light` → `Dark` and `compact_tabs` → `top_bar_style`
  fix-ups in `config.rs`) plus updates to the bundled presets, or old configs/presets fail
  to parse and silently no-op instead of applying.
- See also [[coop]] for `coop_*` fields that are part of `MINISETTINGS_KEYS` (name/hue
  are deliberately excluded — session identity, not a preset value).
