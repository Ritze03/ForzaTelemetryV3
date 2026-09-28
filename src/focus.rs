//! Shared "is the game the focused window?" detector. One poll thread updates a
//! cached AtomicBool at the configured rate; the hotkey gate and the synthetic-
//! input gate both read it. Fail-open: if a query errors, we report focused=true
//! and surface a red status, so the feature never silently blocks. See spec.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::FocusMethod;

/// Status shown by the settings light.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusStatus { Ok = 0, ToolMissing = 1, QueryFailed = 2, Idle = 3 }

/// Case-insensitive substring match; an empty needle never matches.
pub fn window_matches(active: &str, needle: &str) -> bool {
    !needle.is_empty() && active.to_lowercase().contains(&needle.to_lowercase())
}

/// Settings the poll thread reads each tick (cheap clone via Arc<Mutex>).
#[derive(Clone)]
pub struct FocusParams {
    pub method: FocusMethod,
    pub custom_cmd: String,
    pub game_match: String,
    pub poll_hz: f32,
    pub enabled: bool, // false → thread idles and reports focused=true
}

/// Cached detector state shared with consumers.
pub struct FocusDetector {
    focused: Arc<AtomicBool>,
    status: Arc<AtomicU8>,
    params: Arc<Mutex<FocusParams>>,
}

impl FocusDetector {
    pub fn new(params: FocusParams) -> Self {
        let focused = Arc::new(AtomicBool::new(true)); // fail-open default
        let status = Arc::new(AtomicU8::new(FocusStatus::Idle as u8));
        let params = Arc::new(Mutex::new(params));
        let d = FocusDetector { focused: focused.clone(), status: status.clone(), params: params.clone() };
        thread::spawn(move || poll_loop(focused, status, params));
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
}

fn poll_loop(focused: Arc<AtomicBool>, status: Arc<AtomicU8>, params: Arc<Mutex<FocusParams>>) {
    loop {
        let p = params.lock().unwrap().clone();
        if !p.enabled {
            focused.store(true, Ordering::Relaxed);
            status.store(FocusStatus::Idle as u8, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        match query_active_window(p.method, &p.custom_cmd) {
            Ok(name) => {
                focused.store(window_matches(&name, &p.game_match), Ordering::Relaxed);
                status.store(FocusStatus::Ok as u8, Ordering::Relaxed);
            }
            Err(_) => {
                // Fail-open: allow input/hotkeys, but flag the failure.
                focused.store(true, Ordering::Relaxed);
                status.store(FocusStatus::QueryFailed as u8, Ordering::Relaxed);
            }
        }
        let hz = p.poll_hz.clamp(1.0, 20.0);
        thread::sleep(Duration::from_secs_f32(1.0 / hz));
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
