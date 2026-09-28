//! Offscreen PNG harness: renders the round-4 spec states through the overlay's own path
//! (`Renderer` → egui → egui_glow) into an FBO on a headless EGL device, reads the pixels
//! back and writes PNGs to `target/hud_png/`. Compiled as `overlay::render::png` (see the
//! `#[path]` there). Run: `cargo test render_spec_states -- --ignored --nocapture`.
//!
//! Each widget state is a tile of (w + 20) × (h + 20) design px, the widget at (10, 10), on
//! the mockup's snow-white state background, at 1× and 3×. Plus one 1920 × 1080 composite of
//! the default layout on a grey backdrop. Time is pinned (`NOW`), so flash phases, fades and
//! chips are deterministic.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{pos2, Color32, ColorImage, Painter, Vec2};
use egui_glow::glow::{self, HasContext};

use super::super::gl::Headless;
use super::{paint, Renderer};
use crate::config::{ClusterStyle, OverlayConfig};
use crate::hud::minimap::{MapAnim, MapTex};
use crate::hud::prims::Xf;
use crate::hud::{cluster, drift, minimap, race};
use crate::minimap::{MapCalibration, Season, OVERLAY_MAP_TEXTURE_OPTIONS};
use crate::overlay::snapshot::{DriftChip, DriftInfo, HudMode, HudSnapshot, PlaceChange};

/// Pinned clock; `.05` into a 100 ms flash period = the flash's "on" phase.
const NOW: f64 = 1000.05;
/// The mockup's state tiles sit on `linear-gradient(#F6F8FA, #D5DEE7)`; this is its middle.
const TILE_BG: Color32 = Color32::from_rgb(230, 235, 240);
const SCREEN_BG: Color32 = Color32::from_rgb(128, 138, 150);
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

fn race_state(pos: u8, lap0: u16, cur: f32) -> HudSnapshot {
    let mut s = base(OverlayConfig::default());
    s.pkt.race_position = pos;
    s.pkt.lap_number = lap0;
    s.pkt.current_lap = cur;
    s.pkt.best_lap = 52.804;
    s
}

fn drift_state(score: f32, chip: Option<(f32, f64)>, cycle: Option<f64>) -> HudSnapshot {
    let mut s = base(OverlayConfig::default());
    s.mode = HudMode::Drift;
    s.drift = DriftInfo {
        score,
        best: 61_200.0,
        window_start: cycle.map(|c| NOW - c * 5.0),
        interval: 5.0,
        chip: chip.map(|(gain, age)| DriftChip { gain, at: NOW - age }),
    };
    s
}

/// A procedural stand-in map (the real one takes 2 s to load): fields, a road grid, a
/// highway and a lake, so rotation and scale are visible.
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
fn tile(r: &mut Renderer, size: Vec2, s: f32, mut draw: impl FnMut(&Painter, &Xf)) -> ColorImage {
    let px = [((size.x + 20.0) * s).round() as u32, ((size.y + 20.0) * s).round() as u32];
    paint(&r.ctx, &mut r.painter, r.start, px, TILE_BG.to_normalized_gamma_f32(), |_, p| {
        draw(p, &Xf { o: pos2(10.0 * s, 10.0 * s), s, a: 1.0 });
    });
    r.painter.read_screen_rgba(px)
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

/// `c` at alpha `a` over opaque `bg`, gamma space (how egui_glow and CSS blend).
fn over(c: [u8; 3], a: f32, bg: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 * a + bg[i] as f32 * (1.0 - a)).round() as u8)
}

#[test]
#[ignore = "needs a GPU / EGL device; writes target/hud_png/*.png"]
fn render_spec_states() -> Result<(), String> {
    let headless = Headless::new().map_err(|e| format!("headless EGL unavailable: {e}"))?;
    let gl = headless.glow.clone();
    // SAFETY: plain GL object setup on the current (surfaceless) context.
    let fbo = unsafe {
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        let rb = gl.create_renderbuffer()?;
        gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rb));
        gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, 1920, 1080);
        gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(rb));
        if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            return Err("FBO incomplete".into());
        }
        (fbo, rb)
    };
    // Declared after `headless`, so it drops first (the painter needs the context).
    let mut r = Renderer::new(gl.clone())?;
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
        ("counting", drift_state(54_648.0, None, Some(0.45))),
        ("chip", drift_state(58_687.0, Some((4039.0, 1.2)), Some(0.2))),
        ("idle", drift_state(58_687.0, None, None)),
    ];

    let mut compass_off = base(OverlayConfig { compass: false, ..Default::default() });
    compass_off.pkt.speed = 20.0;
    let mut driving = base(OverlayConfig::default());
    driving.pkt.speed = 20.0;
    let maps = [("summer", driving.clone(), map_s), ("winter", driving, map_w), ("compass_off", compass_off, map_s)];

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
                        cluster::draw_pill(p, xf, snap, NOW);
                    } else {
                        cluster::draw_halo(p, xf, snap, NOW);
                    }
                });
                let id = format!("{style}_{name}_{sfx}");
                if s == 1.0 && style == "d1a" {
                    let cell = over([255; 3], 0.13, plate);
                    check(&mut failures, &img, &id, (178, 23), plate, "plate");
                    check(&mut failures, &img, &id, (54, 38), if *name == "shift" { [0x5B, 0x8B, 0xF0] } else { [245, 247, 251] }, "rev seg 0 lit");
                    check(&mut failures, &img, &id, (129, 38), if *name == "cruise" || *name == "pulse" || *name == "rpm_label" { over([255; 3], 0.15, plate) } else if *name == "shift" { [0x5B, 0x8B, 0xF0] } else { [245, 247, 251] }, "rev seg 10");
                    let gear = match *name {
                        "shift" => [0x3C, 0x6B, 0xDE],
                        "pulse" => [245, 247, 251],
                        _ => cell,
                    };
                    check(&mut failures, &img, &id, (23, 7), gear, "gear cell");
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
                if *name == "counting" {
                    check(&mut failures, &img, &id, (70, 39), [0xFF, 0xB0, 0x2E], "bar fill");
                    check(&mut failures, &img, &id, (180, 39), over([255; 3], 0.16, plate), "bar track");
                }
            }
            written.push(save(&img, &id)?);
        }
        for (name, snap, map) in &maps {
            let img = tile(&mut r, minimap::SIZE, s, |p, xf| {
                minimap::draw(p, xf, snap, NOW, &mut MapAnim::default(), Some(*map), &[]);
            });
            let id = format!("m2_{name}_{sfx}");
            if s == 1.0 {
                check(&mut failures, &img, &id, (104, 70), [255, 255, 255], "car marker");
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
    }

    // Full-screen composite of the default layout (map bottom-left, D1a bottom-centre, R1′
    // top-left), then with D3a′ and in drift mode.
    let mut composite = race_state(3, 2, 41.273);
    composite.cfg = Arc::new(OverlayConfig { fade: false, ..Default::default() });
    composite.pkt.current_engine_rpm = 0.52 * 8000.0;
    composite.pkt.gear = 4;
    composite.pkt.speed = 142.0 / 3.6;
    composite.lap_delta = Some(-0.42);
    let halo = HudSnapshot { cfg: Arc::new(OverlayConfig { fade: false, cluster_style: ClusterStyle::Halo, ..Default::default() }), ..composite.clone() };
    let drifting = HudSnapshot { mode: HudMode::Drift, drift: drift_state(58_687.0, Some((4039.0, 1.2)), Some(0.2)).drift, ..composite.clone() };
    r.map.set(summer.clone(), orig, Season::Summer);
    for (name, snap) in [("composite_1080p", composite), ("composite_halo_1080p", halo), ("composite_drift_1080p", drifting)] {
        r.hud = crate::hud::Hud::default();
        r.frame_at([1920, 1080], Some(&snap), false, NOW, SCREEN_BG.to_normalized_gamma_f32());
        let img = r.painter.read_screen_rgba([1920, 1080]);
        if img.size != [1920, 1080] {
            failures.push(format!("{name}: size {:?}", img.size));
        }
        if name == "composite_1080p" {
            let sbg = [128, 138, 150];
            let splate = over([9, 13, 21], 0.68, sbg);
            // D1a at (868, 990), R1′ at (44, 44), M2′ at (44, 900) per D22 (margin 44).
            for (what, (x, y)) in [("D1a plate", (868 + 178, 990 + 23)), ("R1 plate", (44 + 192, 44 + 23))] {
                let got = px(&img, x, y);
                let ok = got.iter().zip(splate).all(|(a, b)| a.abs_diff(b) <= 4);
                println!("  {name} {what} @({x},{y}): got {got:?}, want {splate:?}");
                if !ok {
                    failures.push(format!("{name} {what}: got {got:?}, want {splate:?}"));
                }
            }
            let marker = px(&img, 44 + 104, 900 + 70);
            if marker != [255, 255, 255] {
                failures.push(format!("{name}: map car marker at (148, 970) is {marker:?}"));
            }
            // Outside every widget: untouched backdrop.
            if px(&img, 960, 540) != sbg {
                failures.push(format!("{name}: screen centre not backdrop: {:?}", px(&img, 960, 540)));
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
