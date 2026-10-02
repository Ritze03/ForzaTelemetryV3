//! One overlay frame: the overlay's own `egui::Context` runs with no input, gets tessellated
//! and painted by `egui_glow` into whatever EGL surface (or FBO) is current. The HUD itself
//! is `crate::hud`; this file owns the GPU side, the fonts and the minimap texture.

use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Instant;

use egui::{Color32, CornerRadius, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use egui_glow::glow;

use super::snapshot::{hud_clock, HudSnapshot};
use crate::coop::CoopReader;
use crate::hud::minimap::{CoopInput, CoopLayer, MapLoader};
use crate::hud::{fonts, Hud};

pub struct Renderer {
    ctx: egui::Context,
    painter: egui_glow::Painter,
    start: Instant,
    hud: Hud,
    map: MapLoader,
    /// Co-op session handle for M2′, read per frame (never through the UI thread).
    coop: Option<CoopReader>,
    /// What M2′ draws from the session: teammates, trails, waypoints.
    layer: CoopLayer,
}

impl Renderer {
    /// Needs the GL context current (surfaceless is fine): compiles the shaders.
    pub fn new(gl: Arc<glow::Context>, coop: Option<CoopReader>) -> Result<Self, String> {
        // Dithering on: removes banding in long, faint vertex-colour fades on an 8-bit buffer.
        let painter = egui_glow::Painter::new(gl, "", None, true).map_err(|e| format!("egui_glow: {e}"))?;
        let ctx = egui::Context::default();
        fonts::install(&ctx);
        Ok(Self { ctx, painter, start: Instant::now(), hud: Hud::default(), map: MapLoader::default(), coop, layer: CoopLayer::default() })
    }

    /// Draw one frame of `size` physical px. Returns true while an animation still needs
    /// frames without a new packet — including the show/hide fade: once `snapshot.visible`
    /// is false and this returns false, the fade-out is done and the surface can go.
    pub fn frame(&mut self, size: [u32; 2], snapshot: Option<&HudSnapshot>, test_pattern: bool) -> bool {
        self.frame_at(size, snapshot, test_pattern, hud_clock(), [0.0; 4])
    }

    /// [`Self::frame`] at a pinned `now` ([`hud_clock`] seconds) over a premultiplied
    /// `clear` colour (the PNG harness renders over an opaque backdrop).
    fn frame_at(&mut self, size: [u32; 2], snapshot: Option<&HudSnapshot>, test_pattern: bool, now: f64, clear: [f32; 4]) -> bool {
        let (hud, map, coop, layer) = (&mut self.hud, &mut self.map, &self.coop, &mut self.layer);
        paint(&self.ctx, &mut self.painter, self.start, size, clear, |ctx, p| {
            if test_pattern {
                draw_test_pattern(p, ctx.content_rect(), ctx.cumulative_pass_nr());
            }
            let Some(snap) = snapshot else { return false };
            let tex = map.poll(ctx, now, snap.cfg.minimap_on);
            // Co-op layer: teammates, trails, waypoints (empty without a session or with the
            // minimap off). The overlay thread records the trails itself, see `CoopLayer`.
            layer.update(coop.as_ref().map(coop_input), snap, Instant::now());
            hud.draw(p, ctx.content_rect(), snap, now, tex, layer)
        })
    }
}

/// The session state M2′ draws from (one lock per read; `remote_players` also advances the
/// jitter buffers, as the UI's `tick` stops while the game covers the window).
fn coop_input(coop: &CoopReader) -> CoopInput {
    let in_session = coop.in_session();
    CoopInput {
        in_session,
        remotes: if in_session { coop.remote_players() } else { Vec::new() },
        waypoints: if in_session { coop.waypoints().into_iter().map(|(_, x, z, hue)| (x, z, hue)).collect() } else { Vec::new() },
    }
}

/// Run one egui frame over `size` px with `draw` on a background-layer painter, then
/// tessellate and paint it into the current framebuffer. `draw` runs once per egui pass (a
/// discarded pass would re-run it at the same time, so the HUD's dt-based state doesn't move).
fn paint<R: Default>(
    ctx: &egui::Context,
    painter: &mut egui_glow::Painter,
    start: Instant,
    [w, h]: [u32; 2],
    clear: [f32; 4],
    mut draw: impl FnMut(&egui::Context, &Painter) -> R,
) -> R {
    let mut raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32))),
        // Must be set or time-based animation never advances.
        time: Some(start.elapsed().as_secs_f64()),
        // Caps a font-atlas rebuild's re-upload well below a 16k-wide texture.
        max_texture_side: Some(painter.max_texture_side().min(8192)),
        ..Default::default()
    };
    // ponytail: scale 1.0 (all target monitors are 1080p @1). Upgrade: feed the output's
    // integer scale here and set_buffer_scale on the wl_surface.
    raw.viewports.entry(raw.viewport_id).or_default().native_pixels_per_point = Some(1.0);

    let mut result = R::default();
    let out = ctx.run(raw, |ctx| {
        let p = ctx.layer_painter(LayerId::new(Order::Background, Id::new("hud")));
        result = draw(ctx, &p);
    });
    let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
    // Premultiplied clear (transparent for the overlay); egui_glow blends premultiplied, as
    // Wayland expects.
    painter.clear([w, h], clear);
    painter.paint_and_update_textures([w, h], out.pixels_per_point, &prims, &out.textures_delta);
    result
}

impl Drop for Renderer {
    /// Frees GL objects; needs the context current (surfaceless is fine), which holds because
    /// every owner drops `Renderer` before `Gl`: the `(Renderer, Gl)` tuple in `init`, and
    /// `Overlay`'s field order after its `Drop` has released the window surface.
    fn drop(&mut self) {
        self.painter.destroy(); // idempotent
    }
}

/// Dev pattern (`FORZA_OVERLAY_TEST=1`/`2`): a pill plate, outlined text and a ring, bottom
/// centre, to check the overlay sits above fullscreen FH6 and passes clicks/keys through.
/// The text carries the frame counter (mod 1000, so it fits the pill), so the 60 Hz mode (`2`) visibly redraws.
fn draw_test_pattern(p: &Painter, screen: Rect, frame: u64) {
    // rgba(9,13,21,.68) from the mockup; `from_rgba_unmultiplied` because Color32 is premultiplied.
    let plate = Color32::from_rgba_unmultiplied(9, 13, 21, 173);
    let rim = Color32::from_rgba_unmultiplied(255, 255, 255, 30);

    // Pill plate.
    let pill = Rect::from_center_size(Pos2::new(screen.center().x, screen.bottom() - 70.0), Vec2::new(300.0, 56.0));
    p.rect(pill, CornerRadius::same(28), plate, Stroke::new(1.5, rim), StrokeKind::Inside);

    // Outlined text: one layout, 8 dark offset copies (round outline), then the white fill.
    let galley = p.layout_no_wrap(format!("OVERLAY TEST {:03}", frame % 1000), FontId::proportional(30.0), Color32::WHITE);
    let pos = pill.center() - galley.size() / 2.0;
    let outline = Color32::from_rgba_unmultiplied(0, 0, 0, 153);
    for i in 0..8 {
        let a = i as f32 * PI / 4.0;
        p.galley_with_override_text_color(pos + 2.0 * Vec2::angled(a), galley.clone(), outline);
    }
    p.galley(pos, galley, Color32::WHITE);

    // Ring: disc + rim, then a 260° arc of 24 annular sectors (feathered convex quads), the
    // last quarter in redline red.
    let c = Pos2::new(pill.center().x, pill.top() - 90.0);
    p.circle(c, 56.0, plate, Stroke::new(1.5, rim));
    let (start, sweep, n) = (PI * 0.5 + 50f32.to_radians(), 260f32.to_radians(), 24);
    let step = sweep / n as f32;
    for i in 0..n {
        let (a0, a1) = (start + i as f32 * step + 0.02, start + (i + 1) as f32 * step - 0.02);
        let pt = |r: f32, a: f32| c + r * Vec2::angled(a);
        let col = if i >= n * 3 / 4 { Color32::from_rgb(0xE1, 0x55, 0x54) } else { Color32::WHITE };
        p.add(Shape::convex_polygon(vec![pt(40.0, a0), pt(52.0, a0), pt(52.0, a1), pt(40.0, a1)], col, Stroke::NONE));
    }
}

/// The offscreen PNG harness (`cargo test render_spec_states -- --ignored`). The file lives
/// with the HUD code but is compiled here, as a child of this module, so it can reach the
/// private `Renderer` internals and `gl::Headless` (a binary crate has no `examples/` access).
#[cfg(all(test, target_os = "linux"))]
#[path = "../hud/png.rs"]
mod png;
