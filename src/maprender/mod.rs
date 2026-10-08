//! The shared map renderer (phase J, D61): one 2D renderer for the Dashboard map and the HUD
//! minimap, fed with the same world data, parameterised per map.
//!
//! ```text
//! cfg      MapLayerConfig and friends: what a map draws (serde; Dashboard + HUD defaults, D62)
//! data     render-ready layer data: road chains per type, POIs + cell grid, race lines + segment grid
//! store    process-wide loader / cache of that data (thread "map-layers"), keyed on install + override file
//! view     Camera (flat or tilted), world boxes, thinning, polygon clipping
//! style    draw order, zoom-dependent widths, dash patterns, the POI category table
//! racesel  which race lines to draw ("current race" is a best-effort guess)
//! paint2d  draw_base + draw_layers: the egui Painter output
//! ```
//!
//! # Why this shape
//!
//! * **One renderer, two call sites (D61).** The user wants the HUD to look like the Dashboard
//!   and a later 3D mode to be shared too, so the drawing lives here and the maps only differ in
//!   the parameters they pass (`MapLayerConfig::dashboard()` / `::hud()`, the outline polygon,
//!   the size factor). `hud::map_shared` shares the *markers*; this module shares the *world*.
//! * **Process-global store, not a field of the app or the HUD snapshot.** The UI frame loop
//!   stops while the game covers the window, which is exactly when the HUD is used; anything
//!   the UI thread has to forward would stall then. Both maps poll the store themselves and one
//!   load serves both. See `store` for the cache keys.
//! * **CPU `Painter`, no baking.** Roads are one `Shape::line` per visible chain with 2 px
//!   thinning: 1.6 ms for the whole island in the design benchmark, and road types can change
//!   on every editor Save, so nothing is baked into textures.
//! * **`Camera` with pitch (D65, K).** The tilted view is a flat perspective of the 2D map done
//!   on the CPU (a subdivided map mesh plus projecting every layer vertex), and its camera is
//!   the one phase K's GL 3D scene reuses; pitch 0 is exactly `minimap::MapView`.
//! * **Turnarounds are never drawn (D52)**: they exist so the game's AI can route, not for
//!   people; no setting exists for them.
//! * **POI icons are a hook (D64).** `paint2d::IconAtlas` is a texture id plus a UV rect per
//!   category; without one, coloured markers are drawn.

pub mod cfg;
pub mod data;
pub mod paint2d;
pub mod racesel;
pub mod store;
pub mod style;
pub mod view;

// The everyday API; the rest (`cfg::MapLayerConfig`, `data::MapLayers`, `paint2d::{IconAtlas,
// CornerClip, ImageLook}`, `view::*`) is reached through its module.
pub use paint2d::{draw_base, draw_layers, BaseParams, LayerCtx};
pub use racesel::RaceSel;
pub use store::{layers, refresh_now, LayerStatus};
pub use view::Camera;

use egui::TextureId;

/// An uploaded season map: texture plus the original image size its calibration is in.
#[derive(Clone, Copy, Debug)]
pub struct MapTex {
    pub id: TextureId,
    pub orig_size: [u32; 2],
    pub winter: bool,
}
