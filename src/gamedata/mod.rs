//! Data read at runtime from the user's own Forza Horizon 6 install (never shipped, never
//! decrypted). See `docs/game-data/` for the file formats and `docs/game-data/fh6-cars-names-icons.md`
//! for the car-name pipeline.

pub mod cars;
pub mod install;
pub mod strtable;
pub mod process;
#[cfg(windows)]
pub mod winsys;
