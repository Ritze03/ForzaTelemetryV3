//! The FH6 map editor inside the app (plan D50): the Leaflet editor + three.js 3D page are
//! served from a small local HTTP server, with all game data generated here in Rust from the
//! user's install (D54: no Python in the app). I26a = [`data`] (the generators), I26b = [`server`] (the local
//! web server + Save); the Setup UI is I27.

pub mod data;
pub mod server;

pub use server::{MapEvent, MapServer, MapServerState, StartFrom};
