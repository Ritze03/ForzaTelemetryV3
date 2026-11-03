//! The shared map renderer (phase J, D61): one 2D renderer for the Dashboard map and the HUD
//! minimap, fed with the same world data, parameterised per map.
//!
//! ```text
//! cfg      MapLayerConfig and friends: what a map draws (serde; Dashboard + HUD defaults, D62)
//! data     render-ready layer data: road chains per type, POIs + cell grid, race lines + segment grid
//! store    process-wide loader / cache of that data (thread "map-layers"), keyed on install + override file
//! icontex  the POI icon atlas uploaded per egui context (Dashboard and HUD each have their own)
//! view     Camera (flat, tilted, or with a Relief the 3D camera), world boxes, thinning, polygon clipping
//! terrain  3D: the filled 8 m height grid (K1), hole fill + sea skirt shared with the map editor
//! mesh3d   3D: the road mesh (resampled ribbons with decks, tiles, LOD sets, in-race focus flags), pure CPU (K1)
//! gl3d     3D: the GL renderer of both (clipmap terrain + ribbon roads, per-context state, soft fallback to 2D), K2
//! style    draw order, zoom-dependent widths, dash patterns, the POI category table
//! racesel  which race lines to draw ("current race" is a best-effort guess) + the in-race road focus
//! paint2d  draw_base + draw_layers: the egui Painter output
//! ui       the settings cards of all of the above (the Map tab's three map pages, D63)
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
//! * **3D = the same camera plus heights (phase K, D61, D51).** The tilted view is a pinhole
//!   camera on a flat world; `Camera::relief` adds terrain heights, so `Camera::project` (the call
//!   every layer already makes) follows the ground with no call-site change, and at height 0 it
//!   *is* the tilt maths (tested to 1e-3 px). Terrain (`terrain`) and road mesh (`mesh3d`) are pure
//!   CPU data built lazily on their own threads (`store::terrain` / `store::road_mesh`) only while a
//!   map is in 3D mode; the GL scene that draws them is `gl3d` (K2). `Camera` lost
//!   `Copy` for the `Arc<Terrain>`: see `view` and `docs/features/minimap.md` ("3D: data and camera").
//! * **Turnarounds are never drawn (D52)**: they exist so the game's AI can route, not for
//!   people; no setting exists for them.
//! * **POI icons (D64).** The loader thread reads the game's icons once (`MapLayers::icons`, CPU
//!   pixels); each context uploads them itself (`icontex::IconTex`, no GL object is shared) into
//!   a `paint2d::IconAtlas`: a texture id plus a UV rect per category. A category without an
//!   icon, or no icons at all, is drawn as a coloured shape marker.

pub mod cfg;
pub mod data;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod gl3d;
pub mod icontex;
pub mod mesh3d;
pub mod paint2d;
pub mod racesel;
pub mod store;
pub mod style;
pub mod terrain;
pub mod ui;
pub mod view;

// The everyday API; the rest (`cfg::MapLayerConfig`, `data::MapLayers`, `paint2d::{IconAtlas,
// CornerClip, ImageLook}`, `view::*`) is reached through its module.
pub use paint2d::{draw_base, BaseParams, LayerCtx};
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
