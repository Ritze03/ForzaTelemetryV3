# UI Architecture

How the egui/eframe UI layer is organized: the tab bar, the per-tab modules under
`src/ui/`, the "Graphite" theme and its settings-style helpers, the mini-settings
popup, i18n, and the small chrome helpers (labels/icons/iconcache). See [[overview]]
for the whole app and [[state-and-config]] for what backs all this UI (`AppConfig`,
`ForzaApp`). Full control-layout rules live in [[STYLING-GUIDE]] — this doc only
covers the architecture that makes those rules possible.

## Tab structure & dispatch

`Tab` (`src/app.rs:345-355`) is a plain `PartialEq, Clone, Copy` enum: `Dashboard,
Backfire, Gearbox, PowerCurve, EngineSwaps, Coop, Settings, Changelog`. There is no
`Tab::ALL` array or `impl Tab` — instead the top tab bar and the dispatch match each
enumerate the variants by hand, so adding a tab means touching both by name.

- **Dispatch** — inside `impl eframe::App for ForzaApp::update`, the central panel
  matches `self.current_tab` and calls one function per tab (`src/app.rs:2267-2276`):
  `crate::ui::dashboard::show`, `ui::backfire::show_backfire`, `ui::gearbox::show_gearbox`,
  `ui::power_curve::show`, `ui::engine_swaps::show`, `ui::coop::show`, `ui::settings::show`,
  `ui::changelog::show`. Every tab body lives in its own module under `src/ui/`
  (`src/ui/mod.rs` just declares them: `backfire`, `changelog`, `coop`, `dashboard`,
  `engine_swaps`, `gearbox`, `power_curve`, `settings`) — one module per tab, each
  exposing a `show(ui, app: &mut ForzaApp)`-shaped entry point that owns everything
  drawn in that tab.
- **Tab bar** — drawn inline (no dedicated function) in the top panel
  (`src/app.rs:1355-1435`). Two hand-written arrays, `left` (Dashboard/PowerCurve/Coop/
  Backfire/Gearbox/EngineSwaps, `src/app.rs:1362-1369`) and `right` (Settings/Changelog,
  `src/app.rs:1370-1373`), pair each `Tab` with an icon glyph from `icons.rs` and feed
  the shared `tab_button` helper (`src/app.rs:370-423`) once per entry, under whichever
  `TopBarStyle` (`Legacy | Simple | Modern`) is active. `tab_title(tab: Tab)`
  (`src/app.rs:426-437`) supplies the English label used by the Modern-style "current
  tab pill" (`page_pill`). The pill reserves the widest tab title's width
  (`max_pill_width`) so the icon tabs after it don't jump as the current tab's name
  changes length — they shift only, and uniformly, once the bar itself gets narrow.

**To add a new tab**: add the `Tab` variant (`src/app.rs:345-355`), a new `src/ui/<name>.rs`
module with a `show` function + its `pub mod` line in `src/ui/mod.rs`, a tuple in
`left`/`right` (`src/app.rs:1362-1373`), a `tab_title` arm, a `CentralPanel` dispatch
arm (`src/app.rs:2267-2276`), and — if the tab needs its own mini-settings — an entry
in the popup's tab-selector array and a `PageSettingsTab::Tab(Tab::<name>)` arm (see
below). `Tab`, the tab-bar arrays, and the dispatch match are all shared scaffolding
in `app.rs`, so per this repo's sub-agent partitioning rule they're a one-agent-at-a-time
edit when work is parallelized across agents.

## Theme system & helpers

`src/theme.rs` is the single "Graphite" dark theme; `theme::apply(ctx)` installs its
`egui::Visuals` and type scale once at startup. Its contract: **reference chrome by
role token, never a hard-coded hex, at call sites** — tokens like `ACCENT`, `PANEL`,
`PANEL2`, `BORDER`, `TEXT`, `TEXT_DIM`, `TEXT_FAINT`, `DIM`, `FAINT`, `FIELD`, `BTN`,
`BTNBD`, `DANGER`, `GOOD`, plus a blue-tinted neutral scale via `steel(lum)` for
dashboard chrome (`TRACK`, `WELL`, `STROKE_DIM`, `STROKE_MID`). Semantic *data* colours
(tyre temperature, input bars, warning greens/reds) are deliberately the exception —
they encode meaning, not chrome, so they live at their call sites instead of in
`theme.rs`.

On top of the tokens, `theme.rs` exports the reusable settings-UI building blocks used
across every "category card" tab (Backfire, Gearbox, Co-Op, Power Curve, the
mini-settings popup, …):

- `card(ui, title, body)` — a bordered group with an uppercase accent-coloured
  `section_label` title and the fixed 8px trailing gap that separates stacked cards.
- `section_label(text)` — the accent/uppercase/bold `RichText` `card` uses for its
  title; called directly when a title is needed outside a card.
- `slider_row(ui, label, value, range, step, decimals, suffix)` — label in the left
  half, slider + fixed-width (`VALUE_W = 72.0`) `DragValue` spinner pinned to the right
  in the right half. Generic over any `egui::emath::Numeric`.
- `checkbox_row` / `checkbox_row_with` — a half-card-minimum, full-card-maximum
  checkbox (optionally with a control, e.g. a combobox, to its right), built on the
  custom-painted `styled_checkbox` (18px rounded box, ink-centred check glyph).

These helpers are what let a dozen visually distinct tabs (Backfire, Gearbox, Co-Op,
Power Curve, the cog popup) read as one consistent settings surface without each
reimplementing row layout. The exact spacing/width rules (why 8px, why `VALUE_W`,
column-split conventions, text input sizing) are documented once in [[STYLING-GUIDE]]
rather than duplicated here — read that before building a new settings-style tab.

## Mini-settings popup architecture

The cog button in the status bar (`src/app.rs:1490-1514`) and `Ctrl+S`
(`src/app.rs:1333-1340`) both open the same popup: they set `page_settings_open = true`
and `page_settings_tab = PageSettingsTab::Tab(self.current_tab)`, so the popup always
opens on the page for whichever tab is currently active.

The popup window fades to 0.5 opacity when the pointer isn't over it (animated via
`page_settings_opacity`, `src/app.rs`), so it doesn't obscure the dashboard while you
glance past it. The `minisettings_transparent` config flag (General page, default on)
disables the fade — when off the target opacity is pinned to 1.0.

- **`PageSettingsTab`** (`src/app.rs:359-363`) — `General` (global options) or
  `Tab(Tab)` (per-tab options), so the popup's own page selection piggybacks on the
  main `Tab` enum.
- **`DashboardSubTab`** (`src/app.rs:460-479`, `Default = General`) — one variant per
  Dashboard widget group: `General, Modules, Kmh, Gear, Rpm, SprintTimes, Tires,
  Suspension, Shift, Engine, GForce, Inputs, Boost, Graphs, MiniMap, Config`. A nested
  `MiniMapTab` (`General | Coop`, `src/app.rs:482-487`) further splits the MiniMap page.
- **No `page_settings_*` functions exist** — the whole popup body is one large inline
  `match` inside `if self.page_settings_open { … }` (`src/app.rs:1519-2265`): first
  `match self.page_settings_tab` (`src/app.rs:1550`) picks `General`
  (`src/app.rs:1551-1573`) vs `Tab(Tab::Dashboard)` (`src/app.rs:1574-2169`, which then
  runs a **second** `match self.page_dashboard_sub_tab` at `src/app.rs:1601` with one
  arm per `DashboardSubTab` variant) vs `Tab(Tab::PowerCurve)` (`src/app.rs:2170-2209`)
  vs `Tab(Tab::Gearbox)` (`src/app.rs:2210-2224`) vs a `_ =>` fallback
  (`src/app.rs:2225-2232`, "No options for this page") covering every tab without a
  dedicated arm (Backfire, EngineSwaps, Coop, Settings, Changelog).

**To add a mini-settings page for a new Dashboard widget**: add a `DashboardSubTab`
variant (`src/app.rs:460-479`), add it to the sub-tab label array
(`src/app.rs:1577-1593`), and add a matching arm to the nested match
(`src/app.rs:1601-2029`) with the controls (typically `theme::slider_row`/
`checkbox_row` calls against `self.config.<field>`). New per-widget config fields go
on `AppConfig` in `src/config.rs`, and — if the setting should travel with
export/presets — into `MINISETTINGS_KEYS` (see [[state-and-config]] / [[presets]]).
**To add a mini-settings page for a whole new top-level tab**, additionally add a
`PageSettingsTab::Tab(Tab::<name>)` arm before the `_ =>` fallback
(`src/app.rs:2225`) and an entry in the popup's own tab-selector array
(`src/app.rs:1536-1544`).

## i18n flow

`src/i18n.rs` is a small, deliberately non-generic i18n layer: English source strings
*are* the lookup keys. `Language` (`English | German`, `#[default] English`) is stored
in a process-wide `AtomicU8` (`CURRENT`, relaxed ordering — the whole immediate-mode
UI tree renders between two `set_language` calls, so a global is fine; see the
`ponytail:` comment at the top of the file for the upgrade path if that ever changes).

- `tr(s: &'static str) -> &'static str` is called at every user-facing string's call
  site. English passes `s` straight through; German looks it up in `de(s)`
  (`src/i18n.rs:75-688`), a single big `match` keyed by the exact English source
  string, grouped by area with `// ── Section ──` comments (tabs, page-settings,
  dashboard widgets, gearbox tab, Co-Op, …).
- **Fallback, not blank**: any string missing from `de()` (the `_ => return None` arm)
  falls back to the English source via `.unwrap_or(s)` in `tr` — nothing ever renders
  empty for want of a translation.
- **To add a translated string**: wrap the call site in `tr("Your English Text")`,
  then add `"Your English Text" => "German text",` to the `de()` match. Duplicate
  match arms (the same English key listed twice) are a compile-time warning in a plain
  `match`, which is the project's built-in guard against silently-overwritten
  translations.
- **To add a language**: add a `Language` enum variant + label, extend `Language::ALL`,
  add a branch in `current()`, and write a new lookup function alongside `de()`.

## Chrome helpers: labels, icons, iconcache

Three small modules provide non-widget visual chrome shared across tabs:

- **`src/icons.rs`** — Nerd Font (`GeistMonoNerdFont`) codepoint constants
  (`&'static str`, Font Awesome v4 range `U+F000`–`U+F2FF`): `DASHBOARD`, `BOLT`,
  `STOP`, `LINE_CHART`, `WRENCH`, `COG`, `CIRCLE`, `PLUG`, `NO_SIGNAL`, `PENCIL`,
  `SEARCH`, `FLOPPY`, `CHECK`, `TIMES`, `CLOCK`, `GEARBOX`, `ENGINE`, `USERS`, `COPY`, `GLOBE`,
  `LINK`, `PAUSE`, `BULLHORN`, etc. These are just glyph strings; call sites (tab bar,
  buttons, status indicators) draw them with `ui.painter().text(...)` or as part of a
  `RichText`, no rendering logic lives here.
- **`src/iconcache.rs`** — `IconCenterCache`, because anchoring an icon glyph with
  `Align2::CENTER_CENTER` centres its *layout* box, not its visible ink, which makes a
  row of different icons look ragged (each off by a different amount). It lays a
  glyph out once, reads the tight mesh bounds of the rendered triangles, and caches
  the resulting ink-centre offset per `(glyph, font size)`. Any call site drawing an
  icon in a fixed box calls `IconCenterCache::centered_pos(ui, icon, font, center)` to
  get the draw position, then paints at `Align2::LEFT_TOP` — this is the mechanism
  behind the "high contrast icons" / compact tab icon centering.
- **`src/labels.rs`** — `Labels`, a loader + renderer for the car-class (`class_d` …
  `class_x`, 8 textures) and drivetrain (`FWD`/`RWD`/`AWD`, 3 textures) badge images
  under `assets/labels/`, decoded once via `Labels::load(ctx)` and cached as
  `TextureHandle`s. `paint_class`/`paint_drivetrain` draw a badge at a given
  `top_left`/`scale`, snapping the rect to the physical pixel grid
  (`snap_rect`) so thin badge borders don't vanish when drawn small (e.g. in a list
  row) — `paint_class` additionally stamps the car's rating (PI) centred in the
  label's built-in number box when `rating > 0`. Used by the Car dashboard widget and
  the Co-Op on-map player list — see [[dashboard]] and [[coop]].

## See also

- [[overview]] — where the UI layer fits in the whole app (threads, per-frame loop,
  file map).
- [[state-and-config]] — `AppConfig`, `ForzaApp`, `MINISETTINGS_KEYS`/`LAYOUT_KEYS`
  that back every mini-settings control.
- [[STYLING-GUIDE]] — the full settings-style layout rules (spacing, column splits,
  reserved spinner width, text inputs) that the `theme.rs` helpers implement.
- [[dashboard]] — the widget grid that most `DashboardSubTab` pages configure.
