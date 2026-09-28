//! Shared "is the game the focused window?" detector. One poll thread updates a
//! cached AtomicBool at the configured rate; the hotkey gate and the synthetic-
//! input gate both read it. Fail-open: if a query errors, we report focused=true
//! and surface a red status, so the feature never silently blocks. See spec.
//!
//! The same thread also runs the HUD overlay's **monitor detection** (D18) while the
//! overlay is on: it takes the monitor only while the game is the focused window and
//! pushes changes to the overlay through an [`OutputSink`]. It runs here, never on the UI
//! thread, because the UI loop stops while the game covers the window.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::{FocusMethod, MonitorMethod};

/// Status shown by the settings light.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusStatus { Ok = 0, ToolMissing = 1, QueryFailed = 2, Idle = 3 }

/// Case-insensitive substring match; an empty needle never matches.
pub fn window_matches(active: &str, needle: &str) -> bool {
    !needle.is_empty() && active.to_lowercase().contains(&needle.to_lowercase())
}

/// Settings the poll thread reads each tick (cheap clone via Arc<Mutex>).
#[derive(Clone, PartialEq)]
pub struct FocusParams {
    pub method: FocusMethod,
    pub custom_cmd: String,
    pub game_match: String,
    pub poll_hz: f32,
    pub enabled: bool, // false → thread idles and reports focused=true
    /// Overlay monitor detection; `None` = off (overlay disabled).
    pub monitor: Option<MonitorParams>,
}

/// How the overlay's monitor is found (D18); mirrors `OverlayConfig::monitor_*`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MonitorParams {
    pub method: MonitorMethod,
    /// [`MonitorMethod::Custom`]: run through `sh -c`, prints a monitor name.
    pub cmd: String,
    /// [`MonitorMethod::Fixed`]: output name; empty = the first output.
    pub fixed: String,
}

/// Receives the overlay's target output (`None` = the first output) whenever it changes.
/// `app.rs` wraps `OverlaySender::send(OverlayCmd::SetOutput(..))`; must not block.
pub type OutputSink = Box<dyn Fn(Option<String>) + Send>;

/// Monitor detection state, for the Overlay tab's status dot (I9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MonitorStatus {
    /// Detection off (overlay disabled).
    Idle,
    /// Hyprland/Custom: the game isn't the focused window, so the monitor isn't taken.
    WaitingForGame,
    /// Last detection succeeded.
    Ok,
    /// Last detection failed (tool missing, command error, unparsable output); the last
    /// known output is kept. See [`FocusDetector::monitor_error`].
    Failed,
}

struct MonitorShared {
    status: MonitorStatus,
    error: Option<String>,
    /// Last detected output: `None` = nothing detected yet, `Some(None)` = the first output.
    detected: Option<Option<String>>,
    /// What the sink was last given; reset when a new sink is attached so it gets the
    /// current output on the next tick.
    sent: Option<Option<String>>,
    sink: Option<OutputSink>,
}

/// Cached detector state shared with consumers.
pub struct FocusDetector {
    focused: Arc<AtomicBool>,
    status: Arc<AtomicU8>,
    params: Arc<Mutex<FocusParams>>,
    monitor: Arc<Mutex<MonitorShared>>,
}

impl FocusDetector {
    pub fn new(params: FocusParams) -> Self {
        let focused = Arc::new(AtomicBool::new(true)); // fail-open default
        let status = Arc::new(AtomicU8::new(FocusStatus::Idle as u8));
        let params = Arc::new(Mutex::new(params));
        let monitor = Arc::new(Mutex::new(MonitorShared {
            status: MonitorStatus::Idle,
            error: None,
            detected: None,
            sent: None,
            sink: None,
        }));
        let d = FocusDetector {
            focused: focused.clone(),
            status: status.clone(),
            params: params.clone(),
            monitor: monitor.clone(),
        };
        thread::spawn(move || poll_loop(focused, status, params, monitor));
        d
    }

    /// Latest cached "game focused?" (fail-open true when disabled/erroring).
    pub fn focused(&self) -> bool { self.focused.load(Ordering::Relaxed) }
    pub fn status(&self) -> FocusStatus {
        match self.status.load(Ordering::Relaxed) {
            0 => FocusStatus::Ok, 1 => FocusStatus::ToolMissing,
            2 => FocusStatus::QueryFailed, _ => FocusStatus::Idle,
        }
    }
    /// Push updated params (called when settings change).
    pub fn set_params(&self, p: FocusParams) { *self.params.lock().unwrap() = p; }

    /// One-shot active-window query for the Detect button / Custom preview.
    /// Returns the active window name or an error string.
    pub fn query_now(&self) -> Result<String, String> {
        let p = self.params.lock().unwrap().clone();
        query_active_window(p.method, &p.custom_cmd)
    }

    /// Attach (`Some`) or detach the overlay's output sink. A new sink is sent the last
    /// known output on the next poll tick.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))] // the overlay is Linux-only
    pub fn set_output_sink(&self, sink: Option<OutputSink>) {
        let mut m = lock(&self.monitor);
        m.sink = sink;
        m.sent = None;
    }

    /// Last detected overlay output (`None` = none yet, or the first output).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn monitor_output(&self) -> Option<String> {
        lock(&self.monitor).detected.clone().flatten()
    }

    pub fn monitor_status(&self) -> MonitorStatus {
        lock(&self.monitor).status
    }

    /// Why the last detection failed (with [`MonitorStatus::Failed`]).
    pub fn monitor_error(&self) -> Option<String> {
        lock(&self.monitor).error.clone()
    }
}

/// Poison-tolerant lock: the guarded state stays valid even if a holder panicked.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn poll_loop(
    focused: Arc<AtomicBool>,
    status: Arc<AtomicU8>,
    params: Arc<Mutex<FocusParams>>,
    monitor: Arc<Mutex<MonitorShared>>,
) {
    loop {
        let p = params.lock().unwrap().clone();
        if !p.enabled {
            focused.store(true, Ordering::Relaxed);
            status.store(FocusStatus::Idle as u8, Ordering::Relaxed);
            monitor_idle(&monitor);
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        // The real answer, not the fail-open one: monitor detection must only act on a
        // confirmed "the game is focused".
        let game_focused = match query_active_window(p.method, &p.custom_cmd) {
            Ok(name) => {
                let m = window_matches(&name, &p.game_match);
                focused.store(m, Ordering::Relaxed);
                status.store(FocusStatus::Ok as u8, Ordering::Relaxed);
                m
            }
            Err(_) => {
                // Fail-open: allow input/hotkeys, but flag the failure.
                focused.store(true, Ordering::Relaxed);
                status.store(FocusStatus::QueryFailed as u8, Ordering::Relaxed);
                false
            }
        };
        match &p.monitor {
            Some(m) => monitor_tick(&monitor, m, game_focused),
            None => monitor_idle(&monitor),
        }
        let hz = p.poll_hz.clamp(1.0, 20.0);
        thread::sleep(Duration::from_secs_f32(1.0 / hz));
    }
}

fn monitor_idle(monitor: &Mutex<MonitorShared>) {
    let mut s = lock(monitor);
    s.status = MonitorStatus::Idle;
    s.error = None;
}

/// One detection step (D18). Fixed always applies; Hyprland/Custom only run while the game
/// is focused, since only then is the focused monitor the game's monitor. A failure keeps
/// the last known output. The sink hears only changes.
fn monitor_tick(monitor: &Mutex<MonitorShared>, m: &MonitorParams, game_focused: bool) {
    let result = match m.method {
        MonitorMethod::Fixed => Some(Ok(Some(m.fixed.trim().to_string()).filter(|s| !s.is_empty()))),
        _ if !game_focused => None,
        method => Some(query_monitor(method, &m.cmd).map(Some)),
    };
    let mut s = lock(monitor);
    match result {
        None => s.status = MonitorStatus::WaitingForGame,
        Some(Ok(name)) => {
            s.status = MonitorStatus::Ok;
            s.error = None;
            s.detected = Some(name);
        }
        Some(Err(e)) => {
            // Logged once per distinct error, not every poll.
            if s.error.as_deref() != Some(e.as_str()) {
                eprintln!("overlay monitor detection: {e}");
            }
            s.status = MonitorStatus::Failed;
            s.error = Some(e);
        }
    }
    if s.detected.is_some() && s.detected != s.sent {
        if let Some(sink) = &s.sink {
            sink(s.detected.clone().flatten());
            s.sent = s.detected.clone();
        }
    }
}

/// First line of `hyprctl activeworkspace`, e.g. `workspace ID 1 (1) on monitor DP-1:` →
/// `DP-1`. `None` for anything else.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_hyprland_monitor(out: &str) -> Option<String> {
    let line = out.lines().next()?;
    let (_, rest) = line.rsplit_once(" on monitor ")?;
    let name = rest.trim_end().strip_suffix(':')?.trim();
    (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| name.to_string())
}

/// A custom command's stdout → monitor name: its first non-blank line, trimmed.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn trim_monitor_name(out: &str) -> Option<String> {
    out.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

/// Run the Hyprland/Custom monitor query (Fixed never gets here). Also the Overlay tab's
/// synchronous Test / Detect.
pub fn query_monitor(method: MonitorMethod, cmd: &str) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let out = match method {
            MonitorMethod::Custom => {
                if cmd.trim().is_empty() {
                    return Err("empty monitor command".into());
                }
                Command::new("sh").arg("-c").arg(cmd).output().map_err(|e| format!("custom: {e}"))?
            }
            _ => Command::new("hyprctl").arg("activeworkspace").output().map_err(|e| format!("hyprctl: {e}"))?,
        };
        if !out.status.success() {
            return Err(format!("monitor command exited {}", out.status));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let name = match method {
            MonitorMethod::Custom => trim_monitor_name(&text),
            _ => parse_hyprland_monitor(&text),
        };
        name.ok_or_else(|| format!("no monitor name in {:?}", text.lines().next().unwrap_or("")))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (method, cmd);
        Err("unsupported platform".into())
    }
}

/// Run the platform/method query, returning the active window's name/class.
pub fn query_active_window(method: FocusMethod, custom_cmd: &str) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    { linux_query(method, custom_cmd) }
    #[cfg(target_os = "windows")]
    { let _ = (method, custom_cmd); windows_query() }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    { let _ = (method, custom_cmd); Err("unsupported platform".into()) }
}

#[cfg(target_os = "linux")]
fn linux_query(method: FocusMethod, custom_cmd: &str) -> Result<String, String> {
    use std::process::Command;
    let run = |cmd: &str, args: &[&str]| -> Result<String, String> {
        Command::new(cmd).args(args).output()
            .map_err(|e| format!("{cmd}: {e}"))
            .and_then(|o| if o.status.success() {
                Ok(String::from_utf8_lossy(&o.stdout).into_owned())
            } else {
                Err(format!("{cmd} exited {}", o.status))
            })
    };
    match method {
        FocusMethod::Hyprland => {
            let out = run("hyprctl", &["activewindow", "-j"])?;
            // Pull "class" and "title" values without a JSON dep.
            let field = |k: &str| out.split(&format!("\"{k}\""))
                .nth(1).and_then(|s| s.split('"').nth(1)).unwrap_or("").to_string();
            Ok(format!("{} {}", field("class"), field("title")))
        }
        FocusMethod::X11 => {
            let root = run("xdotool", &["getactivewindow", "getwindowname"]);
            // Prefer xdotool if present; fall back to xprop.
            match root {
                Ok(s) => Ok(s),
                Err(_) => {
                    let id_line = run("xprop", &["-root", "_NET_ACTIVE_WINDOW"])?;
                    let id = id_line.rsplit(' ').next().unwrap_or("").trim().to_string();
                    let props = run("xprop", &["-id", &id, "WM_CLASS", "_NET_WM_NAME"])?;
                    Ok(props)
                }
            }
        }
        FocusMethod::Gnome => {
            // Window Calls extension (extensions.gnome.org/extension/4724): GNOME on Wayland
            // has no built-in focused-window API. Direct gdbus, no shell: see parse_gnome_list.
            // stderr is kept (unlike `run`) so the preview can say e.g. the extension is missing.
            let o = Command::new("gdbus")
                .args(["call", "--session", "--timeout", "1", "--dest", "org.gnome.Shell",
                       "--object-path", "/org/gnome/Shell/Extensions/Windows",
                       "--method", "org.gnome.Shell.Extensions.Windows.List"])
                .output().map_err(|e| format!("gdbus: {e}"))?;
            if !o.status.success() {
                return Err(format!("gdbus exited {}: {}", o.status,
                    String::from_utf8_lossy(&o.stderr).trim()));
            }
            parse_gnome_list(&String::from_utf8_lossy(&o.stdout))
        }
        FocusMethod::Custom => {
            if custom_cmd.trim().is_empty() { return Err("empty custom command".into()); }
            let out = Command::new("sh").arg("-c").arg(custom_cmd).output()
                .map_err(|e| format!("custom: {e}"))?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err(format!("custom exited {}", out.status))
            }
        }
    }
}

/// `gdbus call … Windows.List` output → `"{wm_class} {wm_class_instance} {title}"` of the
/// focused window, or `""` when none is focused (e.g. the overview has focus).
///
/// gdbus prints a GVariant `(s)` tuple: `('[…json…]',)`. If the string contains a `'` (any
/// window title with an apostrophe), GLib switches to `"` delimiters and escapes inner `"`
/// as `\"` — which is why a naive `sed` strip breaks. So: strip the tuple, unescape the
/// GVariant string, then parse the JSON.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_gnome_list(out: &str) -> Result<String, String> {
    let bad = || format!("unexpected gdbus output: {:?}", out.chars().take(80).collect::<String>());
    let inner = out.trim().strip_prefix('(').and_then(|s| s.strip_suffix(",)")).ok_or_else(bad)?;
    let q = inner.chars().next().filter(|c| *c == '\'' || *c == '"').ok_or_else(bad)?;
    let body = inner.strip_prefix(q).and_then(|s| s.strip_suffix(q)).ok_or_else(bad)?;
    let json = gvariant_unescape(body).ok_or_else(bad)?;
    let list: Vec<serde_json::Value> =
        serde_json::from_str(&json).map_err(|e| format!("Window Calls JSON: {e}"))?;
    let Some(w) = list.iter().find(|w| w["focus"] == serde_json::Value::Bool(true)) else {
        return Ok(String::new()); // nothing focused → not the game, not an error
    };
    let f = |k: &str| w[k].as_str().unwrap_or("");
    Ok(format!("{} {} {}", f("wm_class"), f("wm_class_instance"), f("title")))
}

/// Undo GVariant text-format string escapes (`g_variant_print`). `None` on a bad escape.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn gvariant_unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' { out.push(c); continue; }
        let e = it.next()?;
        out.push(match e {
            '\\' | '\'' | '"' => e,
            'n' => '\n', 't' => '\t', 'r' => '\r',
            'b' => '\u{8}', 'f' => '\u{c}', 'v' => '\u{b}', 'a' => '\u{7}',
            'u' | 'U' => {
                let n = if e == 'u' { 4 } else { 8 };
                let hex: String = it.by_ref().take(n).collect();
                if hex.chars().count() != n { return None; }
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            _ => return None,
        });
    }
    Some(out)
}

#[cfg(target_os = "windows")]
fn windows_query() -> Result<String, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() { return Err("no foreground window".into()); }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len <= 0 { return Err("empty window title".into()); }
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_is_case_insensitive_substring() {
        assert!(window_matches("Forza Horizon 6", "forza"));
        assert!(window_matches("gamescope[123]: Forza", "Forza"));
        assert!(!window_matches("Firefox", "Forza"));
    }

    #[test]
    fn empty_match_string_never_matches() {
        // An empty game_match would match everything; treat it as "no match" so
        // hotkeys aren't accidentally allowed for every window.
        assert!(!window_matches("Forza", ""));
    }

    #[test]
    fn hyprland_monitor_from_first_line() {
        let out = "workspace ID 1 (1) on monitor DP-1:\n\tmonitorID: 0\n\twindows: 2\n";
        assert_eq!(parse_hyprland_monitor(out), Some("DP-1".into()));
        assert_eq!(parse_hyprland_monitor("workspace ID 3 (3) on monitor DP-3:"), Some("DP-3".into()));
    }

    #[test]
    fn hyprland_monitor_names_with_dashes_and_named_workspaces() {
        assert_eq!(
            parse_hyprland_monitor("workspace ID 4 (4) on monitor HDMI-A-1:\n"),
            Some("HDMI-A-1".into())
        );
        // A workspace name containing the marker: the last one is the monitor.
        assert_eq!(
            parse_hyprland_monitor("workspace ID 7 (x on monitor y) on monitor eDP-1:"),
            Some("eDP-1".into())
        );
    }

    #[test]
    fn hyprland_monitor_rejects_garbage() {
        assert_eq!(parse_hyprland_monitor(""), None);
        assert_eq!(parse_hyprland_monitor("HYPRLAND_INSTANCE_SIGNATURE not set"), None);
        assert_eq!(parse_hyprland_monitor("workspace ID 1 (1) on monitor :"), None);
        assert_eq!(parse_hyprland_monitor("workspace ID 1 (1) on monitor DP-1"), None);
        // Only the first line counts.
        assert_eq!(parse_hyprland_monitor("error\nworkspace ID 1 (1) on monitor DP-1:"), None);
    }

    #[test]
    fn custom_monitor_output_is_trimmed() {
        assert_eq!(trim_monitor_name("DP-2\n"), Some("DP-2".into()));
        assert_eq!(trim_monitor_name("  \n\t HDMI-A-1  \nextra\n"), Some("HDMI-A-1".into()));
        assert_eq!(trim_monitor_name(""), None);
        assert_eq!(trim_monitor_name(" \n\n"), None);
    }

    #[test]
    fn gnome_list_picks_focused_window() {
        let out = r#"('[{"wm_class":"steam_app_1234","wm_class_instance":"forzahorizon6.exe","title":"Forza Horizon 6","focus":true},{"wm_class":"firefox","title":"x","focus":false}]',)"#;
        let got = parse_gnome_list(&format!("{out}\n")).unwrap();
        assert_eq!(got, "steam_app_1234 forzahorizon6.exe Forza Horizon 6");
        assert!(window_matches(&got, "Forza"));
    }

    #[test]
    fn gnome_list_apostrophe_switches_to_double_quotes() {
        let out = r#"("[{\"wm_class\":\"firefox\",\"title\":\"Bob's\",\"focus\":true}]",)"#;
        // Missing wm_class_instance → "".
        assert_eq!(parse_gnome_list(out).unwrap(), "firefox  Bob's");
    }

    #[test]
    fn gnome_list_unescapes_backslash_and_unicode() {
        // JSON title `C:\x<U+0001> é` is `"C:\\x\u0001 é"`; GVariant then doubles each backslash.
        let out = r#"('[{"wm_class":"a","title":"C:\\\\x\\u0001 é","focus":true}]',)"#;
        assert_eq!(parse_gnome_list(out).unwrap(), "a  C:\\x\u{1} é");
        // A GVariant-level escape (non-printable char) decodes too.
        assert_eq!(gvariant_unescape(r"a\u00e9\tb\\").as_deref(), Some("a\u{e9}\tb\\"));
    }

    #[test]
    fn gnome_list_without_focus_is_empty_not_error() {
        assert_eq!(parse_gnome_list(r#"('[{"wm_class":"a","title":"b","focus":false}]',)"#), Ok(String::new()));
        assert_eq!(parse_gnome_list("('[]',)"), Ok(String::new()));
    }

    #[test]
    fn gnome_list_rejects_garbage() {
        assert!(parse_gnome_list("").is_err());
        assert!(parse_gnome_list("Error: GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown").is_err());
        assert!(parse_gnome_list(r#"('[]",)"#).is_err()); // mismatched delimiters
        assert!(parse_gnome_list("('not json',)").is_err());
        assert!(parse_gnome_list(r"('\q',)").is_err()); // unknown escape
        assert!(parse_gnome_list(r"('\u12',)").is_err()); // short \u
    }
}
