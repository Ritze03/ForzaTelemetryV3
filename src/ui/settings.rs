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
    crate::theme::columns(ui, 2, |c| {
        crate::theme::row_label(&mut c[0], label);
        c[1].horizontal(|ui| right(ui));
    });
}

/// [`control_row`] with a tooltip on the label (the explanation that would otherwise be a
/// helper line under the control; see the styling guide's "No helper text under options").
fn control_row_tip(ui: &mut Ui, label: &str, tip: &str, right: impl FnOnce(&mut Ui)) {
    crate::theme::columns(ui, 2, |c| {
        crate::theme::row_label(&mut c[0], label).on_hover_text(tip);
        c[1].horizontal(|ui| right(ui));
    });
}

/// Natural width of a [`crate::theme::styled_radio`] mark + `label`, for use as a
/// shared `col_w` (see [`crate::theme::styled_radio_w`]) so a set of radio rows lines
/// up into columns.
fn radio_col_width(ui: &Ui, label: &str) -> f32 {
    const BOX: f32 = 18.0;
    const GAP: f32 = 7.0;
    let font = egui::TextStyle::Body.resolve(ui.style());
    BOX + GAP + ui.painter().layout_no_wrap(label.to_owned(), font, Color32::WHITE).size().x
}

/// A dim sub-heading inside a category card (e.g. "Global (while in-game)").
fn sub_heading(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(11.0).color(crate::theme::TEXT_DIM).strong());
}

/// The state a [`status_dot`] shows: green, amber (waiting / not focused), red, or grey (off).
#[derive(Clone, Copy)]
enum Dot { Ok, Warn, Bad, Off }

impl Dot {
    fn color(self) -> Color32 {
        match self {
            Dot::Ok => crate::theme::GOOD,
            Dot::Warn => crate::theme::WARN,
            Dot::Bad => crate::theme::DANGER,
            Dot::Off => crate::theme::TEXT_DIM,
        }
    }
}

/// A coloured status dot + message (● renders in the font; emoji don't).
fn status_dot(ui: &mut Ui, dot: Dot, msg: &str) {
    ui.horizontal(|ui| {
        let col = dot.color();
        ui.label(RichText::new("\u{25CF}").color(col));
        ui.label(RichText::new(msg).size(11.0));
    });
}

/// A small grey status line (a Test result, a source note), never an option explanation.
fn result_line(ui: &mut Ui, text: &str) {
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
        crate::theme::columns(ui, 2, |cols| {
            // ── LEFT COLUMN ──────────────────────────────────────────
            let left = &mut cols[0];
            left.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap

            // Left column: Profiles (with Export/Import inside it) and Hotkey.
            crate::theme::card(left, tr("Profiles"), |ui| profiles_card(ui, app));
            crate::theme::card(left, tr("Hotkey"), |ui| hotkey_card(ui, app));
            crate::theme::card(left, tr("Controller"), |ui| controller_card(ui, app));

            crate::theme::card(left, tr("Display"), |ui| {
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
                // The three unit rows below share one column width for their first radio
                // (measured from the widest first-position label, "km/h"), so the second
                // radio in every row starts at the same x — they read as two columns
                // instead of each row hugging its own label width.
                let unit_col_w = radio_col_width(ui, "km/h");
                control_row(ui, tr("Speed unit"), |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::styled_radio_w(ui, &mut app.config.use_mph, false, "km/h", unit_col_w);
                        crate::theme::styled_radio(ui, &mut app.config.use_mph, true, "mph");
                    });
                });
                control_row(ui, tr("Tire temp unit"), |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::styled_radio_w(ui, &mut app.config.use_fahrenheit, false, "°C", unit_col_w);
                        crate::theme::styled_radio(ui, &mut app.config.use_fahrenheit, true, "°F");
                    });
                });
                control_row(ui, tr("Boost / pressure"), |ui| {
                    ui.horizontal(|ui| {
                        crate::theme::styled_radio_w(ui, &mut app.config.use_bar, true, "bar", unit_col_w);
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

            // ── RIGHT COLUMN ─────────────────────────────────────────
            let right = &mut cols[1];
            right.spacing_mut().item_spacing.y = 0.0;

            crate::theme::card(right, tr("Repository / Credits"), |ui| repo_card(ui));

            crate::theme::card(right, tr("Network"), |ui| {
                control_row_tip(ui, tr("Listen port"), tr("Avoid ports 5200–5300 (used by the game)."), |ui| {
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
                crate::theme::checkbox_row(ui, &mut app.config.experimental_pause_detection, tr("Experimental pause detection"))
                    .on_hover_text(tr("Detects the garage and menus by a level, motionless car with the handbrake fully on. May miss a garage view where the car is rotated."));
            });

            crate::theme::card(right, tr("Game Install"), |ui| game_install_card(ui, app));

            crate::theme::card(right, tr("Map data"), |ui| map_data_card(ui, app));

            // Linux-only: Windows needs no input permissions.
            if cfg!(target_os = "linux") {
                crate::theme::card(right, tr("Input Permissions"), |ui| input_perm_card(ui, app));
            }

            crate::theme::card(right, tr("Window Detection"), |ui| input_card(ui, app));
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
    crate::theme::columns(ui, 4, |c| {
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
    crate::theme::columns(ui, 2, |c| {
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

/// A fix command in a monospace box with a Copy button (same look as the co-op share
/// code). `idx` identifies the command for the "Copied" flash in `app.input_perm_copied`.
fn command_box(ui: &mut Ui, app: &mut ForzaApp, idx: usize, cmd: &str) {
    use crate::icons;
    egui::Frame::new()
        .fill(crate::theme::FIELD)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let just_copied = app
                    .input_perm_copied
                    .is_some_and(|(i, t)| i == idx && t.elapsed().as_secs_f32() < 1.5);
                let label = if just_copied {
                    format!("{}  {}", icons::CHECK, tr("Copied"))
                } else {
                    format!("{}  {}", icons::COPY, tr("Copy"))
                };
                if ui.button(label).clicked() {
                    ui.ctx().copy_text(cmd.to_string());
                    app.input_perm_copied = Some((idx, std::time::Instant::now()));
                }
                // Wraps (the udev command is long); selectable as a copy fallback.
                ui.add(egui::Label::new(RichText::new(cmd).monospace().size(12.0)).wrap().selectable(true));
            });
        });
}

/// What is missing + the fix commands + the log-out note, shared by the modal and the
/// Setup card.
fn input_perm_fixes(ui: &mut Ui, app: &mut ForzaApp, report: &crate::input::InputReport) {
    if report.hotkeys_missing {
        status_dot(ui, Dot::Bad, tr("Hotkeys: cannot read keyboard devices in /dev/input"));
    }
    if report.uinput_missing {
        status_dot(ui, Dot::Bad, tr("Gearbox / Backfire key input: cannot write /dev/uinput"));
    }
    if report.group_missing {
        status_dot(ui, Dot::Bad, tr("Not a member of the input group"));
    }
    ui.add_space(4.0);
    for (i, (label, cmd)) in report.commands.iter().enumerate() {
        ui.label(RichText::new(format!("{}. {}", i + 1, tr(label))).size(12.0));
        command_box(ui, app, i, cmd);
    }
    ui.add_space(2.0);
    let note = if report.commands.len() >= 2 {
        tr("Run all commands above, then log out and back in.")
    } else {
        tr("Log out and back in for it to take effect.")
    };
    ui.label(RichText::new(note).size(11.0).color(crate::theme::TEXT_DIM));
}

/// The "Input Permissions" category (Linux): one status light per requirement, plus the
/// fix commands when something is missing and the startup-reminder toggle.
fn input_perm_card(ui: &mut Ui, app: &mut ForzaApp) {
    let p = app.input_probe;
    let dot = |ok: bool| if ok { Dot::Ok } else { Dot::Bad };
    status_dot(ui, dot(p.hotkeys_ok), tr("Hotkeys: read keyboard devices (/dev/input)"));
    status_dot(ui, dot(p.uinput_ok), tr("Key input: write /dev/uinput"));
    // A real requirement: without the group a user practically can't read keyboards or write
    // /dev/uinput. A false alarm (access via ACLs / udev) is muted by "Remind me on startup".
    status_dot(ui, dot(p.in_input_group), tr("Member of the input group"));
    let report = crate::input::evaluate(&p);
    if report.any_missing() {
        ui.add_space(4.0);
        input_perm_fixes(ui, app, &report);
    }
    ui.add_space(4.0);
    let mut remind = !app.config.input_perm_dont_remind;
    if crate::theme::checkbox_row(ui, &mut remind, tr("Remind me on startup"))
        .on_hover_text(tr("Show the missing-permissions dialog at launch while something is missing."))
        .changed()
    {
        app.config.input_perm_dont_remind = !remind;
    }
    if ui.add(crate::theme::secondary_button(tr("Re-check"))).clicked() {
        // Reopen keyboards and retry the virtual keyboard first, so access that appeared since
        // launch works without a restart.
        app.hotkeys.rescan();
        app.recheck_uinput();
        refresh_input_facts(app, true);
    }
}

/// Re-read the input probe (the Setup cards, the modal and the "Controller" light read it from
/// the app). Called **every frame from `ForzaApp::update`**, throttled to
/// once per ~2 s, or at once with `force` (Re-check). *Why app-level and live:* the probe used to
/// run once at startup (and only while Setup was open afterwards), so a reader thread or the key
/// sender dying later left a stale green, and the startup modal could never notice a fix or a
/// new breakage. Cost: sysfs reads + `open()` of the event nodes + the uinput open (sub-ms).
///
/// Also applies the modal rule ([`crate::input::modal_should_open`]): it re-opens once when the
/// status turns from fine to missing, never repeatedly while it stays missing.
pub fn refresh_input_facts(app: &mut ForzaApp, force: bool) {
    const EVERY: std::time::Duration = std::time::Duration::from_secs(2);
    if !force && app.input_probe_at.is_some_and(|t| t.elapsed() < EVERY) {
        return;
    }
    app.input_probe = crate::input::probe(app.hotkeys.active_keyboards(), app.uinput_ready());
    app.input_probe_at = Some(std::time::Instant::now());
    let missing = crate::input::evaluate(&app.input_probe).any_missing();
    if cfg!(target_os = "linux")
        && crate::input::modal_should_open(app.input_prev_missing, missing, !app.config.input_perm_dont_remind)
    {
        app.input_perm_modal_open = true;
    }
    app.input_prev_missing = missing;
}

/// Setup → Game Install state: the last check of the configured (or auto-detected) FH6 install,
/// run on a thread (it loads the car DB for the count, up to ~1 s cold).
#[derive(Default)]
pub struct Fh6Setup {
    /// The `fh6_install_dir` value the running / finished check was started for.
    key: Option<String>,
    rx: Option<std::sync::mpsc::Receiver<(crate::gamedata::install::InstallCheck, Option<usize>)>>,
    result: Option<(crate::gamedata::install::InstallCheck, Option<usize>)>,
    /// Feedback of the last button press (dot, text).
    note: Option<(Dot, &'static str)>,
}

impl Fh6Setup {
    /// The install's `media` folder, once the last check found it.
    fn media(&self) -> Option<std::path::PathBuf> {
        match &self.result {
            Some((crate::gamedata::install::InstallCheck::Found(m), _)) => Some(m.clone()),
            _ => None,
        }
    }

    /// No check result yet (the first check is still running).
    fn checking(&self) -> bool {
        self.result.is_none()
    }

    /// (Re)start the check when the configured folder changed; not while the field is being typed in.
    fn poll(&mut self, ctx: &egui::Context, dir: &str, editing: bool) {
        use crate::gamedata::{cars::CarDb, install};
        if self.key.as_deref() != Some(dir) && !editing {
            let (tx, rx) = std::sync::mpsc::channel();
            let dir_s = dir.trim().to_string();
            let lang = crate::i18n::language_code();
            std::thread::spawn(move || {
                let over = (!dir_s.is_empty()).then(|| std::path::PathBuf::from(&dir_s));
                let chk = match &over {
                    Some(p) => install::check(p),
                    None => install::find_media(None).map_or(install::InstallCheck::NotFound, install::InstallCheck::Found),
                };
                let n = matches!(chk, install::InstallCheck::Found(_))
                    .then(|| CarDb::load_from(over.as_deref(), lang).ok().map(|d| d.len()))
                    .flatten();
                let _ = tx.send((chk, n));
            });
            self.key = Some(dir.to_string());
            self.rx = Some(rx);
            self.result = None;
        }
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.result = Some(r);
                    self.rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(_) => self.rx = None,
            }
        }
    }
}

/// The "Game Install" category: where the FH6 install is (car names come from it). Empty =
/// auto-detect through Steam; otherwise the typed / detected folder (game folder or `media`).
fn game_install_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::gamedata::{install::InstallCheck, process};
    let id = egui::Id::new("fh6_install_dir");
    let editing = ui.ctx().memory(|m| m.has_focus(id));
    app.fh6_setup.poll(ui.ctx(), &app.config.fh6_install_dir, editing);
    ui.add(
        egui::TextEdit::singleline(&mut app.config.fh6_install_dir)
            .id(id)
            .hint_text(tr("Auto (Steam)"))
            .desired_width(f32::INFINITY),
    );
    ui.horizontal_wrapped(|ui| {
        if ui.add(crate::theme::secondary_button(tr("Auto-detect"))).clicked() {
            app.fh6_setup.note = Some(match crate::gamedata::install::steam_game_dir() {
                Some(g) => {
                    app.config.fh6_install_dir = g.display().to_string();
                    (Dot::Ok, tr("Found through Steam"))
                }
                None => (Dot::Bad, tr("Not found. Enter the path manually.")),
            });
        }
        if ui
            .add(crate::theme::secondary_button(tr("Detect from running game")))
            .on_hover_text(tr("Start Forza first"))
            .clicked()
        {
            app.fh6_setup.note = Some(match process::detect_running() {
                Some(d) => {
                    app.config.fh6_install_dir = d.display().to_string();
                    (Dot::Ok, tr("Found from the running game"))
                }
                None => (Dot::Bad, tr("Forza Horizon 6 is not running")),
            });
        }
        if !app.config.fh6_install_dir.is_empty() && ui.add(crate::theme::secondary_button(tr("Clear"))).clicked() {
            app.config.fh6_install_dir.clear();
            app.fh6_setup.note = None;
        }
    });
    match &app.fh6_setup.result {
        None => result_line(ui, tr("Checking...")),
        Some((InstallCheck::Found(m), n)) => {
            let cars = n.map_or(String::new(), |n| format!(" ({n} {})", tr("cars")));
            status_dot(ui, Dot::Ok, &format!("{}{cars}", m.display()));
        }
        Some((InstallCheck::NotReadable, _)) => status_dot(ui, Dot::Warn, tr("Found but not readable (permissions)")),
        Some((InstallCheck::NotFound, _)) => status_dot(ui, Dot::Bad, tr("media folder not found")),
    }
    if let Some((dot, msg)) = app.fh6_setup.note {
        status_dot(ui, dot, msg);
    }
}

/// What the Map data card shows about the road types the app uses (a summary of
/// [`crate::gamedata::roadtypes::Current`], so the full road data isn't kept around).
#[derive(Clone)]
struct RoadStatus {
    source: crate::gamedata::roadtypes::Source,
    note: Option<String>,
    updated: bool,
}

impl RoadStatus {
    fn of(c: &crate::gamedata::roadtypes::Current) -> Self {
        RoadStatus { source: c.source, note: c.note.clone(), updated: c.project_updated_since_save }
    }
}

/// What invalidates the cached [`RoadStatus`]: the install and the override file (mtime + size).
type MapDataKey = (std::path::PathBuf, Option<(Option<std::time::SystemTime>, u64)>);

/// Setup → Map data state: the road-type status (computed on a thread while no editor server
/// runs: it loads the nav graph, ~20 ms, never on the UI thread) and the card's own UI state.
#[derive(Default)]
pub struct MapData {
    key: Option<MapDataKey>,
    rx: Option<std::sync::mpsc::Receiver<Result<RoadStatus, String>>>,
    status: Option<Result<RoadStatus, String>>,
    /// Start-mode choice: false = Current, true = Raw.
    raw: bool,
    /// The "Reset road types" button is waiting for its confirmation.
    reset_confirm: bool,
    /// Whether the user's override file exists (stat'ed every frame with the key).
    has_override: bool,
    /// Feedback of the last button press (dot, text).
    note: Option<(Dot, String)>,
}

impl MapData {
    /// Recompute the status when the install or the override file changed.
    fn poll(&mut self, ctx: &egui::Context, media: &std::path::Path) {
        use crate::gamedata::{nav::Nav, roadtypes::{override_path, RoadTypes}};
        let meta = std::fs::metadata(override_path()).ok().map(|m| (m.modified().ok(), m.len()));
        self.has_override = meta.is_some();
        let key: MapDataKey = (media.to_path_buf(), meta);
        if self.key.as_ref() != Some(&key) {
            let (tx, rx) = std::sync::mpsc::channel();
            let media = media.to_path_buf();
            std::thread::spawn(move || {
                let r = Nav::load(&media).map(|nav| RoadStatus::of(&RoadTypes::current(&override_path(), &nav)));
                let _ = tx.send(r);
            });
            self.key = Some(key);
            self.rx = Some(rx);
        }
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.status = Some(r);
                    self.rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(_) => self.rx = None,
            }
        }
    }
}

/// Fire-and-forget: show `dir` (created if missing) in the system file manager.
fn open_folder(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    let prog = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Ok(mut child) = std::process::Command::new(prog).arg(dir).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait(); // reap it, so no zombie is left behind
        });
    }
}

/// A status line that wraps inside the card: dot, then `msg`.
fn status_wrap(ui: &mut Ui, dot: Dot, msg: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("\u{25CF}").color(dot.color()));
        ui.label(RichText::new(msg).size(11.0));
    });
}

const CONTRIBUTING_URL: &str = "https://github.com/Ritze03/ForzaTelemetryV3/blob/master/CONTRIBUTING.md";

/// What the Map data card needs from the app to draw itself (so the drawing is testable
/// without a `ForzaApp`).
struct MapInputs {
    /// The install's media folder was found.
    have_install: bool,
    /// The first install check is still running.
    checking: bool,
    /// Editor server state (`None` = not running).
    state: Option<crate::mapedit::MapServerState>,
    /// The road types in use (`None` = not known yet).
    status: Option<Result<RoadStatus, String>>,
    /// The last Save / error event of the editor.
    last: Option<crate::mapedit::MapEvent>,
}

/// A button press on the Map data card, carried out by [`map_data_card`].
enum MapAction {
    Open(crate::mapedit::StartFrom),
    OpenFolder,
    Contribute,
    ResetOverride,
    Rebuild,
    StopEditor,
}

/// The "Map data" category: open the road-type map editor (built from the user's own install),
/// see which road types the app uses, reset / rebuild / contribute. See `docs/features/map-editor.md`.
fn map_data_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::gamedata::roadtypes::override_path;

    let media = app.fh6_setup.media();
    if let Some(m) = &media {
        app.map_data.poll(ui.ctx(), m);
    }
    let have_install = media.is_some();
    let state = app.map_editor_state();
    let status = match app.map_editor_current() {
        Some(c) => Some(Ok(RoadStatus::of(&c))),
        None => app.map_data.status.clone().filter(|_| have_install),
    };
    let inputs = MapInputs {
        have_install,
        checking: app.fh6_setup.checking(),
        state: state.clone(),
        status,
        last: app.map_editor_last.clone(),
    };
    match map_data_view(ui, &mut app.map_data, &inputs) {
        None => {}
        Some(MapAction::Open(mode)) => {
            let ctx = ui.ctx().clone();
            app.start_map_editor(&ctx, mode);
        }
        Some(MapAction::OpenFolder) => open_folder(&crate::config::app_data_dir().join("map_editor")),
        Some(MapAction::Contribute) => ui.ctx().open_url(egui::OpenUrl::new_tab(CONTRIBUTING_URL)),
        Some(MapAction::ResetOverride) => {
            // Only the override file; the cache and the project data stay.
            app.map_editor_last = None;
            app.map_data.note = Some(match std::fs::remove_file(override_path()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    (Dot::Bad, format!("{}: {e}", tr("Could not delete the file")))
                }
                _ if state.is_some() => {
                    // A running editor keeps the old data in memory: stop it; reopening shows the reset.
                    app.stop_map_editor();
                    (Dot::Ok, tr("Reset to project data. Reopen the map editor.").to_string())
                }
                _ => (Dot::Ok, tr("Reset to project data").to_string()),
            });
        }
        Some(MapAction::StopEditor) => app.stop_map_editor(),
        Some(MapAction::Rebuild) => {
            // Only the cache: never the user's override file.
            app.map_editor_last = None;
            let _ = std::fs::remove_dir_all(crate::gamedata::terrain::cache_dir());
            app.stop_map_editor();
            app.map_data.note = Some((Dot::Ok, tr("Map data cache cleared").to_string()));
        }
    }
}

/// Draws the Map data card body; returns the button pressed, if any.
fn map_data_view(ui: &mut Ui, md: &mut MapData, inp: &MapInputs) -> Option<MapAction> {
    use crate::gamedata::roadtypes::Source;
    use crate::mapedit::{MapEvent, MapServerState, StartFrom};

    let preparing = matches!(inp.state, Some(MapServerState::Preparing { .. }));
    let mut action = None;

    // ── status ──────────────────────────────────────────────────────
    if inp.checking {
        result_line(ui, tr("Checking..."));
    } else if !inp.have_install {
        status_wrap(ui, Dot::Bad, tr("Needs your Forza Horizon 6 install"));
    }
    match &inp.status {
        Some(Ok(s)) => {
            let what = if s.source == Source::Override { tr("Your saved file") } else { tr("Project data") };
            let dot = if s.note.is_some() || s.updated { Dot::Warn } else { Dot::Ok };
            status_wrap(ui, dot, &format!("{}: {what}", tr("Road types")));
            if let Some(n) = &s.note {
                status_wrap(ui, Dot::Warn, n);
            }
            if s.updated {
                status_wrap(ui, Dot::Warn, tr("Project data updated since your save"));
            }
        }
        Some(Err(e)) => status_wrap(ui, Dot::Bad, &format!("{}: {e}", tr("Road types"))),
        None => {}
    }
    let (dot, text) = match &inp.state {
        None => (Dot::Off, tr("Not running").to_string()),
        Some(MapServerState::Preparing { progress }) => {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            (Dot::Warn, format!("{} {:.0} %", tr("Preparing"), progress * 100.0))
        }
        Some(MapServerState::Ready) => (Dot::Ok, tr("Running").to_string()),
        Some(MapServerState::Failed(e)) => (Dot::Bad, format!("{}: {e}", tr("Failed"))),
    };
    status_wrap(ui, dot, &format!("{}: {text}", tr("Editor")));

    // ── start mode + buttons ────────────────────────────────────────
    control_row_tip(
        ui,
        tr("Start from"),
        tr("Current: your saved road types, or the project data if you have none. Raw: the game's untouched road network with no road types."),
        |ui| {
            crate::theme::radio_group(ui, &mut md.raw, &[(false, tr("Current")), (true, tr("Raw"))]);
        },
    );
    ui.horizontal_wrapped(|ui| {
        let open = ui
            .add_enabled(inp.have_install, crate::theme::primary_button(tr("Open map editor")))
            .on_hover_text(tr("Opens the road-type editor in your browser: 2D map, Preview mode and live 3D."))
            .on_disabled_hover_text(tr("Needs your Forza Horizon 6 install — the map is read from it"));
        if open.clicked() {
            md.note = None;
            md.reset_confirm = false;
            action = Some(MapAction::Open(if md.raw { StartFrom::Raw } else { StartFrom::Current }));
        }
        let running = matches!(inp.state, Some(MapServerState::Preparing { .. } | MapServerState::Ready));
        if ui
            .add_enabled(running, crate::theme::secondary_button(tr("Stop map editor")))
            .on_hover_text(tr("Stops the editor's local server; the open browser tab stops working. Save first."))
            .on_disabled_hover_text(tr("The map editor is not running."))
            .clicked()
        {
            action = Some(MapAction::StopEditor);
        }
        if ui
            .add(crate::theme::secondary_button(tr("Open data folder")))
            .on_hover_text(tr("Your saved road types and the cached map data live here."))
            .clicked()
        {
            action = Some(MapAction::OpenFolder);
        }
        if ui
            .add(crate::theme::secondary_button(tr("Contribute")))
            .on_hover_text(tr("How to send your road-type edits back to the project."))
            .clicked()
        {
            action = Some(MapAction::Contribute);
        }
        let reset_resp = ui
            .add_enabled(md.has_override && !md.reset_confirm, crate::theme::secondary_button(tr("Reset road types to project data")))
            .on_hover_text(tr("Deletes your saved road types; the project data is used again."));
        // While the confirm row is open the button is disabled too; the row asks the question, so no "no saved road types" text then.
        let reset_resp = if md.has_override { reset_resp } else { reset_resp.on_disabled_hover_text(tr("You have no saved road types.")) };
        if reset_resp.clicked() {
            md.reset_confirm = true;
        }
        if ui
            .add_enabled(inp.have_install && !preparing, crate::theme::secondary_button(tr("Rebuild map data")))
            .on_hover_text(tr("Deletes the cached terrain and road data; it is rebuilt the next time the editor opens. Your saved road types are kept."))
            .on_disabled_hover_text(if inp.have_install {
                tr("Wait until the map data is prepared.")
            } else {
                tr("Needs your Forza Horizon 6 install — the map is read from it")
            })
            .clicked()
        {
            action = Some(MapAction::Rebuild);
        }
    });
    // Why: the confirm is its own block (question + buttons as one wrapping unit) so a narrow
    // pane never splits the sentence from Reset / Cancel.
    if md.reset_confirm {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tr("Delete your saved road types?")).size(11.0));
            if ui.add(crate::theme::danger_button(tr("Reset"))).clicked() {
                md.reset_confirm = false;
                action = Some(MapAction::ResetOverride);
            }
            if ui.add(crate::theme::secondary_button(tr("Cancel"))).clicked() {
                md.reset_confirm = false;
            }
        });
    }
    // Why: transient result lines sit BELOW the buttons so their appearing never moves a button.
    match &inp.last {
        Some(MapEvent::Saved { edges, points, .. }) => status_wrap(
            ui,
            Dot::Ok,
            &format!("{}: {edges} {}, {points} {}", tr("Saved"), tr("edges"), tr("points")),
        ),
        Some(MapEvent::Error(e)) => status_wrap(ui, Dot::Bad, e),
        _ => {}
    }
    if let Some((dot, msg)) = md.note.clone() {
        status_wrap(ui, dot, &msg);
    }
    action
}

/// Startup modal (Linux): shown once per launch while hotkey / synthetic-input access is
/// missing. X closes it for this session; "Don't remind me again" persists in config.
/// Rendered from `ForzaApp::update` so it appears over any tab.
pub fn input_perm_modal(ctx: &egui::Context, app: &mut ForzaApp) {
    if !app.input_perm_modal_open {
        return;
    }
    let report = crate::input::evaluate(&app.input_probe);
    if !report.any_missing() {
        app.input_perm_modal_open = false;
        return;
    }
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("input_perm_backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(true)
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(160));
            ui.allocate_response(screen.size(), egui::Sense::click());
        });

    let mut close = false;
    let mut dont_remind = false;
    egui::Window::new("input_perm_modal")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(440.0_f32.min(screen.width() - 48.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Input permissions missing")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::icons::TIMES).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            input_perm_fixes(ui, app, &report);
            ui.add_space(12.0);
            if ui.add(crate::theme::secondary_button(tr("Don't remind me again"))).clicked() {
                dont_remind = true;
            }
        });
    if dont_remind {
        app.config.input_perm_dont_remind = true;
        close = true;
    }
    if close {
        app.input_perm_modal_open = false;
    }
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
                        .hint_text(crate::theme::placeholder(tr("Profile name"))),
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

/// Read-only JSON preview in a fixed-height, vertical-only bordered scroll box. A
/// **selectable** monospace label — you can select text and copy it, but not edit —
/// that **wraps** long lines so there's no horizontal scrollbar.
fn json_preview(ui: &mut Ui, id: &str, height: f32, json: &str) {
    scroll_box(ui, id, height, false, |ui| {
        let text = if json.is_empty() { "{}" } else { json };
        // A plain left layout so wrapped lines aren't justified (Label picks up the
        // container's `horizontal_justify`, which would space the glyphs out to fill).
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(text).monospace())
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Wrap),
            );
        });
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
                .hint_text(crate::theme::placeholder(tr("Paste JSON here"))),
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
                crate::theme::columns(ui, 2, |c| {
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
                        crate::theme::columns(ui, 2, |c| {
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
                crate::theme::columns(ui, 2, |c| {
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

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let cancel_w = 96.0;
                let big_w = (ui.available_width() - cancel_w - ui.spacing().item_spacing.x).max(120.0);
                let label = if is_export { tr("Copy to clipboard") } else { tr("Import") };
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
        result_line(ui, tr("Using a bundled preset as the source."));
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
                .hint_text(crate::theme::placeholder(tr("New profile name")))
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

    for (scope, heading) in [
        (HotkeyScope::Global, tr("Global (while in-game)")),
        (HotkeyScope::AppFocused, tr("In-app")),
    ] {
        sub_heading(ui, heading);
        for action in HotkeyAction::ALL.iter().copied().filter(|a| a.scope() == scope) {
            let capturing = app.rebinding == Some(action);
            let text = if capturing {
                RichText::new(tr("Press a key…"))
            } else {
                match app.config.hotkeys.bindings.get(&action) {
                    Some(b) => RichText::new(b.label()),
                    None => RichText::new(tr("Not set")).color(crate::theme::FAINT),
                }
            };
            control_row(ui, tr(action.label()), |ui| {
                let h = ui.spacing().interact_size.y;
                let resp = ui
                    .add_sized([ui.available_width(), h], egui::Button::new(text))
                    .on_hover_text(tr("Esc cancels. Backspace or Delete clears the binding."));
                if resp.clicked() {
                    app.rebinding = if capturing { None } else { Some(action) };
                }
                app.track_rebind_button(action, &resp);
            });
        }
    }
    // Key capture (bind / Backspace or Delete unbind / Esc cancel) is `ForzaApp::capture_rebind`,
    // which runs before the UI and re-syncs the hotkeys itself.
}

/// The "Controller" category: enable, detected pad, deadzones, and one capture-bind row per
/// global action (the same action set as the keyboard hotkeys).
fn controller_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config::{HotkeyAction, HotkeyScope};

    crate::theme::checkbox_row(ui, &mut app.config.gamepad.enabled, tr("Enable controller input"));
    let pads = app.gamepad.devices();
    if !app.config.gamepad.enabled {
        status_dot(ui, Dot::Warn, tr("Controller input is off"));
    } else if let Some(first) = pads.first() {
        let more = if pads.len() > 1 { format!(" (+{})", pads.len() - 1) } else { String::new() };
        status_dot(ui, Dot::Ok, &format!("{first}{more}"));
    } else if cfg!(target_os = "linux") && !app.input_probe.hotkeys_ok {
        status_dot(ui, Dot::Bad, tr("Can't read /dev/input (see Input Permissions)"));
    } else {
        status_dot(ui, Dot::Warn, tr("No controller detected"));
    }

    let g = &mut app.config.gamepad;
    crate::theme::slider_row(ui, tr("Stick deadzone"), &mut g.stick_deadzone, 0.0..=0.5, 0.01, 2, "");
    crate::theme::slider_row(ui, tr("Trigger deadzone"), &mut g.trigger_deadzone, 0.0..=0.5, 0.01, 2, "");

    // Capture: the backend stores the next pad press; Esc or leaving the tab cancels,
    // Backspace / Delete clears the binding (the keyboard counterpart of the removed ✕).
    if let Some(action) = app.pad_rebinding {
        if let Some(c) = app.gamepad.take_captured() {
            app.config.gamepad.bind(action, c);
            app.pad_rebinding = None;
        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) || !app.config.gamepad.enabled {
            app.gamepad.cancel_capture();
            app.pad_rebinding = None;
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    sub_heading(ui, tr("Bindings"));
    for action in HotkeyAction::ALL.iter().copied().filter(|a| a.scope() == HotkeyScope::Global) {
        let capturing = app.pad_rebinding == Some(action);
        let bound = app.config.gamepad.bindings.get(&action).copied();
        let text = if capturing {
            RichText::new(tr("Press a controller button…"))
        } else {
            match bound {
                Some(c) => RichText::new(tr(c.label())),
                None => RichText::new(tr("Not set")).color(crate::theme::FAINT),
            }
        };
        control_row(ui, tr(action.label()), |ui| {
            // One fixed rect = the column's full width × h, not cursor flow. Why: egui's
            // `Region::expand_to_include_rect` grows the parent's *max_rect* on overflow, so a
            // widget wider than its slot widens the card, the next row's `ui.columns` gets a
            // wider (further-right) right column, and the rows stagger. `put`ting the button
            // into a rect inside the column (text `truncate()`d) can't overflow.
            let h = ui.spacing().interact_size.y;
            let col = ui.available_rect_before_wrap();
            let rect = egui::Rect::from_min_size(col.min, egui::vec2(col.width(), h));
            let resp = ui
                .scope_builder(
                    egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)),
                    |ui| ui.add_enabled(app.config.gamepad.enabled, egui::Button::new(text).truncate()),
                )
                .inner
                .on_hover_text(tr("Esc cancels. Backspace or Delete clears the binding."));
            // Backspace / Delete while this row captures: unbind and end the capture. Skipped
            // while another widget (a text field) holds focus — the armed button itself may.
            let clear_key = capturing
                && ui.ctx().memory(|m| m.focused()).is_none_or(|f| f == resp.id)
                && ui.input(|i| i.key_pressed(egui::Key::Backspace) || i.key_pressed(egui::Key::Delete));
            if clear_key {
                app.config.gamepad.bindings.remove(&action);
                app.gamepad.cancel_capture();
                app.pad_rebinding = None;
            } else if resp.clicked() {
                if capturing {
                    app.gamepad.cancel_capture();
                    app.pad_rebinding = None;
                } else {
                    app.gamepad.arm_capture();
                    app.pad_rebinding = Some(action);
                }
            }
        });
    }
}

/// The "Window Detection" category: detection method + what it gates (hotkeys, the
/// in-game overlay, synthetic input).
fn input_card(ui: &mut Ui, app: &mut ForzaApp) {
    use crate::config::GateMode;
    let mut changed = false;

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

    // The method / title rows matter whenever anything consults the detector, not only
    // for hotkey gating. The enabled overlay counts too: its monitor detection needs the
    // detector to match the game window, so a user stuck on "Game window not focused"
    // must be able to reach the title / method.
    let uses_focus = app.config.hotkeys.gate_mode == GateMode::WindowFocus
        || app.config.hotkeys.input_focus_gate
        || app.config.overlay.focus_only
        || app.config.overlay.enabled;
    if uses_focus {
        #[cfg(target_os = "linux")]
        {
            use crate::config::FocusMethod;
            let method_before = app.config.hotkeys.focus_method;
            control_row_tip(ui, tr("Window Detection Method"), tr("Requires the \"Window Calls\" GNOME Shell extension (extensions.gnome.org/extension/4724)."), |ui| {
                egui::ComboBox::from_id_salt("hk_focus_method")
                    .selected_text(match app.config.hotkeys.focus_method {
                        FocusMethod::Hyprland => "Hyprland",
                        FocusMethod::X11 => "X11",
                        FocusMethod::Custom => tr("Custom"),
                        FocusMethod::Gnome => tr("GNOME (Window Calls extension)"),
                    })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Hyprland, "Hyprland").changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::X11, "X11").changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Gnome, tr("GNOME (Window Calls extension)")).changed();
                        changed |= ui.selectable_value(&mut app.config.hotkeys.focus_method, FocusMethod::Custom, tr("Custom")).changed();
                    });
            });
            if app.config.hotkeys.focus_method != method_before {
                app.focus_preview.clear(); // a preview from the old method would mislead
            }
            if app.config.hotkeys.focus_method == FocusMethod::Gnome {
                control_row(ui, tr("Active window"), |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr("Test")).clicked() {
                            app.focus_preview = app.focus.query_now().unwrap_or_else(|e| format!("error: {e}"));
                        }
                    });
                });
                if !app.focus_preview.is_empty() {
                    result_line(ui, &format!("\u{2192} {}", app.focus_preview));
                }
            }
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
                    result_line(ui, &format!("\u{2192} {}", app.focus_preview));
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
    // (Mirrors `focus_params` in app.rs: the overlay runs the detector too.)
    let detector_active = uses_focus || app.config.overlay.enabled;
    if detector_active {
        if app.focus.status() == crate::focus::FocusStatus::QueryFailed {
            status_dot(ui, Dot::Bad, tr("Focus detection failed — check the method/command"));
        } else if app.focus.focused() {
            status_dot(ui, Dot::Ok, tr("Game window focused"));
        } else {
            status_dot(ui, Dot::Warn, tr("Game window not focused"));
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
        tr("Geist font — Vercel (OFL)"),
        "https://github.com/vercel/geist-font",
    );
    ui.hyperlink_to(
        tr("Nerd Fonts — Ryan L McIntyre (MIT)"),
        "https://github.com/ryanoasis/nerd-fonts",
    );
    ui.hyperlink_to(
        tr("Trystero — Dan Motzenbecker (MIT), P2P co-op design"),
        "https://github.com/dmotz/trystero",
    );
    result_line(ui, tr("Font licences: assets/fonts/"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::roadtypes::Source;
    use crate::mapedit::{MapEvent, MapServerState};
    use crate::ui::test_render;

    /// The Map data card in its busiest state (long note, update warning, reset confirm, Save
    /// result, a failure) stays inside its column at narrow and wider widths. Set
    /// `FORZA_UI_SNAPSHOT_DIR` to look at it (`map_data_<w>.png`).
    #[test]
    fn map_data_card_stays_inside_its_pane() {
        let failed = MapServerState::Failed("could not build the terrain: something went quite wrong here".into());
        // Failed = longest status text; Ready = the Stop map editor button is enabled.
        for (w, state) in [700.0, 1000.0, 1235.0].into_iter().flat_map(|w| [(w, failed.clone()), (w, MapServerState::Ready)]) {
            let ctx = test_render::context();
            let mut md = MapData {
                reset_confirm: true,
                has_override: true,
                note: Some((Dot::Ok, "Map data cache cleared".into())),
                ..Default::default()
            };
            let inp = MapInputs {
                have_install: true,
                checking: false,
                state: Some(state.clone()),
                status: Some(Ok(RoadStatus {
                    source: Source::Override,
                    note: Some("Your saved road types were ignored: they were made for a different version of the game's road network. The project data is used instead.".into()),
                    updated: true,
                })),
                last: Some(MapEvent::Saved { edges: 21345, points: 87, bytes: 1 }),
            };
            let mut cols = Vec::new();
            let (out, tex) = test_render::run(&ctx, w, 600.0, |ui| {
                cols.clear();
                ui.spacing_mut().item_spacing.x = 8.0;
                crate::theme::columns(ui, 2, |c| {
                    cols.push(c[1].max_rect());
                    c[1].spacing_mut().item_spacing.y = 0.0;
                    crate::theme::card(&mut c[1], "Map data", |ui| {
                        map_data_view(ui, &mut md, &inp);
                    });
                });
            });
            test_render::snapshot(&ctx, &out, &tex, w as u32, 600, &format!("map_data_{w}"));
            for r in test_render::visible_rects(&out) {
                assert!(
                    r.left() >= cols[0].left() - 4.5 && r.right() <= cols[0].right() + 4.5,
                    "at {w} px a shape leaves the column: {r:?} (column {:?})",
                    cols[0]
                );
            }
        }
    }

    /// Text shapes of a frame as (text, top-left).
    fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        fn walk(s: &egui::Shape, v: &mut Vec<(String, egui::Pos2)>) {
            match s {
                egui::Shape::Text(t) => v.push((t.galley.text().to_string(), t.pos)),
                egui::Shape::Vec(c) => c.iter().for_each(|s| walk(s, v)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut v));
        v
    }

    /// The reset question and its Reset button share a row, and a transient result line
    /// appearing never moves the buttons above it.
    #[test]
    fn map_data_confirm_is_one_unit_and_buttons_do_not_jump() {
        let inp = MapInputs { have_install: true, checking: false, state: None, status: None, last: None };
        let render = |w: f32, confirm: bool, note: bool| {
            let ctx = test_render::context();
            let mut md = MapData {
                reset_confirm: confirm,
                has_override: true,
                note: note.then(|| (Dot::Ok, "Reset to project data".to_string())),
                ..Default::default()
            };
            let (out, _) = test_render::run(&ctx, w, 600.0, |ui| {
                crate::theme::card(ui, "Map data", |ui| {
                    map_data_view(ui, &mut md, &inp);
                });
            });
            texts(&out)
        };
        let n = render(900.0, false, true).iter().filter(|(x, _)| x == "Reset to project data").count();
        assert_eq!(n, 1, "note must render exactly once");
        let y_of = |t: &[(String, egui::Pos2)], s: &str| t.iter().find(|(x, _)| x.contains(s)).map(|(_, p)| p.y);
        for w in [700.0, 1000.0, 1235.0] {
            let t = render(w, true, false);
            let q = y_of(&t, "Delete your saved").unwrap();
            let open = y_of(&t, "Open map editor").unwrap();
            assert!(q > open, "confirm block starts below the button row at {w} px");
            let reset_btn = t.iter().filter(|(x, _)| x == "Reset").map(|(_, p)| p.y).last().unwrap();
            assert!((q - reset_btn).abs() < 6.0, "question and Reset on one row at {w} px: {q} vs {reset_btn}");
            let (a, b) = (render(w, false, false), render(w, false, true));
            assert_eq!(y_of(&a, "Open map editor"), y_of(&b, "Open map editor"), "buttons jumped at {w} px");
        }
    }
}
