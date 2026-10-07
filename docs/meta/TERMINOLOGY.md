# Terminology

Project-specific vocabulary. When the user uses a term defined here, use the same meaning.

**If the user uses a non-standard term that is not defined here, ask what they mean by it before acting on it.** Once its meaning is clear, add it to this file. Keep this file up to date automatically: whenever the user introduces or clarifies a term, add or amend the entry here.

## Terms

- **Mini-Settings** — the settings accessible through the cog wheel on the status bar.
- **Widget / Module** — an individual module on the Dashboard.
- **WSL overlay / WLR overlay** — the user's name for the **in-game HUD overlay**. "WSL" here means **wlr-layer-shell** (the Wayland protocol it's built on), **not** Windows Subsystem for Linux. It's the `src/overlay/` + `src/hud/` feature, configured in the Overlay tab. See `docs/features/overlay.md`.
- **HUD** — the in-game overlay's content drawn over the game (not the Dashboard). "Hide HUD" is its hotkey (default J). Not to be confused with a *config overlay* (preset/profile JSON merged onto the config).
- **HUD module** — one of the overlay's three placeable pieces: Minimap, Drive cluster, Race / Drift. Placed in a 3×3 grid on the Overlay tab. (Distinct from a Dashboard *Widget / Module*.)
- **Drive cluster** — the HUD module showing RPM, gear and speed together; styles shown in the UI as **Pill** and **Halo** (D1a / D3a′ are their mockup spec names).
- **Race block / Drift counter** — the HUD's race position + lap module (R1′) and the drift-score module (X1′) that takes its slot while drifting is detected. Drift counter styles: **Position + Gain** (default) and **Total**.
- **D1a / D3a′ / M2′ / R1′ / X1′** — the HUD widget names from the design mockup's spec sheet (Pill cluster, Halo cluster, minimap, race block, drift counter).
- **Category** — a bordered card with a blue uppercase title that groups related controls in a tab (e.g. "SESSION", "RPM Range"). Rendered via `theme::card`. See @docs/ui/STYLING-GUIDE.md.
- **Minimap** (also "minimap renderer") — the **HUD minimap (M2′)** in the in-game overlay (`src/hud/minimap.rs`). Not the Dashboard's map widget.
- **Dashboard map** — the Dashboard's Map widget (`show_minimap_widget`, `src/ui/dashboard.rs`; config keys are `minimap_*` for historical reasons). Distinct from the HUD **Minimap**.
- **Turnaround** — a nav link that exists only so the game's AI can get back onto the right road (roads look cross-connected, the in-game map does not show it). Marked in the road editor for later navigation, hidden from the 2D preview. See `docs/game-data/fh6-map-tooling.md`.
- **Trail / Cross-country / Highway / Jump line / Tunnel** — road-editor edge types: small game trails (dashed) / user-drawn line for things not in the game, for routing / motorway marked apart from Road / one-way take-off to landing, routing may opt in / counts as asphalt for routing.
- **Project file / override / current / raw** — road-type data states (D57, D60): the **project file** is the committed, embedded `assets/map/fh6-road-types.json`; the **override** is the user's own saved copy (`<app_data>/map_editor/fh6-road-types.user.json`); **current** = project, *replaced wholesale* by the override when that is valid (no per-entry merge); **raw** = the bare nav graph with no types. See `docs/game-data/fh6-map-tooling.md`.
- **Map data** — the Setup tab category (below Game Install) that opens the road-type map editor and shows/resets/rebuilds its data (the road types in use, the editor server, the cache). Not the Dashboard map's tiles or the Mini-Settings *Rebuild Map Cache*. See `docs/features/map-editor.md`.
