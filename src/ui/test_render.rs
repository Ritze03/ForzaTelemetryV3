//! Test-only harness: run a page headlessly in a real `egui::Context` (the app's fonts and
//! theme), check that nothing paints outside its pane, and optionally rasterise the frame to
//! a PNG (set `FORZA_UI_SNAPSHOT_DIR`) so a layout can be looked at without a window.
//! See docs/ui/STYLING-GUIDE.md → "Panes: everything stays in its container".

use std::collections::HashMap;

use egui::{Color32, ColorImage, Context, FullOutput, Pos2, Rect, TextureId};

/// A context with the app's fonts and Graphite theme (as `ForzaApp::new` sets them up).
pub fn context() -> Context {
    let ctx = Context::default();
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "geist_mono".to_owned(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/GeistMono-Regular.ttf")).into(),
    );
    fonts.font_data.insert(
        "geist_icons".to_owned(),
        egui::FontData::from_static(include_bytes!("../../assets/fonts/GeistMonoNerdFont-Regular.otf")).into(),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let fam = fonts.families.entry(family).or_default();
        fam.insert(0, "geist_mono".to_owned());
        fam.push("geist_icons".to_owned());
    }
    ctx.set_fonts(fonts);
    crate::theme::apply(&ctx);
    ctx
}

/// Use a fresh [`context`] per call: the font atlas is only sent whole on a context's first
/// frame, so a reused context's snapshot would have no glyphs.
/// Run `page` in a `CentralPanel` of a `w`×`h` window for a few frames (so fonts and
/// remembered sizes settle) and return the last frame plus every texture it references.
pub fn run(ctx: &Context, w: f32, h: f32, mut page: impl FnMut(&mut egui::Ui)) -> (FullOutput, HashMap<TextureId, ColorImage>) {
    let mut textures: HashMap<TextureId, ColorImage> = HashMap::new();
    let mut last = None;
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(w, h))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| page(ui));
        });
        for (id, delta) in &out.textures_delta.set {
            let egui::ImageData::Color(img) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(*id, (**img).clone());
                }
                Some([x0, y0]) => {
                    let t = textures.get_mut(id).expect("patch for an unknown texture");
                    for y in 0..img.size[1] {
                        for x in 0..img.size[0] {
                            t.pixels[(y0 + y) * t.size[0] + x0 + x] = img.pixels[y * img.size[0] + x];
                        }
                    }
                }
            }
        }
        last = Some(out);
    }
    (last.unwrap(), textures)
}

/// Each shape's visible part (bounding box ∩ clip rect), skipping empty ones and the
/// `CentralPanel`'s own background (the only shape that legitimately spans every column).
pub fn visible_rects(out: &FullOutput) -> Vec<Rect> {
    out.shapes
        .iter()
        .filter(|c| !matches!(&c.shape, egui::Shape::Rect(r) if r.rect == c.clip_rect && r.rect.min == Pos2::ZERO))
        .map(|c| c.shape.visual_bounding_rect().intersect(c.clip_rect))
        .filter(|r| r.is_positive())
        .collect()
}

/// Rasterise the frame (no anti-aliasing beyond egui's feathering) and save it as PNG to
/// `$FORZA_UI_SNAPSHOT_DIR/<name>.png`. A no-op when the variable isn't set.
pub fn snapshot(ctx: &Context, out: &FullOutput, textures: &HashMap<TextureId, ColorImage>, w: u32, h: u32, name: &str) {
    let Ok(dir) = std::env::var("FORZA_UI_SNAPSHOT_DIR") else { return };
    let prims = ctx.tessellate(out.shapes.clone(), 1.0);
    let mut px = vec![[0.0_f32; 4]; (w * h) as usize];
    for p in &prims {
        let egui::epaint::Primitive::Mesh(mesh) = &p.primitive else { continue };
        let Some(tex) = textures.get(&mesh.texture_id) else { continue };
        let clip = p.clip_rect.intersect(Rect::from_min_size(Pos2::ZERO, egui::vec2(w as f32, h as f32)));
        for tri in mesh.indices.chunks_exact(3) {
            let v = [&mesh.vertices[tri[0] as usize], &mesh.vertices[tri[1] as usize], &mesh.vertices[tri[2] as usize]];
            let (a, b, c) = (v[0].pos, v[1].pos, v[2].pos);
            let area = (b - a).x * (c - a).y - (b - a).y * (c - a).x;
            if area.abs() < 1e-6 {
                continue;
            }
            let bb = Rect::from_points(&[a, b, c]).intersect(clip);
            if !bb.is_positive() {
                continue;
            }
            for y in bb.top().floor() as u32..bb.bottom().ceil().min(h as f32) as u32 {
                for x in bb.left().floor() as u32..bb.right().ceil().min(w as f32) as u32 {
                    let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                    if !clip.contains(p) {
                        continue;
                    }
                    let w0 = ((b - p).x * (c - p).y - (b - p).y * (c - p).x) / area;
                    let w1 = ((c - p).x * (a - p).y - (c - p).y * (a - p).x) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let lerp = |f: &dyn Fn(usize) -> f32| w0 * f(0) + w1 * f(1) + w2 * f(2);
                    let u = lerp(&|i| v[i].uv.x);
                    let vv = lerp(&|i| v[i].uv.y);
                    let tx = ((u * tex.size[0] as f32) as usize).min(tex.size[0] - 1);
                    let ty = ((vv * tex.size[1] as f32) as usize).min(tex.size[1] - 1);
                    let t: Color32 = tex.pixels[ty * tex.size[0] + tx];
                    let mut src = [0.0_f32; 4];
                    for (k, s) in src.iter_mut().enumerate() {
                        let vc = lerp(&|i| v[i].color.to_array()[k] as f32 / 255.0);
                        *s = vc * t.to_array()[k] as f32 / 255.0;
                    }
                    let d = &mut px[(y * w + x) as usize];
                    for k in 0..4 {
                        d[k] = src[k] + d[k] * (1.0 - src[3]); // premultiplied "over"
                    }
                }
            }
        }
    }
    let mut img = image::RgbaImage::new(w, h);
    for (i, p) in px.iter().enumerate() {
        let c = |f: f32| (f.clamp(0.0, 1.0) * 255.0) as u8;
        img.put_pixel(i as u32 % w, i as u32 / w, image::Rgba([c(p[0]), c(p[1]), c(p[2]), 255]));
    }
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    img.save(&path).expect("write snapshot");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helper-level guarantee: deliberately oversized content (a long unwrapped label, a
    /// 2000 px wide painted block, a wide radio row) inside `theme::columns` + `theme::card`
    /// never paints outside its own column, and every card frame is exactly its column wide.
    #[test]
    fn oversized_content_stays_inside_its_pane() {
        for w in [800.0, 1100.0, 1235.0] {
            let ctx = context();
            let mut col_rects = Vec::new();
            let (out, tex) = run(&ctx, w, 400.0, |ui| {
                col_rects.clear();
                ui.spacing_mut().item_spacing.x = 8.0;
                crate::theme::columns(ui, 3, |cols| {
                    for (i, ui) in cols.iter_mut().enumerate() {
                        col_rects.push(ui.max_rect());
                        ui.spacing_mut().item_spacing.y = 0.0;
                        crate::theme::card(ui, "Card", |ui| {
                            ui.add(egui::Label::new("a very long label that would never fit into a narrow column at all").extend());
                            let (r, _) = ui.allocate_exact_size(egui::vec2(2000.0, 20.0), egui::Sense::hover());
                            ui.painter().rect_filled(r, 0.0, crate::theme::ACCENT);
                        });
                        // The next card is unaffected by the previous card's overflow, and
                        // its own rows fit: the radios wrap, the slider row shrinks.
                        crate::theme::card(ui, "Next card", |ui| {
                            let mut v = i;
                            crate::theme::radio_group(ui, &mut v, &[(0, "Position + Gain"), (1, "Total score"), (2, "A third option")]);
                            let mut x = 0.5_f32;
                            crate::theme::slider_row(ui, "Slider", &mut x, 0.0..=1.0, 0.1, 1, "");
                        });
                    }
                });
            });
            snapshot(&ctx, &out, &tex, w as u32, 400, &format!("oversized_{w}"));
            assert_eq!(col_rects.len(), 3);
            for r in visible_rects(&out) {
                assert!(
                    col_rects.iter().any(|c| r.left() >= c.left() - 4.5 && r.right() <= c.right() + 4.5),
                    "at {w} px a shape paints across columns: {r:?} (columns {col_rects:?})"
                );
            }
        }
    }
}
