//! First-run onboarding guide (plan I17, decision D33): a modal, step-by-step helper that gets a
//! new user to a working setup. Opened at launch while `AppConfig::onboarding_done` is false (only
//! the embedded fresh-install default says so), and re-openable from Setup → Getting started.
//! Skipping or finishing sets the flag. See `docs/features/onboarding.md`.
//!
//! The Game Install / Input Permissions / Window Detection steps reuse the Setup cards' own
//! functions (`ui::settings`), so the guide and Setup can never disagree.

use egui::{Color32, RichText, Ui};

use crate::app::{ForzaApp, Tab};
use crate::i18n::tr;

/// One page of the guide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    GameInstall,
    InputPermissions,
    WindowDetection,
    ForzaSetup,
    Basics,
}

const ALL_STEPS: &[Step] = &[Step::GameInstall, Step::InputPermissions, Step::WindowDetection, Step::ForzaSetup, Step::Basics];
const STEPS_WITHOUT_PERMISSIONS: &[Step] = &[Step::GameInstall, Step::WindowDetection, Step::ForzaSetup, Step::Basics];

impl Step {
    /// The pages in order. The input-permissions page exists on Linux only (Windows needs none).
    pub fn sequence(linux: bool) -> &'static [Step] {
        if linux { ALL_STEPS } else { STEPS_WITHOUT_PERMISSIONS }
    }

    fn title(self) -> &'static str {
        match self {
            Step::GameInstall => tr("Game Install"),
            Step::InputPermissions => tr("Input Permissions"),
            Step::WindowDetection => tr("Window Detection"),
            Step::ForzaSetup => tr("Forza in-game setup"),
            Step::Basics => tr("The basics"),
        }
    }
}

/// Runtime state of the open guide (not persisted; `ForzaApp::onboarding` is `Some` while open).
pub struct State {
    /// Index into [`Step::sequence`].
    step: usize,
    /// The Data Out screenshot, decoded on first use.
    texture: Option<egui::TextureHandle>,
    /// This PC's LAN address for the "game on another PC" hint: `None` = not looked up yet.
    lan_ip: Option<Option<String>>,
}

impl State {
    pub fn new() -> Self {
        State { step: 0, texture: None, lan_ip: None }
    }

    fn steps() -> &'static [Step] {
        Step::sequence(cfg!(target_os = "linux"))
    }
}

/// Mark the guide finished or skipped, so it does not open again on the next launch.
pub fn complete(cfg: &mut crate::config::AppConfig) {
    cfg.onboarding_done = true;
}

/// Best-effort LAN address of this PC (the one a game on another PC would send to). The
/// "connect a UDP socket to pick the route" trick: no packet is sent. Same approach as the
/// Co-Op host's `coop::local_ip` (private there), so no network crate is needed.
fn local_lan_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback()).then(|| ip.to_string())
}

fn decode_screenshot(ctx: &egui::Context) -> egui::TextureHandle {
    const BYTES: &[u8] = include_bytes!("../../assets/onboarding/forza-data-out.png");
    let color = image::load_from_memory(BYTES)
        .map(|img| {
            let rgba = img.into_rgba8();
            let (w, h) = rgba.dimensions();
            egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw())
        })
        .unwrap_or_else(|_| egui::ColorImage::new([1, 1], vec![Color32::TRANSPARENT]));
    ctx.load_texture("onboarding-forza-data-out", color, egui::TextureOptions::LINEAR)
}

/// What the footer / header buttons asked for this frame.
enum Action {
    None,
    Back,
    Next,
    Goto(usize),
    /// Close the guide (skip or finish), optionally jumping to the Setup tab.
    Close { open_setup: bool },
}

/// Draw the guide (a modal over the whole window) if it is open.
pub fn show(ctx: &egui::Context, app: &mut ForzaApp) {
    let Some(mut st) = app.onboarding.take() else { return };
    let steps = State::steps();
    st.step = st.step.min(steps.len() - 1);
    let step = steps[st.step];
    let last = st.step + 1 == steps.len();

    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("onboarding_backdrop"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::Pos2::ZERO)
        .interactable(true)
        .show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(170));
            ui.allocate_response(screen.size(), egui::Sense::click());
        });

    let mut action = Action::None;
    egui::Window::new("onboarding_win")
        .title_bar(false)
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            let width = 700.0_f32.min(screen.width() - 48.0).max(300.0);
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Setup guide")).size(16.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::icons::TIMES).on_hover_text(tr("Skip the guide")).clicked() {
                        action = Action::Close { open_setup: false };
                    }
                });
            });
            ui.add_space(6.0);
            if let Some(i) = step_indicator(ui, steps, st.step) {
                action = Action::Goto(i);
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            let body_h = (screen.height() - 250.0).clamp(160.0, 640.0);
            egui::ScrollArea::vertical()
                .id_salt(("onboarding_scroll", st.step))
                .max_height(body_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0; // card() owns the 8px gap
                    // Leave room for the scrollbar so content is never clipped under it.
                    ui.set_width(width - 14.0);
                    if let Some(a) = page(ui, app, &mut st, step) {
                        action = a;
                    }
                });

            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if !last && ui.add(crate::theme::secondary_button(tr("Skip"))).clicked() {
                    action = Action::Close { open_setup: false };
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, next) = if last { (tr("Finish"), Action::Close { open_setup: false }) } else { (tr("Next"), Action::Next) };
                    if ui.add(crate::theme::primary_button(label)).clicked() {
                        action = next;
                    }
                    if st.step > 0 && ui.add(crate::theme::secondary_button(tr("Back"))).clicked() {
                        action = Action::Back;
                    }
                });
            });
        });

    match action {
        Action::None => {}
        Action::Back => st.step = st.step.saturating_sub(1),
        Action::Next => st.step = (st.step + 1).min(steps.len() - 1),
        Action::Goto(i) => st.step = i.min(steps.len() - 1),
        Action::Close { open_setup } => {
            complete(&mut app.config);
            app.config.save();
            if open_setup {
                app.current_tab = Tab::Settings;
            }
            return; // the guide is closed: leave `app.onboarding` as None
        }
    }
    app.onboarding = Some(st);
}

/// "Step n of N · Title" plus one clickable dot per page. Returns the page clicked, if any.
fn step_indicator(ui: &mut Ui, steps: &[Step], current: usize) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        for i in 0..steps.len() {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            let col = if i == current {
                crate::theme::ACCENT
            } else if i < current {
                crate::theme::SELBD
            } else {
                crate::theme::STROKE_MID
            };
            let r = if i == current { 6.0 } else { 4.5 };
            ui.painter().circle_filled(rect.center(), r, col);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked = Some(i);
            }
        }
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("{} {} / {}  \u{b7}  {}", tr("Step"), current + 1, steps.len(), steps[current].title()))
                .color(crate::theme::TEXT_DIM),
        );
    });
    clicked
}

/// An explanatory paragraph at the top of a page.
fn intro(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(crate::theme::TEXT_DIM)).wrap());
    ui.add_space(8.0);
}

/// One page's body. Returns an action when the page itself asks for one (the Basics page's
/// "Open Setup" button).
fn page(ui: &mut Ui, app: &mut ForzaApp, st: &mut State, step: Step) -> Option<Action> {
    match step {
        Step::GameInstall => {
            intro(ui, tr("The app reads car names and the map from your Forza Horizon 6 install. Steam installs are found automatically. Otherwise start the game and use \"Detect from running game\", or enter the folder yourself."));
            crate::theme::card(ui, tr("Game Install"), |ui| crate::ui::settings::game_install_card(ui, app));
        }
        Step::InputPermissions => {
            intro(ui, tr("Hotkeys and the gearbox / backfire key presses need access to your keyboard devices. Green means ready. If something is red, run the commands, then log out and back in and press Re-check."));
            crate::theme::card(ui, tr("Input Permissions"), |ui| crate::ui::settings::input_perm_card(ui, app));
        }
        Step::WindowDetection => {
            intro(
                ui,
                if cfg!(target_os = "linux") {
                    tr("Hotkeys and the overlay only react while the game window is in front, so the app has to read the active window. Pick the method for your desktop, then press Test: it shows the window that is active right now. Use Detect and click into the game within 3 seconds to set the game window title.")
                } else {
                    tr("Hotkeys and the overlay only react while the game window is in front. Press Test: it shows the window that is active right now. Use Detect and click into the game within 3 seconds to set the game window title.")
                },
            );
            crate::theme::card(ui, tr("Window Detection"), |ui| crate::ui::settings::window_test_card(ui, app));
        }
        Step::ForzaSetup => forza_setup_page(ui, app, st),
        Step::Basics => return basics_page(ui),
    }
    None
}

/// "Label  |  value" row for the values to type into the game.
fn value_row(ui: &mut Ui, label: &str, value: &str, hint: Option<&str>) {
    crate::ui::settings::control_row(ui, label, |ui| {
        ui.label(RichText::new(value).monospace().strong());
        if let Some(h) = hint {
            ui.label(RichText::new(h).color(crate::theme::TEXT_DIM));
        }
    });
}

fn forza_setup_page(ui: &mut Ui, app: &mut ForzaApp, st: &mut State) {
    let port = app.config.listen_port;
    intro(ui, tr("Tell the game to send its telemetry to this app. In Forza Horizon 6 open SETTINGS > HUD AND GAMEPLAY > Data Out and set the three values below. The screenshot shows how that screen looks."));

    let tex = st.texture.get_or_insert_with(|| decode_screenshot(ui.ctx())).clone();
    ui.add(egui::Image::new(&tex).max_width(ui.available_width()).corner_radius(4.0));
    ui.add_space(8.0);

    let lan = st.lan_ip.get_or_insert_with(local_lan_ip).clone();
    crate::theme::card(ui, tr("Enter in the game"), |ui| {
        value_row(ui, "Data Out", tr("On"), None);
        value_row(ui, "Data Out IP Address", "127.0.0.1", Some(tr("(game and app on the same PC)")));
        value_row(ui, "Data Out IP Port", &port.to_string(), Some(tr("(the app's listen port)")));
        ui.add_space(2.0);
        let other = match &lan {
            Some(ip) => format!("{} {ip}", tr("Game on another PC? Use this PC's address instead:")),
            None => tr("Game on another PC? Use this PC's address in your network instead.").to_string(),
        };
        ui.add(egui::Label::new(RichText::new(other).size(11.0).color(crate::theme::TEXT_DIM)).wrap());
        ui.add(
            egui::Label::new(
                RichText::new(tr("The port in the screenshot is only an example: the game's port must equal the listen port above. Change it under Setup → Network."))
                    .size(11.0)
                    .color(crate::theme::TEXT_DIM),
            )
            .wrap(),
        );
    });

    // Live check, from the same state the status bar shows.
    crate::theme::card(ui, tr("Packets arriving?"), |ui| {
        ui.horizontal(|ui| {
            if app.telemetry.is_connected {
                ui.label(RichText::new("\u{25CF}").color(crate::theme::GOOD));
                ui.label(format!("{}  {:.0} {}", tr("Connected"), app.telemetry.packets_per_sec, tr("packets per second")));
            } else {
                ui.label(RichText::new("\u{25CF}").color(crate::theme::WARN));
                ui.label(tr("Waiting for packets. Check the three values in the game, then start driving."));
            }
        });
    });
    // The status turns to "disconnected" by a timeout, not by an event, so keep polling.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
}

fn basics_page(ui: &mut Ui) -> Option<Action> {
    let mut action = None;
    intro(ui, tr("That is the setup done. A few pointers for getting around:"));
    crate::theme::card(ui, tr("Mini-Settings"), |ui| {
        ui.add(egui::Label::new(tr("The cog wheel at the right end of the status bar (bottom) opens the Mini-Settings: the settings of the tab you are on, such as a Dashboard widget's options.")).wrap());
    });
    crate::theme::card(ui, tr("Hotkeys"), |ui| {
        ui.add(egui::Label::new(tr("Setup → Hotkey lists every key binding, for example Hide HUD. Click a binding and press a key; Esc cancels, Backspace clears it.")).wrap());
        if ui.add(crate::theme::secondary_button(tr("Open Setup"))).clicked() {
            action = Some(Action::Close { open_setup: true });
        }
    });
    crate::theme::card(ui, tr("Where to find things"), |ui| {
        for line in [
            tr("Dashboard: your live telemetry widgets. Edit the layout to move or add them."),
            tr("Overlay: the in-game HUD (minimap, drive cluster, race block)."),
            tr("Map: the full-size map and all map settings."),
            tr("Setup → Profiles: save and switch whole setups."),
            tr("This guide: Setup → Getting started → Open setup guide."),
        ] {
            ui.add(egui::Label::new(format!("\u{2022} {line}")).wrap());
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_order_on_linux() {
        assert_eq!(
            Step::sequence(true),
            [Step::GameInstall, Step::InputPermissions, Step::WindowDetection, Step::ForzaSetup, Step::Basics]
        );
    }

    #[test]
    fn windows_has_no_permissions_step() {
        let s = Step::sequence(false);
        assert_eq!(s, [Step::GameInstall, Step::WindowDetection, Step::ForzaSetup, Step::Basics]);
        assert!(!s.contains(&Step::InputPermissions));
    }

    #[test]
    fn new_guide_starts_on_the_first_step() {
        assert_eq!(State::new().step, 0);
        assert_eq!(State::steps()[0], Step::GameInstall);
    }

    #[test]
    fn finishing_or_skipping_marks_it_done() {
        let mut cfg = crate::config::AppConfig::default();
        cfg.onboarding_done = false;
        complete(&mut cfg);
        assert!(cfg.onboarding_done);
    }

    #[test]
    fn every_step_has_a_title() {
        for s in Step::sequence(true) {
            assert!(!s.title().is_empty());
        }
    }

    #[test]
    fn screenshot_decodes() {
        let img = image::load_from_memory(include_bytes!("../../assets/onboarding/forza-data-out.png")).unwrap();
        assert_eq!((img.width(), img.height()), (1000, 213));
    }
}
