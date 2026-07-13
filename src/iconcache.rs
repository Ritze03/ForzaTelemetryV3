//! Reusable visual centering for icon glyphs.
//!
//! Font glyphs carry their own bearings and line metrics, so anchoring an icon
//! with `Align2::CENTER_CENTER` centers its *layout* box, not its visible ink —
//! most icons end up a pixel or two off, and by different amounts, so a row of
//! them looks ragged. This lays each glyph out once, reads the tight bounding
//! box of the actually-rendered mesh (`Galley::mesh_bounds`), and caches the
//! resulting ink centre. After that, any icon can be drawn dead-centre in a
//! fixed cell for free.
//!
//! Reuse anywhere you paint an icon in a box: call [`IconCenterCache::centered_pos`]
//! to get the draw position, then `painter.text(pos, Align2::LEFT_TOP, ..)`.

use std::collections::HashMap;

use egui::{Color32, FontId, Pos2, Ui, Vec2};

/// Per-(glyph, size) cache of the rendered ink centre, in galley-local points.
#[derive(Default)]
pub struct IconCenterCache {
    // Key: (first char of the icon string, font size bits). Icon fonts render one
    // glyph per code point, so the leading char identifies the glyph.
    ink_center: HashMap<(char, u32), Vec2>,
}

impl IconCenterCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the top-left position at which to paint `icon` (as a single-glyph
    /// galley, anchored `Align2::LEFT_TOP`) so its visible ink is centred on
    /// `center`. The measurement is cached per glyph + size.
    pub fn centered_pos(&mut self, ui: &Ui, icon: &str, font: FontId, center: Pos2) -> Pos2 {
        let key = (icon.chars().next().unwrap_or(' '), font.size.to_bits());
        let ink_center = *self.ink_center.entry(key).or_insert_with(|| {
            let galley = ui
                .painter()
                .layout_no_wrap(icon.to_owned(), font.clone(), Color32::WHITE);
            // mesh_bounds is the tight box around the rendered triangles — i.e. the
            // glyph's actual ink, not the font's line box. Empty glyphs fall back
            // to the layout centre.
            if galley.mesh_bounds.is_positive() {
                galley.mesh_bounds.center().to_vec2()
            } else {
                galley.rect.center().to_vec2()
            }
        });
        center - ink_center
    }
}
