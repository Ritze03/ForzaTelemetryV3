//! Offscreen PNG harness: renders the round-4 spec states through the overlay's own path
//! (`Renderer` → egui → egui_glow) into an FBO on a headless EGL device, reads the pixels
//! back and writes PNGs to `target/hud_png/`. Compiled as `overlay::render::png` (see the
//! `#[path]` there). Run: `cargo test render_spec_states -- --ignored --nocapture`.
//!
//! Each widget state is a tile of (w + 20) × (h + 20) design px, the widget at (10, 10), on
//! the mockup's snow-white state background, at 1× and 3×. Plus 1920 × 1080 composites of
//! the default layout on a grey backdrop (and variants, one with custom margin/gap). Time is
//! pinned (`NOW`), so flash phases, fades and chips are deterministic.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use egui::{pos2, vec2, Color32, ColorImage, Painter, Vec2};
use egui_glow::glow::{self, HasContext};

use super::super::gl::Headless;
use super::{paint, Renderer};
use crate::config::{ClusterStyle, DriftStyle, OverlayConfig};
use crate::hud::map_shared::Remote;
use crate::hud::minimap::{CoopLayer, MapAnim, MapTex, Scene3dIn};
use crate::hud::prims::Xf;
use crate::gamedata::poi::{week_index_now, treasure_chest_number, Poi, PoiKind};
use crate::hud::{cluster, drift, minimap, race};
use crate::maprender::cfg::{MapLayerConfig, ViewMode};
use crate::maprender::data::{Chain, MapLayers, PoiLayer};
use crate::maprender::gl3d::{Gl3dHandle, Gl3dOptions, Gl3dStatus};
use crate::maprender::mesh3d::RoadMesh;
use crate::maprender::terrain::Terrain;
use crate::maprender::icontex::{synthetic_icons, IconTex};
use crate::maprender::paint2d::IconAtlas;
use crate::maprender::store::{LayerStatus, Layers, TerrainStatus};
use crate::minimap::{MapCalibration, Season, OVERLAY_MAP_TEXTURE_OPTIONS};
use crate::overlay::snapshot::{DriftChip, DriftInfo, DriveMode, HudMode, HudSnapshot, PlaceChange};

/// Pinned clock; `.05` into a 100 ms flash period = the flash's "on" phase.
const NOW: f64 = 1000.05;
/// The mockup's state tiles sit on `linear-gradient(#F6F8FA, #D5DEE7)`; this is its middle.
const TILE_BG: Color32 = Color32::from_rgb(230, 235, 240);
const SCREEN_BG: Color32 = Color32::from_rgb(128, 138, 150);
/// A dark stand-in for the game behind the minimap (the user's demo setting "backdrop: dark"): the
/// map is dimmed to 50 % over no plate, so what is behind it shows through.
const GAME_BG: Color32 = Color32::from_rgb(11, 13, 16);
const CAR: (f32, f32) = (1200.0, -800.0);

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("hud_png")
}

fn base(cfg: OverlayConfig) -> HudSnapshot {
    let mut s = HudSnapshot { visible: true, connected: true, cfg: Arc::new(cfg), built_at: NOW, ..Default::default() };
    let p = &mut s.pkt;
    p.is_race_on = 1;
    p.engine_max_rpm = 8000.0;
    p.engine_idle_rpm = 900.0;
    p.position_x = CAR.0;
    p.position_z = CAR.1;
    p.yaw = 0.6;
    s.redline_rpm = 0.85 * 8000.0;
    s.shift_rpm = 0.93 * 8000.0;
    let c = MapCalibration::DEFAULT;
    s.minimap = crate::overlay::snapshot::MinimapCalib { px_per_m: c.px_per_m, origin_x: c.origin_x, origin_z: c.origin_z };
    s
}

/// The mockup's cluster states: fraction of max rpm, gear, km/h.
fn cluster_state(cfg: OverlayConfig, frac: f32, gear: u8, kmh: f32) -> HudSnapshot {
    let mut s = base(cfg);
    s.pkt.current_engine_rpm = frac * 8000.0;
    s.pkt.gear = gear;
    s.pkt.speed = kmh / 3.6;
    s
}

/// A cruise-rpm cluster state with the auto gearbox on in `mode`.
fn auto_state(mode: DriveMode, gear: u8) -> HudSnapshot {
    let mut s = cluster_state(OverlayConfig::default(), 0.52, gear, 142.0);
    s.auto_gear = Some(mode);
    s
}

fn race_state(pos: u8, lap0: u16, cur: f32) -> HudSnapshot {
    let mut s = base(OverlayConfig::default());
    s.pkt.race_position = pos;
    s.pkt.lap_number = lap0;
    s.pkt.current_lap = cur;
    s.pkt.best_lap = 52.804;
    s
}

fn drift_state(score: f32, chip: Option<(f32, f64)>, cycle: Option<f64>, scoring: bool) -> HudSnapshot {
    let mut s = base(OverlayConfig { drift_style: DriftStyle::Total, ..Default::default() });
    s.mode = HudMode::Drift;
    s.drift = DriftInfo {
        score,
        best: 61_200.0,
        window_start: cycle.map(|c| NOW - c * 5.0),
        interval: 5.0,
        chip: chip.map(|(gain, age)| DriftChip { gain, at: NOW - age }),
        last_rise_at: Some(if scoring { NOW - 0.1 } else { NOW - 5.0 }),
    };
    s
}

/// A Position + Gain state: `pos`, the last window's gain (`None` = none closed yet) and the
/// `shown` count-up value, plus an optional place change 1 s ago (`Some(gained)`).
fn pg_state(pos: u8, gain: Option<f32>, shown: f32, scoring: bool, place: Option<bool>) -> (HudSnapshot, f32) {
    let mut s = drift_state(58_687.0, gain.map(|g| (g, 1.2)), Some(0.45), scoring);
    s.cfg = Arc::new(OverlayConfig::default());
    s.pkt.race_position = pos;
    s.events.place_change = place.map(|gained| PlaceChange { at: NOW - 1.0, gained });
    (s, shown)
}

/// A procedural stand-in map (the real one takes 2 s to load): fields, a road grid, a
/// highway and a lake, so rotation and scale are visible.
/// Two teammates around `snap`'s car, placed along its heading (the map is heading-up, so
/// "ahead" is screen-up): one ahead-left turning right, one right-behind heading back. At the
/// default driving zoom (1500 m, 0.045 px/m) they land about (−18, −36) and (54, 23) px from
/// the car.
fn coop_mates(snap: &HudSnapshot) -> CoopLayer {
    let (x, z, yaw) = (snap.pkt.position_x, snap.pkt.position_z, snap.pkt.yaw);
    let at = |ahead: f32, right: f32| (x + ahead * yaw.sin() + right * yaw.cos(), z + ahead * yaw.cos() - right * yaw.sin());
    let mate = |id: &str, (x, z): (f32, f32), dyaw: f32, name: &str, hue: f32| Remote {
        id: id.into(),
        x,
        z,
        yaw: yaw + dyaw,
        name: name.into(),
        colour: crate::ui::coop::hue_color(hue),
        paused: false,
    };
    // A short trail behind the car and behind "Kai", in the same coordinates the arrows use.
    let now = std::time::Instant::now();
    let trail = |from: (f32, f32), back: (f32, f32)| -> crate::minimap::Trail {
        (0..8).map(|i| (from.0 + back.0 * (7 - i) as f32 * 40.0, from.1 + back.1 * (7 - i) as f32 * 40.0, now)).collect()
    };
    let mut layer = CoopLayer::default();
    layer.in_session = true;
    layer.teammates = vec![mate("kai", at(800.0, -400.0), 0.4, "Kai", 36.0), mate("mo", at(-500.0, 1200.0), -2.0, "Mo", 200.0)];
    layer.trails.insert("local".into(), trail((x, z), (-yaw.sin(), -yaw.cos())));
    layer.trails.insert("kai".into(), trail(at(800.0, -400.0), (-yaw.sin(), -yaw.cos())));
    layer
}

fn synthetic_map(winter: bool) -> ColorImage {
    let n = 1024usize;
    let mut px = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32, y as f32);
            let field = ((x / 48 + y / 40) % 3) as u8;
            let land = if winter { [222 - field * 12, 229 - field * 9, 236 - field * 6] } else { [94 + field * 17, 122 + field * 10, 62 + field * 5] };
            let road = x % 64 < 3 || y % 64 < 3;
            let highway = ((fx - fy * 0.6) - 180.0).abs() < 4.0;
            let lake = ((fx - 560.0).powi(2) + (fy - 470.0).powi(2)).sqrt() < 60.0;
            let c = if lake {
                if winter { [140, 166, 190] } else { [52, 110, 150] }
            } else if highway || road {
                if winter { [111, 122, 134] } else { [228, 220, 200] }
            } else {
                land
            };
            px.push(Color32::from_rgb(c[0], c[1], c[2]));
        }
    }
    ColorImage::new([n, n], px)
}

/// Render one tile of a `size`-design-px widget at scale `s` and read it back.
fn tile(r: &mut Renderer, size: Vec2, s: f32, draw: impl FnMut(&Painter, &Xf)) -> ColorImage {
    tile_on(r, size, s, TILE_BG, draw)
}

/// [`tile`] over the background `bg`.
fn tile_on(r: &mut Renderer, size: Vec2, s: f32, bg: Color32, mut draw: impl FnMut(&Painter, &Xf)) -> ColorImage {
    let px = [((size.x + 20.0) * s).round() as u32, ((size.y + 20.0) * s).round() as u32];
    paint(&r.ctx, &mut r.painter, r.start, px, bg.to_normalized_gamma_f32(), |_, p| {
        draw(p, &Xf { o: pos2(10.0 * s, 10.0 * s), s, a: 1.0 });
    });
    r.painter.read_screen_rgba(px)
}

/// The harness's layer data: the stock synthetic road cross and race ring around the world origin,
/// plus a POI set that exercises every drawing path (game icons, gate lines, a danger sign, the
/// current treasure chest). With `icons` the synthetic atlas rides along like the store's.
fn harness_layers(icons: bool) -> Arc<MapLayers> {
    let mut m = MapLayers::synthetic();
    let chest_no = treasure_chest_number(week_index_now()).max(1) as u32;
    let p = |kind, name: &str, n: u32, x: f32, z: f32, gate| Poi { kind, x, z, y: 0.0, name: name.into(), n, gate };
    let gate = |x: f32, z: f32, dx: f32, dz: f32| Some([[x - dx, z - dz], [x + dx, z + dz]]);
    m.pois = Arc::new(PoiLayer::from_items([
        p(PoiKind::House, "house", 0, -180.0, -60.0, None),
        p(PoiKind::FastTravel, "ft", 0, 150.0, 60.0, None),
        p(PoiKind::CarMeet, "meet", 0, 20.0, 150.0, None),
        p(PoiKind::BarnFind, "barn", 0, -200.0, 120.0, None),
        p(PoiKind::SpeedTrap, "SPEEDCAMERA_07", 7, 60.0, -120.0, gate(60.0, -120.0, 9.0, 0.0)),
        p(PoiKind::SpeedZone, "sz_gate1", 1, -90.0, 30.0, gate(-90.0, 30.0, 0.0, 9.0)),
        p(PoiKind::Trailblazer, "tb_gate1", 1, 120.0, 20.0, gate(120.0, 20.0, 0.0, 9.0)),
        p(PoiKind::DriftZone, "dz_gate1", 1, -40.0, -200.0, gate(-40.0, -200.0, 9.0, 0.0)),
        p(PoiKind::DangerSign, "bm_01", 1, 200.0, -40.0, None),
        p(PoiKind::HorizonJob, "job", 0, -120.0, -150.0, None),
        p(PoiKind::HorizonStory, "story", 0, 90.0, -60.0, None),
        p(PoiKind::XpBoard, "xp", 1, 10.0, -60.0, None),
        p(PoiKind::AftermarketSpot, "am", 0, 40.0, 220.0, None),
        p(PoiKind::TreasureChest, &format!("DISCOUNT_BOARD_TREASURE_CHEST_{chest_no:03}"), chest_no, -60.0, 60.0, None),
    ]));
    // A road along the race ring (5 m outside it): the "relevant" road of the in-race focus, in
    // colour among the muted cross roads (D66).
    let ring: Vec<[f32; 2]> = (0..=64)
        .map(|i| {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            [404.8 * a.cos(), 303.6 * a.sin()]
        })
        .collect();
    let mut roads = (*m.roads).clone();
    roads.by_type[crate::gamedata::roadtypes::RoadType::Road.index() as usize].push(crate::maprender::data::Chain::new(ring.clone(), vec![0.0; ring.len()]));
    m.roads = Arc::new(roads);
    if icons {
        m.icons = Some(Arc::new(synthetic_icons()));
    }
    Arc::new(m)
}

/// The harness's stand-in for `maprender::layers()` (see `Renderer::layers_fn`).
fn synthetic_store_layers() -> Layers {
    static L: OnceLock<Layers> = OnceLock::new();
    L.get_or_init(|| Layers { status: LayerStatus::Ready, data: Some(harness_layers(true)) }).clone()
}

/// The D62 HUD map config, flat (the tilt switched off): the old flat checks apply to it.
fn flat_hud() -> OverlayConfig {
    let mut layers = MapLayerConfig::hud();
    layers.tilt.on = false;
    OverlayConfig { map_layers: layers, ..Default::default() }
}

/// The map layers of the real install (project road types, no user override), or `None` without
/// one. For the "real" PNG states: the look of actual roads, POIs and the game's own icons.
fn real_layers() -> Option<Arc<MapLayers>> {
    use crate::gamedata::roadtypes::RoadTypes;
    let media = crate::gamedata::install::find_media(None)?;
    let g = crate::maprender::data::GameData::load(&media).ok()?;
    let cur = RoadTypes::current_with(RoadTypes::project(), &RoadTypes::project_sha1(), std::path::Path::new("/nonexistent"), &g.nav);
    Some(Arc::new(g.layers(&cur, 1)))
}

/// Real-world spots worth a picture: (name, car x, z, yaw, in a race).
fn real_spots(l: &MapLayers) -> Vec<(&'static str, f32, f32, f32, bool)> {
    let first = |k: PoiKind| l.pois.items.iter().find(|p| p.kind == k);
    let mut out = Vec::new();
    // 110 m before the POI, heading at it (north-ish: yaw 0 = +z).
    let near = |p: &Poi| (p.x, p.z - 110.0);
    if let Some(c) = l.pois.current_chest(week_index_now()) {
        let (x, z) = near(c);
        out.push(("chest", x, z, 0.0, false));
    }
    if let Some(p) = first(PoiKind::DangerSign) {
        let (x, z) = near(p);
        out.push(("danger", x, z, 0.0, false));
    }
    if let Some(p) = first(PoiKind::SpeedZone) {
        let (x, z) = near(p);
        out.push(("gates", x, z, 0.0, false));
    }
    if let Some(p) = first(PoiKind::CarMeet) {
        let (x, z) = near(p);
        out.push(("carmeet", x, z, 0.6, false));
    }
    // On the start of the first race line, in a race.
    if let Some(r) = l.races.lines.first() {
        out.push(("race", r.pts[2][0], r.pts[2][1], (r.pts[3][0] - r.pts[2][0]).atan2(r.pts[3][1] - r.pts[2][1]), true));
    }
    out
}

/// The Minimap frame options (D75) as the PNG states use them: a circle of diameter 200 or a 300 x 200
/// rounded rect (radius 40), both with a solid, clearly coloured 4-6 px outline, and for the rect a
/// blue plate showing through the half-transparent image.
fn frame_cfg(kind: &str, tilted: bool) -> OverlayConfig {
    let mut c = if tilted { OverlayConfig::default() } else { flat_hud() };
    c.map_border_opacity = 1.0;
    match kind {
        "circle" => {
            c.map_shape = crate::config::MapShape::Circle;
            c.map_width = 200.0;
            c.map_border_width = 4.0;
            c.map_border_color = [230, 60, 60];
        }
        _ => {
            c.map_width = 300.0;
            c.map_height = 200.0;
            c.map_corner_radius = 40.0;
            c.map_border_width = 6.0;
            c.map_border_color = [60, 160, 230];
            c.map_plate_color = [30, 60, 120];
            c.map_plate_opacity = 0.6;
            c.map_layers.image.opacity = 0.5;
        }
    }
    c
}

/// Signed distance (design px, negative inside) of `p` to a rounded rect of `size` with corner
/// radius `r` (`size / 2` = a circle).
fn frame_sdf(p: (f32, f32), size: Vec2, r: f32) -> f32 {
    let (qx, qy) = ((p.0 - size.x / 2.0).abs() - (size.x / 2.0 - r), (p.1 - size.y / 2.0).abs() - (size.y / 2.0 - r));
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Checks of a 1x frame tile (widget at +10): every pixel more than 1.5 px outside the shape is the
/// backdrop (the map is clipped exactly to the circle / rounded rect, nothing pokes out), the
/// outline ring is `ring` in colour where it has one, and the shape's inside is not backdrop.
fn check_frame(failures: &mut Vec<String>, img: &ColorImage, id: &str, cfg: &OverlayConfig, ring: Option<[u8; 3]>) {
    let f = minimap::Frame::of(cfg);
    let size = vec2(f.w, f.h);
    let bgc = [GAME_BG.r(), GAME_BG.g(), GAME_BG.b()];
    let mut bad = Vec::new();
    for y in 0..img.size[1] {
        for x in 0..img.size[0] {
            let p = (x as f32 + 0.5 - 10.0, y as f32 + 0.5 - 10.0);
            if frame_sdf(p, size, f.radius) > 1.5 && px(img, x, y).iter().zip(bgc).any(|(a, b)| a.abs_diff(b) > 4) {
                bad.push((x, y));
            }
        }
    }
    println!("  {id}: {} pixels outside the shape are not the backdrop", bad.len());
    if !bad.is_empty() {
        failures.push(format!("{id}: the frame leaks outside its shape at {:?}", &bad[..bad.len().min(8)]));
    }
    if let Some(want) = ring {
        let mid = f.border / 2.0;
        let pts: Vec<(f32, f32)> = if f.circle {
            (0..12).map(|i| {
                let a = i as f32 / 12.0 * std::f32::consts::TAU + 0.2;
                (f.w / 2.0 + (f.w / 2.0 - mid) * a.cos(), f.h / 2.0 + (f.h / 2.0 - mid) * a.sin())
            }).collect()
        } else {
            vec![(f.w / 2.0, mid), (f.w / 2.0, f.h - mid), (mid, f.h / 2.0), (f.w - mid, f.h / 2.0)]
        };
        for (x, y) in pts {
            check(failures, img, id, (x.round() as usize, y.round() as usize), want, "outline ring");
        }
    }
}

fn save(img: &ColorImage, name: &str) -> Result<PathBuf, String> {
    let path = out_dir().join(format!("{name}.png"));
    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let [w, h] = img.size;
    image::save_buffer(&path, &raw, w as u32, h as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

fn px(img: &ColorImage, x: usize, y: usize) -> [u8; 3] {
    let c = img.pixels[y * img.size[0] + x];
    [c.r(), c.g(), c.b()]
}

/// Sample the pixel at design (x, y) of a 1× tile (widget at +10) and compare with `want`.
fn check(failures: &mut Vec<String>, img: &ColorImage, name: &str, (x, y): (usize, usize), want: [u8; 3], what: &str) {
    let got = px(img, x + 10, y + 10);
    let ok = got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 4);
    println!("  {name} @({x},{y}) {what}: got {got:?}, want {want:?} {}", if ok { "ok" } else { "MISMATCH" });
    if !ok {
        failures.push(format!("{name} @({x},{y}) {what}: got {got:?}, want {want:?}"));
    }
}

/// Bounding box `[x0, y0, x1, y1]` (widget design px, 1×) of near-white pixels within
/// radius `r` of `c`: the gear glyph's ink.
fn ink_box(img: &ColorImage, c: (f32, f32), r: f32) -> Option<[f32; 4]> {
    let mut b: Option<[f32; 4]> = None;
    for y in 0..img.size[1] {
        for x in 0..img.size[0] {
            let (dx, dy) = (x as f32 + 0.5 - 10.0 - c.0, y as f32 + 0.5 - 10.0 - c.1);
            if dx * dx + dy * dy > r * r || px(img, x, y).iter().any(|&v| v < 200) {
                continue;
            }
            let (fx, fy) = (x as f32 - 10.0, y as f32 - 10.0);
            b = Some(match b {
                None => [fx, fy, fx + 1.0, fy + 1.0],
                Some([x0, y0, x1, y1]) => [x0.min(fx), y0.min(fy), x1.max(fx + 1.0), y1.max(fy + 1.0)],
            });
        }
    }
    b
}

/// `c` at alpha `a` over opaque `bg`, gamma space (how egui_glow and CSS blend).
fn over(c: [u8; 3], a: f32, bg: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 * a + bg[i] as f32 * (1.0 - a)).round() as u8)
}

/// A 1920 x 1080 RGBA8 render target on the current (surfaceless) context, bound.
fn target_fbo(gl: &glow::Context) -> Result<(glow::Framebuffer, glow::Renderbuffer), String> {
    // SAFETY: plain GL object setup on the current (surfaceless) context.
    unsafe {
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        let rb = gl.create_renderbuffer()?;
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, 1920, 1080);
        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(rb));
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            return Err("FBO incomplete".into());
        }
        Ok((fbo, rb))
    }
}

#[test]
#[ignore = "needs a GPU / EGL device; writes target/hud_png/*.png"]
fn render_spec_states() -> Result<(), String> {
    let headless = Headless::new().map_err(|e| format!("headless EGL unavailable: {e}"))?;
    let gl = headless.glow.clone();
    let fbo = target_fbo(&gl)?;
    // Declared after `headless`, so it drops first (the painter needs the context).
    let mut r = Renderer::new(gl.clone(), None)?;
    r.layers_fn = synthetic_store_layers;
    std::fs::create_dir_all(out_dir()).map_err(|e| format!("{}: {e}", out_dir().display()))?;

    let summer = r.ctx.load_texture("test-map-summer", synthetic_map(false), OVERLAY_MAP_TEXTURE_OPTIONS);
    let winter = r.ctx.load_texture("test-map-winter", synthetic_map(true), OVERLAY_MAP_TEXTURE_OPTIONS);
    let orig = [8192, 8192];
    let map_s = MapTex { id: summer.id(), orig_size: orig, winter: false };
    let map_w = MapTex { id: winter.id(), orig_size: orig, winter: true };

    let rpm_cfg = OverlayConfig { rpm_label: true, ..Default::default() };
    let mut pulse = cluster_state(OverlayConfig::default(), 0.52, 4, 142.0);
    pulse.events.gear_changed_at = Some(NOW - 0.1);
    let clusters = [
        ("cruise", cluster_state(OverlayConfig::default(), 0.52, 4, 142.0)),
        ("redline", cluster_state(OverlayConfig::default(), 0.89, 3, 176.0)),
        ("shift", cluster_state(OverlayConfig::default(), 0.95, 3, 184.0)),
        ("pulse", pulse),
        ("rpm_label", cluster_state(rpm_cfg, 0.52, 4, 142.0)),
        ("auto_street", auto_state(DriveMode::Street, 4)),
        ("auto_sport", auto_state(DriveMode::Sport, 3)),
        ("auto_race", auto_state(DriveMode::Race, 5)),
        ("auto_ten", auto_state(DriveMode::Street, 10)),
        ("auto_reverse", auto_state(DriveMode::Race, 0)),
    ];

    let mut gained = race_state(2, 2, 47.910);
    gained.events.place_change = Some(PlaceChange { at: NOW - 1.0, gained: true });
    let mut lost = race_state(4, 2, 52.336);
    lost.events.place_change = Some(PlaceChange { at: NOW - 1.0, gained: false });
    let mut hold = race_state(2, 3, 3.1);
    hold.pkt.last_lap = 64.882;
    hold.pkt.best_lap = 64.882;
    hold.events.lap_completed_at = Some(NOW - 1.0);
    let mut ahead = race_state(3, 2, 41.273);
    ahead.lap_delta = Some(-0.42);
    let mut behind = race_state(3, 2, 41.273);
    behind.lap_delta = Some(0.37);
    let races = [("running", race_state(3, 2, 41.273)), ("gained", gained), ("lost", lost), ("lap_hold", hold), ("delta_ahead", ahead), ("delta_behind", behind)];

    let drifts = [
        ("counting", drift_state(54_648.0, None, Some(0.45), true)),
        ("chip", drift_state(58_687.0, Some((4039.0, 1.2)), Some(0.2), true)),
        // The spec's "Not scoring": grey dot, the bar still cycling (0.8).
        ("idle", drift_state(58_687.0, None, Some(0.8), false)),
    ];
    // Position + Gain (the default style): (snapshot, shown count-up value).
    let pgs = [
        ("running", pg_state(3, Some(4039.0), 4039.0, true, None)),
        ("gained", pg_state(2, Some(4039.0), 4039.0, true, Some(true))),
        ("lost", pg_state(4, Some(1250.0), 1250.0, true, Some(false))),
        ("counting", pg_state(3, Some(12_345.0), 5210.6, true, None)),
        // A window that scored nothing: dimmed "+0", grey dot.
        ("not_scoring", pg_state(3, Some(0.0), 0.0, false, None)),
        // Widest realistic gain: shrinks to stay clear of the dot.
        ("wide", pg_state(12, Some(123_456.0), 123_456.0, true, None)),
    ];

    // The old flat states keep the 1500 m radius their teammate positions were laid out for.
    let zoomed_out = OverlayConfig { zoom_driving_m: 1500.0, ..flat_hud() };
    let mut compass_off = base(OverlayConfig { compass: false, ..zoomed_out.clone() });
    compass_off.pkt.speed = 20.0;
    let mut driving = base(zoomed_out);
    driving.pkt.speed = 20.0;
    let mates = coop_mates(&driving);
    let none = CoopLayer::default();
    let maps = [
        ("summer", driving.clone(), map_s, &none),
        ("winter", driving.clone(), map_w, &none),
        ("compass_off", compass_off, map_s, &none),
        ("coop", driving, map_s, &mates),
    ];

    // Minimap states with roads, race lines, POIs and icons (D62 defaults: 300 m radius, heading-up,
    // dimmed image, tilted), over a dark stand-in for the game. The synthetic world is a road cross
    // around the origin; the race states put the car on its ring.
    let lcar = |mut s: HudSnapshot, x: f32, z: f32, yaw: f32| {
        s.pkt.position_x = x;
        s.pkt.position_z = z;
        s.pkt.yaw = yaw;
        s.pkt.speed = 25.0;
        s
    };
    let tilted = || base(OverlayConfig::default());
    let flat = || base(flat_hud());
    let mut north = OverlayConfig::default();
    north.map_north_up = true;
    let mut in_race = lcar(tilted(), 398.0, 20.0, 0.0);
    in_race.pkt.race_position = 2;
    let mut in_race_flat = lcar(flat(), 398.0, 20.0, 0.0);
    in_race_flat.pkt.race_position = 2;
    // The same race with the focus switched off (everything drawn as before D66).
    let mut unfocused = flat_hud();
    unfocused.map_layers.race_lines.focus.other_roads = crate::maprender::cfg::OtherRoads::Normal;
    unfocused.map_layers.race_lines.focus.hide_pois = false;
    let mut in_race_unfocused = lcar(base(unfocused), 398.0, 20.0, 0.0);
    in_race_unfocused.pkt.race_position = 2;
    let free = lcar(tilted(), 0.0, -60.0, 0.6);
    let hud_cfg = |f: &dyn Fn(&mut OverlayConfig)| {
        let mut c = OverlayConfig::default();
        f(&mut c);
        c
    };
    let wide = base(hud_cfg(&|c| c.zoom_driving_m = 700.0));
    let wide = lcar(wide, 0.0, -60.0, 0.6);
    let layered_mates = coop_mates(&free);
    // (name, snapshot, with icons, teammates)
    let layered: Vec<(&str, HudSnapshot, bool, &CoopLayer)> = vec![
        ("layers_flat", lcar(flat(), 0.0, -60.0, 0.6), true, &none),
        ("layers_tilted", free.clone(), true, &none),
        ("layers_tilted_markers", free.clone(), false, &none),
        ("layers_tilted_race", in_race, true, &none),
        ("layers_flat_race", in_race_flat, true, &none),
        ("layers_flat_race_unfocused", in_race_unfocused, true, &none),
        ("layers_tilted_northup", HudSnapshot { cfg: Arc::new(north), ..free.clone() }, true, &none),
        ("layers_tilted_wide", wide, true, &none),
        ("layers_tilted_coop", free, true, &layered_mates),
    ];
    let mut icon_tex = IconTex::default();
    let atlas: Option<Arc<IconAtlas>> = icon_tex.ensure(&r.ctx, Some(&Arc::new(synthetic_icons())));
    let layer_data = harness_layers(false);
    // The real install's layers and icons, if there is one (a visual check only, no assertions).
    let mut real_tex = IconTex::default();
    let real = real_layers().map(|d| {
        let atlas = real_tex.ensure(&r.ctx, d.icons.as_ref());
        (d, atlas)
    });

    let mut failures = Vec::new();
    let mut written = Vec::new();
    let bg = [230, 235, 240];
    let plate = over([9, 13, 21], 0.68, bg);
    for s in [1.0, 3.0] {
        let sfx = if s == 1.0 { "1x" } else { "3x" };
        for (name, snap) in &clusters {
            for (style, size) in [("d1a", cluster::PILL_SIZE), ("d3a", cluster::HALO_SIZE)] {
                let img = tile(&mut r, size, s, |p, xf| {
                    if style == "d1a" {
                        cluster::draw_pill(p, xf, snap, NOW, cluster::speed(snap));
                    } else {
                        cluster::draw_halo(p, xf, snap, NOW, cluster::speed(snap));
                    }
                });
                let id = format!("{style}_{name}_{sfx}");
                if s == 1.0 && style == "d1a" {
                    let cell = over([255; 3], 0.13, plate);
                    check(&mut failures, &img, &id, (178, 23), plate, "plate");
                    check(&mut failures, &img, &id, (54, 38), if *name == "shift" { [0x5B, 0x8B, 0xF0] } else { [245, 247, 251] }, "rev seg 0 lit");
                    check(&mut failures, &img, &id, (129, 38), if *name == "cruise" || *name == "pulse" || *name == "rpm_label" || name.starts_with("auto") { over([255; 3], 0.15, plate) } else if *name == "shift" { [0x5B, 0x8B, 0xF0] } else { [245, 247, 251] }, "rev seg 10");
                    let gear = match *name {
                        "shift" => [0x3C, 0x6B, 0xDE],
                        "pulse" => [245, 247, 251],
                        _ => cell,
                    };
                    check(&mut failures, &img, &id, (23, 7), gear, "gear cell");
                    // User: the gear sat too high. Its ink (near-white, inside the cell) is now
                    // centred at y 23, the cell centre (it was 24, which read a px too low in-game; keep in sync with
                    // `cluster::PILL_GEAR.y`, which is private); allow ±1 for AA and glyphs.
                    if *name != "pulse" {
                        match ink_box(&img, (23.0, 23.0), 18.0) {
                            Some([_, top, _, bot]) if (22.0..=24.0).contains(&((top + bot) / 2.0)) => {
                                println!("  {id} gear ink y {top}–{bot}: ok");
                            }
                            other => failures.push(format!("{id}: gear ink box {other:?} not centred on y 23")),
                        }
                    }
                    if *name == "redline" {
                        check(&mut failures, &img, &id, (23, 5), [255, 67, 56], "redline ring");
                    }
                }
                if s == 1.0 && style == "d3a" {
                    check(&mut failures, &img, &id, (15, 84), if *name == "shift" { [0x5B, 0x8B, 0xF0] } else { [245, 247, 251] }, "ring sector 0 lit");
                }
                written.push(save(&img, &id)?);
            }
        }
        for (name, snap) in &races {
            let img = tile(&mut r, race::SIZE, s, |p, xf| {
                race::draw(p, xf, snap, NOW);
            });
            let id = format!("r1_{name}_{sfx}");
            if s == 1.0 {
                let cap = over([58, 66, 82], 0.95, plate);
                let want = match *name {
                    "gained" => [0x2E, 0x9E, 0x48],
                    "lost" => [0xD8, 0x32, 0x2B],
                    _ => cap,
                };
                check(&mut failures, &img, &id, (8, 23), want, "cap");
                check(&mut failures, &img, &id, (192, 23), plate, "plate");
            }
            written.push(save(&img, &id)?);
        }
        for (name, snap) in &drifts {
            let img = tile(&mut r, drift::SIZE, s, |p, xf| {
                drift::draw(p, xf, snap, NOW, snap.drift.score);
            });
            let id = format!("x1_{name}_{sfx}");
            if s == 1.0 {
                let cap = over([58, 66, 82], 0.95, plate);
                check(&mut failures, &img, &id, (8, 23), if *name == "chip" { [0xFF, 0xB0, 0x2E] } else { cap }, "cap");
                match *name {
                    "counting" => check(&mut failures, &img, &id, (32, 15), [0xFF, 0xB0, 0x2E], "dot"),
                    "idle" => check(&mut failures, &img, &id, (32, 15), over([255; 3], 0.22, cap), "dot"),
                    _ => {}
                }
                if *name != "chip" {
                    check(&mut failures, &img, &id, (70, 39), [0xFF, 0xB0, 0x2E], "bar fill");
                    check(&mut failures, &img, &id, (180, 39), over([255; 3], 0.16, plate), "bar track");
                }
            }
            written.push(save(&img, &id)?);
        }
        for (name, (snap, shown)) in &pgs {
            let img = tile(&mut r, drift::SIZE, s, |p, xf| {
                drift::draw_position_gain(p, xf, snap, NOW, *shown);
            });
            let id = format!("x1pg_{name}_{sfx}");
            if s == 1.0 {
                let cap = over([58, 66, 82], 0.95, plate);
                let want = match *name {
                    "gained" => [0x2E, 0x9E, 0x48],
                    "lost" => [0xD8, 0x32, 0x2B],
                    _ => cap,
                };
                check(&mut failures, &img, &id, (8, 23), want, "cap");
                let dot = if *name == "not_scoring" { over([255; 3], 0.22, plate) } else { [0xFF, 0xB0, 0x2E] };
                check(&mut failures, &img, &id, (72, 20), dot, "dot");
                check(&mut failures, &img, &id, (70, 39), [0xFF, 0xB0, 0x2E], "bar fill");
                check(&mut failures, &img, &id, (180, 39), over([255; 3], 0.16, plate), "bar track");
                check(&mut failures, &img, &id, (192, 23), plate, "plate");
            }
            written.push(save(&img, &id)?);
        }
        for (name, snap, map, mates) in &maps {
            let img = tile(&mut r, minimap::SIZE, s, |p, xf| {
                minimap::draw(p, xf, snap, NOW, &mut MapAnim::default(), Some(*map), mates);
            });
            let id = format!("m2_{name}_{sfx}");
            if s == 1.0 {
                // The own arrow is white solo, in the co-op colour (`coop_hue`) in a session.
                let own = if *name == "coop" {
                    let [r, g, b, _] = crate::ui::coop::hue_color(snap.coop_hue).to_array();
                    [r, g, b]
                } else {
                    [255, 255, 255]
                };
                check(&mut failures, &img, &id, (104, 70), own, "car marker");
                if *name == "coop" {
                    let [r, g, b, _] = crate::ui::coop::hue_color(36.0).to_array();
                    check(&mut failures, &img, &id, (87, 30), [r, g, b], "teammate arrow fill");
                }
                let frame = px(&img, 11, 78);
                println!("  {id} frame border (1,68): {frame:?}");
                if frame.iter().any(|&c| c > 70) {
                    failures.push(format!("{id}: frame border not dark: {frame:?}"));
                }
                if *name != "compass_off" {
                    let dial = px(&img, 10 + 18, 10 + 8);
                    println!("  {id} compass disc (18,8): {dial:?}");
                }
            }
            written.push(save(&img, &id)?);
        }
        for (name, snap, icons, mates) in &layered {
            let render = |r: &mut Renderer, layers: bool| {
                tile_on(r, minimap::SIZE, s, GAME_BG, |p, xf| {
                    let mut anim = MapAnim::default();
                    if layers {
                        anim.set_layers(Some(layer_data.clone()));
                        anim.set_icons(atlas.clone().filter(|_| *icons));
                    }
                    minimap::draw(p, xf, snap, NOW, &mut anim, Some(map_s), mates);
                })
            };
            let img = render(&mut r, true);
            let id = format!("m2_{name}_{sfx}");
            if s == 1.0 {
                // Nothing of the vectors pokes into the transparent surround of the rounded corners.
                let bgc = [GAME_BG.r(), GAME_BG.g(), GAME_BG.b()];
                for (cx, cy) in [(2, 2), (205, 2), (2, 133), (205, 133)] {
                    check(&mut failures, &img, &id, (cx, cy), bgc, "rounded corner stays clear");
                }
                // The own arrow: at the pill's centre flat, 85 % down when tilted.
                let tilt = snap.cfg.map_layers.tilt.on;
                let own = if name.contains("coop") {
                    let [r, g, b, _] = crate::ui::coop::hue_color(snap.coop_hue).to_array();
                    [r, g, b]
                } else {
                    [255, 255, 255]
                };
                check(&mut failures, &img, &id, (104, if tilt { 114 } else { 70 }), own, "car marker");
                // The layers changed the picture compared with the image alone.
                let plain = render(&mut r, false);
                let diff = img.pixels.iter().zip(&plain.pixels).filter(|(a, b)| a != b).count();
                println!("  {id}: {diff} px differ from the image-only tile");
                if diff < 1500 {
                    failures.push(format!("{id}: layers barely drew ({diff} px differ)"));
                }
                // User: tilted, the image did not fill the pill (the plane ended ~26 px under its
                // top and faded over the next 41). With plate 0 an unpainted pixel is the game's
                // background: none may be, edge to edge, rounded top corners included.
                // (Teammates' and the own arrows' dark outlines are skipped.)
                if tilt && !name.contains("coop") {
                    let mut empty = Vec::new();
                    for y in (4..=132).step_by(4) {
                        for x in (4..=204).step_by(4) {
                            // Inside the rounded rect within the 3 px border (corner radius 19).
                            let (cx, cy) = ((x as f32).clamp(22.0, 186.0), (y as f32).clamp(22.0, 114.0));
                            if (x as f32 - cx).hypot(y as f32 - cy) > 18.0 || ((x as i32 - 104).abs() < 14 && (y as i32 - 114).abs() < 14) {
                                continue;
                            }
                            let got = px(&plain, x + 10, y + 10);
                            if got.iter().zip(bgc).all(|(a, b)| a.abs_diff(b) < 8) {
                                empty.push((x, y));
                            }
                        }
                    }
                    println!("  {id}: {} unpainted samples in the pill", empty.len());
                    if !empty.is_empty() {
                        failures.push(format!("{id}: the tilted image leaves the pill unpainted at {:?}", &empty[..empty.len().min(12)]));
                    }
                }
            }
            written.push(save(&img, &id)?);
        }
        // Frame options (D75): a circle (tilted, flat) and a custom 300 x 200 rect with its own outline
        // and plate, layers and icons on, teammates and trails in.
        for (name, kind, tilt) in [("frame_circle_tilted", "circle", true), ("frame_circle_flat", "circle", false), ("frame_rect_300x200", "rect", true), ("frame_rect_300x200_flat", "rect", false)] {
            let cfg = frame_cfg(kind, tilt);
            let snap = lcar(base(cfg.clone()), 0.0, -60.0, 0.6);
            let size = minimap::size(&cfg);
            let img = tile_on(&mut r, size, s, GAME_BG, |p, xf| {
                let mut anim = MapAnim::default();
                anim.set_layers(Some(layer_data.clone()));
                anim.set_icons(atlas.clone());
                minimap::draw(p, xf, &snap, NOW, &mut anim, Some(map_s), &layered_mates);
            });
            let id = format!("m2_{name}_{sfx}");
            if s == 1.0 {
                let ring = Some(cfg.map_border_color);
                check_frame(&mut failures, &img, &id, &cfg, ring);
                // The own arrow: at the centre flat, 85 % down when tilted.
                let car_y = if tilt { size.y * 0.85 } else { size.y / 2.0 };
                let [or, og, ob, _] = crate::ui::coop::hue_color(snap.coop_hue).to_array();
                check(&mut failures, &img, &id, ((size.x / 2.0) as usize, car_y as usize), [or, og, ob], "car marker (co-op colour)");
                if kind == "rect" {
                    // The plate shows through the half-transparent image corner (tilted: the far fade
                    // reveals it at the top).
                    let top = px(&img, 10 + (size.x / 2.0) as usize, 10 + 12);
                    println!("  {id}: top-centre {top:?}");
                }
            }
            written.push(save(&img, &id)?);
        }
        if let Some((data, atlas)) = &real {
            for (name, x, z, yaw, in_race) in real_spots(data) {
                println!("  real spot {name}: car ({x:.0}, {z:.0}) yaw {yaw:.2}");
                let mut snap = lcar(tilted(), x, z, yaw);
                snap.pkt.race_position = u8::from(in_race) * 3;
                for (variant, cfg) in [("tilted", OverlayConfig::default()), ("flat", flat_hud())] {
                    snap.cfg = Arc::new(cfg);
                    let img = tile_on(&mut r, minimap::SIZE, s, GAME_BG, |p, xf| {
                        let mut anim = MapAnim::default();
                        anim.set_layers(Some(data.clone()));
                        anim.set_icons(atlas.clone());
                        minimap::draw(p, xf, &snap, NOW, &mut anim, Some(map_s), &none);
                    });
                    written.push(save(&img, &format!("m2_real_{name}_{variant}_{sfx}"))?);
                }
            }
        }
    }

    // Full-screen composite of the default layout (map bottom-left, D1a bottom-centre, R1′
    // top-left), then with D3a′ and in drift mode.
    let mut composite = race_state(3, 2, 41.273);
    composite.cfg = Arc::new(OverlayConfig { fade: false, ..Default::default() });
    // The default HUD map is tilted: the car sits 85 % down the pill (design y 116).
    composite.pkt.current_engine_rpm = 0.52 * 8000.0;
    composite.pkt.gear = 4;
    composite.pkt.speed = 142.0 / 3.6;
    composite.lap_delta = Some(-0.42);
    let halo = HudSnapshot { cfg: Arc::new(OverlayConfig { fade: false, cluster_style: ClusterStyle::Halo, ..Default::default() }), ..composite.clone() };
    let drifting = HudSnapshot { mode: HudMode::Drift, drift: drift_state(58_687.0, Some((4039.0, 1.2)), Some(0.2), true).drift, ..composite.clone() };
    let drift_total = HudSnapshot { cfg: Arc::new(OverlayConfig { fade: false, drift_style: DriftStyle::Total, ..Default::default() }), ..drifting.clone() };
    // Custom spacing: margin 100, gap 30, the cluster stacked on the map (bottom-left).
    let spaced_cfg = OverlayConfig { fade: false, margin_px: 100.0, gap_px: 30.0, cluster_cell: crate::config::HudCell::BottomLeft, ..Default::default() };
    let spaced = HudSnapshot { cfg: Arc::new(spaced_cfg), ..composite.clone() };
    r.map.set(summer.clone(), orig, Season::Summer);
    for (name, snap) in [("composite_1080p", composite), ("composite_halo_1080p", halo), ("composite_drift_1080p", drifting), ("composite_drift_total_1080p", drift_total), ("composite_spacing_1080p", spaced)] {
        r.hud = crate::hud::Hud::default();
        r.frame_at([1920, 1080], Some(&snap), false, NOW, SCREEN_BG.to_normalized_gamma_f32());
        let img = r.painter.read_screen_rgba([1920, 1080]);
        if img.size != [1920, 1080] {
            failures.push(format!("{name}: size {:?}", img.size));
        }
        if name == "composite_1080p" {
            let sbg = [128, 138, 150];
            let splate = over([9, 13, 21], 0.68, sbg);
            // The default layout at margin 4: D1a at (868, 1030) bottom centre, R1′ at (1720, 1030)
            // bottom right, M2′ at (4, 940) bottom left.
            for (what, (x, y)) in [("D1a plate", (868 + 178, 1030 + 23)), ("R1 plate", (1720 + 192, 1030 + 23))] {
                let got = px(&img, x, y);
                let ok = got.iter().zip(splate).all(|(a, b)| a.abs_diff(b) <= 4);
                println!("  {name} {what} @({x},{y}): got {got:?}, want {splate:?}");
                if !ok {
                    failures.push(format!("{name} {what}: got {got:?}, want {splate:?}"));
                }
            }
            let marker = px(&img, 4 + 104, 940 + 114);
            if marker != [255, 255, 255] {
                failures.push(format!("{name}: map car marker at (108, 1054) is {marker:?}"));
            }
            // Outside every widget: untouched backdrop.
            if px(&img, 960, 540) != sbg {
                failures.push(format!("{name}: screen centre not backdrop: {:?}", px(&img, 960, 540)));
            }
        }
        if name == "composite_spacing_1080p" {
            let sbg = [128, 138, 150];
            let splate = over([9, 13, 21], 0.68, sbg);
            // Map at (100, 844); D1a 30 px above it at (100, 768); R1′ bottom right at (1624, 934).
            for (what, (x, y), want) in [
                ("D1a plate", (100 + 178, 768 + 23), splate),
                ("R1 plate", (1624 + 192, 934 + 23), splate),
                ("gap D1a/map", (100 + 92, 814 + 15), sbg),
                ("left of margin", (90, 900), sbg),
            ] {
                let got = px(&img, x, y);
                println!("  {name} {what} @({x},{y}): got {got:?}, want {want:?}");
                if !got.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 4) {
                    failures.push(format!("{name} {what}: got {got:?}, want {want:?}"));
                }
            }
        }
        written.push(save(&img, name)?);
    }

    println!("wrote {} PNGs to {}:", written.len(), out_dir().display());
    for p in &written {
        if let Ok(img) = image::image_dimensions(p) {
            println!("  {} {}×{}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), img.0, img.1);
        }
    }

    drop(r);
    // SAFETY: the painter is gone; delete our FBO objects on the still-current context.
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(fbo.0);
        gl.delete_renderbuffer(fbo.1);
    }
    drop(headless);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("pixel checks failed:\n{}", failures.join("\n")))
    }
}

// ── the Minimap in 3D (phase K, K3) ──────────────────────────────────────────────────────────
//
// The same tiles as above, but through the shared GL renderer (`maprender::gl3d`) on the headless
// context, with the synthetic terrain (three hills on a flat 2 km square) or, when there is an
// install, the real terrain, roads and satellite image.
// Run: `cargo test render_3d_states -- --ignored --nocapture`.

/// The calibration of [`world_map`]: 512 px over the synthetic terrain's 2048 m square.
const WORLD_CAL: MapCalibration = MapCalibration { px_per_m: 0.25, origin_x: -1024.0, origin_z: 1024.0 };

fn synth_terrain() -> Arc<Terrain> {
    static T: OnceLock<Arc<Terrain>> = OnceLock::new();
    T.get_or_init(|| Arc::new(Terrain::synthetic())).clone()
}

fn synth_terrain_status() -> TerrainStatus {
    TerrainStatus::Ready(synth_terrain())
}

fn loading_terrain_status() -> TerrainStatus {
    TerrainStatus::Loading
}

/// The synthetic roads' mesh, built once (the store would build it on its own thread).
fn synth_mesh(l: &Arc<MapLayers>, t: &Arc<Terrain>) -> Option<Arc<RoadMesh>> {
    static M: OnceLock<Arc<RoadMesh>> = OnceLock::new();
    Some(M.get_or_init(|| Arc::new(RoadMesh::build(&l.roads, t, l.rev))).clone())
}

/// The harness layers plus an elevated highway: 22 m above the flat ground in the south, a bridge.
fn layers_3d(terrain: &Terrain) -> Arc<MapLayers> {
    let mut m = (*harness_layers(true)).clone();
    let pts: Vec<[f32; 2]> = (0..=35).map(|i| [-700.0 + i as f32 * 40.0, -500.0]).collect();
    let ys: Vec<f32> = pts.iter().map(|p| terrain.height(p[0], p[1]) + 22.0).collect();
    let mut roads = (*m.roads).clone();
    roads.by_type[crate::gamedata::roadtypes::RoadType::Highway.index() as usize].push(Chain::new(pts, ys));
    m.roads = Arc::new(roads);
    Arc::new(m)
}

/// A satellite stand-in calibrated to the synthetic terrain ([`WORLD_CAL`]): hill-shaded greens, a
/// 200 m grid to judge the draping by, a red patch at the origin.
fn world_map(terrain: &Terrain) -> ColorImage {
    let n = 512usize;
    let mut px = Vec::with_capacity(n * n);
    for r in 0..n {
        for c in 0..n {
            let (x, z) = (-1024.0 + (c as f32 + 0.5) * 4.0, 1024.0 - (r as f32 + 0.5) * 4.0);
            let hill = ((terrain.height(x, z) - 100.0) / 220.0).clamp(0.0, 1.0);
            let mut col = [60.0 + 120.0 * hill, 105.0 + 40.0 * hill, 55.0 + 60.0 * hill];
            if x.rem_euclid(200.0) < 6.0 || z.rem_euclid(200.0) < 6.0 {
                col = col.map(|v| v * 0.55);
            }
            if x.abs() < 14.0 && z.abs() < 14.0 {
                col = [220.0, 40.0, 40.0];
            }
            px.push(Color32::from_rgb(col[0] as u8, col[1] as u8, col[2] as u8));
        }
    }
    ColorImage::new([n, n], px)
}

/// The default HUD map config in the *3D* view mode, then `f`.
fn cfg_3d(f: impl FnOnce(&mut OverlayConfig)) -> OverlayConfig {
    let mut c = OverlayConfig::default();
    c.map_layers.tilt.set_view_mode(ViewMode::Relief);
    f(&mut c);
    c
}

/// A driving snapshot at (`x`, `z`) heading `yaw`, with the car on the `terrain` and the map
/// calibrated to `cal`.
fn snap_3d(cfg: OverlayConfig, terrain: &Terrain, (x, z): (f32, f32), yaw: f32, cal: MapCalibration) -> HudSnapshot {
    let mut s = base(cfg);
    s.pkt.position_x = x;
    s.pkt.position_z = z;
    s.pkt.position_y = terrain.height(x, z);
    s.pkt.yaw = yaw;
    s.pkt.speed = 25.0;
    s.minimap = crate::overlay::snapshot::MinimapCalib { px_per_m: cal.px_per_m, origin_x: cal.origin_x, origin_z: cal.origin_z };
    s
}

/// One world of 3D tiles: the data, the map, and the GL handle that draws them.
struct Rig3d<'a> {
    r: &'a mut Renderer,
    gl: Arc<glow::Context>,
    h: Gl3dHandle,
    terrain: Arc<Terrain>,
    mesh: Arc<RoadMesh>,
    layers: Arc<MapLayers>,
    atlas: Option<Arc<IconAtlas>>,
    map: MapTex,
}

impl Rig3d<'_> {
    /// One frame of the Minimap on a `GAME_BG` tile at scale `s` and fade `a`; `scene` = through the
    /// 3D renderer, else the 2D map.
    fn frame(&mut self, snap: &HudSnapshot, mates: &CoopLayer, s: f32, a: f32, scene: bool) -> ColorImage {
        let (h, terrain, mesh, layers, atlas, map) = (self.h.clone(), self.terrain.clone(), self.mesh.clone(), self.layers.clone(), self.atlas.clone(), self.map);
        let size = minimap::size(&snap.cfg);
        let px = [((size.x + 20.0) * s).round() as u32, ((size.y + 20.0) * s).round() as u32];
        let r = &mut *self.r;
        paint(&r.ctx, &mut r.painter, r.start, px, GAME_BG.to_normalized_gamma_f32(), |_, p| {
            let mut anim = MapAnim::default();
            anim.set_layers(Some(layers.clone()));
            anim.set_icons(atlas.clone());
            anim.set_scene3d(scene.then(|| Scene3dIn { gl3d: h.clone(), terrain: terrain.clone(), mesh: Some(mesh.clone()) }));
            minimap::draw(p, &Xf { o: pos2(10.0 * s, 10.0 * s), s, a }, snap, NOW, &mut anim, Some(map), mates);
        });
        r.painter.read_screen_rgba(px)
    }

    /// [`Self::frame`] once the staged init has finished (or the renderer gave up): the 3D scene
    /// needs a few frames (programs, heights, roads), and the frame that flips it to `Ready` still
    /// has the 2D markers, so one more is drawn after.
    fn tile(&mut self, snap: &HudSnapshot, mates: &CoopLayer, s: f32, a: f32) -> Result<ColorImage, String> {
        for _ in 0..24 {
            self.frame(snap, mates, s, a, true);
            let done = match self.h.status() {
                Gl3dStatus::Ready => !self.h.busy(),
                Gl3dStatus::Failed(_) => true,
                Gl3dStatus::Untried => false,
            };
            if done {
                return Ok(self.frame(snap, mates, s, a, true));
            }
        }
        Err(format!("the 3D scene did not settle: {:?}", self.h.status()))
    }

    fn gl_error(&self) -> u32 {
        // SAFETY: plain error query on the current context.
        unsafe { self.gl.get_error() }
    }
}

/// Design-px sample points inside the pill (3 px border and corners excluded) whose colour
/// differs from the game backdrop: how much of the pill the map paints.
fn pill_coverage(img: &ColorImage, skip_arrow: bool) -> (usize, usize) {
    let bgc = [GAME_BG.r(), GAME_BG.g(), GAME_BG.b()];
    let (mut painted, mut total) = (0, 0);
    for y in (6..=130).step_by(4) {
        for x in (6..=202).step_by(4) {
            let (cx, cy) = ((x as f32).clamp(22.0, 186.0), (y as f32).clamp(22.0, 114.0));
            if (x as f32 - cx).hypot(y as f32 - cy) > 17.0 || (skip_arrow && (x as i32 - 104).abs() < 14 && (y as i32 - 114).abs() < 14) {
                continue;
            }
            total += 1;
            if px(img, x + 10, y + 10).iter().zip(bgc).any(|(a, b)| a.abs_diff(b) >= 8) {
                painted += 1;
            }
        }
    }
    (painted, total)
}

/// Sum of per-channel differences from the backdrop, over the whole tile.
fn ink_from_bg(img: &ColorImage) -> u64 {
    let bg = [GAME_BG.r(), GAME_BG.g(), GAME_BG.b()];
    img.pixels.iter().map(|c| [c.r(), c.g(), c.b()].iter().zip(bg).map(|(a, b)| a.abs_diff(b) as u64).sum::<u64>()).sum()
}

fn differing(a: &ColorImage, b: &ColorImage) -> usize {
    a.pixels.iter().zip(&b.pixels).filter(|(p, q)| p != q).count()
}

#[test]
#[ignore = "needs a GPU / EGL device; writes target/hud_png/*.png"]
fn render_3d_states() -> Result<(), String> {
    let headless = Headless::new().map_err(|e| format!("headless EGL unavailable: {e}"))?;
    let gl = headless.glow.clone();
    let fbo = target_fbo(&gl)?;
    let mut r = Renderer::new(gl.clone(), None)?;
    std::fs::create_dir_all(out_dir()).map_err(|e| format!("{}: {e}", out_dir().display()))?;
    let guardless = || Gl3dHandle::with_options(Gl3dOptions { guard: None, ..Gl3dOptions::default() });

    let terrain = synth_terrain();
    let layers = layers_3d(&terrain);
    let mesh = Arc::new(RoadMesh::build(&layers.roads, &terrain, layers.rev));
    let tex = r.ctx.load_texture("test-world-map", world_map(&terrain), OVERLAY_MAP_TEXTURE_OPTIONS);
    let map = MapTex { id: tex.id(), orig_size: [512, 512], winter: false };
    let mut icon_tex = IconTex::default();
    let atlas = icon_tex.ensure(&r.ctx, Some(&Arc::new(synthetic_icons())));
    let none = CoopLayer::default();

    let mut failures: Vec<String> = Vec::new();
    let mut written = Vec::new();
    let bgc = [GAME_BG.r(), GAME_BG.g(), GAME_BG.b()];
    let h3 = guardless();
    let mut rig = Rig3d { r: &mut r, gl: gl.clone(), h: h3.clone(), terrain: terrain.clone(), mesh, layers, atlas, map };

    // The car approaching the big hill; on the elevated highway; on the race ring.
    let hill_yaw = (-180.0f32).atan2(240.0);
    let on_hills = |cfg: OverlayConfig| snap_3d(cfg, &terrain, (-120.0, -40.0), hill_yaw, WORLD_CAL);
    let mut bridge = snap_3d(cfg_3d(|_| {}), &terrain, (-100.0, -500.0), std::f32::consts::FRAC_PI_2, WORLD_CAL);
    bridge.pkt.position_y = terrain.height(-100.0, -500.0) + 22.0;
    let mut race = snap_3d(cfg_3d(|_| {}), &terrain, (398.0, 20.0), 0.0, WORLD_CAL);
    race.pkt.race_position = 2;
    let hills = on_hills(cfg_3d(|_| {}));
    let horizon = on_hills(cfg_3d(|c| c.map_layers.tilt.angle_deg = 72.0));
    let steep = on_hills(cfg_3d(|c| c.map_layers.tilt.relief.exaggeration = 2.5));
    let no_roads = on_hills(cfg_3d(|c| c.map_layers.roads.on = false));
    let no_image = on_hills(cfg_3d(|c| c.map_layers.image.on = false));
    let mates = coop_mates(&hills);

    let states: Vec<(&str, &HudSnapshot, &CoopLayer)> = vec![
        ("hills", &hills, &none),
        ("bridge", &bridge, &none),
        ("race", &race, &none),
        ("horizon", &horizon, &none),
        ("exaggerated", &steep, &none),
        ("no_roads", &no_roads, &none),
        ("no_image", &no_image, &none),
        ("coop", &hills, &mates),
    ];
    let mut imgs: std::collections::HashMap<&str, ColorImage> = Default::default();
    for s in [1.0, 3.0] {
        let sfx = if s == 1.0 { "1x" } else { "3x" };
        for (name, snap, mt) in &states {
            let img = rig.tile(snap, mt, s, 1.0)?;
            let id = format!("m2_3d_{name}_{sfx}");
            if s == 1.0 {
                let err = rig.gl_error();
                println!("  {id}: GL error {err:#x}, status {:?}", rig.h.status());
                if err != 0 || rig.h.status() != Gl3dStatus::Ready {
                    failures.push(format!("{id}: GL error {err:#x}, status {:?}", rig.h.status()));
                }
                // The pill's rounded corners stay clear of the 3D scene (the composite's mask).
                for (cx, cy) in [(2, 2), (205, 2), (2, 133), (205, 133)] {
                    check(&mut failures, &img, &id, (cx, cy), bgc, "rounded corner stays clear");
                }
                // The own arrow above the scene, at the car's row (85 % down); the co-op colour in a session.
                let own = if *name == "coop" {
                    let [r, g, b, _] = crate::ui::coop::hue_color(hills.coop_hue).to_array();
                    [r, g, b]
                } else {
                    [255, 255, 255]
                };
                check(&mut failures, &img, &id, (104, 114), own, "car marker over the scene");
                let (painted, total) = pill_coverage(&img, true);
                println!("  {id}: map paints {painted}/{total} samples");
                // (no image = backing colour only; the horizon view fades out above the far edge on purpose)
                if !matches!(*name, "no_image" | "horizon") && painted * 100 < total * 85 {
                    failures.push(format!("{id}: the map paints only {painted}/{total} samples of the pill"));
                }
                imgs.insert(name, img.clone());
            }
            written.push(save(&img, &id)?);
        }
    }
    println!("device {} ({})", headless.device, rig.h.caps().map_or("?".to_string(), |c| c.renderer));

    // Frame options (D75) in 3D: the composite masks the scene to the circle (the rounded-rect mask
    // with radius = half the size) and to a custom 300 x 200 rect.
    for (name, kind) in [("frame_circle", "circle"), ("frame_rect_300x200", "rect")] {
        let cfg = {
            let mut c = frame_cfg(kind, true);
            c.map_layers.tilt.set_view_mode(ViewMode::Relief);
            c
        };
        let snap = on_hills(cfg.clone());
        let size = minimap::size(&cfg);
        for s in [1.0, 3.0] {
            let sfx = if s == 1.0 { "1x" } else { "3x" };
            let img = rig.tile(&snap, &mates, s, 1.0)?;
            let id = format!("m2_3d_{name}_{sfx}");
            if s == 1.0 {
                let err = rig.gl_error();
                println!("  {id}: GL error {err:#x}, status {:?}", rig.h.status());
                if err != 0 || rig.h.status() != Gl3dStatus::Ready {
                    failures.push(format!("{id}: GL error {err:#x}, status {:?}", rig.h.status()));
                }
                check_frame(&mut failures, &img, &id, &cfg, Some(cfg.map_border_color));
                let [or, og, ob, _] = crate::ui::coop::hue_color(snap.coop_hue).to_array();
                check(&mut failures, &img, &id, ((size.x / 2.0) as usize, (size.y * 0.85) as usize), [or, og, ob], "car marker over the scene (co-op colour)");
                // The scene fills the shape: most samples well inside differ from the backdrop.
                let (mut painted, mut total) = (0, 0);
                for y in (0..size.y as usize).step_by(4) {
                    for x in (0..size.x as usize).step_by(4) {
                        if frame_sdf((x as f32, y as f32), size, minimap::Frame::of(&cfg).radius) < -(cfg.map_border_width + 4.0) {
                            total += 1;
                            painted += px(&img, x + 10, y + 10).iter().zip(bgc).any(|(a, b)| a.abs_diff(b) >= 8) as usize;
                        }
                    }
                }
                println!("  {id}: the scene paints {painted}/{total} samples inside the shape");
                if painted * 100 < total * 80 {
                    failures.push(format!("{id}: the 3D scene paints only {painted}/{total} samples of the shape"));
                }
            }
            written.push(save(&img, &id)?);
        }
    }

    // Against the same view in Tilted: relief changed the picture; the roads are GL ones.
    let tilted_cfg = {
        let mut c = cfg_3d(|_| {});
        c.map_layers.tilt.set_view_mode(ViewMode::Tilted);
        c
    };
    let tilted = rig.frame(&on_hills(tilted_cfg), &none, 1.0, 1.0, false);
    written.push(save(&tilted, "m2_3d_ref_tilted_1x")?);
    let d_relief = differing(&imgs["hills"], &tilted);
    let d_roads = differing(&imgs["hills"], &imgs["no_roads"]);
    let d_image = differing(&imgs["hills"], &imgs["no_image"]);
    println!("  hills vs tilted: {d_relief} px; vs no roads: {d_roads} px; vs no image: {d_image} px");
    for (what, d, min) in [("relief vs the tilted 2D map", d_relief, 3000), ("GL roads", d_roads, 300), ("the satellite image", d_image, 3000)] {
        if d < min {
            failures.push(format!("3D hills: {what} barely changed the picture ({d} px < {min})"));
        }
    }
    // A steeper pitch brings the horizon into view: the far edge fades into the game.
    let (hp, ht) = pill_coverage(&imgs["horizon"], true);
    let (sp, st) = pill_coverage(&imgs["hills"], true);
    println!("  horizon view paints {hp}/{ht}, default {sp}/{st}");
    if hp >= ht {
        failures.push("3D horizon: the far edge did not fade out at 72 deg".into());
    }

    // The show/hide fade: a = 0.5 brings everything (terrain, roads, markers) about halfway to the backdrop.
    let half = rig.tile(&hills, &none, 1.0, 0.5)?;
    written.push(save(&half, "m2_3d_fade_half_1x")?);
    let ratio = ink_from_bg(&half) as f64 / ink_from_bg(&imgs["hills"]) as f64;
    println!("  fade 0.5: {ratio:.3} of the full ink");
    if !(0.35..0.65).contains(&ratio) {
        failures.push(format!("3D fade 0.5: ink ratio {ratio:.3} not near 0.5"));
    }

    // Fallback: a renderer that cannot start shows today's tilted 2D map, pixel for pixel.
    let broken = Gl3dHandle::with_options(Gl3dOptions { break_shader: true, guard: None, ..Gl3dOptions::default() });
    rig.h = broken.clone();
    let fallback = rig.tile(&hills, &none, 1.0, 1.0)?;
    let reason = broken.status();
    println!("  broken shader: {reason:?}");
    if !matches!(reason, Gl3dStatus::Failed(_)) {
        failures.push(format!("fallback: expected Failed, got {reason:?}"));
    }
    let fb = differing(&fallback, &tilted);
    println!("  fallback vs tilted: {fb} px differ");
    if fb != 0 {
        failures.push(format!("fallback: the failed 3D HUD differs from the tilted 2D map in {fb} px"));
    }
    written.push(save(&fallback, "m2_3d_fallback_1x")?);
    broken.destroy(&gl);
    rig.h = h3.clone();

    // Through the real `Renderer::frame_at` path: terrain / mesh from the (injected) store,
    // the staged init reported as "animating", destroyed when the Minimap leaves 3D.
    drop(rig);
    r.layers_fn = synthetic_store_layers;
    r.terrain_fn = synth_terrain_status;
    r.mesh_fn = synth_mesh;
    r.gl3d.destroy(&gl);
    r.gl3d = guardless();
    r.map.set(tex.clone(), [512, 512], Season::Summer);
    let sbg = SCREEN_BG.to_normalized_gamma_f32();
    let mut comp = snap_3d(cfg_3d(|c| c.fade = false), &terrain, (-120.0, -40.0), hill_yaw, WORLD_CAL);
    comp.pkt.race_position = 3;
    comp.pkt.current_engine_rpm = 0.52 * 8000.0;
    comp.pkt.gear = 4;
    r.hud = crate::hud::Hud::default();
    let mut frames = 0;
    while r.frame_at([1920, 1080], Some(&comp), false, NOW, sbg) {
        frames += 1;
        if frames > 30 {
            failures.push("frame_at: still animating after 30 frames in 3D".into());
            break;
        }
    }
    println!("  frame_at: settled after {} frames, status {:?}, stats {:?}", frames + 1, r.gl3d.status(), r.gl3d.stats());
    if r.gl3d.status() != Gl3dStatus::Ready || frames < 2 {
        failures.push(format!("frame_at: 3D {:?} after {frames} animating frames (the staged init needs >= 2)", r.gl3d.status()));
    }
    let img = r.painter.read_screen_rgba([1920, 1080]);
    // The map sits bottom-left at margin 4: its car marker at (4 + 104, 940 + 114).
    if px(&img, 108, 1054) != [255, 255, 255] {
        failures.push(format!("composite_3d: car marker is {:?}", px(&img, 108, 1054)));
    }
    written.push(save(&img, "composite_3d_1080p")?);
    // Tilted again: the 3D objects are freed (and a later 3D switch starts afresh).
    let tilted_comp = HudSnapshot { cfg: Arc::new(OverlayConfig { fade: false, ..Default::default() }), ..comp.clone() };
    r.frame_at([1920, 1080], Some(&tilted_comp), false, NOW, sbg);
    if r.gl3d.status() != Gl3dStatus::Untried {
        failures.push(format!("frame_at: leaving 3D left the renderer {:?}", r.gl3d.status()));
    }
    // Terrain still loading: the 2D map, and frames keep coming until it is there.
    r.terrain_fn = loading_terrain_status;
    if !r.frame_at([1920, 1080], Some(&comp), false, NOW, sbg) || r.gl3d.status() != Gl3dStatus::Untried {
        failures.push("frame_at: a loading terrain should ask for more frames and not touch GL".into());
    }
    // A hidden HUD never pins the frame loop, whatever the 3D inputs are doing.
    r.terrain_fn = synth_terrain_status;
    r.hud = crate::hud::Hud::default();
    let hidden = HudSnapshot { visible: false, ..comp.clone() };
    if r.frame_at([1920, 1080], Some(&hidden), false, NOW, sbg) {
        failures.push("frame_at: a hidden HUD keeps asking for frames".into());
    }
    // SAFETY: plain error query on the current context.
    let err = unsafe { gl.get_error() };
    if err != 0 {
        failures.push(format!("frame_at: GL error {err:#x}"));
    }

    // The real install, if there is one: the real satellite image, terrain and roads.
    if let (Some(media), Some(real)) = (crate::gamedata::install::find_media(None), real_layers()) {
        match (Terrain::load(&media, &|_| {}), crate::minimap::overlay_map_image(Season::Summer)) {
            (Ok(rt), Ok((img, orig))) => {
                let rt = Arc::new(rt);
                let rtex = r.ctx.load_texture("real-map", img, OVERLAY_MAP_TEXTURE_OPTIONS);
                let rmesh = Arc::new(RoadMesh::build(&real.roads, &rt, real.rev));
                let mut real_tex = IconTex::default();
                let ratlas = real_tex.ensure(&r.ctx, real.icons.as_ref());
                let h = guardless();
                let mut rig = Rig3d { r: &mut r, gl: gl.clone(), h: h.clone(), terrain: rt.clone(), mesh: rmesh, layers: real.clone(), atlas: ratlas, map: MapTex { id: rtex.id(), orig_size: orig, winter: false } };
                let c = MapCalibration::DEFAULT;
                let mut timing_snap = None;
                for (name, x, z, yaw, in_race) in real_spots(&real) {
                    let mut snap = snap_3d(cfg_3d(|_| {}), &rt, (x, z), yaw, c);
                    snap.pkt.race_position = u8::from(in_race) * 3;
                    for s in [1.0, 3.0] {
                        let sfx = if s == 1.0 { "1x" } else { "3x" };
                        let img = rig.tile(&snap, &none, s, 1.0)?;
                        let id = format!("m2_real_{name}_3d_{sfx}");
                        if s == 1.0 {
                            let err = rig.gl_error();
                            println!("  {id}: GL error {err:#x}, status {:?}, car ({x:.0}, {z:.0}) yaw {yaw:.2} height {:.1} m", rig.h.status(), snap.pkt.position_y);
                            if err != 0 || rig.h.status() != Gl3dStatus::Ready {
                                failures.push(format!("{id}: GL error {err:#x}, status {:?}", rig.h.status()));
                            }
                            for (cx, cy) in [(2, 2), (205, 2), (2, 133), (205, 133)] {
                                check(&mut failures, &img, &id, (cx, cy), bgc, "rounded corner stays clear");
                            }
                            let (painted, total) = pill_coverage(&img, true);
                            println!("  {id}: map paints {painted}/{total} samples");
                        }
                        written.push(save(&img, &id)?);
                    }
                    timing_snap.get_or_insert(snap);
                }
                // Frame cost of the HUD map in 3D: 240 frames without read-back, then a finish.
                if let Some(snap) = timing_snap {
                    let n = 240;
                    let t0 = std::time::Instant::now();
                    for _ in 0..n {
                        rig.frame(&snap, &none, 1.0, 1.0, true);
                    }
                    // SAFETY: plain GL sync on the current context.
                    unsafe { gl.finish() };
                    let wall = t0.elapsed().as_secs_f64() * 1e3 / n as f64;
                    let t1 = std::time::Instant::now();
                    for _ in 0..n {
                        rig.frame(&snap, &none, 1.0, 1.0, false);
                    }
                    // SAFETY: as above.
                    unsafe { gl.finish() };
                    let wall2d = t1.elapsed().as_secs_f64() * 1e3 / n as f64;
                    let st = h.stats();
                    println!(
                        "  TIMING real HUD map on {} ({}): 3D {wall:.2} ms/frame total (incl. egui and the read-back), tilted 2D {wall2d:.2} ms; gl3d callback {:.2} ms, ema {:.2} ms, gpu {:?}, {} tri, {} draws, {} frames",
                        headless.device,
                        h.caps().map_or("?".to_string(), |c| c.renderer),
                        st.callback_ms,
                        st.ema_ms,
                        st.last.gpu_ms,
                        st.last.triangles,
                        st.last.draws,
                        st.frames
                    );
                }
                h.destroy(&gl);
            }
            (a, b) => println!("  real 3D states skipped: terrain {:?}, image {:?}", a.err(), b.err().map(|e| e.to_string())),
        }
    } else {
        println!("  real 3D states skipped: no install");
    }

    println!("wrote {} PNGs to {}", written.len(), out_dir().display());
    h3.destroy(&gl);
    drop(r);
    // SAFETY: the painter is gone; delete our FBO objects on the still-current context.
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(fbo.0);
        gl.delete_renderbuffer(fbo.1);
    }
    drop(headless);
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("pixel checks failed:\n{}", failures.join("\n")))
    }
}
