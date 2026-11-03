//! The game's own map icons for the POI layer, read at runtime from the user's install (never
//! bundled; Playground Games' art). A port of the *selection* half of `tools/fh6-extract/`
//! `extract_icons.py` + `build_icon_mapping.py`: ~45 of the 1014 swatchbins in
//! `UI/Textures/HiRes/Data_Bound/Horizon_Map.zip` are decoded (BC7, [`super::bc7`]), scaled down
//! and packed into one small RGBA atlas. See `docs/game-data/fh6-cars-names-icons.md`
//! ("Rust icon reader").
//!
//! Egui-free: [`PoiIcons`] is plain pixels plus UV rectangles; the renderer uploads the atlas.
//!
//! Where the icons come from:
//! * a swatchbin file of `Horizon_Map.zip` ([`Src::File`]), or
//! * a cell of the shared sheet `ForteMapIconSheet` (2048×1024 BC7, [`Src::Atlas`]). The cell
//!   grid (`sx × sy` slots) and each symbol's cell come from the game's `MapProfiles` XML in
//!   `UI.zip` (`atlas_4x4` = 8×4 slots of 256 px, `atlas_2x2` = 16×8 slots of 128 px;
//!   `AtlasPos` per symbol). The cells are **baked into [`ICON_TABLE`]** rather than parsed at
//!   runtime: the XML routes each symbol through templates and colour-blind variants (a parser
//!   would be ~150 lines for 16 cells), and a game update that moves a cell only changes
//!   cosmetics. `extract_icons.py` is the way to re-derive them.
//!
//! **No disk cache.** Reading, decoding (the sheet cells alone, not the whole sheet) and scaling
//! the 43 icons takes ~22 ms in a release build on 6 threads (~105 ms single-threaded), cheaper
//! than a cache and its invalidation. Call it off the UI thread anyway.

use std::collections::HashMap;
use std::path::Path;

use super::bc7::decode_bc7_region;
use super::install::ci;
use super::poi::PoiKind;
use super::tiles::{open_zip, parse_swatch, read_entry, PixelFormat, Swatch};

/// Default edge of one atlas cell, px. The map draws POIs at 32 px; 64 keeps them sharp on a
/// 2× display.
pub const DEFAULT_SIZE: u32 = 64;
/// Atlas columns. The atlas is `COLUMNS * size` wide and as tall as its rows need.
const COLUMNS: u32 = 8;
/// Transparent margin around an icon inside its cell, px: keeps bilinear sampling from bleeding
/// into the neighbouring cell.
const MARGIN: u32 = 1;

/// How the game map picks a race pin: by event class, not by POI kind (a `ResourceFilter` in the
/// map XML), so the route's class decides. The classes of the 167 race pins come from the route's
/// type (`docs/game-data/fh6-game-files.md`, "Race starts").
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RaceClass {
    AsphaltP2p,
    AsphaltCircuit,
    CrosscountryP2p,
    CrosscountryCircuit,
    MixedsurfaceP2p,
    MixedsurfaceCircuit,
    Dragracing,
    Streetracing,
    Touge,
    MidnightBattle,
}

impl RaceClass {
    pub const ALL: [RaceClass; 10] = [
        Self::AsphaltP2p,
        Self::AsphaltCircuit,
        Self::CrosscountryP2p,
        Self::CrosscountryCircuit,
        Self::MixedsurfaceP2p,
        Self::MixedsurfaceCircuit,
        Self::Dragracing,
        Self::Streetracing,
        Self::Touge,
        Self::MidnightBattle,
    ];
}

/// Which icon a row of [`ICON_TABLE`] is for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IconKey {
    Kind(PoiKind),
    Race(RaceClass),
    /// A mascot by region 1..=9 (`Poi::n` of a `Mascot`).
    Mascot(u32),
}

/// Where the pixels of an icon are.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Src {
    /// A swatchbin in `Horizon_Map.zip`: the entry path without `.swatchbin` (case-insensitive).
    File(&'static str),
    /// A cell of `ForteMapIconSheet`: cell (`x`, `y`) of an `sx × sy` slot grid.
    Atlas { x: u32, y: u32, sx: u32, sy: u32 },
}

/// How sure the table row is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Basis {
    /// The game's own `MapProfiles` XML draws this icon for the map-element type tag whose name
    /// matches the category (`build_icon_mapping.py`: "derived"). The tag-to-category link is by
    /// name; the icon-to-tag link is the game's.
    Derived,
    /// Chosen by file name; wants a human look.
    Guessed,
}

/// One row of [`ICON_TABLE`].
#[derive(Clone, Copy, Debug)]
pub struct IconRow {
    pub key: IconKey,
    pub src: Src,
    pub basis: Basis,
}

const fn kind(k: PoiKind, src: Src, basis: Basis) -> IconRow {
    IconRow { key: IconKey::Kind(k), src, basis }
}
const fn race(c: RaceClass, src: Src, basis: Basis) -> IconRow {
    IconRow { key: IconKey::Race(c), src, basis }
}
const fn mascot(region: u32, name: &'static str) -> IconRow {
    IconRow { key: IconKey::Mascot(region), src: Src::File(name), basis: Basis::Derived }
}
/// A cell of `ForteMapIconSheet` in the 8×4 grid (`atlas_4x4`, 256 px cells).
const fn a4(x: u32, y: u32) -> Src {
    Src::Atlas { x, y, sx: 8, sy: 4 }
}

use Basis::{Derived, Guessed};
use PoiKind as K;

/// POI kind (or race class, or mascot region) → icon. One row per distinct meaning; rows with the
/// same [`Src`] share one atlas cell. Kinds without a row have no game icon (landmarks are name
/// badges with the English text baked in, creature zones / parking areas / flag-rush flags are
/// geometry only, piñatas are far too many to draw as icons; eliminator spawns have one in
/// `Eliminator.zip`, not read here). Derived from `/home/mo/fh6-viewer-work/icons/mapping.json`
/// (`build_icon_mapping.py`, Sept-2026 build) unless a comment says otherwise.
pub const ICON_TABLE: &[IconRow] = &[
    // --- stunts: the atlas cells of `pos_prstunt_*` (derived: XML types ambient_speed_trap / _speed_zone / _danger_sign / _drift_zone / _trailblazer_gate_start)
    kind(K::SpeedTrap, a4(0, 0), Derived),
    kind(K::DangerSign, a4(1, 0), Derived),
    kind(K::DriftZone, a4(2, 0), Derived),
    kind(K::SpeedZone, a4(3, 0), Derived),
    // start-gate icon; the finish gate has its own (`pos_prstunt_trailblazer_end`, cell (1, 1)), not used since the start / end gate is unverified
    kind(K::Trailblazer, a4(0, 1), Derived),
    // --- collectibles
    // derived: XML type xp_board draws `pos_influenceboard_notfound` (the game calls them influence boards), atlas_2x2 grid
    kind(K::XpBoard, Src::Atlas { x: 8, y: 3, sx: 16, sy: 8 }, Derived),
    // GUESSED: repo type treasure_chest = DISCOUNT_BOARD_TREASURE_CHEST_N objects, linked to the Treasure Hunt chest icon by name only
    kind(K::TreasureChest, Src::File("icons/MapIcons/TreasureHunt/Icon_Treasure_Discovered"), Guessed),
    kind(K::TreasureChestBoard, Src::File("icons/MapIcons/TreasureHunt/Icon_Treasure_Discovered"), Guessed),
    kind(K::BarnFind, Src::File("icons/MapIcons/BarnFind"), Derived),
    // the search-radius badge (barn_find_region draws it inside the radius ellipse)
    kind(K::BarnFindHint, Src::File("icons/MapIcons/Barnfind_Radius_Icon"), Derived),
    kind(K::TreasureCar, Src::File("icons/MapIcons/SeeItDriveIt/TreasureCar"), Derived),
    // --- places
    kind(K::House, Src::File("icons/MapIcons/PlayerHouse/Icon_PlayerHouse_Default"), Derived),
    // estate arrival and entrance both draw the player_house XML type's Estate icon (entrance by name only)
    kind(K::Estate, Src::File("icons/MapIcons/PlayerHouse/Icon_Estate_Default"), Derived),
    kind(K::EstateEntrance, Src::File("icons/MapIcons/PlayerHouse/Icon_Estate_Default"), Derived),
    kind(K::FestivalSite, Src::File("icons/MapIcons/FestivalSites/Icon_HorizonFestival_Main"), Derived),
    // GUESSED, weakest row: XML type travel_board points at atlas cells (`pos_fasttravelboard_*`) that are BLANK in
    // ForteMapIconSheet (FH6 reuses the xp-board tile); this is the pause-menu version of the board (302x176, landscape)
    kind(K::FastTravel, Src::File("pins/discount_board_fasttravel"), Guessed),
    kind(K::CarMeet, Src::File("icons/MapIcons/Icon_Car_Meet"), Derived),
    kind(K::Showcase, Src::File("icons/MapIcons/Showcases/Icon_Showcase_Mech"), Derived), // the default of the XML; planes / mech differ per showcase (not told apart here)
    // the aftermarket board is the same car-shaped pin as the spot (by name)
    kind(K::AftermarketSpot, Src::File("icons/MapIcons/SeeItDriveIt/AftermarketCar"), Derived),
    kind(K::AftermarketBoard, Src::File("icons/MapIcons/SeeItDriveIt/AftermarketCar"), Derived),
    kind(K::Upsell, Src::File("icons/MapIcons/SeeItDriveIt/PlaylistCar"), Derived),
    // --- events
    kind(K::DragMeet, Src::File("icons/MapIcons/HorizonLife/Icon_DragEvent"), Derived),
    // GUESSED: the drag finish badge of CampaignObjective (the XML has no finish-line symbol of its own)
    kind(K::DragMeetFinish, Src::File("icons/MapIcons/CampaignObjective/drag_finish"), Guessed),
    kind(K::RushEvent, Src::File("icons/MapIcons/Rush/Icon_Rush_Docks"), Derived), // Docks / Ski / Rocket variants exist; Docks is the XML default
    // GUESSED catch-all: invitational / legend events have no kind field here, the Hall of Fame star stands for all
    kind(K::SpecialEvent, Src::File("icons/MapIcons/Icon_HallOfFame"), Guessed),
    // --- Horizon Stories and Jobs: the background tile (the story-specific glyph is drawn on top in the game)
    kind(K::HorizonStory, a4(1, 2), Derived),
    kind(K::StoryActivation, a4(1, 2), Derived),
    kind(K::HorizonJob, Src::File("icons/MapIcons/HorizonStories/Jobs_Background"), Derived),
    kind(K::JobActivation, Src::File("icons/MapIcons/HorizonStories/Jobs_Background"), Derived),
    // --- races: by event class (ResourceFilter in the XML), asphalt circuit is the XML's own fallback for a RacePin without a class
    kind(K::RacePin, a4(3, 3), Derived),
    kind(K::TougeEvent, a4(4, 2), Derived),
    race(RaceClass::AsphaltP2p, a4(2, 3), Derived),
    race(RaceClass::AsphaltCircuit, a4(3, 3), Derived),
    race(RaceClass::CrosscountryP2p, a4(0, 3), Derived),
    race(RaceClass::CrosscountryCircuit, a4(1, 3), Derived),
    race(RaceClass::MixedsurfaceP2p, a4(4, 3), Derived),
    race(RaceClass::MixedsurfaceCircuit, a4(5, 3), Derived),
    race(RaceClass::Dragracing, a4(6, 3), Derived),
    race(RaceClass::Streetracing, a4(7, 3), Derived),
    race(RaceClass::Touge, a4(4, 2), Derived),
    race(RaceClass::MidnightBattle, Src::File("icons/MapIcons/Icon_Midnight_Battle"), Derived),
    // --- mascots: one per region 1..9 (the XML types mascot_region_N)
    mascot(1, "regions/Mascots/Ramen"),
    mascot(2, "regions/Mascots/Dango"),
    mascot(3, "regions/Mascots/Omurice"),
    mascot(4, "regions/Mascots/Curry"),
    mascot(5, "regions/Mascots/Matcha"),
    mascot(6, "regions/Mascots/Kakigori"),
    mascot(7, "regions/Mascots/Edamame"),
    mascot(8, "regions/Mascots/Onigiri"),
    mascot(9, "regions/Mascots/Tempura"),
];

/// The icon atlas and where each icon sits in it.
#[derive(Clone, Debug, Default)]
pub struct PoiIcons {
    /// Edge of one (square) atlas cell, px. An icon keeps its aspect ratio and is centred in its
    /// cell, so a UV rectangle is always square and the quad can be drawn centred on the POI.
    pub size: u32,
    /// RGBA8, straight (non-premultiplied) alpha, row-major, `atlas_w * atlas_h * 4` bytes.
    pub atlas_rgba: Vec<u8>,
    pub atlas_w: u32,
    pub atlas_h: u32,
    /// `[u0, v0, u1, v1]` in 0..1 of the whole cell, per POI kind that has an icon.
    pub uv: HashMap<PoiKind, [f32; 4]>,
    /// Same, per race class (a `RacePin` is drawn with the class of its route).
    pub race: HashMap<RaceClass, [f32; 4]>,
    /// Same, per mascot region 1..=9.
    pub mascot: HashMap<u32, [f32; 4]>,
    /// Icons that could not be read or decoded (source + reason). The renderer falls back to its
    /// shapes for the kinds involved.
    pub skipped: Vec<String>,
}

impl PoiIcons {
    /// Read, decode and pack the icons of [`ICON_TABLE`] at [`DEFAULT_SIZE`]. `media` is the
    /// install's `media` folder. `Err` only if `Horizon_Map.zip` or its sheet can't be read, or
    /// no icon at all decoded; a single bad icon is skipped and listed in [`PoiIcons::skipped`].
    pub fn load(media: &Path) -> Result<PoiIcons, String> {
        Self::load_sized(media, DEFAULT_SIZE)
    }

    /// [`PoiIcons::load`] with an explicit cell edge (8..=256).
    pub fn load_sized(media: &Path, size: u32) -> Result<PoiIcons, String> {
        if !(8..=256).contains(&size) {
            return Err(format!("icon size {size} out of range"));
        }
        let zip_path = ci(media, "UI/Textures/HiRes/Data_Bound/Horizon_Map.zip")
            .filter(|p| p.is_file())
            .ok_or("no Horizon_Map.zip in the install")?;
        let mut z = open_zip(&zip_path).map_err(|e| e.to_string())?;
        // entry names are mixed-case on disk: look them up lower-cased
        let names: HashMap<String, String> = z.file_names().map(|n| (n.to_ascii_lowercase(), n.to_owned())).collect();
        let mut read_raw = |path: &str| -> Result<Vec<u8>, String> {
            let real = names.get(&format!("{}.swatchbin", path.to_ascii_lowercase())).ok_or_else(|| format!("{path}: not in Horizon_Map.zip"))?;
            read_entry(&mut z, real).map_err(|e| format!("{path}: {e}"))
        };

        // Distinct sources in table order, each decoded once.
        let mut order: Vec<Src> = Vec::new();
        for row in ICON_TABLE {
            if !order.contains(&row.src) {
                order.push(row.src);
            }
        }
        let sheet_raw = if order.iter().any(|s| matches!(s, Src::Atlas { .. })) {
            Some(read_raw(SHEET).map_err(|e| format!("icon sheet: {e}"))?)
        } else {
            None
        };
        let sheet = sheet_raw.as_deref().map(parse_swatch).transpose().map_err(|e| format!("icon sheet: {e}"))?;

        // The zip handle is single-threaded: read the raw files first, then decode + scale them in parallel.
        let raws: Vec<Result<Vec<u8>, String>> = order
            .iter()
            .map(|s| match s {
                Src::File(p) => read_raw(p),
                Src::Atlas { .. } => Ok(Vec::new()),
            })
            .collect();
        let jobs: Vec<(&Src, &Result<Vec<u8>, String>)> = order.iter().zip(&raws).collect();
        let fitted: Vec<Result<Fitted, String>> = par_map(&jobs, |&(src, raw)| {
            let (w, h, px) = match *src {
                Src::File(p) => {
                    let sw = parse_swatch(raw.as_ref().map_err(String::clone)?).map_err(|e| format!("{p}: {e}"))?;
                    (sw.w, sw.h, sw.to_rgba())
                }
                Src::Atlas { x, y, sx, sy } => decode_cell(sheet.as_ref().expect("sheet read above"), x, y, sx, sy)?,
            };
            Ok(fit_icon(&px, w, h, size))
        });

        let rows = (order.len() as u32).div_ceil(COLUMNS);
        let (atlas_w, atlas_h) = (COLUMNS * size, rows * size);
        let mut atlas = vec![0u8; (atlas_w * atlas_h * 4) as usize];
        let mut cell_of: HashMap<Src, [f32; 4]> = HashMap::new();
        let mut skipped = Vec::new();
        for (n, (src, icon)) in order.iter().zip(fitted).enumerate() {
            let icon = match icon {
                Ok(i) => i,
                Err(e) => {
                    skipped.push(e);
                    continue;
                }
            };
            let (cx, cy) = ((n as u32 % COLUMNS) * size, (n as u32 / COLUMNS) * size);
            place_icon(&mut atlas, atlas_w, cx, cy, size, &icon);
            let (fw, fh) = (atlas_w as f32, atlas_h as f32);
            cell_of.insert(*src, [cx as f32 / fw, cy as f32 / fh, (cx + size) as f32 / fw, (cy + size) as f32 / fh]);
        }
        if cell_of.is_empty() {
            return Err(format!("no map icon could be decoded ({})", skipped.first().map_or("none listed", String::as_str)));
        }
        let mut out = PoiIcons { size, atlas_rgba: atlas, atlas_w, atlas_h, skipped, ..Default::default() };
        for row in ICON_TABLE {
            let Some(&uv) = cell_of.get(&row.src) else { continue };
            match row.key {
                IconKey::Kind(k) => {
                    out.uv.insert(k, uv);
                }
                IconKey::Race(c) => {
                    out.race.insert(c, uv);
                }
                IconKey::Mascot(r) => {
                    out.mascot.insert(r, uv);
                }
            }
        }
        Ok(out)
    }
}

/// Entry of the shared icon sheet in `Horizon_Map.zip`.
const SHEET: &str = "icons/MapIcons/ForteMapIconSheet";

/// Cell (`x`, `y`) of an `sx × sy` slot grid on a sheet swatch, decoded as its own RGBA image.
/// A block-aligned BC7 cell is decoded alone; anything else goes through a full decode.
fn decode_cell(sheet: &Swatch, x: u32, y: u32, sx: u32, sy: u32) -> Result<(usize, usize, Vec<u8>), String> {
    let (cw, ch) = (sheet.w / sx.max(1) as usize, sheet.h / sy.max(1) as usize);
    let (x0, y0) = (x as usize * cw, y as usize * ch);
    if sheet.format == PixelFormat::Bc7 && x < sx && y < sy && [x0, y0, cw, ch].iter().all(|v| v % 4 == 0) && cw > 0 && ch > 0 {
        return Ok((cw, ch, decode_bc7_region(sheet.data, sheet.w, x0, y0, cw, ch)));
    }
    crop(&sheet.to_rgba(), sheet.w, sheet.h, x, y, sx, sy)
}

/// Cell (`x`, `y`) of an `sx × sy` slot grid on a `w × h` sheet, as its own RGBA image.
fn crop(px: &[u8], w: usize, h: usize, x: u32, y: u32, sx: u32, sy: u32) -> Result<(usize, usize, Vec<u8>), String> {
    let (cw, ch) = (w / sx as usize, h / sy as usize);
    let (x0, y0) = (x as usize * cw, y as usize * ch);
    if cw == 0 || ch == 0 || x >= sx || y >= sy {
        return Err(format!("atlas cell ({x}, {y}) of {sx}x{sy} is outside the {w}x{h} sheet"));
    }
    let mut out = Vec::with_capacity(cw * ch * 4);
    for row in y0..y0 + ch {
        out.extend_from_slice(&px[(row * w + x0) * 4..][..cw * 4]);
    }
    Ok((cw, ch, out))
}

/// An icon scaled to fit a cell: `(width, height, RGBA)`.
type Fitted = (usize, usize, Vec<u8>);

/// Scale `src` (RGBA, `sw × sh`) to fit a `size × size` cell minus [`MARGIN`], keeping its aspect ratio.
fn fit_icon(src: &[u8], sw: usize, sh: usize, size: u32) -> Fitted {
    let inner = (size - 2 * MARGIN) as f32;
    let scale = (inner / sw as f32).min(inner / sh as f32);
    let (dw, dh) = (((sw as f32 * scale).round() as usize).max(1), ((sh as f32 * scale).round() as usize).max(1));
    (dw, dh, resize_area(src, sw, sh, dw, dh))
}

/// Write a [`Fitted`] icon centred into the cell at (`cx`, `cy`) of the atlas (stride `atlas_w`).
fn place_icon(atlas: &mut [u8], atlas_w: u32, cx: u32, cy: u32, size: u32, (dw, dh, px): &Fitted) {
    let (ox, oy) = (cx as usize + (size as usize - dw) / 2, cy as usize + (size as usize - dh) / 2);
    for y in 0..*dh {
        let o = ((oy + y) * atlas_w as usize + ox) * 4;
        atlas[o..o + dw * 4].copy_from_slice(&px[y * dw * 4..][..dw * 4]);
    }
}

/// `f` over `items` on up to 8 threads (strided), results in item order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8).min(items.len().max(1));
    let f = &f;
    let mut slots: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| s.spawn(move || (t..items.len()).step_by(threads).map(|i| (i, f(&items[i]))).collect::<Vec<_>>()))
            .collect();
        for h in handles {
            for (i, r) in h.join().expect("icon worker panicked") {
                slots[i] = Some(r);
            }
        }
    });
    slots.into_iter().map(|r| r.expect("every item mapped")).collect()
}

/// Area-average (box) resize of RGBA8 to `dw × dh`, in premultiplied alpha so transparent texels
/// don't darken the edges; the result has straight alpha again. Exact for any ratio (each
/// destination texel averages the fractional source span it covers).
pub fn resize_area(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    // horizontal pass: sw -> dw, into f32 premultiplied RGBA rows
    let mut tmp = vec![0f32; dw * sh * 4];
    let xs = sw as f32 / dw as f32;
    for y in 0..sh {
        for x in 0..dw {
            let (a, b) = (x as f32 * xs, (x + 1) as f32 * xs);
            let mut acc = [0f32; 4];
            let mut i = a.floor() as usize;
            while (i as f32) < b && i < sw {
                let wgt = (b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0);
                let p = &src[(y * sw + i) * 4..][..4];
                let al = p[3] as f32 / 255.0;
                acc[0] += p[0] as f32 * al * wgt;
                acc[1] += p[1] as f32 * al * wgt;
                acc[2] += p[2] as f32 * al * wgt;
                acc[3] += p[3] as f32 * wgt;
                i += 1;
            }
            for (k, v) in acc.iter().enumerate() {
                tmp[(y * dw + x) * 4 + k] = v / xs;
            }
        }
    }
    // vertical pass: sh -> dh
    let mut out = vec![0u8; dw * dh * 4];
    let ys = sh as f32 / dh as f32;
    for y in 0..dh {
        let (a, b) = (y as f32 * ys, (y + 1) as f32 * ys);
        for x in 0..dw {
            let mut acc = [0f32; 4];
            let mut j = a.floor() as usize;
            while (j as f32) < b && j < sh {
                let wgt = (b.min(j as f32 + 1.0) - a.max(j as f32)).max(0.0);
                for (k, v) in acc.iter_mut().enumerate() {
                    *v += tmp[(j * dw + x) * 4 + k] * wgt;
                }
                j += 1;
            }
            let (alpha, o) = (acc[3] / ys, (y * dw + x) * 4);
            if alpha > 0.0 {
                // premultiplied colour is (colour * alpha/255); undo with the alpha fraction
                let af = alpha / 255.0;
                for k in 0..3 {
                    out[o + k] = (acc[k] / ys / af).round().clamp(0.0, 255.0) as u8;
                }
            }
            out[o + 3] = alpha.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::install::find_media;
    use crate::gamedata::tiles::PixelFormat;

    #[test]
    fn resize_area_averages_and_keeps_alpha_straight() {
        // 2x2 -> 1x1: two opaque red texels, two fully transparent (colour must not darken the result)
        let px = [255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(resize_area(&px, 2, 2, 1, 1), vec![255, 0, 0, 128]);
        // identity size
        let id = [1, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(resize_area(&id, 2, 1, 2, 1), id.to_vec());
        // 3 -> 2 fractional spans: weights 1, 0.5 / 0.5, 1 (flat colour stays flat)
        let flat: Vec<u8> = (0..3).flat_map(|_| [10, 20, 30, 255]).collect();
        assert_eq!(resize_area(&flat, 3, 1, 2, 1), vec![10, 20, 30, 255, 10, 20, 30, 255]);
    }

    #[test]
    fn blit_centres_and_keeps_the_margin() {
        // a 40x20 opaque white image into a 16 cell: fits 14 wide (aspect 2:1 -> 14x7), centred
        let src = vec![255u8; 40 * 20 * 4];
        let mut atlas = vec![0u8; 16 * 16 * 4];
        let icon = fit_icon(&src, 40, 20, 16);
        assert_eq!((icon.0, icon.1), (14, 7));
        place_icon(&mut atlas, 16, 0, 0, 16, &icon);
        let a = |x: usize, y: usize| atlas[(y * 16 + x) * 4 + 3];
        assert_eq!(a(1, 8), 255, "left edge of the 14-wide icon at the margin");
        assert_eq!(a(14, 8), 255);
        assert_eq!(a(0, 8), 0, "margin column stays transparent");
        assert_eq!(a(15, 8), 0);
        assert_eq!(a(8, 3), 0, "above the 7-tall icon (rows 4..=10)");
        assert_eq!(a(8, 4), 255);
        assert_eq!(a(8, 10), 255);
        assert_eq!(a(8, 11), 0);
    }

    #[test]
    fn crop_takes_the_cell() {
        // 4x2 sheet in a 2x1 grid: right cell
        let sheet: Vec<u8> = (0..8u8).flat_map(|i| [i, 0, 0, 255]).collect();
        let (w, h, px) = crop(&sheet, 4, 2, 1, 0, 2, 1).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(px.iter().step_by(4).copied().collect::<Vec<_>>(), vec![2, 3, 6, 7]);
        assert!(crop(&sheet, 4, 2, 2, 0, 2, 1).is_err());
    }

    /// Every kind the user switched on for the map has an icon row, and the tables are coherent.
    #[test]
    fn table_covers_the_enabled_kinds() {
        use PoiKind::*;
        let has = |k| ICON_TABLE.iter().any(|r| r.key == IconKey::Kind(k));
        // barn finds, car meets, fast travel, festival sites, houses, aftermarket spots / boards,
        // Horizon jobs / stories, XP boards, speed traps / zones, trailblazers, drift zones,
        // danger signs, treasure chest (+ the board it is drawn from)
        for k in [
            BarnFind, CarMeet, FastTravel, FestivalSite, House, AftermarketSpot, AftermarketBoard, HorizonJob, HorizonStory, XpBoard, SpeedTrap, SpeedZone, Trailblazer,
            DriftZone, DangerSign, TreasureChest, TreasureChestBoard,
        ] {
            assert!(has(k), "no icon row for {k:?}");
        }
        // all ten race classes and nine mascot regions
        for c in RaceClass::ALL {
            assert!(ICON_TABLE.iter().any(|r| r.key == IconKey::Race(c)), "{c:?}");
        }
        for r in 1..=9 {
            assert!(ICON_TABLE.iter().any(|row| row.key == IconKey::Mascot(r)), "mascot {r}");
        }
        // one row per key
        let mut keys: Vec<_> = ICON_TABLE.iter().map(|r| format!("{:?}", r.key)).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), n, "duplicate key in ICON_TABLE");
        // atlas cells stay inside their grids
        for r in ICON_TABLE {
            if let Src::Atlas { x, y, sx, sy } = r.src {
                assert!(x < sx && y < sy, "{r:?}");
            }
        }
        // The brief's counts: 5 of the table's kind rows are guesses (fast travel weakest)
        let guessed: Vec<_> = ICON_TABLE.iter().filter(|r| r.basis == Guessed).map(|r| r.key).collect();
        assert_eq!(guessed.len(), 5, "{guessed:?}");
    }

    /// FNV-1a over `w`, `h` and the RGBA bytes: a compact fingerprint of a decoded icon.
    fn fnv(w: usize, h: usize, px: &[u8]) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        for b in (w as u32).to_le_bytes().iter().chain((h as u32).to_le_bytes().iter()).chain(px) {
            hash = (hash ^ *b as u64).wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Decode a source in full resolution (no scaling).
    fn decode_full(media: &Path, src: Src) -> (usize, usize, Vec<u8>) {
        let zip = ci(media, "UI/Textures/HiRes/Data_Bound/Horizon_Map.zip").unwrap();
        let mut z = open_zip(&zip).unwrap();
        let names: HashMap<String, String> = z.file_names().map(|n| (n.to_ascii_lowercase(), n.to_owned())).collect();
        let mut raw = |p: &str| read_entry(&mut z, &names[&format!("{}.swatchbin", p.to_ascii_lowercase())]).unwrap();
        match src {
            Src::File(p) => {
                let raw = raw(p);
                let sw = parse_swatch(&raw).unwrap();
                (sw.w, sw.h, sw.to_rgba())
            }
            Src::Atlas { x, y, sx, sy } => {
                let raw = raw(SHEET);
                let sw = parse_swatch(&raw).unwrap();
                // the full-sheet decode + crop and the single-cell decode the loader uses must agree
                let by_crop = crop(&sw.to_rgba(), sw.w, sw.h, x, y, sx, sy).unwrap();
                assert_eq!(decode_cell(&sw, x, y, sx, sy).unwrap(), by_crop, "cell ({x},{y}) of {sx}x{sy}");
                by_crop
            }
        }
    }

    /// Label of a source in the reference table below.
    fn label(src: Src) -> String {
        match src {
            Src::File(p) => p.to_string(),
            Src::Atlas { x, y, sx, sy } => format!("atlas:{x},{y}/{sx}x{sy}"),
        }
    }

    /// Fingerprints (label, width, height, FNV-1a of w, h and the RGBA bytes) of every distinct
    /// icon source of [`ICON_TABLE`] as decoded by **Pillow** (`extract_icons.py`, Pillow 12.3,
    /// the Oct-2026 build), computed from those PNGs. BC7 is deterministic, so the Rust decoder
    /// must reproduce them exactly. A game update that redraws an icon fails this on purpose:
    /// re-run the Python extractor, check the new art, regenerate (set `FH6_PRINT_ICON_HASHES=1`
    /// to print this test's own hashes in the same order).
    const REFERENCE: &[(&str, usize, usize, u64)] = &[
        ("atlas:0,0/8x4", 256, 256, 0x2340089eca404e9b),
        ("atlas:1,0/8x4", 256, 256, 0x2c9a1878b3f70c97),
        ("atlas:2,0/8x4", 256, 256, 0x7347dbc7f4c7c444),
        ("atlas:3,0/8x4", 256, 256, 0xafb782a35abba412),
        ("atlas:0,1/8x4", 256, 256, 0xc882cc99fe61c700),
        ("atlas:8,3/16x8", 128, 128, 0xb0c4f289a9c3e97d),
        ("icons/MapIcons/TreasureHunt/Icon_Treasure_Discovered", 256, 256, 0x8c639a07256881df),
        ("icons/MapIcons/BarnFind", 256, 256, 0xfa27107b28c85c6f),
        ("icons/MapIcons/Barnfind_Radius_Icon", 322, 389, 0xdd2ac4c036f178b0),
        ("icons/MapIcons/SeeItDriveIt/TreasureCar", 256, 256, 0xa8953cc3c50bdf2c),
        ("icons/MapIcons/PlayerHouse/Icon_PlayerHouse_Default", 256, 256, 0x8f810c3c537acc8b),
        ("icons/MapIcons/PlayerHouse/Icon_Estate_Default", 380, 340, 0x3e1161d4220ea883),
        ("icons/MapIcons/FestivalSites/Icon_HorizonFestival_Main", 256, 256, 0x3cd6cd71b5aed191),
        ("pins/discount_board_fasttravel", 302, 176, 0xa0c04157a332f705),
        ("icons/MapIcons/Icon_Car_Meet", 256, 256, 0x47c7c87c97e8c69e),
        ("icons/MapIcons/Showcases/Icon_Showcase_Mech", 380, 400, 0x2d54e868651fb2ba),
        ("icons/MapIcons/SeeItDriveIt/AftermarketCar", 256, 256, 0x5247557bbc0e7259),
        ("icons/MapIcons/SeeItDriveIt/PlaylistCar", 256, 256, 0xc1fe4480e8e79599),
        ("icons/MapIcons/HorizonLife/Icon_DragEvent", 256, 256, 0x35f8ec69f39e8405),
        ("icons/MapIcons/CampaignObjective/drag_finish", 92, 92, 0x262ecb1873844ead),
        ("icons/MapIcons/Rush/Icon_Rush_Docks", 380, 400, 0xa37066f096ca1a26),
        ("icons/MapIcons/Icon_HallOfFame", 256, 256, 0x714c94652f2c0167),
        ("atlas:1,2/8x4", 256, 256, 0x99bed60dab835e68),
        ("icons/MapIcons/HorizonStories/Jobs_Background", 257, 257, 0xcc9da5a2182c4378),
        ("atlas:3,3/8x4", 256, 256, 0x69eb5a33fe9354c9),
        ("atlas:4,2/8x4", 256, 256, 0x91001c9033a78bd1),
        ("atlas:2,3/8x4", 256, 256, 0xc1ada2bf12c48e20),
        ("atlas:0,3/8x4", 256, 256, 0x9950769c85d1f342),
        ("atlas:1,3/8x4", 256, 256, 0x602240fc4899975d),
        ("atlas:4,3/8x4", 256, 256, 0x1c4aefb41df208ab),
        ("atlas:5,3/8x4", 256, 256, 0xe0d29288379c9bb1),
        ("atlas:6,3/8x4", 256, 256, 0xd3e3ffd668827643),
        ("atlas:7,3/8x4", 256, 256, 0x24788321c06fe3b6),
        ("icons/MapIcons/Icon_Midnight_Battle", 256, 256, 0x3bc756ea00afca6f),
        ("regions/Mascots/Ramen", 120, 120, 0xdde42119f899a53d),
        ("regions/Mascots/Dango", 120, 120, 0x64d363ac033d71b8),
        ("regions/Mascots/Omurice", 120, 120, 0x839405a4437420f0),
        ("regions/Mascots/Curry", 120, 120, 0x6012a67fa798731f),
        ("regions/Mascots/Matcha", 120, 120, 0x8ef3c6bfc2bb09ac),
        ("regions/Mascots/Kakigori", 120, 120, 0x6131bf9923d8823c),
        ("regions/Mascots/Edamame", 120, 120, 0x09682167a7e7de4d),
        ("regions/Mascots/Onigiri", 120, 120, 0x2a489d38d6720d05),
        ("regions/Mascots/Tempura", 120, 120, 0xe8b1057ba178defb),
    ];

    #[test]
    fn real_install_icons_match_the_pillow_reference() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_icons_match_the_pillow_reference: FH6 install not found");
            return;
        };
        let mut seen = Vec::new();
        for r in ICON_TABLE {
            let l = label(r.src);
            if seen.contains(&l) {
                continue;
            }
            seen.push(l.clone());
            let (w, h, px) = decode_full(&media, r.src);
            let got = fnv(w, h, &px);
            if std::env::var_os("FH6_PRINT_ICON_HASHES").is_some() {
                eprintln!("HASH\t{l}\t{w}\t{h}\t{got:016x}\t{}", px.len());
                continue;
            }
            let Some(&(_, rw, rh, want)) = REFERENCE.iter().find(|e| e.0 == l) else { panic!("no reference for {l}") };
            assert_eq!((w, h), (rw, rh), "{l}");
            assert_eq!(got, want, "{l}: decoded pixels differ from the Pillow reference");
        }
        assert_eq!(REFERENCE.len(), seen.len(), "stale reference entries");
    }

    /// Every BC7 / RGBA8 swatchbin of `Horizon_Map.zip` (not only the ~45 the app uses) against
    /// the full-resolution PNGs `extract_icons.py` writes (`<dir>/png/<name>.png`), pixel for
    /// pixel. Needs `FH6_ICON_PNG_DIR=<the extract_icons.py --out dir>`; ignored by default.
    /// `FH6_ICON_PNG_DIR=... cargo test --release -- --ignored every_icon_matches_the_python_pngs`
    #[test]
    #[ignore = "needs FH6_ICON_PNG_DIR with extract_icons.py output"]
    fn every_icon_matches_the_python_pngs() {
        let (Some(media), Some(dir)) = (find_media(None), std::env::var_os("FH6_ICON_PNG_DIR")) else {
            eprintln!("SKIP every_icon_matches_the_python_pngs: no install or FH6_ICON_PNG_DIR");
            return;
        };
        let zip = ci(&media, "UI/Textures/HiRes/Data_Bound/Horizon_Map.zip").unwrap();
        let mut z = open_zip(&zip).unwrap();
        let names: Vec<String> = z.file_names().filter(|n| n.ends_with(".swatchbin")).map(str::to_owned).collect();
        let (mut compared, mut skipped_fmt, mut max_diff, mut bad) = (0, 0, 0u8, Vec::new());
        let mut modes = [0u64; 9]; // BC7 blocks per mode (index 8 = reserved)
        for n in &names {
            let raw = read_entry(&mut z, n).unwrap();
            let Some(sw) = parse_swatch(&raw).ok().filter(|s| s.format != PixelFormat::Bc1) else {
                skipped_fmt += 1;
                continue;
            };
            let png = std::path::Path::new(&dir).join("png").join(format!("{}.png", n.trim_end_matches(".swatchbin")));
            let Ok(img) = image::open(&png) else {
                eprintln!("no reference PNG for {n}");
                continue;
            };
            let want = img.to_rgba8();
            let got = sw.to_rgba();
            if sw.format == PixelFormat::Bc7 {
                for blk in sw.data.chunks_exact(16) {
                    modes[(blk[0].trailing_zeros() as usize).min(8)] += 1;
                }
            }
            assert_eq!((want.width() as usize, want.height() as usize), (sw.w, sw.h), "{n}");
            let diff = got.iter().zip(want.as_raw()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            max_diff = max_diff.max(diff);
            if diff != 0 {
                bad.push(format!("{n}: max channel diff {diff}"));
            }
            compared += 1;
        }
        eprintln!("compared {compared} icons ({skipped_fmt} BC1/other skipped), max channel diff {max_diff}, BC7 blocks per mode 0..7 + reserved: {modes:?}");
        assert!(bad.is_empty(), "{} of {compared} differ: {:?}", bad.len(), &bad[..bad.len().min(5)]);
        assert!(compared > 900, "only {compared} compared");
    }

    /// Contact sheet for a human look: the atlas on a dark checker-free grey, 2x nearest-neighbour,
    /// to `$FH6_ICON_ATLAS_OUT` (a .png path). `FH6_ICON_ATLAS_OUT=/tmp/atlas.png cargo test
    /// --release -- --ignored dump_atlas_png`.
    #[test]
    #[ignore = "writes a PNG to $FH6_ICON_ATLAS_OUT"]
    fn dump_atlas_png() {
        let (Some(media), Some(out)) = (find_media(None), std::env::var_os("FH6_ICON_ATLAS_OUT")) else {
            eprintln!("SKIP dump_atlas_png: no install or FH6_ICON_ATLAS_OUT");
            return;
        };
        let icons = PoiIcons::load(&media).expect("icons");
        let (w, h) = (icons.atlas_w as usize, icons.atlas_h as usize);
        let mut img = image::RgbImage::new(w as u32 * 2, h as u32 * 2);
        for y in 0..h * 2 {
            for x in 0..w * 2 {
                let o = ((y / 2) * w + x / 2) * 4;
                let (a, bg) = (icons.atlas_rgba[o + 3] as u32, 40u32);
                let px = [0, 1, 2].map(|k| ((icons.atlas_rgba[o + k] as u32 * a + bg * (255 - a)) / 255) as u8);
                img.put_pixel(x as u32, y as u32, image::Rgb(px));
            }
        }
        img.save(out).expect("write png");
    }

    #[test]
    fn real_install_atlas_and_timing() {
        let Some(media) = find_media(None) else {
            eprintln!("SKIP real_install_atlas_and_timing: FH6 install not found");
            return;
        };
        let t = std::time::Instant::now();
        let icons = PoiIcons::load(&media).expect("icons");
        eprintln!("PoiIcons::load: {:?}, atlas {}x{}, {} kinds, {} race, {} mascots, skipped {:?}", t.elapsed(), icons.atlas_w, icons.atlas_h, icons.uv.len(), icons.race.len(), icons.mascot.len(), icons.skipped);
        assert!(icons.skipped.is_empty(), "{:?}", icons.skipped);
        assert_eq!(icons.size, DEFAULT_SIZE);
        assert_eq!(icons.atlas_rgba.len(), (icons.atlas_w * icons.atlas_h * 4) as usize);
        // every table row resolved
        for r in ICON_TABLE {
            let found = match r.key {
                IconKey::Kind(k) => icons.uv.contains_key(&k),
                IconKey::Race(c) => icons.race.contains_key(&c),
                IconKey::Mascot(n) => icons.mascot.contains_key(&n),
            };
            assert!(found, "{r:?}");
        }
        // the enabled kinds
        for k in [PoiKind::BarnFind, PoiKind::DangerSign, PoiKind::XpBoard, PoiKind::FastTravel, PoiKind::TreasureChest] {
            assert!(icons.uv.contains_key(&k), "{k:?}");
        }
        // UVs are 0..1, ordered, cell-sized; every cell holds opaque pixels (not an empty slot)
        let (fw, fh) = (icons.atlas_w as f32, icons.atlas_h as f32);
        for uv in icons.uv.values().chain(icons.race.values()).chain(icons.mascot.values()) {
            assert!(uv[0] >= 0.0 && uv[1] >= 0.0 && uv[2] <= 1.0 && uv[3] <= 1.0 && uv[0] < uv[2] && uv[1] < uv[3], "{uv:?}");
            let (x0, y0) = ((uv[0] * fw).round() as usize, (uv[1] * fh).round() as usize);
            assert_eq!(((uv[2] - uv[0]) * fw).round() as u32, icons.size);
            let mut opaque = 0;
            for y in y0..y0 + icons.size as usize {
                for x in x0..x0 + icons.size as usize {
                    opaque += usize::from(icons.atlas_rgba[(y * icons.atlas_w as usize + x) * 4 + 3] > 200);
                }
            }
            assert!(opaque > 200, "cell at {uv:?} is (nearly) empty: {opaque} opaque texels");
        }
        // shared sources share a cell
        assert_eq!(icons.uv[&PoiKind::AftermarketSpot], icons.uv[&PoiKind::AftermarketBoard]);
        assert_eq!(icons.uv[&PoiKind::RacePin], icons.race[&RaceClass::AsphaltCircuit]);
        assert_ne!(icons.uv[&PoiKind::SpeedTrap], icons.uv[&PoiKind::SpeedZone]);
    }
}
