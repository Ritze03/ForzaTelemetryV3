//! Data read at runtime from the user's own Forza Horizon 6 install (never shipped, never
//! decrypted). See `docs/game-data/` for the file formats and `docs/game-data/fh6-cars-names-icons.md`
//! for the car-name pipeline.

// The map-editor data layer (I25), consumed by `mapedit::data` (I26a) and the local server
// (I26b); burg and lz4 are fully used through it. nav / pgzp / roadtypes / terrain keep their
// dead_code allowances (checked after I26b: removing them still warns) for the few leftovers only
// I27's Setup card reads (`Current::note` & co, `Elevation::valid_fraction`) or nothing does yet
// (`Nav::edges`, `RoadType::from_index`, `Terrain` stats). Remove them as those land.
pub mod burg;
// BC7 decoder + the POI icon atlas (I28b): `PoiIcons::load` is what the next renderer task (I29b)
// calls; nothing consumes them yet.
#[allow(dead_code)]
pub mod bc7;
pub mod cars;
#[allow(dead_code)]
pub mod icons;
pub mod install;
pub mod lz4;
#[allow(dead_code)]
pub mod nav;
#[allow(dead_code)]
pub mod pgzp;
// POIs and race lines (I28): consumed by the map renderer (`maprender::data`, I29a). The allows stay
// for what it doesn't read yet (checked with I29a merged: removing them warns): `Poi::{y, name, n,
// gate}`, `Region`, `Pois::{of, current_treasure_chest, load_danger_signs}` + the week helpers
// (I29b wires the danger signs, chest and gate lines), `racelines::race_pins` and the unread
// `RaceLine` fields.
#[allow(dead_code)]
pub mod poi;
pub mod process;
#[allow(dead_code)]
pub mod racelines;
#[allow(dead_code)]
pub mod roadtypes;
pub mod strtable;
#[allow(dead_code)]
pub mod terrain;
pub mod tiles;
#[cfg(windows)]
pub mod winsys;

/// A fresh, empty temp folder for a test (unique per process + thread).
#[cfg(test)]
pub(crate) fn tempdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("fh6test_{tag}_{}_{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}
