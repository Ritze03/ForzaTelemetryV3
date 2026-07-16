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
}
