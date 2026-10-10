//! What the user chooses for navigation: which road types may be driven ([`RouteFilters`], the
//! six checkboxes of D83), the "faster roads <-> more curves" slider and the persisted
//! [`NavConfig`]. `AppConfig` gets the field from the Navigation tab task (L5); the filter bits
//! and the curve value are also what a co-op shared destination carries (D85).

use serde::{Deserialize, Serialize};

use crate::gamedata::roadtypes::RoadType;
use crate::maprender::data::N_TYPES;

/// The one-byte wire layout of [`RouteFilters`] (co-op `Dest.f`, D85). Defined here, not imported
/// from `coop.rs`, so `nav` stays standalone; a test pins them to `coop::DEST_*` by value
/// (coop.rs: `DEST_ROAD` .. `DEST_JUMPS`, `DEST_FILTER_MASK`).
pub const BIT_ROAD: u8 = 1;
pub const BIT_HIGHWAY: u8 = 2;
pub const BIT_DIRT: u8 = 4;
pub const BIT_TRAIL: u8 = 8;
pub const BIT_CROSS_COUNTRY: u8 = 16;
pub const BIT_JUMPS: u8 = 32;
/// Bits 6-7 are reserved.
#[cfg_attr(not(test), allow(dead_code))] // documents the layout; a test pins it to `coop::DEST_FILTER_MASK`
pub const BITS_MASK: u8 = 0x3F;

/// Which road types the route may use. `Copy` and `serde(default)` (an old or partial config
/// keeps the defaults for what it lacks).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct RouteFilters {
    /// Roads, plus tunnels, "other" and untyped edges (see [`RouteFilters::allows`]).
    pub road: bool,
    pub highway: bool,
    /// Offroad / dirt roads (`RoadType::Offroad`).
    pub dirt: bool,
    pub trail: bool,
    pub cross_country: bool,
    /// Jump lines, take-off -> landing only.
    pub jumps: bool,
}

impl Default for RouteFilters {
    /// D92: what a normal road drive wants. Trail / cross-country / jumps are opt-in (a jump is
    /// a risk of a wrecked landing).
    fn default() -> Self {
        RouteFilters { road: true, highway: true, dirt: true, trail: false, cross_country: false, jumps: false }
    }
}

impl RouteFilters {
    /// Everything on (tests, "no filtering").
    #[cfg(test)]
    pub const ALL: RouteFilters = RouteFilters { road: true, highway: true, dirt: true, trail: true, cross_country: true, jumps: true };
    /// Nothing on: no edge may be used.
    #[cfg(test)]
    pub const NONE: RouteFilters = RouteFilters { road: false, highway: false, dirt: false, trail: false, cross_country: false, jumps: false };

    /// The wire / one-byte form (D85 `Dest.f`): road 1, highway 2, dirt 4, trail 8,
    /// cross-country 16, jumps 32.
    pub fn to_bits(self) -> u8 {
        [(self.road, BIT_ROAD), (self.highway, BIT_HIGHWAY), (self.dirt, BIT_DIRT), (self.trail, BIT_TRAIL), (self.cross_country, BIT_CROSS_COUNTRY), (self.jumps, BIT_JUMPS)]
            .into_iter()
            .fold(0, |m, (on, bit)| if on { m | bit } else { m })
    }

    /// Inverse of [`to_bits`](Self::to_bits); bits outside [`BITS_MASK`] (a newer app's filters,
    /// the reserved bits 6-7) are ignored.
    pub fn from_bits(b: u8) -> RouteFilters {
        let on = |bit: u8| b & bit != 0;
        RouteFilters { road: on(BIT_ROAD), highway: on(BIT_HIGHWAY), dirt: on(BIT_DIRT), trail: on(BIT_TRAIL), cross_country: on(BIT_CROSS_COUNTRY), jumps: on(BIT_JUMPS) }
    }

    /// May an edge of this kind (`RoadType::index`, 0 = no type) be driven?
    ///
    /// | kind | allowed when | why |
    /// |---|---|---|
    /// | Road | `road` | |
    /// | Highway | `highway` | |
    /// | Offroad | `dirt` | |
    /// | Trail / Cross-country | `trail` / `cross_country` | |
    /// | Jump | `jumps` | (and only take-off -> landing, a property of the graph's arcs) |
    /// | Tunnel | `road` **or** `highway` | D37: counts as asphalt; the data does not say which kind of road the tunnel is on |
    /// | Other, no type | `road` | "not classifiable" is treated as paved; a raw / partly typed override has every edge untyped and must still route |
    /// | Turnaround | never | a U-turn crossover; no one-way data, so both carriageways are usable already and a crossover adds nothing |
    pub fn allows(&self, kind: u8) -> bool {
        match RoadType::from_index(kind) {
            None | Some(RoadType::Road) | Some(RoadType::Other) => self.road,
            Some(RoadType::Highway) => self.highway,
            Some(RoadType::Offroad) => self.dirt,
            Some(RoadType::Trail) => self.trail,
            Some(RoadType::Crosscountry) => self.cross_country,
            Some(RoadType::Jump) => self.jumps,
            Some(RoadType::Tunnel) => self.road || self.highway,
            Some(RoadType::Turnaround) => false,
        }
    }

    /// [`allows`](Self::allows) as a bit mask: bit `k` for kind `k` (0..=9).
    pub fn mask(&self) -> u16 {
        (0..N_TYPES as u8).filter(|&k| self.allows(k)).fold(0, |m, k| m | 1 << k)
    }
}

/// Everything a route search depends on besides the endpoints.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(default)]
pub struct RoutePrefs {
    pub filters: RouteFilters,
    /// 0.0 = fastest roads (default) .. 1.0 = most curves. Out-of-range / NaN is clamped by
    /// [`RoutePrefs::curves`].
    pub curves: f32,
}

impl RoutePrefs {
    /// The slider value, clamped to 0..=1 (NaN = 0).
    pub fn curves(&self) -> f32 {
        if self.curves.is_nan() {
            0.0
        } else {
            self.curves.clamp(0.0, 1.0)
        }
    }
}

/// The persisted navigation settings (`AppConfig.nav`, added by the Navigation tab task).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct NavConfig {
    pub filters: RouteFilters,
    /// 0.0 = fastest road .. 1.0 = most curves.
    pub curves: f32,
    /// World x, z of this player's own destination. Persisted so a trip survives a restart; the
    /// route is cheap to recompute.
    pub destination: Option<[f32; 2]>,
    /// Co-op: send this player's destination to the room.
    pub share_destination: bool,
    /// Co-op: adopt the room's shared destination.
    pub follow_shared: bool,
}

impl Default for NavConfig {
    fn default() -> Self {
        NavConfig { filters: RouteFilters::default(), curves: 0.0, destination: None, share_destination: true, follow_shared: true }
    }
}

impl NavConfig {
    pub fn prefs(&self) -> RoutePrefs {
        RoutePrefs { filters: self.filters, curves: self.curves }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_d92() {
        let f = RouteFilters::default();
        assert_eq!((f.road, f.highway, f.dirt, f.trail, f.cross_country, f.jumps), (true, true, true, false, false, false));
        let c = NavConfig::default();
        assert_eq!((c.curves, c.destination, c.share_destination, c.follow_shared), (0.0, None, true, true));
    }

    /// The values `coop.rs` puts on the wire (`DEST_ROAD`=1, `DEST_HIGHWAY`=2, `DEST_DIRT`=4,
    /// `DEST_TRAIL`=8, `DEST_CROSS_COUNTRY`=16, `DEST_JUMPS`=32, `DEST_FILTER_MASK`=0x3F). If one
    /// side changes, this and coop's own test must change together.
    #[test]
    fn wire_constants_match_coop() {
        assert_eq!([BIT_ROAD, BIT_HIGHWAY, BIT_DIRT, BIT_TRAIL, BIT_CROSS_COUNTRY, BIT_JUMPS, BITS_MASK], [1, 2, 4, 8, 16, 32, 0x3F]);
        assert_eq!(RouteFilters::ALL.to_bits(), BITS_MASK);
        assert_eq!(RouteFilters::from_bits(0xC0 | BIT_TRAIL), RouteFilters { trail: true, ..RouteFilters::NONE }, "reserved bits ignored");
    }

    #[test]
    fn bits_round_trip_and_ignore_unknown() {
        for b in 0..64u8 {
            assert_eq!(RouteFilters::from_bits(b).to_bits(), b);
        }
        assert_eq!(RouteFilters::from_bits(0xff), RouteFilters::ALL);
        assert_eq!(RouteFilters::from_bits(0b1100_0000), RouteFilters::NONE);
        // the documented layout
        assert_eq!(RouteFilters { road: true, ..RouteFilters::NONE }.to_bits(), 1);
        assert_eq!(RouteFilters { highway: true, ..RouteFilters::NONE }.to_bits(), 2);
        assert_eq!(RouteFilters { dirt: true, ..RouteFilters::NONE }.to_bits(), 4);
        assert_eq!(RouteFilters { trail: true, ..RouteFilters::NONE }.to_bits(), 8);
        assert_eq!(RouteFilters { cross_country: true, ..RouteFilters::NONE }.to_bits(), 16);
        assert_eq!(RouteFilters { jumps: true, ..RouteFilters::NONE }.to_bits(), 32);
    }

    /// The design's filter -> type table, switch by switch.
    #[test]
    fn filter_to_type_table() {
        let idx = |t: RoadType| t.index();
        let only = |f: fn(&mut RouteFilters)| {
            let mut x = RouteFilters::NONE;
            f(&mut x);
            x
        };
        let road = only(|f| f.road = true);
        for t in [RoadType::Road, RoadType::Other, RoadType::Tunnel] {
            assert!(road.allows(idx(t)), "{t:?}");
        }
        assert!(road.allows(0), "unset follows road");
        for t in [RoadType::Highway, RoadType::Offroad, RoadType::Trail, RoadType::Crosscountry, RoadType::Jump, RoadType::Turnaround] {
            assert!(!road.allows(idx(t)), "{t:?}");
        }
        let hw = only(|f| f.highway = true);
        assert!(hw.allows(idx(RoadType::Highway)) && hw.allows(idx(RoadType::Tunnel)));
        assert!(!hw.allows(idx(RoadType::Road)) && !hw.allows(idx(RoadType::Other)) && !hw.allows(0));
        assert!(only(|f| f.dirt = true).allows(idx(RoadType::Offroad)));
        assert!(only(|f| f.trail = true).allows(idx(RoadType::Trail)));
        assert!(only(|f| f.cross_country = true).allows(idx(RoadType::Crosscountry)));
        assert!(only(|f| f.jumps = true).allows(idx(RoadType::Jump)));
        // turnarounds are never routable, whatever is on
        assert!(!RouteFilters::ALL.allows(idx(RoadType::Turnaround)));
        assert_eq!(RouteFilters::NONE.mask(), 0);
        assert_eq!(RouteFilters::ALL.mask(), 0b11_1111_1111 & !(1 << idx(RoadType::Turnaround)));
    }

    #[test]
    fn old_and_partial_json_keep_defaults() {
        let c: NavConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c, NavConfig::default());
        let c: NavConfig = serde_json::from_str(r#"{"filters":{"trail":true},"curves":0.4,"destination":[1.0,2.0]}"#).unwrap();
        assert!(c.filters.trail && c.filters.road && !c.filters.jumps);
        assert_eq!((c.curves, c.destination), (0.4, Some([1.0, 2.0])));
        let back: NavConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn curves_is_clamped() {
        let p = |c| RoutePrefs { curves: c, ..Default::default() }.curves();
        assert_eq!((p(-1.0), p(0.3), p(7.0), p(f32::NAN)), (0.0, 0.3, 1.0, 0.0));
    }
}
