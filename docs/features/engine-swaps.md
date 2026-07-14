# Engine Swaps

A display-only reference table of every engine swap available in Forza Horizon 6.
No automation, no telemetry involvement — it's a searchable lookup table.

## Where it lives

- `src/engines.rs` — `EngineRecord` struct + `load_engines()`, which parses
  `assets/engines.csv` (baked into the binary via `include_str!`) with the `csv` crate.
- `src/ui/engine_swaps.rs` — the tab's `show()` function.
- `assets/engines.csv` — the data itself: `engine_label,source_vehicle,engine_name,horsepower`
  header + one row per swap (75 lines total, so 74 engines as of writing).

## Data model

```rust
pub struct EngineRecord {
    pub engine_label: String,    // e.g. "I4 Motorbike Engine"
    pub source_vehicle: String,  // e.g. "Suzuki Hayabusa"
    pub engine_name: String,     // e.g. "Suzuki 1340cc Hayabusa I4"
    pub horsepower: u32,
}
```

Rows that fail to deserialize are silently dropped (`filter_map(|r| r.ok())`) rather
than erroring the whole load.

`ForzaApp` loads the CSV once at startup into `app.engines: Vec<EngineRecord>`; the tab
just reads that list — no reload/refresh path since the CSV never changes at runtime.

## UI

- A search box (`app.engine_search`, plain `String` state on `ForzaApp`) filters rows by
  substring match (case-insensitive) across `engine_label` + `source_vehicle` +
  `engine_name` combined. A small "×" button clears it.
- A live count ("`N` engines") next to the search box reflects the filtered result count.
- Results render in a 4-column striped `egui::Grid` (In-Game Label / Source Vehicle /
  Engine Name / HP), scrollable vertically. Horsepower is highlighted in a yellow/gold
  colour; source vehicle is dimmed gray.

## Notes

- Nothing here touches the live UDP packet or car identity fields (`CarOrdinal` etc.) —
  it's independent reference data, not tied to whatever car is currently being driven.
- Not part of any preset/mini-settings key list (see [[presets]]) since it has no
  persisted configuration — only transient search-box text.
