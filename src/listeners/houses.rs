//! The player houses on the listener side, for the experimental pause rule (D103, see
//! [`crate::listeners::hud::garage_paused`]).
//!
//! The positions come from the map layers' POIs (`PoiKind::House`, 8 on the island). The
//! listener thread must not call `maprender::store::layers()` per packet: it is one short lock
//! + an `Arc` clone + a `stat` (at most once a second inside the store). So [`HousePoller`]
//! asks at most once per [`POLL_SECS`] and only while the setting is on, and keeps the
//! 8-entry list in a small `Arc<[House]>` that every per-packet caller borrows.
//!
//! Fail closed: no install, still loading or no houses → the list is empty → "not in a house"
//! → the experimental rule never fires, so the HUD cannot hide by mistake.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::gamedata::poi::{Poi, PoiKind};
use crate::maprender::data::PoiLayer;

/// One player house marker (packet frame: x/z ground plane, y up).
#[derive(Debug, Clone, PartialEq)]
pub struct House {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// POI slug (`Poi::name`).
    pub name: String,
}

/// How often the layers are asked for houses. Houses never move; the only reason to ask again
/// is the first load finishing or a rebuild, and a second of delay on either is invisible.
pub const POLL_SECS: f32 = 1.0;

/// The `House` POIs of `items` as [`House`]s.
pub fn houses_of(items: &[Poi]) -> Vec<House> {
    items
        .iter()
        .filter(|p| p.kind == PoiKind::House)
        .map(|p| House { x: p.x, y: p.y, z: p.z, name: p.name.clone() })
        .collect()
}

/// The house closest to the car (3D distance, m), whatever the range.
pub fn nearest_house<'a>(pkt: &crate::packet::ForzaPacket, houses: &'a [House]) -> Option<(&'a House, f32)> {
    let d = |h: &House| {
        let (dx, dy, dz) = (pkt.position_x - h.x, pkt.position_y - h.y, pkt.position_z - h.z);
        (dx * dx + dy * dy + dz * dz).sqrt()
    };
    houses.iter().map(|h| (h, d(h))).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// The list the worker last published, for the Debug tab (UI thread).
static CURRENT: LazyLock<Mutex<Arc<[House]>>> = LazyLock::new(|| Mutex::new(Arc::from([])));

/// The houses the listener currently uses (empty before the map data is up).
pub fn current() -> Arc<[House]> {
    CURRENT.lock().map(|g| g.clone()).unwrap_or_else(|_| Arc::from([]))
}

fn publish(h: &Arc<[House]>) {
    if let Ok(mut g) = CURRENT.lock() {
        *g = h.clone();
    }
}

/// Rate-limited refresh of the house list from the map layers. Worker-thread state.
#[derive(Default)]
pub struct HousePoller {
    last_poll: Option<Instant>,
    /// The `PoiLayer` the current list was built from (pointer-compared, so a rebuild with
    /// the same POIs costs one comparison, not a re-copy).
    source: Option<Arc<PoiLayer>>,
}

impl HousePoller {
    /// Call every loop pass; does real work at most every [`POLL_SECS`]. `out` is replaced
    /// when the POIs changed. `enabled == false` does nothing (and never starts the layer
    /// load, which only a call to `layers()` would).
    pub fn tick(&mut self, enabled: bool, now: Instant, out: &mut Arc<[House]>) {
        self.tick_with(enabled, now, out, || crate::maprender::store::layers().data.map(|d| d.pois.clone()));
    }

    fn tick_with(
        &mut self,
        enabled: bool,
        now: Instant,
        out: &mut Arc<[House]>,
        fetch: impl FnOnce() -> Option<Arc<PoiLayer>>,
    ) {
        if !enabled {
            return;
        }
        if self.last_poll.is_some_and(|t| now.duration_since(t) < Duration::from_secs_f32(POLL_SECS)) {
            return;
        }
        self.last_poll = Some(now);
        let pois = fetch();
        let same = match (&pois, &self.source) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        *out = pois.as_ref().map_or_else(|| Arc::from([]), |p| houses_of(&p.items).into());
        self.source = pois;
        publish(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poi(kind: PoiKind, name: &str, x: f32) -> Poi {
        Poi { kind, x, z: 0.0, y: 1.0, name: name.into(), n: 0, gate: None }
    }

    #[test]
    fn only_house_pois_are_kept() {
        let h = houses_of(&[poi(PoiKind::Estate, "e", 1.0), poi(PoiKind::House, "Futurist", 2.0), poi(PoiKind::FastTravel, "player_house_a", 3.0)]);
        assert_eq!(h, vec![House { x: 2.0, y: 1.0, z: 0.0, name: "Futurist".into() }]);
    }

    #[test]
    fn poll_is_rate_limited_and_only_when_enabled() {
        let layer = Arc::new(PoiLayer::from_items(vec![poi(PoiKind::House, "a", 5.0)]));
        let mut p = HousePoller::default();
        let mut out: Arc<[House]> = Arc::from([]);
        let t0 = Instant::now();
        let calls = std::cell::Cell::new(0);
        let fetch = || {
            calls.set(calls.get() + 1);
            Some(layer.clone())
        };
        p.tick_with(false, t0, &mut out, fetch);
        assert_eq!((calls.get(), out.len()), (0, 0), "off: no fetch");
        p.tick_with(true, t0, &mut out, fetch);
        assert_eq!((calls.get(), out.len()), (1, 1));
        p.tick_with(true, t0 + Duration::from_millis(500), &mut out, fetch);
        assert_eq!(calls.get(), 1, "within the poll interval");
        p.tick_with(true, t0 + Duration::from_millis(1100), &mut out, fetch);
        assert_eq!(calls.get(), 2);
        // Data gone (e.g. a different state): fail closed.
        p.tick_with(true, t0 + Duration::from_secs(3), &mut out, || None);
        assert!(out.is_empty());
    }

    /// Real install: the 8 houses load and the captured garage sample is within 25 m of one.
    /// `FH6_INSTALL_DIR=... cargo test real_install_garage -- --nocapture`
    #[test]
    fn real_install_garage_sample_is_in_a_house() {
        let Some(media) = crate::gamedata::install::find_media(None) else {
            eprintln!("SKIP real_install_garage_sample_is_in_a_house: FH6 install not found (set FH6_INSTALL_DIR)");
            return;
        };
        let pois = crate::gamedata::poi::Pois::load(&media).expect("pois");
        let layer = PoiLayer::from_pois(&pois);
        let houses = houses_of(&layer.items);
        for h in &houses {
            eprintln!("house {} at ({}, {}, {})", h.name, h.x, h.y, h.z);
        }
        assert_eq!(houses.len(), 8);
        let pkt = crate::packet::ForzaPacket {
            position_x: -3761.010,
            position_y: 481.438,
            position_z: -2952.392,
            hand_brake: 255,
            gear: 1,
            ..Default::default()
        };
        let (h, d) = crate::listeners::hud::garage_paused(&pkt, &houses).expect("sample is in a house");
        eprintln!("{} at {d:.2} m", h.name);
        assert!((d - 9.1).abs() < 0.2, "{d}");
    }
}
