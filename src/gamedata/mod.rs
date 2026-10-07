//! Data read at runtime from the user's own Forza Horizon 6 install (never shipped, never
//! decrypted). See `docs/game-data/` for the file formats and `docs/game-data/fh6-cars-names-icons.md`
//! for the car-name pipeline.

// The map-editor data layer (I25), consumed by `mapedit::data` (I26a); burg and lz4 are fully
// used through it. nav / pgzp / roadtypes / terrain keep their dead_code allowances for the parts
// only the later consumers (I26b server, I27 Setup card) call. Remove them as those land.
pub mod burg;
pub mod cars;
pub mod install;
pub mod lz4;
#[allow(dead_code)]
pub mod nav;
#[allow(dead_code)]
pub mod pgzp;
pub mod process;
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
