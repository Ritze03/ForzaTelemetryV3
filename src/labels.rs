//! Car class + drivetrain label images (assets/labels) and a small shared renderer
//! that stamps the car's rating (PI) into the class label's number box. Used by the
//! Car dashboard widget and the co-op on-map player list.

use egui::{
    Align2, Color32, ColorImage, Context, FontId, Painter, Pos2, Rect, TextureHandle,
    TextureOptions, Vec2,
};

/// Rating-number box inside the 111×40 class label, in native pixels.
const RATING_MIN: Vec2 = Vec2::new(46.3, 3.0);
const RATING_MAX: Vec2 = Vec2::new(108.0, 38.0);

pub struct Labels {
    class: [TextureHandle; 8], // by CarClass 0..7 (6 = R falls back to "none")
    class_none: TextureHandle,
    drivetrain: [TextureHandle; 3], // 0=FWD 1=RWD 2=AWD
    drivetrain_none: TextureHandle,
}

fn decode(ctx: &Context, name: &str, bytes: &[u8]) -> TextureHandle {
    let color = image::load_from_memory(bytes)
        .map(|img| {
            let rgba = img.into_rgba8();
            let (w, h) = rgba.dimensions();
            ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw())
        })
        .unwrap_or_else(|_| ColorImage::new([1, 1], vec![Color32::TRANSPARENT]));
    ctx.load_texture(name, color, TextureOptions::LINEAR)
}

macro_rules! lbl {
    ($ctx:expr, $n:literal) => {
        decode($ctx, $n, include_bytes!(concat!("../assets/labels/", $n, ".png")))
    };
}

impl Labels {
    pub fn load(ctx: &Context) -> Self {
        Self {
            class: [
                lbl!(ctx, "class_d"),
                lbl!(ctx, "class_c"),
                lbl!(ctx, "class_b"),
                lbl!(ctx, "class_a"),
                lbl!(ctx, "class_s1"),
                lbl!(ctx, "class_s2"),
                lbl!(ctx, "class_r"),
                lbl!(ctx, "class_x"),
            ],
            class_none: lbl!(ctx, "class_none"),
            drivetrain: [
                lbl!(ctx, "drivetrain_fwd"),
                lbl!(ctx, "drivetrain_rwd"),
                lbl!(ctx, "drivetrain_awd"),
            ],
            drivetrain_none: lbl!(ctx, "drivetrain_none"),
        }
    }

    fn class_tex(&self, class: i32) -> &TextureHandle {
        if (0..8).contains(&class) {
            &self.class[class as usize]
        } else {
            &self.class_none
        }
    }
    fn dt_tex(&self, dt: i32) -> &TextureHandle {
        if (0..3).contains(&dt) {
            &self.drivetrain[dt as usize]
        } else {
            &self.drivetrain_none
        }
    }

    /// Size (px) of a class / drivetrain label at `scale` (1.0 = native).
    pub fn class_size(&self, class: i32, scale: f32) -> Vec2 {
        self.class_tex(class).size_vec2() * scale
    }
    pub fn drivetrain_size(&self, dt: i32, scale: f32) -> Vec2 {
        self.dt_tex(dt).size_vec2() * scale
    }

    /// Draw the class label at `top_left`/`scale` with `rating` (PI) centred in the
    /// label's number box. `rating <= 0` draws just the label. Returns drawn size.
    /// The rect is snapped to the physical pixel grid so the label's thin borders
    /// don't fall between pixels and vanish when drawn small (e.g. list rows).
    pub fn paint_class(&self, painter: &Painter, class: i32, rating: i32, top_left: Pos2, scale: f32) -> Vec2 {
        let tex = self.class_tex(class);
        let (top_left, size) = snap_rect(painter, top_left, tex.size_vec2() * scale);
        painter.image(
            tex.id(),
            Rect::from_min_size(top_left, size),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        if rating > 0 {
            let bmin = top_left + RATING_MIN * scale;
            let bmax = top_left + RATING_MAX * scale;
            let center = Pos2::new((bmin.x + bmax.x) * 0.5, (bmin.y + bmax.y) * 0.5);
            let font_h = (bmax.y - bmin.y) * 0.74;
            painter.text(
                center,
                Align2::CENTER_CENTER,
                rating.to_string(),
                FontId::proportional(font_h),
                Color32::WHITE,
            );
        }
        size
    }

    /// Draw the drivetrain label at `top_left`/`scale`. Returns drawn size.
    pub fn paint_drivetrain(&self, painter: &Painter, dt: i32, top_left: Pos2, scale: f32) -> Vec2 {
        let tex = self.dt_tex(dt);
        let (top_left, size) = snap_rect(painter, top_left, tex.size_vec2() * scale);
        painter.image(
            tex.id(),
            Rect::from_min_size(top_left, size),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        size
    }
}

/// Snap a rect (min + size) to the physical pixel grid.
fn snap_rect(painter: &Painter, min: Pos2, size: Vec2) -> (Pos2, Vec2) {
    let ppp = painter.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    (
        Pos2::new(snap(min.x), snap(min.y)),
        Vec2::new(snap(size.x).max(1.0 / ppp), snap(size.y).max(1.0 / ppp)),
    )
}
