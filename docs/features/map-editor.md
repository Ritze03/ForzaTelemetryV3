# Map editor — the Setup → Map data card

The road-type map editor (2D editor, Preview mode, live 3D) is part of the app: the user opens
it from **Setup → Map data**, checks and fixes road types, and finished edits go back to the
project as a contribution (plan D50). Everything is built from the user's **own Forza Horizon 6
install** at runtime and nothing of the game is bundled or uploaded (licensing).

The editor itself, the server and the data formats are described in
[`game-data/fh6-map-tooling.md`](../game-data/fh6-map-tooling.md); this page covers the app side:
the card, the start modes and the rules around Save / reset / rebuild. Code: `map_data_card` /
`map_data_view` in `src/ui/settings.rs`, `ForzaApp::{start_map_editor, stop_map_editor, map_editor_*}`
in `src/app.rs`. Terms (*project*, *override*, *current*, *raw*): [terminology](../meta/TERMINOLOGY.md).

## The card

Second card under **Game Install** in the right column of the Setup tab. Status lines (wrapping
rows, never cut at the card edge) and then the controls:

- **Road types** — which road types the app uses: *Project data* or *Your saved file*. An
  amber dot adds the reason when the user's saved file was **ignored** (unreadable, or made for
  another version of the game's road network: `Current.note`, shown as the backend wrote it) and
  *Project data updated since your save* (`project_updated_since_save`; fix = **Reset road
  types to project data**). Computed on a background thread (`MapData::poll`, loads the nav
  graph, ~20 ms) while no editor server runs, and re-run when the install or the override file
  (mtime + size) changes; while a server runs its own `Current` is shown.
- **Editor** — *Not running* / *Preparing N %* / *Running* / *Failed: reason*. The last Save
  (*Saved: N edges, M points*) or error of the editor follows.
- **Start from** `Current | Raw` — *Current* = the user's saved road types, else the project
  data; *Raw* = the game's untouched road network without road types (tooltip text).
- **Open map editor** — starts the local server (builds the map data on a thread; the browser
  opens when it is ready) or re-opens a running one. Picking the other start mode while a server
  runs stops it and starts a new one (`start_map_editor` remembers the running mode).
  *Why:* the start mode is only applied to a fresh server; the old browser tab then stops
  working, which was accepted over adding a "reload with other data" request to the server.
- **Open data folder** — `<app data dir>/map_editor` (created if missing) in the file manager
  (`explorer` / `xdg-open` / `open`, fire and forget).
- **Stop map editor** — drops the local server (`stop_map_editor`); enabled only while the editor
  is Preparing / Ready, disabled with a tooltip otherwise. The status row then reads *Not running*;
  the last Save result stays. Why: frees the port and stops serving; unsaved browser edits are
  lost, hence the "Save first" tooltip.
- **Reset road types to project data** — deletes **only** the user's override file, after an
  inline confirm (*Delete your saved road types? Reset | Cancel*). Disabled with a tooltip when
  there is no saved file. A running editor keeps the old data in memory, so it is stopped; the
  card says to reopen it.
- **Rebuild map data** — deletes only `map_editor/cache/` (terrain and generated data) and stops
  a running server, so the next open regenerates everything. Disabled while the server is
  preparing. **It never touches the override file**, which is user data and lives next to the
  cache, not in it.
- **Contribute** — opens `CONTRIBUTING.md` on GitHub (see below).

Without an install the card shows *Needs your Forza Horizon 6 install* (red) and **Open map
editor** / **Rebuild map data** are disabled with a tooltip (like Rebuild Map Cache in the
Mini-Settings); *Open data folder*, *Contribute* and *Reset* stay available because they do not
read the game. No explanatory text sits under the controls: tooltips carry it
([styling guide](../ui/STYLING-GUIDE.md)). `map_data_card_stays_inside_its_pane`
(`FORZA_UI_SNAPSHOT_DIR` for PNGs) checks the card stays inside its column.

## Save, override, replace (D60)

**Save** in the editor (the Export button still downloads the file) writes
`<app data dir>/map_editor/fh6-road-types.user.json` and the app uses it at once: the server
updates the shared `Current` before replying (see the tooling doc). The override **replaces the
project file wholesale, there is no merge**. *Why:* the editor always saves the complete state,
and "this edge is now unset" / "this link was deleted" are expressed by absence, which an
overlay could not represent (the project's entry would come back). Save stamps `based_on` (the
SHA-1 of the embedded project file), which is how the card knows the project data was updated
after the user's save. An unreadable override or one for another nav graph is **ignored, never
deleted**, and the card says why.

**Race marks pass through.** The app build of the editor has no race lines, so marks cannot be
matched to routes; they are kept verbatim and written back, so a Save does not lose the 94
marks of the project file. *Why:* otherwise the first Save would silently wipe them.

## Server, security, port

The editor is served by a small local HTTP server (`src/mapedit/server.rs`) instead of
`file://`, so the 3D page and the Save request work. It listens on 127.0.0.1 only, behind a
random per-session token in the URL path, with Host / Origin / content-type checks and a CSP;
the URL contains the secret and is never shown or logged. Details and the *why* of each check:
[`fh6-map-tooling.md`](../game-data/fh6-map-tooling.md) ("Rust generators and the local server").

**Sticky port (D59):** the last port is stored in `map_editor/port` and tried first. *Why:* a
new port is a new browser origin, so the editor's `localStorage` (view, layers, language, ...)
would start empty every session. It is a file, not a config key, because it is per machine and
not a setting.

## Contributing (D53)

`CONTRIBUTING.md` (repo root) explains: edit, **Save** and **Export**, then send the file either
as a **pull request** replacing `assets/map/fh6-road-types.json` (preferred; the file has one
entry per line, so the diff shows only changed entries) or, for people who do not know pull
requests, as a **full file re-upload** (GitHub web "Upload files" on a fork, or attached to an
issue). *Why both:* a PR is what the maintainer wants, but a newcomer's full file is still
useful, so it is not turned away. The file holds ids and hand-placed points only, no game data.
The **Contribute** button opens that file on GitHub.
