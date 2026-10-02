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
