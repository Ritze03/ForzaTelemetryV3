//! The Map tab's "Map data" page (moved here from Setup, D67): open the road-type map editor
//! (built from the user's own install), see which road types the app uses, reset / rebuild /
//! contribute. See `docs/features/map-editor.md`.
//!
//! *Why it moved:* Setup was bloated, and the map editor belongs with the maps (the Map tab's
//! settings mode, `map_tab`). The code and its pane tests are unchanged; only the host differs.

use egui::{RichText, Ui};

use crate::app::ForzaApp;
use crate::i18n::tr;
use crate::ui::overlay_tab::control_row_tip;
use crate::ui::settings::{result_line, Dot};

/// The page: the one "Map data" card, in the first column like the Overlay tab's single-card
/// pages (three columns from 1100 px, else two), so it is as wide as it was in Setup.
pub fn page(ui: &mut Ui, app: &mut ForzaApp) {
    let n = if ui.available_width() >= crate::ui::overlay_tab::THREE_COLS_MIN_W { 3 } else { 2 };
    ui.spacing_mut().item_spacing.x = 8.0; // inter-column gap
    crate::theme::columns(ui, n, |cols| {
        cols[0].spacing_mut().item_spacing.y = 0.0; // card() owns the 8px inter-card gap
        crate::theme::card(&mut cols[0], tr("Map data"), |ui| map_data_card(ui, app));
    });
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

/// Map tab → Map data state: the road-type status (computed on a thread while no editor server
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

    // The install check normally runs from Setup → Game Install; run it here too, so this page
    // works when Setup was never opened (the state is shared, so it is one check, not two).
    app.fh6_setup.poll(ui.ctx(), &app.config.fh6_install_dir, false);
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
        // Compares English texts: hold the language lock (see `i18n::with_language`).
        crate::i18n::with_language(crate::i18n::Language::English, confirm_is_one_unit);
    }

    fn confirm_is_one_unit() {
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
