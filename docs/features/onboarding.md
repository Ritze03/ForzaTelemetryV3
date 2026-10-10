# First-run setup guide (onboarding, I17 / D33)

A modal, step-by-step guide that gets a new user to a working setup. Code: `src/ui/onboarding.rs`
(flow, Forza and Basics pages) plus the Setup cards it reuses (`src/ui/settings.rs`). It is
a centred window over a dimmed backdrop (same pattern as the input-permissions dialog) with a
step indicator (clickable dots + "Step n / N · Title"), **Back / Next**, **Skip** (hidden on the
last page) and a **Finish** button on the last page. The X in the corner is Skip.

*Why a modal and not a page:* it has to be there on the first frame whatever tab the app opens on,
and it must not change the tab layout. The body scrolls inside the window (max height follows the
window), so it works in small windows.

## Steps (D33 order)

1. **Game Install** — `settings::game_install_card`: the very card from Setup (Steam auto-detect,
   detect from running game, manual path, the found-install status).
2. **Input Permissions** — **Linux only** (`Step::sequence(linux)`; Windows has no such step).
   `settings::input_perm_card`: the D13 status lights, fix commands, Re-check.
3. **Window Detection** — `settings::window_test_card`: method dropdown (Linux), a **Test** button
   that reads the active window once, and the Game Window Title with **Detect**.
4. **Forza in-game setup** — text + the user's screenshot (`assets/onboarding/forza-data-out.png`,
   embedded with `include_bytes!`, decoded with `image` like `labels.rs`), the values to enter
   (Data Out **On**; IP `127.0.0.1` for the same PC, the PC's LAN address as a hint for another
   PC; port = **the app's actual `listen_port`**), and a live **Packets arriving?** light fed
   by the same `telemetry.is_connected` / `packets_per_sec` as the status bar (green when packets
   come in, amber while waiting). The page repaints every 500 ms because "disconnected" is a
   timeout, not an event.
5. **The basics** — Mini-Settings (the cog on the status bar), the Hotkey card in Setup (with an
   **Open Setup** button that also closes the guide), and where the tabs / profiles / this guide are.

*Why reuse the Setup cards:* the guide and Setup can never disagree, and a fix to one is a fix to
both. Only Window Detection got its own `window_test_card`: the Setup card hides its method / title
rows until something gates on focus and only offers Test for GNOME / Custom, which is wrong for a
first-time user who has not turned any of that on yet. The method dropdown and the title row are
shared functions (`focus_method_row`, `game_title_row`) used by both.

*Why the screenshot's port is not shown as-is:* 1337 in the screenshot is just the author's value.
The page prints the configured `listen_port` and says the game's port must equal it (change it in
Setup → Network). The LAN address uses the same UDP-connect trick as `coop::local_ip` (private
there, and `coop.rs` was out of scope), so no network crate is needed.

## First run, and why existing users do not see it

`AppConfig::onboarding_done: bool` (serde default **true**, `Default` **true**). The embedded
fresh-install default `assets/default-config.json` is the *only* place that says `false`.

- No `config.json` on disk → `AppConfig::load` parses the embedded default → `onboarding_done =
  false` → the guide opens (`ForzaApp::new`: `onboarding = Some(State::new())`).
- An existing config has no such key → the merge in `AppConfig::parse` fills it from
  `AppConfig::default()` → `true` → no guide. Same for unreadable / recovered configs (they
  fall back to the profile mirror or `Default`, never to the embedded fresh default).
- Skip / X / Finish → `onboarding::complete` sets it `true` and saves at once; the autosave
  keeps it. It never reopens by itself.
- Re-open: Setup → **Getting started** → *Open setup guide* (`State::new()`; the flag is not
  touched, so closing it again changes nothing).

*Why a flag that defaults to done, and not "config file missing"* — the loader has already created
and saved the file by the time the UI exists, and the flag has to survive a quit mid-guide (it stays
`false` until finished). *Why not in profiles:* it is machine state — listed in `EXPORT_EXCLUDE`
(never exported / imported) and `apply_profile_file` restores the live value after overlaying a
snapshot, so switching to an old profile snapshot cannot reopen the guide.

## Interaction with the permissions dialog (D13)

While the guide is open, `settings::input_perm_modal` returns early and clears
`input_perm_modal_open`: the guide's step 2 is that information, so they are never both shown, and
the dialog does not pop up the moment the guide closes either (the "fine → missing" transition it
waits for already happened). The usual rules apply again from the next launch.

## Tests

`onboarding::tests` (step order for Linux / Windows, first step, `complete`), and in
`config::tests`: `fresh_install_opens_onboarding_and_done_persists`,
`existing_config_without_the_flag_does_not_get_the_guide`,
`embedded_default_is_the_only_one_with_the_guide_open`, `a_profile_snapshot_never_reopens_the_guide`.
