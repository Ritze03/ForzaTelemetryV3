//! Windows OpenGL for the overlay: a plain WGL context made current on the overlay thread,
//! with every frame rendered into an offscreen FBO and read back as BGRA (see `win32.rs`).
//!
//! *Why offscreen + readback and not a GL window surface:* a per-pixel-alpha window on Windows
//! is a layered window fed by `UpdateLayeredWindow`; an OpenGL window surface can't carry
//! per-pixel alpha through DWM without a DXGI composition swap chain (a lot of new GPU code).
//! Rendering to an FBO lets the existing `Renderer` (egui_glow) run unchanged, and
//! `glReadPixels(BGRA)` straight into the DIB section's memory is exactly the premultiplied
//! BGRA that `UpdateLayeredWindow(ULW_ALPHA | AC_SRC_ALPHA)` wants, with no CPU conversion.
//! Cost: one blocking readback per frame (8 MB at 1080p); fine at the HUD's ~60 Hz.
//!
//! The context is a legacy `wglCreateContext` one on a hidden helper window: drivers return
//! their newest compatibility profile for it (GL 4.x on NVIDIA/AMD/Intel), enough for
//! egui_glow's `#version 140` shaders, FBOs and VAOs. The helper window's own framebuffer is
//! never drawn to, so it can stay hidden. Untested on real hardware.

use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut};
use std::sync::Arc;

use egui_glow::glow::{self, HasContext};
use windows_sys::Win32::Foundation::{HMODULE, HWND};
use windows_sys::Win32::Graphics::Gdi::{GetDC, ReleaseDC, HDC};
use windows_sys::Win32::Graphics::OpenGL::{
    wglCreateContext, wglDeleteContext, wglGetProcAddress, wglMakeCurrent, ChoosePixelFormat, SetPixelFormat, HGLRC,
    PFD_DOUBLEBUFFER, PFD_DRAW_TO_WINDOW, PFD_MAIN_PLANE, PFD_SUPPORT_OPENGL, PFD_TYPE_RGBA, PIXELFORMATDESCRIPTOR,
};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};

use super::win32::wide;

/// The offscreen colour target (RGBA8 texture on an FBO).
struct Target {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
    size: [u32; 2],
}

/// Owns the GL context. Drop order: [`Wgl::drop`] frees the FBO and deletes the context, so
/// the `Renderer` (painter) must be dropped *before* this (`Overlay`'s field order).
pub struct Wgl {
    pub glow: Arc<glow::Context>,
    target: Option<Target>,
    hglrc: HGLRC,
    hdc: HDC,
    hwnd: HWND,
}

impl Wgl {
    /// Hidden helper window, pixel format, context, made current on the calling thread.
    pub fn new() -> Result<Self, String> {
        // SAFETY: plain Win32/WGL calls on this thread; every failure path releases what it made.
        unsafe {
            // "STATIC" is a system class: no registration and no window procedure of ours.
            let class = wide("STATIC");
            let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), null_mut(), null());
            if hwnd.is_null() {
                return Err(format!("helper window: error {}", windows_sys::Win32::Foundation::GetLastError()));
            }
            let hdc = GetDC(hwnd);
            if hdc.is_null() {
                DestroyWindow(hwnd);
                return Err("GetDC failed".into());
            }
            let fail = |e: String| {
                ReleaseDC(hwnd, hdc);
                DestroyWindow(hwnd);
                Err(e)
            };
            let mut pfd: PIXELFORMATDESCRIPTOR = std::mem::zeroed();
            pfd.nSize = std::mem::size_of::<PIXELFORMATDESCRIPTOR>() as u16;
            pfd.nVersion = 1;
            pfd.dwFlags = PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER;
            pfd.iPixelType = PFD_TYPE_RGBA;
            pfd.cColorBits = 32;
            pfd.cAlphaBits = 8;
            pfd.iLayerType = PFD_MAIN_PLANE as u8;
            let format = ChoosePixelFormat(hdc, &pfd);
            if format == 0 || SetPixelFormat(hdc, format, &pfd) == 0 {
                return fail("no usable OpenGL pixel format".into());
            }
            let hglrc = wglCreateContext(hdc);
            if hglrc.is_null() {
                return fail("wglCreateContext failed (no OpenGL driver?)".into());
            }
            if wglMakeCurrent(hdc, hglrc) == 0 {
                wglDeleteContext(hglrc);
                return fail("wglMakeCurrent failed".into());
            }
            let opengl32 = LoadLibraryA(c"opengl32.dll".as_ptr().cast());
            let glow = glow::Context::from_loader_function_cstr(|name| proc_address(opengl32, name));
            let version = glow.version();
            // egui_glow needs at least GL 3.0-class features (FBO, VAO); a GDI-generic 1.1
            // context (no GPU driver, e.g. a bare VM) can't run it.
            if version.major < 3 && !version.is_embedded {
                let msg = format!("OpenGL {}.{} is too old (need a GPU driver with OpenGL 3+)", version.major, version.minor);
                wglMakeCurrent(null_mut(), null_mut());
                wglDeleteContext(hglrc);
                return fail(msg);
            }
            Ok(Self { glow: Arc::new(glow), target: None, hglrc, hdc, hwnd })
        }
    }

    /// Bind the offscreen target for a frame of `size`, (re)creating it when the size changed.
    pub fn bind_target(&mut self, size: [u32; 2]) -> Result<(), String> {
        let gl = self.glow.clone();
        if let Some(t) = self.target.as_ref().filter(|t| t.size == size) {
            // SAFETY: the context is current on this thread for the thread's lifetime.
            unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.fbo)) };
            return Ok(());
        }
        self.free_target();
        // SAFETY: as above.
        unsafe {
            let tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size[0] as i32,
                size[1] as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            gl.bind_texture(glow::TEXTURE_2D, None);
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
                return Err(format!("offscreen framebuffer incomplete (0x{status:X})"));
            }
            self.target = Some(Target { fbo, tex, size });
        }
        Ok(())
    }

    /// Read the bound target as premultiplied BGRA, rows bottom-up, into `dst`
    /// (`w * h * 4` bytes): the memory layout of a positive-height 32-bit DIB.
    pub fn read_bgra(&self, size: [u32; 2], dst: &mut [u8]) {
        debug_assert_eq!(dst.len(), size[0] as usize * size[1] as usize * 4);
        // SAFETY: the context is current; `dst` is exactly the size ReadPixels writes.
        unsafe {
            self.glow.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            self.glow.read_pixels(0, 0, size[0] as i32, size[1] as i32, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(dst)));
        }
    }

    fn free_target(&mut self) {
        if let Some(t) = self.target.take() {
            // SAFETY: the context is current.
            unsafe {
                self.glow.bind_framebuffer(glow::FRAMEBUFFER, None);
                self.glow.delete_framebuffer(t.fbo);
                self.glow.delete_texture(t.tex);
            }
        }
    }
}

impl Drop for Wgl {
    fn drop(&mut self) {
        self.free_target();
        // SAFETY: each handle is released once, here; the painter is already gone.
        unsafe {
            wglMakeCurrent(null_mut(), null_mut());
            wglDeleteContext(self.hglrc);
            ReleaseDC(self.hwnd, self.hdc);
            DestroyWindow(self.hwnd);
        }
    }
}

/// `wglGetProcAddress` only knows post-1.1 entry points (and returns small sentinel values
/// 1/2/3/-1 on some drivers for unknown ones); GL 1.1 functions come from `opengl32.dll`.
unsafe fn proc_address(opengl32: HMODULE, name: &CStr) -> *const c_void {
    // SAFETY: `name` is a valid NUL-terminated string; the context is current.
    unsafe {
        let addr = wglGetProcAddress(name.as_ptr().cast()).map_or(0usize, |f| f as usize);
        if addr > 3 && addr != usize::MAX {
            return addr as *const c_void;
        }
        GetProcAddress(opengl32, name.as_ptr().cast()).map_or(null(), |f| f as *const c_void)
    }
}
