# State & Configuration

Where the app's data lives, split three ways: persisted settings (`AppConfig`), one
in-memory app struct (`ForzaApp`) holding session/runtime state, and small
purpose-built structs (telemetry connection, recording) owned by `ForzaApp`.

## Where runtime state lives

- **`src/config.rs` — `AppConfig`.** Everything that should survive a restart: units,
  theme, per-widget tuning knobs, the dashboard grid layout, DSG/backfire tuning,
  Co-Op identity, language. Serialized as pretty JSON to `config.json` in the app's
  data dir (`config::app_data_dir()` — `dirs::data_dir()/ForzaTelemetryV3`, or
  `FORZA_DATA_DIR` when set for tests). One field, `AppConfig` itself, lives on
  `ForzaApp::config`.
- **`src/app.rs` — `ForzaApp`.** Everything that's derived from the live telemetry
  stream or is pure UI/session state, none of it serialized:
  - **Session maxima** (`ForzaApp` fields, comment `// Session maxima (reset on car
    change)`): `max_power_ps`, `max_torque_nm`, `max_boost_psi`, `max_speed_kmh`,
    `cached_engine_max_rpm`, `fi_detected`, `dynamic_max_rpm` (the dynamically detected
    redline), `wheel_radius_est` (per-wheel EMA-smoothed tire radius). All zeroed and
    recomputed from scratch on a car change — see the `pkt.car_ordinal !=
    self.last_car_ordinal` branch around `app.rs:838`.
  - **Session stats structs**: `suspension_stats: SuspensionStats` (`app.rs:223`, a
    rolling min/max + short history) and `gforce_stats: GForceStats` (`app.rs:265`,
    running max + decaying peak + a short g-history trail for the traction circle).
  - **Cached car identity** (`cached_car_class_str`, `cached_car_pi`,
    `cached_drivetrain_str`, `cached_num_cylinders`, …) — held so the UI keeps showing
    the last known car while `IsRaceOn == 0` (paused/menu) blanks the live packet.
  - **Per-car calibration** (`car_calibrations: HashMap<i32, CarCalibration>`) — loaded
    from its *own* file, `automatic-gearbox-saved-calibrations.json`
    (`config::load_car_calibrations` / `save_car_calibrations`), not `config.json`.
    Only persisted when `config.dsg_save_calibration` is on; flushed at car-change and
    on exit.
  - **Transient UI/session state**: current tab, mini-settings popup open/tab state,
    drag/resize state for the dashboard grid, minimap texture/zoom/season cache,
    Co-Op trails and roster cache, speed-trace ring buffer, changelog filter toggles,
    pending preset selection, recorder handle. None of this round-trips through
    `AppConfig`.
- **`src/telemetry.rs` — `TelemetryState`.** Connection state, held at
  `ForzaApp::telemetry`: `latest: Option<ForzaPacket>` (most recent packet),
  `is_connected: bool`, `packets_per_sec: f32`. `TelemetryState::update()` is called
  per received packet; it flips `is_connected = true` and recomputes
  `packets_per_sec` once per second from a rolling `packet_count`/`last_pps_update`
  window. There is no disconnect detection here — `is_connected` only ever goes
  true; `ForzaApp::last_packet_time: Option<Instant>` is what the UI uses elsewhere to
  notice a stall.
- **`src/recorder.rs` — `RecordState`.** Held as `ForzaApp::recorder: Option<RecordState>`
  — `None` when not recording, `Some` for the duration of one recording. Holds the
  open `BufWriter<File>`, a start `Instant`, and a running `packets` counter; each
  packet is appended as `[u32 elapsed_ms][u16 len][len bytes]` to a `.ftr` file under
  `<app data dir>/recordings/`. `Drop` flushes the writer, so ending a recording is
  just replacing the `Option` with `None`. See [[recording]] for the user-facing
  behaviour and CSV export.

## Config persistence lifecycle

1. **Load** — `AppConfig::load()` (`config.rs:675`), called once in `ForzaApp::new()`.
   Reads `config.json`; if missing or unparseable as JSON, returns `AppConfig::default()`
   outright. Otherwise it parses into a raw `serde_json::Value` first (not straight into
   `AppConfig`) so it can:
   - **Merge in defaults for missing keys** — walks the default config's own
     `serde_json::Value` and `.entry(k).or_insert(v)`s anything the saved file lacks.
     This is what lets a newly-added `AppConfig` field ship without bumping any format
     version: an old `config.json` just gets the field's default value merged in.
   - **Run field/enum migrations** (see below) before the final
     `serde_json::from_value::<AppConfig>`.
   - Finally call `inject_missing_widget_kinds` so a config saved before a new
     `WidgetKind` existed still gets that widget parked into the layout instead of it
     silently never appearing.
2. **Mutate** — the UI (Settings tab, dashboard mini-settings cog popup, per-tab
   controls) writes directly into `app.config.*` fields; there's no central "set and
   save" wrapper.
3. **Save** — `AppConfig::save()` (`config.rs:714`) serializes the whole struct back to
   pretty JSON and writes `config.json`. It is **not** autosaved every frame; call sites
   trigger it explicitly after a change that should stick — e.g. `src/ui/settings.rs:222`,
   `src/ui/dashboard.rs:81` and `:435`, `src/ui/gearbox.rs:163`, `src/ui/coop.rs:153`,
   several spots in `app.rs`'s mini-settings popup — plus unconditionally in
   `ForzaApp::on_exit` (`app.rs:2287`) as a final catch-all.

## The migration pattern (renaming/removing a config field or enum value)

This is the sharp edge: because `load()` merges onto raw JSON and only migrates known
shapes, **removing or renaming an `AppConfig` field or an enum variant needs an explicit
fix-up**, or old `config.json` files (and old presets) fail to deserialize and silently
fall back to defaults / no-op instead of applying. The existing fix-ups in `config.rs`
are the templates to copy:

- **Enum variant removed, value rewritten onto a survivor** —
  `migrate_tire_display_style` (`config.rs:537`): if the saved `tire_display_style` is
  the old `"Separate"` or `"Combined"` string, it's rewritten to `"Tires"` in the raw
  JSON map before deserializing. Called from both `AppConfig::load()` **and**
  `apply_preset_overlay`, since a stale preset can carry the same old value.
- **Enum variant renamed outright** — the `Theme::Light` → `Dark` fix-up in `load()`
  (`config.rs:693`): if `theme == "Light"`, rewrite it to `"Dark"` before parsing.
- **Field replaced by a different field/type** — the `compact_tabs: bool` →
  `top_bar_style: TopBarStyle` fix-up in `load()` (`config.rs:699`): if the old bool key
  is present, translate it into the new enum's string value (`true` → `"Simple"`,
  `false` → `"Legacy"`) and insert that under the new key. The old key is simply left
  in the map afterward (harmless — `AppConfig` has no field for it, so serde ignores
  unknown keys on deserialize).

**Checklist when you remove/rename a field or enum value:**
1. Add a migration in `AppConfig::load()` (raw-JSON rewrite, following the patterns
   above).
2. If the same field/value can appear in a preset, also handle it in
   `apply_preset_overlay` (`config.rs:543`) — `migrate_tire_display_style` is applied
   there specifically because presets are hand-edited/shared JSON, not just
   locally-saved config.
3. Update the bundled presets (`assets/configs/ale.json`, `assets/configs/ritze.json`)
   if they reference the old name/value directly.
4. If the field/value was listed in `LAYOUT_KEYS` or `MINISETTINGS_KEYS`, update the
   key name there too.

Skipping step 1 doesn't crash the app — `serde_json::from_value` failure is caught
(`.unwrap_or(default)` in `load()`, an `if let Ok(...)` in the preset path) — but it
silently discards the *entire* saved config or the *entire* preset overlay, which is
much worse than a partial migration and easy to miss in testing.

## `LAYOUT_KEYS` vs `MINISETTINGS_KEYS`

Both are `&[&str]` lists of `AppConfig` field names in `config.rs`, used only by the
preset overlay/export machinery (`apply_preset_overlay`, `export_preset`,
`import_preset`) — not by `load()`/`save()`, which always read/write every field.

- **`LAYOUT_KEYS`** (`config.rs:568`) — dashboard grid + widget placement:
  `grid_cols`, `grid_rows`, `dashboard_widgets`, `dashboard_edit_mode`,
  `dashboard_show_grid`, `dashboard_show_outlines`, `disabled_modules`. **Always**
  included in an export and always applied on import — there is no toggle to exclude
  these.
- **`MINISETTINGS_KEYS`** (`config.rs:575`) — every other per-widget tuning knob
  exposed in the cog mini-settings popup (tire display style, shift RPM %, G-Force
  widget toggles, minimap calibration, Co-Op list columns, power-curve settings, …).
  **Hand-maintained**: a field only travels with a preset/export if its key string is
  in this list. Adding a new mini-setting that should be preset-portable means adding
  both the `AppConfig` field *and* its key string here — forgetting the second step
  fails silently (no compile error, the setting just never exports/imports).
- Anything **not** in either list (`listen_port`, `coop_name`, `coop_last_code`, DSG
  numeric tuning, backfire tuning, etc.) is local machine/session state and never
  travels with a preset, no matter which toggle is set.
- `export_preset(cfg, include_minisettings)` always pulls `LAYOUT_KEYS`, plus
  `MINISETTINGS_KEYS` when the flag is set. `import_preset(cfg, json,
  include_minisettings)` does the mirror: if the flag is off, `MINISETTINGS_KEYS` are
  stripped from the incoming JSON before the overlay is applied, so only layout takes
  effect. Both bundled presets (`PRESET_NAMES`/`PRESET_DATA`, backed by
  `assets/configs/ale.json` / `ritze.json`) and the Config sub-tab's Export/Import
  paste box go through the same `apply_preset_overlay` — see [[presets]] for the
  user-facing flow and the `apply_preset_overlay` step order.

## See also

- [[overview]] — high-level app architecture.
- [[presets]] — user-facing preset/mini-settings behaviour, key lists (kept in sync
  with the lists documented here), bundled preset details.
- [[ui-architecture]] — how `src/ui/*` and the mini-settings popup consume this state.
- [[recording]] — `RecordState` usage from the Settings tab.
