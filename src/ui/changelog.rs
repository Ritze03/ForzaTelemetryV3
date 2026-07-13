//! "What's New" changelog viewer.
//!
//! Parses the project's `CHANGELOG.md` (embedded at compile time) and renders it
//! with per-category filter toggles. The file is tiny, so it is parsed exactly
//! once into a `LazyLock<Vec<Section>>`.

use std::sync::LazyLock;

use crate::i18n::tr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Added,
    Fixed,
    Removed,
    Info,
}

impl Category {
    /// Parse a `### Heading` line into a category, if it names one.
    fn from_heading(s: &str) -> Option<Category> {
        match s.trim() {
            "Added" => Some(Category::Added),
            "Fixed" => Some(Category::Fixed),
            "Removed" => Some(Category::Removed),
            "Info" => Some(Category::Info),
            _ => None,
        }
    }

    /// Short filter-button label (translated).
    fn button_label(self) -> &'static str {
        match self {
            Category::Added => tr("New"),
            Category::Fixed => tr("Fixes"),
            Category::Removed => tr("Removed"),
            Category::Info => tr("Info"),
        }
    }

    /// Colour of the little category tag.
    fn color(self) -> egui::Color32 {
        match self {
            Category::Added => crate::theme::GOOD,
            Category::Fixed => crate::theme::ACCENT,
            Category::Removed => crate::theme::DANGER,
            Category::Info => crate::theme::TEXT_DIM,
        }
    }
}

pub struct Entry {
    pub category: Category,
    pub title: String,
    pub body: Option<String>,
}

pub struct Section {
    pub version: String,
    pub date: Option<String>,
    pub entries: Vec<Entry>,
}

/// Strip `**bold**` markers for display (we render emphasis via egui, not markdown).
fn strip_bold(s: &str) -> String {
    s.replace("**", "")
}

/// Parse the changelog markdown into newest-first sections.
pub fn parse(md: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut current_cat: Option<Category> = None;

    for line in md.lines() {
        let trimmed = line.trim_end();

        if let Some(rest) = trimmed.strip_prefix("## [") {
            // Version header: `## [x.y.z] – YYYY-MM-DD`
            current_cat = None;
            let (version, tail) = match rest.split_once(']') {
                Some((v, t)) => (v.to_string(), t),
                None => (rest.to_string(), ""),
            };
            // Trim leading en-dash / hyphen / whitespace off the date remainder.
            let date_str = tail
                .trim_start_matches(|c: char| c == '–' || c == '-' || c.is_whitespace())
                .trim();
            let date = if date_str.is_empty() {
                None
            } else {
                Some(date_str.to_string())
            };
            sections.push(Section {
                version,
                date,
                entries: Vec::new(),
            });
        } else if let Some(rest) = trimmed.strip_prefix("### ") {
            current_cat = Category::from_heading(rest);
        } else if let Some(rest) = trimmed.trim_start().strip_prefix("- ") {
            // Only collect bullets once we're inside a version + known category.
            let (Some(cat), Some(section)) = (current_cat, sections.last_mut()) else {
                continue;
            };
            let content = rest.trim();
            // Try `**Title**: body` — split on the first `: ` after the bold close.
            let (title, body) = if let Some(after) = content.strip_prefix("**") {
                if let Some(close) = after.find("**") {
                    let title = after[..close].trim().to_string();
                    let remainder = after[close + 2..].trim_start();
                    let body = remainder
                        .strip_prefix(':')
                        .map(|b| b.trim().to_string())
                        .filter(|b| !b.is_empty());
                    (title, body)
                } else {
                    (strip_bold(content), None)
                }
            } else {
                (strip_bold(content), None)
            };
            section.entries.push(Entry {
                category: cat,
                title,
                body: body.map(|b| strip_bold(&b)),
            });
        }
    }

    sections
}

static SECTIONS: LazyLock<Vec<Section>> =
    LazyLock::new(|| parse(include_str!("../../CHANGELOG.md")));

/// Render the "What's New" viewer.
pub fn show(ui: &mut egui::Ui, app: &mut crate::app::ForzaApp) {
    ui.add_space(6.0);

    // ── Filter bar ────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let filters: [(Category, &mut bool); 4] = [
            (Category::Added, &mut app.changelog_show_added),
            (Category::Fixed, &mut app.changelog_show_fixed),
            (Category::Removed, &mut app.changelog_show_removed),
            (Category::Info, &mut app.changelog_show_info),
        ];
        for (cat, enabled) in filters {
            let label = egui::RichText::new(cat.button_label())
                .color(if *enabled { cat.color() } else { crate::theme::TEXT_DIM });
            if ui.selectable_label(*enabled, label).clicked() {
                *enabled = !*enabled;
            }
        }
    });

    ui.add_space(4.0);
    ui.separator();

    let enabled = |cat: Category| match cat {
        Category::Added => app.changelog_show_added,
        Category::Fixed => app.changelog_show_fixed,
        Category::Removed => app.changelog_show_removed,
        Category::Info => app.changelog_show_info,
    };

    // Fixed width for the category-tag column so the entry text lines up regardless of
    // the tag's length (which varies by category and by language).
    let tag_font = egui::TextStyle::Small.resolve(ui.style());
    let tag_col_w = [Category::Added, Category::Fixed, Category::Removed, Category::Info]
        .iter()
        .map(|c| ui.painter()
            .layout_no_wrap(c.button_label().to_owned(), tag_font.clone(), egui::Color32::WHITE)
            .rect.width())
        .fold(0.0_f32, f32::max)
        + 6.0;

    // ── Section list ──────────────────────────────────────────────────
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for section in SECTIONS.iter() {
                let visible: Vec<&Entry> = section
                    .entries
                    .iter()
                    .filter(|e| enabled(e.category))
                    .collect();
                if visible.is_empty() {
                    continue;
                }

                ui.add_space(10.0);
                let heading = match &section.date {
                    Some(d) => format!("{}  ·  {}", section.version, d),
                    None => section.version.clone(),
                };
                ui.label(crate::theme::section_label(&heading));
                ui.add_space(4.0);

                for entry in visible {
                    ui.horizontal_top(|ui| {
                        // Category tag, padded to a fixed column width so titles line up.
                        let resp = ui.label(
                            egui::RichText::new(entry.category.button_label())
                                .color(entry.category.color())
                                .small()
                                .strong(),
                        );
                        ui.add_space((tag_col_w - resp.rect.width()).max(0.0));
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new(&entry.title).strong());
                            if let Some(body) = &entry.body {
                                ui.label(
                                    egui::RichText::new(body).color(crate::theme::TEXT_DIM),
                                );
                            }
                        });
                    });
                    ui.add_space(3.0);
                }
            }
            ui.add_space(12.0);
        });
}
