//! One overlay frame: the overlay's own `egui::Context` runs with no input, gets tessellated
//! and painted by `egui_glow` into whatever EGL surface is current. `draw` is the single
//! hook the HUD (I6) replaces.

use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Instant;

use egui::{Color32, CornerRadius, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2};
use egui_glow::glow;

use super::snapshot::HudSnapshot;

pub struct Renderer {
    ctx: egui::Context,
    painter: egui_glow::Painter,
    start: Instant,
}

impl Renderer {
    /// Needs the GL context current (surfaceless is fine): compiles the shaders.
    pub fn new(gl: Arc<glow::Context>) -> Result<Self, String> {
        // Dithering on: removes banding in long, faint vertex-colour fades on an 8-bit buffer.
        let painter = egui_glow::Painter::new(gl, "", None, true).map_err(|e| format!("egui_glow: {e}"))?;
        Ok(Self { ctx: egui::Context::default(), painter, start: Instant::now() })
    }

    /// Draw one frame of `size` physical px. Returns true while an animation still needs
    /// frames without a new packet.
    pub fn frame(&mut self, [w, h]: [u32; 2], snapshot: Option<&HudSnapshot>, test_pattern: bool) -> bool {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32))),
            // Must be set or time-based animation never advances.
            time: Some(self.start.elapsed().as_secs_f64()),
            // Caps a font-atlas rebuild's re-upload well below a 16k-wide texture.
            max_texture_side: Some(self.painter.max_texture_side().min(8192)),
            ..Default::default()
        };
        // ponytail: scale 1.0 (all target monitors are 1080p @1). Upgrade: feed the output's
        // integer scale here and set_buffer_scale on the wl_surface.
        raw.viewports.entry(raw.viewport_id).or_default().native_pixels_per_point = Some(1.0);

        let mut animating = false;
        let out = self.ctx.run(raw, |ctx| animating = draw(ctx, snapshot, test_pattern));
        let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        // Premultiplied transparent clear; egui_glow blends premultiplied, as Wayland expects.
        self.painter.clear([w, h], [0.0; 4]);
        self.painter.paint_and_update_textures([w, h], out.pixels_per_point, &prims, &out.textures_delta);
        animating
    }

}

impl Drop for Renderer {
    /// Frees GL objects; needs the context current (surfaceless is fine), which holds because
    /// every owner drops `Renderer` before `Gl`: the `(Renderer, Gl)` tuple in `init`, and
    /// `Overlay`'s field order after its `Drop` has released the window surface.
    fn drop(&mut self) {
        self.painter.destroy(); // idempotent
    }
}

/// The draw hook: everything the overlay shows. Returns true while animating.
pub fn draw(ctx: &egui::Context, snapshot: Option<&HudSnapshot>, test_pattern: bool) -> bool {
    let painter = ctx.layer_painter(LayerId::new(Order::Background, Id::new("hud")));
    if test_pattern {
        draw_test_pattern(&painter, ctx.content_rect(), ctx.cumulative_pass_nr());
    }
    let _ = snapshot; // TODO(I6): the HUD widgets.
    false
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
