//! The Windows overlay (experimental, written blind: nobody could run it while it was built).
//!
//! One click-through, always-on-top, transparent, non-activating **layered window** per shown
//! HUD, covering one monitor. Each frame the existing [`Renderer`] (egui through `egui_glow`)
//! draws into an offscreen FBO on a private WGL context ([`super::wgl`]); the pixels are read
//! back as premultiplied BGRA straight into a DIB section and handed to the compositor with
//! `UpdateLayeredWindow(ULW_ALPHA)`. Same public surface as the Linux `OverlayHandle`, so
//! `app.rs`, the listener's `HudSink` and the focus thread don't care which platform they run on.
//!
//! The thread loop is a Win32 message loop: `MsgWaitForMultipleObjects` on one auto-reset
//! event (the listener's wake-up *and* the command channel's doorbell) plus the window's own
//! messages, with a deadline for the animation timer and a 1 s housekeeping tick. Visibility
//! and pacing follow `wayland.rs` (same snapshot rules, D17 pacing via [`super::pacing`]).
//!
//! Window styles (`WS_POPUP` + extended):
//! - `WS_EX_LAYERED`: per-pixel alpha through `UpdateLayeredWindow`.
//! - `WS_EX_TRANSPARENT` (+ `WM_NCHITTEST` -> `HTTRANSPARENT`): mouse input falls through to the game.
//! - `WS_EX_NOACTIVATE` (+ `WM_MOUSEACTIVATE` -> `MA_NOACTIVATE`, shown with `SWP_NOACTIVATE`):
//!   never takes focus, so the game keeps keyboard/gamepad input and is never "deactivated".
//! - `WS_EX_TOPMOST`, re-asserted every second: a borderless game may re-raise itself.
//! - `WS_EX_TOOLWINDOW`: no taskbar button, not in Alt+Tab.
//!
//! Only a *borderless/windowed* game can be overlaid; exclusive fullscreen owns the display
//! and no window can be drawn above it.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM, ERROR_CLASS_ALREADY_EXISTS, WAIT_FAILED};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, HMONITOR, MONITORINFO, MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent, INFINITE};
use windows_sys::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow, MsgWaitForMultipleObjects, PeekMessageW,
    RegisterClassExW, SetWindowPos, TranslateMessage, UpdateLayeredWindow, HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, MSG, PM_REMOVE,
    QS_ALLINPUT, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, ULW_ALPHA, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use super::monitors::{pick, MonitorInfo};
use super::pacing::{next_wake, wait_ms};
use super::render::Renderer;
use super::snapshot::{HudSnapshot, SnapshotSlot};
use super::wgl::Wgl;
use super::{DisabledReason, OverlayCmd, OverlayOptions};

const CLASS: &str = "ForzaTelemetryHud";
const TITLE: &str = "forza-telemetry-hud";
/// Consecutive failed frames (window/DIB creation, `UpdateLayeredWindow`, FBO) before the
/// overlay gives up and the thread exits (the tab then reports it as stopped).
const MAX_FAILURES: u8 = 3;
/// Housekeeping period while a window exists: re-assert topmost, notice monitor changes.
const TICK: Duration = Duration::from_secs(1);

/// NUL-terminated UTF-16.
pub(super) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ── Handle: thread + doorbell ────────────────────────────────────────────────────────

/// A Win32 event handle that may be shared across threads (kernel objects are thread-safe).
struct Event(HANDLE);
// SAFETY: a HANDLE to an event object is just an id; SetEvent/Wait are thread-safe.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: closed once, when the last `Arc<Shared>` (handle, wakers, thread) is gone.
        unsafe { CloseHandle(self.0) };
    }
}

/// What the listener/UI/focus threads share with the overlay thread: one auto-reset event it
/// waits on, a "frame wanted" flag and the command queue. Held by `Arc` so the event outlives
/// every `Waker`/`OverlaySender` clone even after the thread is gone.
struct Shared {
    event: Event,
    pinged: AtomicBool,
    cmds: Mutex<VecDeque<OverlayCmd>>,
}

impl Shared {
    fn new() -> Result<Arc<Self>, String> {
        // SAFETY: auto-reset (manual = 0), initially unsignalled, unnamed.
        let h = unsafe { CreateEventW(null(), 0, 0, null()) };
        if h.is_null() {
            // SAFETY: trivial.
            return Err(format!("CreateEvent failed: error {}", unsafe { GetLastError() }));
        }
        Ok(Arc::new(Self { event: Event(h), pinged: AtomicBool::new(false), cmds: Mutex::new(VecDeque::new()) }))
    }

    fn ring(&self) {
        // SAFETY: the event handle lives as long as `self`.
        unsafe { SetEvent(self.event.0) };
    }

    fn send(&self, cmd: OverlayCmd) {
        self.cmds.lock().unwrap_or_else(|e| e.into_inner()).push_back(cmd);
        self.ring();
    }

    fn drain(&self) -> Vec<OverlayCmd> {
        self.cmds.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect()
    }
}

/// Wakes the overlay to draw one frame. Cheap, coalescing, `Send + Clone`.
#[derive(Clone)]
pub struct Waker(Arc<Shared>);

impl Waker {
    pub fn wake(&self) {
        self.0.pinged.store(true, Ordering::Release);
        self.0.ring();
    }
}

/// Cloneable command sender for threads other than the handle's owner.
#[derive(Clone)]
pub struct OverlaySender(Arc<Shared>);

impl OverlaySender {
    pub fn send(&self, cmd: OverlayCmd) {
        self.0.send(cmd);
    }
}

/// Owns the overlay thread. Dropping it shuts the thread down and joins it.
pub struct OverlayHandle {
    shared: Arc<Shared>,
    slot: SnapshotSlot,
    join: Option<JoinHandle<()>>,
    /// Set on drop; stops the dev 60 Hz pinger (`FORZA_OVERLAY_TEST=2`).
    dev_stop: Option<Arc<AtomicBool>>,
}

impl OverlayHandle {
    /// Start the overlay thread, hidden; blocks until it has a GL context (or failed), so the
    /// caller learns right away whether the overlay is usable.
    pub fn spawn(opts: OverlayOptions) -> Result<Self, DisabledReason> {
        let shared = Shared::new().map_err(DisabledReason::Win32)?;
        let slot: SnapshotSlot = Arc::new(Mutex::new(None));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (thread_shared, thread_slot) = (shared.clone(), slot.clone());
        let join = thread::Builder::new()
            .name("overlay".into())
            .spawn(move || run(opts, thread_shared, thread_slot, ready_tx))
            .map_err(|e| DisabledReason::Win32(e.to_string()))?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { shared, slot, join: Some(join), dev_stop: None }),
            Ok(Err(reason)) => {
                let _ = join.join();
                Err(reason)
            }
            Err(e) => {
                // Timed out or died during startup: tell a still-starting thread to stop.
                shared.send(OverlayCmd::Shutdown);
                Err(DisabledReason::Win32(format!("overlay thread didn't start: {e}")))
            }
        }
    }

    pub fn send(&self, cmd: OverlayCmd) {
        self.shared.send(cmd);
    }

    pub fn sender(&self) -> OverlaySender {
        OverlaySender(self.shared.clone())
    }

    pub fn waker(&self) -> Waker {
        Waker(self.shared.clone())
    }

    /// Latest-wins snapshot mailbox the overlay draws from.
    pub fn slot(&self) -> SnapshotSlot {
        self.slot.clone()
    }

    /// The thread exited on its own (repeated failures or a panic).
    pub fn is_dead(&self) -> bool {
        self.join.as_ref().is_some_and(|j| j.is_finished())
    }
}

impl Drop for OverlayHandle {
    fn drop(&mut self) {
        if let Some(stop) = &self.dev_stop {
            stop.store(true, Ordering::Relaxed);
        }
        self.send(OverlayCmd::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// `FORZA_OVERLAY_TEST` asks for the dev test pattern ([`spawn_dev_test`]), which then owns the
/// overlay. On Windows this is also the quickest way to check the overlay without the game.
pub fn dev_test_requested() -> bool {
    matches!(std::env::var("FORZA_OVERLAY_TEST").as_deref(), Ok("1" | "2"))
}

/// Dev switch: `FORZA_OVERLAY_TEST=1` shows the static test pattern on the monitor named by
/// `FORZA_OVERLAY_OUTPUT` (`DISPLAY2`, `\\.\DISPLAY2` or `2`; default: the primary monitor).
/// `=2` also wakes it at ~60 Hz with a frame counter. Keep the returned handle alive for as
/// long as the pattern should stay up. While set the app doesn't start the real HUD.
pub fn spawn_dev_test() -> Option<OverlayHandle> {
    let live = match std::env::var("FORZA_OVERLAY_TEST").as_deref() {
        Ok("1") => false,
        Ok("2") => true,
        _ => return None,
    };
    let output = std::env::var("FORZA_OVERLAY_OUTPUT").ok().filter(|s| !s.is_empty());
    match OverlayHandle::spawn(OverlayOptions { output, test_pattern: true, coop: None }) {
        Ok(mut handle) => {
            handle.send(OverlayCmd::Show);
            if live {
                let stop = Arc::new(AtomicBool::new(false));
                let (waker, thread_stop) = (handle.waker(), stop.clone());
                let spawned = thread::Builder::new().name("overlay-dev-ping".into()).spawn(move || {
                    while !thread_stop.load(Ordering::Relaxed) {
                        waker.wake();
                        thread::sleep(Duration::from_micros(16_667));
                    }
                });
                match spawned {
                    Ok(_) => handle.dev_stop = Some(stop),
                    Err(e) => eprintln!("overlay: dev pinger didn't start: {e}"),
                }
            }
            Some(handle)
        }
        Err(reason) => {
            eprintln!("overlay disabled: {reason}");
            None
        }
    }
}

// ── Monitors ─────────────────────────────────────────────────────────────────────────

unsafe extern "system" fn enum_monitor(hmon: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> i32 {
    // SAFETY: `out` is the `&mut Vec<MonitorInfo>` that `monitors()` passed in.
    let list = unsafe { &mut *(out as *mut Vec<MonitorInfo>) };
    if let Some(m) = unsafe { monitor_info(hmon) } {
        list.push(m);
    }
    1 // continue
}

/// `HMONITOR` -> [`MonitorInfo`] (physical pixels when the calling thread is per-monitor aware).
unsafe fn monitor_info(hmon: HMONITOR) -> Option<MonitorInfo> {
    // SAFETY: `mi` is zeroed with the right cbSize; GetMonitorInfoW fills it.
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, (&mut mi as *mut MONITORINFOEXW).cast::<MONITORINFO>()) == 0 {
            return None;
        }
        let r = mi.monitorInfo.rcMonitor;
        let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(mi.szDevice.len());
        Some(MonitorInfo {
            name: String::from_utf16_lossy(&mi.szDevice[..len]),
            x: r.left,
            y: r.top,
            w: (r.right - r.left).max(0) as u32,
            h: (r.bottom - r.top).max(0) as u32,
            primary: mi.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
        })
    }
}

fn monitors() -> Vec<MonitorInfo> {
    let mut list: Vec<MonitorInfo> = Vec::new();
    // SAFETY: the callback only runs during this call, with `list` alive.
    unsafe { EnumDisplayMonitors(null_mut(), null(), Some(enum_monitor), (&mut list as *mut Vec<MonitorInfo>) as LPARAM) };
    list
}

/// GDI name (`\\.\DISPLAY2`) of the monitor the foreground window is on: the monitor the game
/// is on while it is focused. Used by the focus thread's monitor detection
/// (`focus::query_monitor`). `None` when there is no foreground window.
pub fn foreground_monitor_name() -> Option<String> {
    // SAFETY: plain queries; a stale HWND just yields the nearest monitor or `None`.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        monitor_info(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST)).map(|m| m.name)
    }
}

// ── The window ───────────────────────────────────────────────────────────────────────

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Belt and braces next to WS_EX_TRANSPARENT: hit-testing says "not me", so the click
        // goes to whatever is underneath (the game).
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // SAFETY: forwarding the unhandled message untouched.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// Register the window class (idempotent: a second overlay in the same process finds it there).
fn register_class() -> Result<(), String> {
    let class = wide(CLASS);
    // SAFETY: `wc` is fully initialised and `class` outlives the call (the name is copied).
    unsafe {
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = GetModuleHandleW(null());
        wc.lpszClassName = class.as_ptr();
        if RegisterClassExW(&wc) == 0 {
            let e = GetLastError();
            if e != ERROR_CLASS_ALREADY_EXISTS {
                return Err(format!("RegisterClassEx failed: error {e}"));
            }
        }
    }
    Ok(())
}

/// The mapped HUD: a layered window plus the DIB section its frames are read back into.
/// Field order is irrelevant; [`Drop`] releases in the one safe order.
struct Live {
    hwnd: HWND,
    mem_dc: HDC,
    bmp: HBITMAP,
    old_bmp: HGDIOBJ,
    /// The DIB's pixels: `mon.w * mon.h * 4` bytes, bottom-up premultiplied BGRA.
    bits: *mut u8,
    mon: MonitorInfo,
    /// Shown (after the first frame is in), so the window never flashes empty.
    shown: bool,
}

impl Live {
    fn create(mon: MonitorInfo) -> Result<Self, String> {
        let (w, h) = (mon.w as i32, mon.h as i32);
        if w <= 0 || h <= 0 {
            return Err(format!("monitor {} has no size", mon.name));
        }
        let class = wide(CLASS);
        let title = wide(TITLE);
        // SAFETY: standard window/DIB creation; every failure path frees what was made so far.
        unsafe {
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
            let hwnd = CreateWindowExW(ex, class.as_ptr(), title.as_ptr(), WS_POPUP, mon.x, mon.y, w, h, null_mut(), null_mut(), GetModuleHandleW(null()), null());
            if hwnd.is_null() {
                return Err(format!("CreateWindowEx failed: error {}", GetLastError()));
            }
            let mem_dc = CreateCompatibleDC(null_mut());
            if mem_dc.is_null() {
                DestroyWindow(hwnd);
                return Err("CreateCompatibleDC failed".into());
            }
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: h, // positive = bottom-up, the row order glReadPixels produces
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let mut bits: *mut c_void = null_mut();
            let bmp = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(mem_dc);
                DestroyWindow(hwnd);
                return Err(format!("CreateDIBSection failed: error {}", GetLastError()));
            }
            let old_bmp = SelectObject(mem_dc, bmp);
            Ok(Self { hwnd, mem_dc, bmp, old_bmp, bits: bits.cast(), mon, shown: false })
        }
    }

    fn size(&self) -> [u32; 2] {
        [self.mon.w, self.mon.h]
    }

    /// The DIB's pixels, for the GL readback.
    fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: `bits` is the DIB section's w*h*4 bytes, alive as long as `self`.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.mon.w as usize * self.mon.h as usize * 4) }
    }

    /// Hand the DIB to the compositor (also positions and sizes the window on its monitor).
    fn present(&mut self) -> Result<(), String> {
        let dst = POINT { x: self.mon.x, y: self.mon.y };
        let src = POINT { x: 0, y: 0 };
        let size = windows_sys::Win32::Foundation::SIZE { cx: self.mon.w as i32, cy: self.mon.h as i32 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        // SAFETY: all pointers are to locals/our DC for the duration of the call.
        unsafe {
            if UpdateLayeredWindow(self.hwnd, null_mut(), &dst, &size, self.mem_dc, &src, 0, &blend, ULW_ALPHA) == 0 {
                return Err(format!("UpdateLayeredWindow failed: error {}", GetLastError()));
            }
            if !self.shown {
                // Topmost, no activation: the game keeps focus.
                SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                self.shown = true;
            }
        }
        Ok(())
    }

    /// A borderless game may raise itself above us; ask for the top of the topmost band again.
    fn reassert_topmost(&self) {
        // SAFETY: trivial.
        unsafe { SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // SAFETY: each handle is released once; the bitmap must be deselected before deleting.
        unsafe {
            SelectObject(self.mem_dc, self.old_bmp);
            DeleteObject(self.bmp);
            DeleteDC(self.mem_dc);
            DestroyWindow(self.hwnd);
        }
    }
}

// ── The thread ───────────────────────────────────────────────────────────────────────

struct Overlay {
    // Drop order = declaration order (after `Drop::drop` released `live`): the window first,
    // then the painter (needs the context current), then the context.
    live: Option<Live>,
    renderer: Renderer,
    gl: Wgl,
    shared: Arc<Shared>,
    slot: SnapshotSlot,
    latest: Option<HudSnapshot>,
    /// A window is wanted. Test pattern: set by `Show`/`Hide`. HUD: set by a visible
    /// snapshot, cleared once the fade-out has finished.
    visible: bool,
    /// Monitor to cover (`\\.\DISPLAY2`, `DISPLAY2` or `2`, see `monitors::pick`); `None` = primary.
    target: Option<String>,
    test_pattern: bool,
    dirty: bool,
    last_ping: Option<Instant>,
    /// When the animation fallback timer fires (see [`next_wake`]).
    anim_at: Option<Instant>,
    /// Next housekeeping tick; only while a window exists.
    tick_at: Option<Instant>,
    failures: u8,
    exit: bool,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.live = None; // release the window before the painter and context go
    }
}

/// Thread body. Reports startup success/failure on `ready`, then runs until Shutdown.
fn run(opts: OverlayOptions, shared: Arc<Shared>, slot: SnapshotSlot, ready: mpsc::Sender<Result<(), DisabledReason>>) {
    // Per-monitor-v2 DPI awareness for *this thread's* windows (Windows 10 1703+), whatever the
    // process default is: without it Windows would virtualise our coordinates and stretch the
    // bitmap (blurry, and a 4K monitor would report 1920x1080). Failure is harmless.
    // SAFETY: trivial call.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    let init = || -> Result<Overlay, DisabledReason> {
        register_class().map_err(DisabledReason::Win32)?;
        let gl = Wgl::new().map_err(DisabledReason::Wgl)?;
        let renderer = Renderer::new(gl.glow.clone(), opts.coop.clone()).map_err(DisabledReason::Wgl)?;
        Ok(Overlay {
            live: None,
            renderer,
            gl,
            shared: shared.clone(),
            slot,
            latest: None,
            visible: false,
            target: opts.output.clone(),
            test_pattern: opts.test_pattern,
            dirty: false,
            last_ping: None,
            anim_at: None,
            tick_at: None,
            failures: 0,
            exit: false,
        })
    };
    let mut overlay = match init() {
        Ok(o) => o,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    overlay.run_loop();
}

impl Overlay {
    fn run_loop(&mut self) {
        while !self.exit {
            let timeout = [self.anim_at, self.tick_at]
                .into_iter()
                .flatten()
                .min()
                .map_or(INFINITE, |d| wait_ms(d.saturating_duration_since(Instant::now())));
            // SAFETY: one valid event handle; wakes on it, on our window's messages, or on timeout.
            let r = unsafe { MsgWaitForMultipleObjects(1, &self.shared.event.0, 0, timeout, QS_ALLINPUT) };
            if r == WAIT_FAILED {
                eprintln!("overlay: wait failed: error {}", unsafe { GetLastError() });
                break;
            }
            pump_messages();
            for cmd in self.shared.drain() {
                self.command(cmd);
            }
            if self.exit {
                break;
            }
            let now = Instant::now();
            if self.shared.pinged.swap(false, Ordering::AcqRel) {
                self.last_ping = Some(now);
                self.anim_at = None; // packets are driving frames again
                self.dirty = true;
                self.follow_snapshot();
                self.maybe_render();
            }
            if self.anim_at.is_some_and(|d| now >= d) {
                self.anim_at = None;
                self.dirty = true;
                self.maybe_render();
            }
            if self.tick_at.is_some_and(|d| now >= d) {
                self.housekeeping();
            }
        }
    }

    fn command(&mut self, cmd: OverlayCmd) {
        match cmd {
            OverlayCmd::Show => {
                self.visible = true;
                self.ensure_window();
                self.dirty = true;
                self.maybe_render();
            }
            OverlayCmd::Hide => {
                self.visible = false;
                self.destroy_window();
            }
            OverlayCmd::SetOutput(name) => {
                self.target = name;
                self.retarget();
            }
            OverlayCmd::Shutdown => self.exit = true,
        }
    }

    /// The monitor the window should be on now.
    fn wanted_monitor(&self) -> Option<MonitorInfo> {
        let all = monitors();
        pick(self.target.as_deref(), &all).map(|i| all[i].clone())
    }

    /// A window can't change monitor cheaply (the DIB is monitor-sized): recreate it when the
    /// wanted monitor differs from the one it covers (new target, plugged/unplugged, resolution).
    fn retarget(&mut self) {
        if let Some(live) = &self.live {
            if self.wanted_monitor().as_ref() != Some(&live.mon) {
                self.destroy_window();
            }
        }
        self.ensure_window();
        self.dirty = true;
        self.maybe_render();
    }

    /// Create the window if shown and missing. It appears with the first frame (`present`).
    fn ensure_window(&mut self) {
        if !self.visible || self.live.is_some() {
            return;
        }
        let Some(mon) = self.wanted_monitor() else { return };
        match Live::create(mon) {
            Ok(live) => {
                self.live = Some(live);
                self.tick_at = Some(Instant::now() + TICK);
            }
            Err(e) => self.failed(&e),
        }
    }

    /// Unmap by destroying, like the Linux surfaces: a topmost window over a borderless game
    /// can cost it the independent-flip fast path, so a hidden HUD has no window at all. The
    /// GL context and painter stay, so a re-show re-uploads nothing.
    fn destroy_window(&mut self) {
        self.live = None;
        self.tick_at = None;
    }

    fn refresh_snapshot(&mut self) {
        if let Ok(slot) = self.slot.try_lock() {
            self.latest.clone_from(&slot);
        }
    }

    fn target_visible(&self) -> bool {
        self.latest.as_ref().is_some_and(|s| s.visible)
    }

    /// HUD mode, on each wake: the window follows `snapshot.visible` (see `wayland.rs`).
    fn follow_snapshot(&mut self) {
        if self.test_pattern {
            return;
        }
        self.refresh_snapshot();
        if self.target_visible() {
            self.visible = true;
            self.ensure_window();
        } else if self.live.is_none() {
            self.visible = false;
        }
    }

    fn maybe_render(&mut self) {
        if self.exit || !self.dirty {
            return;
        }
        self.refresh_snapshot();
        let Some(live) = self.live.as_mut() else { return };
        let size = live.size();
        let result = self.gl.bind_target(size).and_then(|()| {
            let animating = self.renderer.frame(size, self.latest.as_ref(), self.test_pattern);
            self.gl.read_bgra(size, live.pixels());
            live.present().map(|()| animating)
        });
        match result {
            Ok(animating) => {
                self.dirty = false;
                self.failures = 0;
                if !self.test_pattern && !animating && !self.target_visible() {
                    // Fade-out finished: a hidden HUD has no window.
                    self.visible = false;
                    self.anim_at = None;
                    self.destroy_window();
                    return;
                }
                self.anim_at = next_wake(animating, self.last_ping.map(|t| t.elapsed())).map(|d| Instant::now() + d);
            }
            Err(e) => self.failed(&e),
        }
    }

    /// Count a failure. Below the cap, drop the window so the next wake rebuilds it; at the cap,
    /// stop the thread (`is_dead()`; the tab reports the overlay as stopped).
    fn failed(&mut self, e: &dyn std::fmt::Display) {
        self.failures += 1;
        if self.failures >= MAX_FAILURES {
            eprintln!("overlay: {e}; {MAX_FAILURES} failures in a row, shutting the overlay down");
            self.exit = true;
            return;
        }
        eprintln!("overlay: {e}; recreating the window");
        self.destroy_window();
    }

    /// Once a second while a window exists.
    fn housekeeping(&mut self) {
        self.tick_at = Some(Instant::now() + TICK);
        if let Some(live) = &self.live {
            live.reassert_topmost();
            if self.wanted_monitor().as_ref() != Some(&live.mon) {
                self.retarget();
            }
        }
    }
}

fn pump_messages() {
    // SAFETY: standard message pump for this thread's window.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
