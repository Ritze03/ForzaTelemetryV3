//! What the GL context offers, and whether that is enough for the 3D scene. Run once per context,
//! in the first paint callback (the one place that is guaranteed to have the right context
//! current). A failure is a message for the status line, never a panic: the maps then draw
//! the tilted 2D path.

use egui_glow::glow::{self, HasContext};

use super::Gl3dOptions;

/// What the context can do beyond the required minimum.
#[derive(Clone, Debug)]
pub struct Caps {
    /// `GL_VERSION`, for the debug log and the status line.
    pub version: String,
    pub renderer: String,
    pub es: bool,
    pub max_texture: i32,
    pub max_renderbuffer: i32,
    /// `GL_MAX_SAMPLES`: the most samples a multisampled renderbuffer may have (4 at least in GL 3.3 / ES 3.0).
    pub max_samples: i32,
    /// Anisotropic filtering (extension) and its maximum, if present.
    pub aniso: Option<f32>,
    /// `GL_TIME_ELAPSED` queries (desktop 3.3+; the ES extension is not used).
    pub timer: bool,
}

/// Is this context good enough? Required: desktop GL >= `opts.min_gl` (3.3) or ES >= `opts.min_es`
/// (3.0), a 4096 px texture (the height raster is 2752 px), a vertex texture unit. The shaders
/// compiling and the FBO being complete are checked where they are created.
pub fn probe(gl: &glow::Context, opts: &Gl3dOptions) -> Result<Caps, String> {
    let v = gl.version();
    let (major, minor) = (v.major, v.minor);
    // SAFETY: plain state queries on the current context.
    let (version, renderer, max_texture, max_renderbuffer, vtex, max_samples) = unsafe {
        (
            gl.get_parameter_string(glow::VERSION),
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE),
            gl.get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE),
            gl.get_parameter_i32(glow::MAX_VERTEX_TEXTURE_IMAGE_UNITS),
            gl.get_parameter_i32(glow::MAX_SAMPLES),
        )
    };
    let need = if v.is_embedded { opts.min_es } else { opts.min_gl };
    if (major, minor) < need {
        return Err(if v.is_embedded {
            format!("OpenGL ES {major}.{minor} is too old (needs 3.3 / ES 3.0)")
        } else {
            format!("OpenGL {major}.{minor} is too old (needs 3.3 / ES 3.0)")
        });
    }
    if max_texture < 4096 {
        return Err(format!("the GPU's largest texture is {max_texture} px (needs 4096)"));
    }
    if vtex < 1 {
        return Err("the GPU has no vertex texture units".into());
    }
    let exts = gl.supported_extensions();
    let aniso = (exts.contains("GL_EXT_texture_filter_anisotropic") || exts.contains("GL_ARB_texture_filter_anisotropic"))
        // SAFETY: MAX_TEXTURE_MAX_ANISOTROPY is valid when the extension is present.
        .then(|| unsafe { gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY) }.min(16.0))
        .filter(|a| *a >= 1.0);
    let timer = !v.is_embedded && (major, minor) >= (3, 3);
    Ok(Caps { version, renderer, es: v.is_embedded, max_texture, max_renderbuffer, max_samples, aniso, timer })
}

/// `GL_MAX_TEXTURE_MAX_ANISOTROPY` / `GL_TEXTURE_MAX_ANISOTROPY` (EXT and ARB share the values).
pub const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FF;
pub const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;
/// `GL_TIME_ELAPSED`.
pub const TIME_ELAPSED: u32 = 0x88BF;
