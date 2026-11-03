//! The POI icon atlas of one egui context: uploads the store's CPU pixels (`MapLayers::icons`)
//! as a texture and builds the [`IconAtlas`] the renderer draws with.
//!
//! *Why one of these per context:* the Dashboard (eframe's GL context, UI thread) and the HUD
//! (the overlay's own EGL context and `egui::Context`, overlay thread) cannot share a GL object.
//! They share the decoded pixels instead (read once by the `map-layers` loader thread) and each
//! upload their own copy (512 x 384 RGBA, 0.75 MB). The texture handle lives here, so dropping
//! the owner (the overlay `Renderer` before its GL context, the app) frees it.

use std::sync::Arc;

use egui::{ColorImage, TextureFilter, TextureHandle, TextureOptions, TextureWrapMode};

use super::paint2d::IconAtlas;
use crate::gamedata::icons::PoiIcons;

/// Linear + mipmaps: the 64 px cells are drawn at 16-48 px.
const OPTIONS: TextureOptions = TextureOptions {
    magnification: TextureFilter::Linear,
    minification: TextureFilter::Linear,
    wrap_mode: TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(TextureFilter::Linear),
};

/// One context's uploaded icons, replaced when the store hands out different pixels.
#[derive(Default)]
pub struct IconTex {
    /// The pixels the texture was made from (compared by pointer).
    src: Option<Arc<PoiIcons>>,
    /// Keeps the texture alive.
    handle: Option<TextureHandle>,
    atlas: Option<Arc<IconAtlas>>,
}

impl IconTex {
    /// The atlas for `icons` (the store's, `None` = none available), uploading on the first call
    /// and again only when the store's `Arc` changes (a new install). Cheap otherwise.
    pub fn ensure(&mut self, ctx: &egui::Context, icons: Option<&Arc<PoiIcons>>) -> Option<Arc<IconAtlas>> {
        match icons {
            None => {
                *self = Self::default();
                None
            }
            Some(i) => {
                if !self.src.as_ref().is_some_and(|s| Arc::ptr_eq(s, i)) {
                    self.upload(ctx, i);
                }
                self.atlas.clone()
            }
        }
    }

    fn upload(&mut self, ctx: &egui::Context, icons: &Arc<PoiIcons>) {
        self.src = Some(icons.clone());
        let (w, h) = (icons.atlas_w as usize, icons.atlas_h as usize);
        if w == 0 || h == 0 || icons.atlas_rgba.len() != w * h * 4 {
            self.handle = None;
            self.atlas = None;
            return;
        }
        let img = ColorImage::from_rgba_unmultiplied([w, h], &icons.atlas_rgba);
        let handle = ctx.load_texture("poi-icons", img, OPTIONS);
        self.atlas = Some(Arc::new(IconAtlas::from_poi_icons(handle.id(), icons)));
        self.handle = Some(handle);
    }
}

/// A procedural stand-in for the game's icons (a coloured disc with a white ring per
/// [`ICON_TABLE`] row, in the category's colour): the PNG harness and the tests have no install.
#[cfg(test)]
pub fn synthetic_icons() -> PoiIcons {
    use crate::gamedata::icons::{IconKey, ICON_TABLE};
    let (size, cols) = (64u32, 8u32);
    let rows = (ICON_TABLE.len() as u32).div_ceil(cols);
    let (w, h) = (cols * size, rows * size);
    let mut px = vec![0u8; (w * h * 4) as usize];
    let mut out = PoiIcons { size, atlas_w: w, atlas_h: h, ..Default::default() };
    for (n, row) in ICON_TABLE.iter().enumerate() {
        let (cx, cy) = ((n as u32 % cols) * size, (n as u32 / cols) * size);
        let rgb = match row.key {
            IconKey::Kind(k) => super::style::POI_CATS.iter().find(|c| c.kind == Some(k)).map_or([160, 160, 160], |c| c.color.0),
            IconKey::Race(_) => [255, 159, 67],
            IconKey::Mascot(_) => [240, 171, 252],
        };
        for y in 0..size {
            for x in 0..size {
                let r = ((x as f32 + 0.5 - size as f32 / 2.0).powi(2) + (y as f32 + 0.5 - size as f32 / 2.0).powi(2)).sqrt();
                let a = (30.0 - r).clamp(0.0, 1.0);
                if a <= 0.0 {
                    continue;
                }
                let c = if r > 24.0 { [255, 255, 255] } else { rgb };
                let o = (((cy + y) * w + cx + x) * 4) as usize;
                px[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], (a * 255.0) as u8]);
            }
        }
        let (fw, fh) = (w as f32, h as f32);
        let uv = [cx as f32 / fw, cy as f32 / fh, (cx + size) as f32 / fw, (cy + size) as f32 / fh];
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
    out.atlas_rgba = px;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_happens_once_per_pixel_set_and_not_without_icons() {
        let ctx = egui::Context::default();
        let mut t = IconTex::default();
        assert!(t.ensure(&ctx, None).is_none());
        let icons = Arc::new(synthetic_icons());
        let a = t.ensure(&ctx, Some(&icons)).expect("atlas");
        let b = t.ensure(&ctx, Some(&icons)).expect("atlas");
        assert!(Arc::ptr_eq(&a, &b), "same pixels: no second upload");
        // New pixels (a reload after an install change): a fresh upload with its own texture.
        let other = Arc::new(synthetic_icons());
        let c = t.ensure(&ctx, Some(&other)).expect("atlas");
        assert!(!Arc::ptr_eq(&a, &c) && a.texture != c.texture);
        // Icons gone: the texture is dropped.
        assert!(t.ensure(&ctx, None).is_none() && t.handle.is_none());
        // A malformed atlas (size mismatch) yields none instead of panicking.
        let bad = Arc::new(PoiIcons { atlas_w: 4, atlas_h: 4, atlas_rgba: vec![0; 3], ..Default::default() });
        assert!(t.ensure(&ctx, Some(&bad)).is_none());
    }
}
