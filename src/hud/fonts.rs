//! The HUD's fonts: Big Shoulders Display 800 and 900, static instances baked from the
//! variable font (epaint can't pick a weight, and the variable default is Thin). The
//! mockup's round-4 widgets use only Big Shoulders; the one other family in the spec, Barlow
//! 600 for co-op teammate names, maps to [`W800`] here (only Big Shoulders is baked).

use std::sync::Arc;

use egui::{FontData, FontDefinitions, FontFamily};

/// Named families for [`super::prims::TextStyle::family`].
pub const W800: &str = "hud-800";
pub const W900: &str = "hud-900";

/// Register both weights in the overlay's own context (takes effect on the next pass).
/// The default egui fonts stay as fallbacks for any glyph Big Shoulders lacks.
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let fallback = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (family, key, bytes) in [
        (W800, "big-shoulders-800", &include_bytes!("../../assets/fonts/BigShouldersDisplay-ExtraBold.ttf")[..]),
        (W900, "big-shoulders-900", &include_bytes!("../../assets/fonts/BigShouldersDisplay-Black.ttf")[..]),
    ] {
        defs.font_data.insert(key.into(), Arc::new(FontData::from_static(bytes)));
        let mut list = vec![key.to_string()];
        list.extend(fallback.iter().cloned());
        defs.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(defs);
}
