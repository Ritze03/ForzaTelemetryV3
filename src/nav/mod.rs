//! Navigation: route from the car to a destination over the game's road network. This module is
//! the **routing core** (phase L1): a routable graph built from the nav roads + the road-type
//! data, the user's road-type filters and "faster roads <-> more curves" preference, snapping a
//! world position to the graph, and an A* search. Pure CPU, no egui, no GL: it compiles on every
//! platform and is meant to be called from a worker thread. See `docs/features/navigation.md`.
//!
//! * [`graph`] - [`RouteGraph`], built next to the drawn roads in `maprender::data::GameData::layers`
//!   (shared node positions) and carried as `MapLayers::route_graph`.
//! * [`cfg`] - [`RouteFilters`] / [`RoutePrefs`] / [`NavConfig`]: what the user chooses (and its
//!   one-byte wire form for co-op).
//! * [`cost`] - the assumed speeds and the per-arc cost ([`CostModel`]).
//! * [`snap`] - [`Snap`]: a world position -> a point on an edge.
//! * [`search`] - [`Route`] / [`RouteError`] and the A* ([`RouteGraph::route`], [`RouteGraph::plan`]).
//!
//! The **runtime** (L2) keeps a live route from the car: [`follow`] (progress along a route,
//! off-route / arrival rules; pure) and [`state`] (the listener-thread [`Tracker`], the
//! `nav-route` worker thread, the process-global [`view()`] and the input setters).

// The listener thread uses `Tracker` / `CarSample` / `SharedIn` (L2); the input API, `view()`, the
// config and most graph accessors are consumed by the drawing (L3) and the tab (L5): drop this
// when those land.
#![allow(dead_code, unused_imports)]

pub mod cfg;
pub mod cost;
pub mod follow;
pub mod graph;
pub mod search;
pub mod snap;
pub mod state;

#[cfg(test)]
mod tests;

pub use cfg::{NavConfig, RouteFilters, RoutePrefs};
pub use cost::CostModel;
pub use graph::{Edge, RouteGraph};
pub use search::{Endpoint, Route, RouteError};
pub use snap::Snap;
pub use state::{
    local_destination, set_destination, set_follow_shared, set_prefs, view, CarSample, Dest, DestSource, NavLine, NavStatus, NavView, SharedIn, Tracker,
};
