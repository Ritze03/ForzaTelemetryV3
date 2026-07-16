use egui::{Color32, RichText, Ui};

use crate::app::{ForzaApp, ProfileDialog};
use crate::i18n::{tr, Language};

/// Two-column control row: label in the left half, control in the right half —
/// the styling-guide layout (see docs/ui/STYLING-GUIDE.md).
///
/// The right cell is wrapped in `horizontal` so it's bounded to a single row's
/// height (mirrors `theme::slider_row`). Without it, a right closure that uses
/// `right_to_left(Center)` centers its content across the column's full height
/// and the control drifts to the vertical middle of the panel.
fn control_row(ui: &mut Ui, label: &str, right: impl FnOnce(&mut Ui)) {
    ui.columns(2, |c| {
        crate::theme::row_label(&mut c[0], label);
        c[1].horizontal(|ui| right(ui));
    });
}

/// A dim sub-heading inside a category card (e.g. "Global (while in-game)").
fn sub_heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(11.0).color(crate::theme::TEXT_DIM).strong());
}

/// A coloured status dot + message (● renders in the font; emoji don't).
fn status_dot(ui: &mut Ui, ok: bool, msg: &str) {
    ui.horizontal(|ui| {
        let col = if ok { Color32::from_rgb(80, 200, 120) } else { Color32::from_rgb(220, 100, 100) };
        ui.label(RichText::new("\u{25CF}").color(col));
        ui.label(RichText::new(msg).size(11.0));
    });
}

/// A small grey hint line under a control.
fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(Color32::GRAY));
}

/// Show `area` and, while the pointer is over its viewport, swallow the leftover
/// wheel delta so the outer Settings pane never chain-scrolls. This applies whenever
/// the pointer is over the pane — even when it holds too little to scroll yet — so a
/// fixed-height scroll box (profile list, export/import trees) always feels like its
/// own scroll container, not a pass-through. egui 0.33 has no built-in chaining
/// toggle; zeroing the scroll delta after the inner area consumed its share (but
/// before the parent reads it) is the fix.
fn captured_scroll<R>(
    ui: &mut Ui,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let out = area.show(ui, add);
    let over = ui
        .ctx()
        .input(|i| i.pointer.hover_pos())
        .is_some_and(|p| out.inner_rect.contains(p));
    if over {
        ui.ctx().input_mut(|i| {
            i.smooth_scroll_delta = egui::Vec2::ZERO;
            i.raw_scroll_delta = egui::Vec2::ZERO;
        });
    }
    out.inner
}

pub fn show(ui: &mut Ui, app: &mut ForzaApp) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
        ui.columns(2, |cols| {
            // ── LEFT COLUMN ──────────────────────────────────────────
            let left = &mut cols[0];
            left.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap

            // Left column: Profiles (with Export/Import inside it) and Hotkey.
            crate::theme::card(left, tr("Profiles"), |ui| profiles_card(ui, app));
            crate::theme::card(left, tr("Hotkey"), |ui| hotkey_card(ui, app));

            // ── RIGHT COLUMN ─────────────────────────────────────────
            let right = &mut cols[1];
            right.spacing_mut().item_spacing.y = 0.0;

            crate::theme::card(right, tr("Repository / Credits"), |ui| repo_card(ui));

            crate::theme::card(right, tr("Display"), |ui| {
                control_row(ui, tr("Language"), |ui| {
                    egui::ComboBox::from_id_salt("language_combo")
                        .selected_text(app.config.language.label())
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for lang in Language::ALL {
                                ui.selectable_value(&mut app.config.language, lang, lang.label());
                            }
                        });
                });
                control_row(ui, tr("Speed unit"), |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::styled_radio(ui, &mut app.config.use_mph, false, "km/h");
                        crate::theme::styled_radio(ui, &mut app.config.use_mph, true, "mph");
                    });
                });
                control_row(ui, tr("Tire temp unit"), |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::styled_radio(ui, &mut app.config.use_fahrenheit, false, "°C");
                        crate::theme::styled_radio(ui, &mut app.config.use_fahrenheit, true, "°F");
                    });
                });
                control_row(ui, tr("Boost / pressure"), |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::styled_radio(ui, &mut app.config.use_bar, true, "bar");
                        crate::theme::styled_radio(ui, &mut app.config.use_bar, false, "PSI");
                    });
                });
                let fps_on = app.config.fps_limit_enabled;
                crate::theme::checkbox_row_with(ui, &mut app.config.fps_limit_enabled, tr("FPS limit"), |ui| {
                    if fps_on {
                        ui.add(
                            egui::Slider::new(&mut app.config.fps_limit, 5.0..=120.0)
                                .step_by(1.0)
                                .suffix(" fps"),
                        );
                    }
                });
                crate::theme::checkbox_row(ui, &mut app.config.always_on_top, tr("Always on top"));
            });

            crate::theme::card(right, tr("Network"), |ui| {
                control_row(ui, tr("Listen port"), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let changed = app.pending_port != app.config.listen_port;
                        let btn = egui::Button::new(tr("Apply")).fill(if changed {
                            Color32::from_rgb(60, 120, 200)
                        } else {
                            Color32::TRANSPARENT
                        });
                        if ui.add(btn).clicked() && changed {
                            let port = app.pending_port;
                            app.restart_receiver(port);
                        }
                        ui.add(egui::DragValue::new(&mut app.pending_port).range(1024..=65535));
                    });
                });
                hint(ui, tr("Avoid ports 5200–5300 (used by the game)."));
            });

            crate::theme::card(right, tr("Co-Op"), |ui| {
                control_row(ui, tr("Host port"), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::DragValue::new(&mut app.config.coop_port).range(1024..=65535));
                    });
                });
                hint(ui, tr("Local port the tunnel points at. Change only if it clashes with another app."));
            });

            crate::theme::card(right, tr("Input"), |ui| input_card(ui, app));
        });
    });

    // Float above the whole tab; only visible when the matching dialog is open.
    profile_dialog_modal(ui, app);
    profile_io_modal(ui, app);
}

/// The PROFILES category: a scrollable profile list, the New / Duplicate / Rename /
/// Delete row, and — below a divider in the same card — the Export/Import section
/// ([`export_import_body`]). The buttons open a modal ([`profile_dialog_modal`],
/// rendered at the end of [`show`]).
///
/// Save is continuous (the live config mirrors the active profile on every
/// change — see `AppConfig::save`), so there is no explicit Save button and
/// switching always persists the outgoing profile first.
fn profiles_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config;
    let profiles = config::list_profiles();
    let active = app.config.active_profile.clone();

    // Scrollable list: fixed height, one row per profile, active row washed + checked.
    // Clicking a row switches to it (the list replaces the old dropdown).
    let mut switch_to: Option<String> = None;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let area = egui::ScrollArea::vertical()
                .max_height(210.0)
                .min_scrolled_height(210.0)
                .auto_shrink([false, false]);
            captured_scroll(ui, area, |ui| {
                ui.set_min_height(210.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for name in &profiles {
                    if profile_row(ui, name, *name == active, None) {
                        switch_to = Some(name.clone());
                    }
                }
            });
        });
    if let Some(name) = switch_to {
        app.config.switch_profile(&name);
        app.profile_io_status = format!("{} {}", tr("Loaded profile"), name);
    }

    ui.add_space(6.0);

    // Four equal-width action buttons — each opens a modal dialog.
    ui.columns(4, |c| {
        if c[0].add_sized([c[0].available_width(), 24.0], egui::Button::new(tr("New"))).clicked() {
            open_profile_dialog(app, ProfileDialog::New, String::new());
        }
        if c[1].add_sized([c[1].available_width(), 24.0], egui::Button::new(tr("Duplicate"))).clicked() {
            open_profile_dialog(app, ProfileDialog::Duplicate, format!("{active} copy"));
        }
        if c[2].add_sized([c[2].available_width(), 24.0], egui::Button::new(tr("Rename"))).clicked() {
            open_profile_dialog(app, ProfileDialog::Rename, active.clone());
        }
        let can_delete = profiles.len() > 1;
        if c[3].add_enabled_ui(can_delete, |ui| {
            ui.add_sized([ui.available_width(), 24.0], egui::Button::new(tr("Delete"))).clicked()
        }).inner {
            open_profile_dialog(app, ProfileDialog::ConfirmDelete, String::new());
        }
    });

    // Export / Import each open a large two-pane modal instead of living inline,
    // so the card stays compact (see [`profile_io_modal`]).
    ui.add_space(6.0);
    ui.columns(2, |c| {
        if c[0].add_sized([c[0].available_width(), 24.0],
            egui::Button::new(format!("{}  {}", crate::icons::COPY, tr("Export")))).clicked()
        {
            app.profile_dialog = ProfileDialog::Export;
        }
        if c[1].add_sized([c[1].available_width(), 24.0],
            egui::Button::new(format!("{}  {}", crate::icons::FLOPPY, tr("Import")))).clicked()
        {
            app.profile_dialog = ProfileDialog::Import;
        }
    });

    if !app.profile_io_status.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new(&app.profile_io_status).size(11.0).color(Color32::from_rgb(120, 200, 120)));
    }
}

/// Open a profile dialog: set the kind, seed the name field, and request focus on it.
fn open_profile_dialog(app: &mut ForzaApp, kind: ProfileDialog, name_seed: String) {
    app.profile_dialog = kind;
    app.profile_name_buf = name_seed;
    app.profile_dialog_focus = true;
}

/// The modal for New / Duplicate / Rename / Delete: a dim backdrop that swallows
/// clicks plus a centered window. New/Duplicate/Rename carry a text field; Delete
/// is a plain confirm. Enter confirms, Esc or the backdrop cancels. Rendered at the
/// end of [`show`] so it floats above the whole Settings tab.
fn profile_dialog_modal(ui: &mut Ui, app: &mut ForzaApp) {
    let dialog = app.profile_dialog;
    if dialog == ProfileDialog::None {
        return;
    }
    let ctx = ui.ctx().clone();
    let active = app.config.active_profile.clone();

    let (title, is_text, primary, danger) = match dialog {
        ProfileDialog::New => (tr("New Profile"), true, tr("Create"), false),
        ProfileDialog::Duplicate => (tr("Duplicate Profile"), true, tr("Duplicate"), false),
        ProfileDialog::Rename => (tr("Rename Profile"), true, tr("Rename"), false),
        ProfileDialog::ConfirmDelete => (tr("Delete Profile"), false, tr("Delete"), true),
        // Export / Import are handled by the larger `profile_io_modal`.
        ProfileDialog::None | ProfileDialog::Export | ProfileDialog::Import => return,
    };

    // Backdrop: dim the tab and swallow clicks so only the dialog is interactive.
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("profile_modal_backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(true)
        .show(&ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(160));
            let r = ui.allocate_response(screen.size(), egui::Sense::click());
            if r.clicked() {
                app.profile_dialog = ProfileDialog::None;
            }
        });

    let mut confirm = false;
    let mut cancel = false;
    egui::Window::new(RichText::new(title).strong())
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(&ctx, |ui| {
            ui.set_max_width(320.0);
            ui.add_space(4.0);
            if is_text {
                let sub = match dialog {
                    ProfileDialog::New => tr("Name the new profile — it starts from your current settings."),
                    ProfileDialog::Duplicate => tr("Name the copy of this profile."),
                    ProfileDialog::Rename => tr("Enter a new name for this profile."),
                    _ => "",
                };
                ui.label(RichText::new(sub).size(12.0).color(crate::theme::TEXT_DIM));
                ui.add_space(8.0);
                let te = ui.add(
                    egui::TextEdit::singleline(&mut app.profile_name_buf)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr("Profile name")),
                );
                if app.profile_dialog_focus {
                    te.request_focus();
                    app.profile_dialog_focus = false;
                }
                if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            } else {
                ui.label(format!("{} \"{}\"? {}", tr("Delete profile"), active, tr("This cannot be undone.")));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = !is_text || !app.profile_name_buf.trim().is_empty();
                let primary_btn = if danger {
                    crate::theme::danger_button(primary)
                } else {
                    crate::theme::primary_button(primary)
                };
                if ui.add_enabled(ok, primary_btn).clicked() {
                    confirm = true;
                }
                if ui.add(crate::theme::secondary_button(tr("Cancel"))).clicked() {
                    cancel = true;
                }
            });
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }

    if confirm && (!is_text || !app.profile_name_buf.trim().is_empty()) {
        let name = app.profile_name_buf.clone();
        match dialog {
            ProfileDialog::New => {
                let n = app.config.new_profile(&name);
                app.profile_io_status = format!("{} {}", tr("Created profile"), n);
            }
            ProfileDialog::Duplicate => {
                let n = app.config.duplicate_profile_as(&active, &name);
                app.profile_io_status = format!("{} {}", tr("Created profile"), n);
            }
            ProfileDialog::Rename => {
                let n = app.config.rename_active_profile(&name);
                app.profile_io_status = format!("{} {}", tr("Renamed to"), n);
            }
            ProfileDialog::ConfirmDelete => {
                app.config.delete_profile(&active);
                app.profile_io_status = format!("{} {}", tr("Deleted profile"), active);
            }
            ProfileDialog::None | ProfileDialog::Export | ProfileDialog::Import => {}
        }
        app.profile_dialog = ProfileDialog::None;
    }
    if cancel {
        app.profile_dialog = ProfileDialog::None;
    }
}

/// One row in a profile list: full-width click target, subtle wash + right-aligned
/// check when active, hover wash otherwise. An optional accent-coloured leading icon
/// (e.g. a `+` for the "New profile" row). Returns true when clicked (and inactive).
fn profile_row(ui: &mut Ui, name: &str, active: bool, icon: Option<&str>) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        if active {
            p.rect_filled(rect, egui::CornerRadius::same(4), Color32::from_rgba_unmultiplied(91, 140, 255, 36));
        } else if resp.hovered() {
            p.rect_filled(rect, egui::CornerRadius::same(4), Color32::from_rgba_unmultiplied(255, 255, 255, 10));
        }
        let font = egui::TextStyle::Body.resolve(ui.style());
        let mut x = rect.left() + 8.0;
        if let Some(ic) = icon {
            p.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, ic, font.clone(), crate::theme::ACCENT);
            x += 20.0;
        }
        p.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            font.clone(),
            if active { crate::theme::TEXT } else { crate::theme::TEXT_DIM },
        );
        if active {
            p.text(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                crate::icons::CHECK,
                font,
                crate::theme::ACCENT,
            );
        }
    }
    resp.clicked() && !active
}

/// A **fixed-height** rounded, bordered scroll box: the border stays put while the
/// content scrolls inside it (unlike a bare growing widget in a ScrollArea, whose own
/// frame scrolls away). Always occupies `height` regardless of content, so an empty
/// preview doesn't collapse. `both` also enables horizontal scroll (long JSON lines).
/// The corner radius equals the margin so the rectangular viewport clears the rounding.
/// Modal-only — no wheel capture needed since the window itself doesn't scroll.
fn scroll_box<R>(ui: &mut Ui, id: &str, height: f32, both: bool, content: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(4))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            let inner = (height - 12.0).max(30.0);
            ui.set_width(ui.available_width());
            ui.set_height(inner); // fix the box height so it never collapses to content
            let area = if both { egui::ScrollArea::both() } else { egui::ScrollArea::vertical() };
            area.auto_shrink([false, false]).max_height(inner).id_salt(id).show(ui, content).inner
        })
        .inner
}

/// The group selection tree in a fixed-height bordered scroll box.
fn tree_box(ui: &mut Ui, id: &str, height: f32, sel: &mut [bool], present: Option<&[bool]>) {
    scroll_box(ui, id, height, false, |ui| group_tree(ui, sel, present));
}

/// Recompute which groups the current import source (bundled preset or paste buffer)
/// contains, and pre-check exactly those.
fn recompute_import_present(app: &mut ForzaApp) {
    let src = match app.profile_import_builtin {
        Some(i) => crate::config::PRESET_DATA[i].to_string(),
        None => app.profile_import_buf.clone(),
    };
    app.profile_import_present = crate::config::groups_present(&src);
    app.profile_import_sel = app.profile_import_present.clone();
}

/// The effective import-source JSON (paste buffer or the chosen bundled preset).
fn import_source_json(app: &ForzaApp) -> String {
    match app.profile_import_builtin {
        Some(i) => crate::config::PRESET_DATA[i].to_string(),
        None => app.profile_import_buf.clone(),
    }
}

/// Read-only JSON preview in a fixed-height bordered scroll box. The editor is
/// **frameless** so its own border can't scroll out from under the box's rounded frame.
fn json_preview(ui: &mut Ui, id: &str, height: f32, json: &str) {
    scroll_box(ui, id, height, true, |ui| {
        let mut text = if json.is_empty() { "{}".to_string() } else { json.to_string() };
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .frame(false)
                .code_editor()
                .desired_width(f32::INFINITY)
                .interactive(false),
        );
    });
}

/// Fixed-height paste box: a rounded frame that stays put around an internally
/// scrolling, frameless editor. Returns true if edited.
fn paste_box(ui: &mut Ui, id: &str, height: f32, buf: &mut String) -> bool {
    scroll_box(ui, id, height, false, |ui| {
        ui.add(
            egui::TextEdit::multiline(buf)
                .frame(false)
                .desired_width(f32::INFINITY)
                .code_editor()
                .hint_text(tr("Paste JSON here")),
        )
        .changed()
    })
}

/// The large two-pane Export / Import modal (opened from the Profiles card buttons):
/// a dim backdrop plus a centered window. Left pane = what to include (+ source &
/// destination for import); right pane = a live JSON preview filtered by the ticks;
/// a big accent action button plus Cancel along the bottom. Esc or a backdrop-click
/// cancels. Rendered at the end of [`show`] so it floats over the whole tab.
fn profile_io_modal(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config;
    let is_export = match app.profile_dialog {
        ProfileDialog::Export => true,
        ProfileDialog::Import => false,
        _ => return,
    };
    if app.profile_export_sel.len() != config::KEY_GROUPS.len() {
        app.profile_export_sel = vec![true; config::KEY_GROUPS.len()];
    }
    if app.profile_import_sel.len() != config::KEY_GROUPS.len() {
        app.profile_import_sel = vec![true; config::KEY_GROUPS.len()];
        app.profile_import_present = vec![false; config::KEY_GROUPS.len()];
    }

    let ctx = ui.ctx().clone();

    // Backdrop.
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("profile_io_backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(true)
        .show(&ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(160));
            if ui.allocate_response(screen.size(), egui::Sense::click()).clicked() {
                app.profile_dialog = ProfileDialog::None;
            }
        });

    const PANE_H: f32 = 430.0;
    let title = if is_export { tr("Export Profile") } else { tr("Import Profile") };
    let mut do_action = false;
    let mut cancel = false;

    egui::Window::new(RichText::new(title).strong())
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(760.0)
        .show(&ctx, |ui| {
            ui.set_width(760.0);
            if is_export {
                // Single split: What to export (left) | Preview (right).
                ui.columns(2, |c| {
                    {
                        let ui = &mut c[0];
                        ui.set_min_height(PANE_H);
                        ui.label(crate::theme::section_label(tr("What to export")));
                        ui.add_space(4.0);
                        tree_box(ui, "exp_tree_modal", PANE_H - 26.0, &mut app.profile_export_sel, None);
                    }
                    {
                        let ui = &mut c[1];
                        ui.set_min_height(PANE_H);
                        ui.label(crate::theme::section_label(tr("Preview")));
                        ui.add_space(4.0);
                        let preview = config::export_selected(&app.config, &app.profile_export_sel);
                        json_preview(ui, "io_preview", PANE_H - 26.0, &preview);
                    }
                });
            } else {
                // Top: Source (left) | Destination (right), fixed height so the split
                // below lines up. Bottom: What to import (left) | Preview (right).
                const TOP_H: f32 = 156.0;
                const BOT_H: f32 = 270.0;
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), TOP_H),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.columns(2, |c| {
                            {
                                let ui = &mut c[0];
                                ui.set_min_height(TOP_H);
                                import_source_col(ui, app);
                            }
                            {
                                let ui = &mut c[1];
                                ui.set_min_height(TOP_H);
                                import_dest_col(ui, app);
                            }
                        });
                    },
                );
                ui.add_space(8.0);
                ui.columns(2, |c| {
                    {
                        let ui = &mut c[0];
                        ui.set_min_height(BOT_H);
                        ui.label(crate::theme::section_label(tr("What to import")));
                        ui.add_space(4.0);
                        tree_box(ui, "imp_tree_modal", BOT_H - 26.0, &mut app.profile_import_sel, Some(&app.profile_import_present));
                    }
                    {
                        let ui = &mut c[1];
                        ui.set_min_height(BOT_H);
                        ui.label(crate::theme::section_label(tr("Preview")));
                        ui.add_space(4.0);
                        let preview = config::filter_selected(&import_source_json(app), &app.profile_import_sel);
                        json_preview(ui, "io_preview", BOT_H - 26.0, &preview);
                    }
                });
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let cancel_w = 96.0;
                let big_w = (ui.available_width() - cancel_w - ui.spacing().item_spacing.x).max(120.0);
                let label = if is_export { tr("Export") } else { tr("Import") };
                let ok = is_export || !import_source_json(app).trim().is_empty();
                let clicked = ui
                    .add_enabled_ui(ok, |ui| ui.add_sized([big_w, 34.0], crate::theme::primary_button(label)).clicked())
                    .inner;
                if clicked {
                    do_action = true;
                }
                if ui.add_sized([cancel_w, 34.0], crate::theme::secondary_button(tr("Cancel"))).clicked() {
                    cancel = true;
                }
            });
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }

    if do_action {
        if is_export {
            ctx.copy_text(config::export_selected(&app.config, &app.profile_export_sel));
            app.profile_io_status = tr("Copied to clipboard.").to_string();
            app.profile_dialog = ProfileDialog::None;
        } else {
            let src = import_source_json(app);
            if !src.trim().is_empty() {
                // Land on the target profile, then overlay only the ticked groups.
                if app.profile_import_new {
                    let base = if app.profile_import_new_name.trim().is_empty() {
                        "Imported".to_string()
                    } else {
                        app.profile_import_new_name.clone()
                    };
                    app.config.new_profile(&base);
                } else {
                    let target = app.profile_import_overwrite.clone();
                    if !target.is_empty() {
                        app.config.switch_profile(&target);
                    }
                }
                if config::import_selected(&mut app.config, &src, &app.profile_import_sel) {
                    app.config.save();
                    if app.profile_import_builtin.is_none() {
                        app.profile_import_buf.clear();
                    }
                    app.profile_io_status = format!("{} {}", tr("Imported into"), app.config.active_profile);
                } else {
                    app.profile_io_status = tr("Invalid JSON — nothing imported.").to_string();
                }
                app.profile_dialog = ProfileDialog::None;
            }
        }
    }
    if cancel {
        app.profile_dialog = ProfileDialog::None;
    }
}

/// Import modal top-left column: the Source picker (Paste JSON / bundled preset). The
/// paste box fills the column's remaining height so it ends level with the Destination
/// column's name field.
fn import_source_col(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config;
    ui.label(crate::theme::section_label(tr("Source")));
    ui.add_space(4.0);

    let mut source_changed = false;
    let sel_text = match app.profile_import_builtin {
        Some(i) => config::PRESET_NAMES[i],
        None => tr("Paste JSON"),
    };
    egui::ComboBox::from_id_salt("io_source_combo")
        .selected_text(sel_text)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            if ui.selectable_label(app.profile_import_builtin.is_none(), tr("Paste JSON")).clicked() {
                app.profile_import_builtin = None;
                source_changed = true;
            }
            for (i, name) in config::PRESET_NAMES.iter().enumerate() {
                if ui.selectable_label(app.profile_import_builtin == Some(i), *name).clicked() {
                    app.profile_import_builtin = Some(i);
                    source_changed = true;
                }
            }
        });
    if source_changed {
        recompute_import_present(app);
    }
    ui.add_space(4.0);

    let box_h = ui.available_height().max(60.0); // fill to the bottom of the top row
    if app.profile_import_builtin.is_none() {
        if paste_box(ui, "io_paste", box_h, &mut app.profile_import_buf) {
            recompute_import_present(app);
        }
    } else {
        hint(ui, tr("Using a bundled preset as the source."));
    }
}

/// Import modal top-right column: the Destination — a profile list (its first row a blue
/// "+ New profile") that fills the column down to an always-present name field (disabled
/// unless "New profile" is selected), so it ends level with the Source paste box and
/// never jumps.
fn import_dest_col(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config;
    let profiles = config::list_profiles();
    let active = app.config.active_profile.clone();

    ui.label(crate::theme::section_label(tr("Destination")));
    ui.add_space(4.0);

    // The list fills the column, leaving room for the name field pinned at the bottom.
    let name_reserve = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    let dest_h = (ui.available_height() - name_reserve).max(56.0);
    let mut pick_new = false;
    let mut pick_overwrite: Option<String> = None;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(4))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let inner = (dest_h - 12.0).max(40.0);
            let area = egui::ScrollArea::vertical()
                .max_height(inner)
                .min_scrolled_height(inner)
                .auto_shrink([false, false]);
            captured_scroll(ui, area, |ui| {
                ui.set_min_height(inner);
                ui.spacing_mut().item_spacing.y = 0.0;
                if profile_row(ui, tr("New profile"), app.profile_import_new, Some(crate::icons::PLUS)) {
                    pick_new = true;
                }
                for name in &profiles {
                    let sel = !app.profile_import_new && app.profile_import_overwrite == *name;
                    if profile_row(ui, name, sel, None) {
                        pick_overwrite = Some(name.clone());
                    }
                }
            });
        });
    if pick_new {
        app.profile_import_new = true;
    }
    if let Some(name) = pick_overwrite {
        app.profile_import_new = false;
        app.profile_import_overwrite = name;
    }
    if app.profile_import_overwrite.is_empty() {
        app.profile_import_overwrite = active;
    }
    ui.add_space(4.0);
    ui.add_enabled_ui(app.profile_import_new, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.profile_import_new_name)
                .hint_text(tr("New profile name"))
                .desired_width(f32::INFINITY),
        );
    });
}

/// Two-level checkbox tree over `config::KEY_GROUPS`, aligned to `sel` by index,
/// drawn with the app's styled checkbox ([`crate::theme::styled_checkbox_enabled`]).
/// A section's parent toggles all its children; when `present` is given (import),
/// groups absent from the pasted JSON are disabled and force-unchecked.
fn group_tree(ui: &mut Ui, sel: &mut [bool], present: Option<&[bool]>) {
    use crate::config::KEY_GROUPS;
    let enabled = |j: usize| present.map_or(true, |p| p.get(j).copied().unwrap_or(false));
    let mut i = 0;
    while i < KEY_GROUPS.len() {
        let section = KEY_GROUPS[i].section;
        let start = i;
        while i < KEY_GROUPS.len() && KEY_GROUPS[i].section == section {
            i += 1;
        }
        let end = i;
        let section_on = present.is_none() || (start..end).any(enabled);
        let mut parent = (start..end).all(|j| sel[j]);
        if crate::theme::styled_checkbox_enabled(ui, &mut parent, tr(section), section_on).changed() {
            for j in start..end {
                sel[j] = parent && enabled(j);
            }
        }
        for j in start..end {
            let en = enabled(j);
            if !en {
                sel[j] = false;
            }
            ui.horizontal(|ui| {
                ui.add_space(16.0); // indent children under their section
                let mut c = sel[j];
                if crate::theme::styled_checkbox_enabled(ui, &mut c, tr(KEY_GROUPS[j].name), en).changed() {
                    sel[j] = c;
                }
            });
        }
    }
}

/// The "Hotkey" category: rebind rows grouped by scope.
fn hotkey_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config::{HotkeyAction, HotkeyScope};
    let mut changed = false;

    for (scope, heading) in [
        (HotkeyScope::Global, tr("Global (while in-game)")),
        (HotkeyScope::AppFocused, tr("In-app")),
    ] {
        sub_heading(ui, heading);
        for action in HotkeyAction::ALL.iter().copied().filter(|a| a.scope() == scope) {
            let capturing = app.rebinding == Some(action);
            let text = if capturing {
                tr("Press a key…").to_string()
            } else {
                app.config.hotkeys.bindings.get(&action).map(|b| b.label()).unwrap_or_default()
            };
            control_row(ui, tr(action.label()), |ui| {
                let h = ui.spacing().interact_size.y;
                if ui.add_sized([ui.available_width(), h], egui::Button::new(text)).clicked() {
                    app.rebinding = if capturing { None } else { Some(action) };
                }
            });
        }
    }

    // Capture the next key while rebinding.
    if let Some(action) = app.rebinding {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.rebinding = None;
        } else if let Some(hk) = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, .. } => crate::keymap::HotKey::from_egui(*key),
                _ => None,
            })
        }) {
            if hk != crate::keymap::HotKey::Escape {
                let m = ui.input(|i| i.modifiers);
                app.config.hotkeys.bindings.insert(action, crate::keymap::HotkeyBinding {
                    mods: crate::keymap::Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift, sup: false },
                    key: hk,
                });
                app.rebinding = None;
                changed = true;
            }
        }
    }

    if changed { app.sync_hotkeys(); }
}

/// The "Input" category: window detection + synthetic-input gating.
fn input_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config::GateMode;
    let mut changed = false;

    // ── Window Detection ──
    sub_heading(ui, tr("Window Detection"));
    control_row(ui, tr("Active if"), |ui| {
        egui::ComboBox::from_id_salt("hk_gate_mode")
            .selected_text(match app.config.hotkeys.gate_mode {
                GateMode::TelemetryLive => tr("Telemetry live"),
                GateMode::WindowFocus => tr("Game window focused"),
            })
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::TelemetryLive, tr("Telemetry live")).changed();
                changed |= ui.selectable_value(&mut app.config.hotkeys.gate_mode, GateMode::WindowFocus, tr("Game window focused")).changed();
            });
    });

    if app.config.hotkeys.gate_mode == GateMode::WindowFocus {
        #[cfg(target_os = "linux")]
        {
            use crate::config::FocusMethod;
            control_row(ui, tr("Window Detection Method"), |ui| {
                egui::ComboBox::from_id_salt("hk_focus_method")
                    .selected_text(match app.config.hotkeys.focus_method {
                        FocusMethod::Hyprland => "Hyprland",
                        FocusMethod::X11 => "X11",
                        FocusMethod::Custom => tr("Custom"),
                    })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Hyprland, "Hyprland").changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::X11, "X11").changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Custom, tr("Custom")).changed();
                    });
            });
            if app.config.hotkeys.focus_method == FocusMethod::Custom {
                control_row(ui, tr("Command"), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("Test")).clicked() {
                            app.focus_preview = app.focus.query_now().unwrap_or_else(|e| format!("error: {e}"));
                        }
                        changed |= ui.add(egui::TextEdit::singleline(&mut app.config.hotkeys.custom_cmd).desired_width(ui.available_width())).changed();
                    });
                });
                if !app.focus_preview.is_empty() {
                    hint(ui, &format!("\u{2192} {}", app.focus_preview));
                }
            }
        }

        control_row(ui, tr("Game Window Title"), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = match app.detect_until {
                    Some(t) => {
                        let secs = t.saturating_duration_since(std::time::Instant::now()).as_secs() + 1;
                        format!("{} {}", tr("Detecting…"), secs)
                    }
                    None => tr("Detect").to_string(),
                };
                if ui.button(label).clicked() && app.detect_until.is_none() {
                    app.detect_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                }
                changed |= ui.add(egui::TextEdit::singleline(&mut app.config.hotkeys.game_match).desired_width(ui.available_width())).changed();
            });
        });

    }

    // Poll rate for window detection (drives both hotkey gating and the input gate).
    changed |= crate::theme::slider_row(ui, tr("Focus check rate"), &mut app.config.hotkeys.focus_poll_hz, 1.0..=20.0, 1.0, 0, " Hz").changed();

    // Live game-window status light (last entry). The detector polls whenever
    // window-focus gating or the input gate is on.
    let detector_active = app.config.hotkeys.input_focus_gate
        || app.config.hotkeys.gate_mode == GateMode::WindowFocus;
    if detector_active {
        if app.focus.status() == crate::focus::FocusStatus::QueryFailed {
            status_dot(ui, false, tr("Focus detection failed — check the method/command"));
        } else {
            let focused = app.focus.focused();
            let msg = if focused { tr("Game window focused") } else { tr("Game window not focused") };
            status_dot(ui, focused, msg);
        }
    }

    // ── Send Input ──
    sub_heading(ui, tr("Send Input"));
    changed |= crate::theme::checkbox_row(ui, &mut app.config.hotkeys.input_focus_gate, tr("Only send inputs when game focused")).changed();

    if changed { app.sync_hotkeys(); }
}

/// The "Repository" category: project link + credits.
fn repo_card(ui: &mut Ui) {
    ui.hyperlink_to(
        "github.com/Ritze03/ForzaTelemetryV3",
        "https://github.com/Ritze03/ForzaTelemetryV3",
    );
    ui.add_space(4.0);
    ui.label(tr("Credits"));
    ui.hyperlink_to(
        tr("Le0_X8 — seasonal map images"),
        "https://www.reddit.com/r/ForzaHorizon/comments/1td6qzb/8096x_hires_seasonal_maps_of_fh6_from_the_early/",
    );
    ui.hyperlink_to(
        tr("Geist font — Vercel (OFL)"),
        "https://github.com/vercel/geist-font",
    );
    ui.hyperlink_to(
        tr("Nerd Fonts — Ryan L McIntyre (MIT)"),
        "https://github.com/ryanoasis/nerd-fonts",
    );
    hint(ui, tr("Font licences: assets/fonts/"));
}
