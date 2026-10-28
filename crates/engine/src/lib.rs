//! What the window and the command line share.
//!
//! - [`builtin_tools`]: the tools of this build (the composition root),
//!   including the AI denoise node with the `ai-denoise` feature (default).
//! - [`Engine`]: renders a recipe's node graph on the GPU when there is one,
//!   on the CPU otherwise, and falls back to the CPU if the GPU fails mid-way.
//! - [`downscale`]: a smaller copy of a developed image for the preview.
//! - [`to_display`]: upright 8-bit sRGB pixels for the screen.
//! - [`export_jpeg`]: upright JPEG with an sRGB profile, written atomically
//!   under a name that never overwrites an existing file.
//! - [`lens_database`] and [`profile_library`]: the lens data and the camera
//!   profiles the development stage can use on this computer.
//!
//! The first-version output render (clip to sRGB) lives in `render-cpu`;
//! this crate only decides where and in which order things run.

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

use core_color::ProfileLibrary;
use lens_db::LensDatabase;
use platform::{APP_DIR, Os};

use core_model::{ImageError, ImageF32, NodeGraph, NodeId, Orientation, Rgba, ToolId};
use tool_api::{RegistryError, ToolError, ToolRegistry};

/// Every tool compiled into this build, registered once.
///
/// # Errors
/// [`RegistryError`] if two tools share an id or a descriptor is invalid (a build bug).
pub fn builtin_tools() -> Result<ToolRegistry, RegistryError> {
    let mut r = ToolRegistry::new();
    r.register(tool_exposure::Exposure::shared())?;
    #[cfg(feature = "ai-denoise")]
    r.register(ai_denoise())?;
    Ok(r)
}

/// The built-in lens database, extended with newer lensfun files from the
/// user's `lensfun` folder when there are any. Files there that cannot be
/// read are returned as problems, so the caller can show them.
pub fn lens_database() -> (Cow<'static, LensDatabase>, Vec<String>) {
    let builtin = LensDatabase::builtin();
    let Some(dir) = platform::lensfun_dir(Os::current(), APP_DIR, |k| std::env::var_os(k)) else {
        return (Cow::Borrowed(builtin), Vec::new());
    };
    let has_xml = std::fs::read_dir(&dir).is_ok_and(|mut entries| {
        entries.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("xml"))))
    });
    if !has_xml {
        return (Cow::Borrowed(builtin), Vec::new());
    }
    let mut db = builtin.clone();
    let problems = db.add_dir(&dir);
    (Cow::Owned(db), problems)
}

/// Camera profiles (DCP) in the editor's own `profiles` folder. Other
/// programs' profile folders are not searched.
#[must_use]
pub fn profile_library() -> ProfileLibrary {
    ProfileLibrary::new(platform::camera_profile_dirs(Os::current(), APP_DIR, |k| std::env::var_os(k)))
}

/// The AI denoise tool: one instance per process, shared by every registry,
/// because it keeps the loaded network and its last outputs.
#[cfg(feature = "ai-denoise")]
fn ai_denoise() -> std::sync::Arc<tool_denoise_ai::AiDenoise> {
    static TOOL: std::sync::OnceLock<std::sync::Arc<tool_denoise_ai::AiDenoise>> = std::sync::OnceLock::new();
    std::sync::Arc::clone(TOOL.get_or_init(tool_denoise_ai::AiDenoise::from_environment))
}

/// What the AI denoise node last ran on ("GPU (DirectML), 960 px tiles"),
/// or why it could not run; `None` before its first run or without the
/// `ai-denoise` feature.
#[cfg(feature = "ai-denoise")]
#[must_use]
pub fn ai_denoise_status() -> Option<String> {
    ai_denoise().status()
}

/// Without the `ai-denoise` feature there is no AI denoise node.
#[cfg(not(feature = "ai-denoise"))]
#[must_use]
pub fn ai_denoise_status() -> Option<String> {
    None
}

/// Where a render may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineChoice {
    /// The GPU if one works, else the CPU.
    Auto,
    /// Always the CPU (for comparisons and machines with broken drivers).
    CpuOnly,
}

/// Where a render actually ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// On the GPU named here ("NVIDIA GeForce RTX 4070 · Vulkan").
    Gpu(String),
    /// On the CPU.
    Cpu,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gpu(name) => write!(f, "GPU: {name}"),
            Self::Cpu => f.write_str("CPU"),
        }
    }
}

/// Why rendering, conversion or export failed.
#[derive(Debug)]
pub enum EngineError {
    /// The graph cannot be evaluated.
    Render(render_cpu::RenderError),
    /// An image could not be built (size limits).
    Image(ImageError),
    /// Encoding or writing the file failed.
    Export(export::ExportError),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Render(e) => e.fmt(f),
            Self::Image(e) => e.fmt(f),
            Self::Export(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Render(e) => Some(e),
            Self::Image(e) => Some(e),
            Self::Export(e) => Some(e),
        }
    }
}

impl From<ImageError> for EngineError {
    fn from(e: ImageError) -> Self {
        Self::Image(e)
    }
}

impl From<export::ExportError> for EngineError {
    fn from(e: export::ExportError) -> Self {
        Self::Export(e)
    }
}

/// A rendered image and how it was made.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// Linear image in the source's working space.
    pub image: ImageF32,
    /// Tools used by the recipe but missing from this build (passed through).
    pub missing_tools: Vec<ToolId>,
    /// AI mask components whose raster is not computed: (mask node,
    /// component index). They selected nothing; the UI offers to compute them.
    pub missing_masks: Vec<(core_model::NodeId, usize)>,
    /// Where it ran.
    pub backend: Backend,
    /// Why the GPU was skipped for this render, if it failed and the CPU took over.
    pub gpu_failure: Option<String>,
    /// Nodes whose tool could not run (for example the AI denoise node
    /// without its model); they passed the image through.
    pub tool_errors: Vec<(NodeId, ToolError)>,
}

/// Renders node graphs with the built-in tools, on the GPU when possible.
///
/// Creating it may take a moment (the GPU driver starts): do it on a worker
/// thread. It is `Send + Sync`; share it with an `Arc`.
#[derive(Debug)]
pub struct Engine {
    registry: ToolRegistry,
    gpu: Option<gpu::GpuRenderer>,
    gpu_problem: Option<String>,
}

impl Engine {
    /// An engine with the given tools.
    #[must_use]
    pub fn new(registry: ToolRegistry, choice: EngineChoice) -> Self {
        let (gpu, gpu_problem) = match choice {
            EngineChoice::CpuOnly => (None, None),
            EngineChoice::Auto => match gpu::GpuContext::new() {
                Ok(ctx) => (Some(gpu::GpuRenderer::new(std::sync::Arc::new(ctx))), None),
                Err(e) => (None, Some(e.to_string())),
            },
        };
        Self { registry, gpu, gpu_problem }
    }

    /// The tools this engine renders with.
    #[must_use]
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// Where renders run by default.
    #[must_use]
    pub fn backend(&self) -> Backend {
        self.gpu.as_ref().map_or(Backend::Cpu, |g| Backend::Gpu(g.context().adapter_name()))
    }

    /// Why no GPU is used, when one was wanted but could not start.
    #[must_use]
    pub fn gpu_problem(&self) -> Option<&str> {
        self.gpu_problem.as_deref()
    }

    /// Evaluates `graph` on `source`. A GPU failure is not an error: the CPU
    /// renders instead and [`Rendered::gpu_failure`] says why.
    ///
    /// # Errors
    /// [`EngineError::Render`] when the graph itself cannot be evaluated.
    pub fn render(&self, graph: &NodeGraph, source: &ImageF32) -> Result<Rendered, EngineError> {
        self.render_with_masks(graph, source, &masks::NoRasters)
    }

    /// Like [`Self::render`], taking the rasters of AI masks from `rasters`.
    ///
    /// # Errors
    /// [`EngineError::Render`] when the graph itself cannot be evaluated.
    pub fn render_with_masks(
        &self,
        graph: &NodeGraph,
        source: &ImageF32,
        rasters: &dyn masks::RasterSource,
    ) -> Result<Rendered, EngineError> {
        let mut gpu_failure = None;
        if let Some(g) = &self.gpu {
            match g.render_with_masks(graph, &self.registry, source, rasters) {
                Ok(r) => {
                    return Ok(Rendered {
                        image: r.image,
                        missing_tools: r.missing_tools,
                        missing_masks: r.missing_masks,
                        backend: Backend::Gpu(g.context().adapter_name()),
                        gpu_failure: None,
                        tool_errors: r.tool_errors,
                    });
                }
                Err(e) => gpu_failure = Some(e.to_string()),
            }
        }
        let r = render_cpu::render_with_masks(graph, &self.registry, source, rasters).map_err(EngineError::Render)?;
        Ok(Rendered {
            image: r.image,
            missing_tools: r.missing_tools,
            missing_masks: r.missing_masks,
            backend: Backend::Cpu,
            gpu_failure,
            tool_errors: r.tool_errors,
        })
    }
}

/// 8-bit sRGB pixels ready for the screen: `R G B` per pixel, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb8Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 3` bytes.
    pub pixels: Vec<u8>,
}

/// A copy of `image` whose longer side is at most `max_side`, made by
/// averaging square blocks of pixels (a box low-pass before decimation, so
/// fine detail does not alias). Returns a plain copy when it already fits.
///
/// # Errors
/// [`ImageError`] only if `max_side` is 0.
pub fn downscale(image: &ImageF32, max_side: u32) -> Result<ImageF32, ImageError> {
    if max_side == 0 {
        return Err(ImageError::Empty);
    }
    let (w, h) = (image.width(), image.height());
    let factor = w.max(h).div_ceil(max_side).max(1);
    if factor == 1 {
        return Ok(image.clone());
    }
    let (ow, oh) = (w.div_ceil(factor), h.div_ceil(factor));
    let src = image.pixels();
    let w_us = w as usize;
    let mut out: Vec<Rgba> = Vec::with_capacity(ow as usize * oh as usize);
    for oy in 0..oh {
        let y0 = oy * factor;
        let y1 = (y0 + factor).min(h);
        for ox in 0..ow {
            let x0 = ox * factor;
            let x1 = (x0 + factor).min(w);
            let mut sum = [0.0_f64; 4];
            for y in y0..y1 {
                let row = &src[y as usize * w_us..][x0 as usize..x1 as usize];
                for p in row {
                    for (s, v) in sum.iter_mut().zip(p) {
                        *s += f64::from(*v);
                    }
                }
            }
            let n = f64::from((y1 - y0) * (x1 - x0));
            #[allow(clippy::cast_possible_truncation)] // an average of f32 values fits an f32
            out.push(sum.map(|s| (s / n) as f32));
        }
    }
    ImageF32::from_pixels(ow, oh, image.space(), out)
}

/// Upright 8-bit sRGB pixels of `image` for the screen, no larger than
/// `max_side` on either side (the GPU's texture limit).
///
/// # Errors
/// [`EngineError::Image`] when `max_side` is 0.
pub fn to_display(image: &ImageF32, orientation: Orientation, max_side: u32) -> Result<Rgb8Image, EngineError> {
    if max_side == 0 {
        return Err(EngineError::Image(ImageError::Empty));
    }
    // Most renders already fit: convert and turn upright in one pass, no copy.
    let fitted = if image.width().max(image.height()) > max_side {
        Cow::Owned(downscale(image, max_side)?)
    } else {
        Cow::Borrowed(image)
    };
    let (width, height, pixels) = render_cpu::to_srgb8_oriented(&fitted, orientation);
    Ok(Rgb8Image { width, height, pixels })
}

/// Writes `image` upright as a JPEG with an sRGB profile. `wanted` is the
/// preferred path; if a file already exists there, a free name like
/// `photo (2).jpg` is used. Returns the path actually written.
///
/// # Errors
/// [`EngineError`] when the image is too large for JPEG, encoding fails or the file cannot be written.
pub fn export_jpeg(
    image: &ImageF32,
    orientation: Orientation,
    quality: u8,
    wanted: &Path,
) -> Result<PathBuf, EngineError> {
    let (width, height, rgb) = render_cpu::to_srgb8_oriented(image, orientation);
    let jpeg = export::encode_jpeg(&rgb, width, height, quality)?;
    let target = export::non_destructive_path(wanted);
    export::write_atomic(&target, &jpeg)?;
    Ok(target)
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;
    use core_model::WorkingSpace;

    fn ramp(w: u32, h: u32) -> ImageF32 {
        let pixels = (0..w * h).map(|i| [i as f32, 1.0, 0.5, 1.0]).collect();
        ImageF32::from_pixels(w, h, WorkingSpace::LinearRec2020, pixels).unwrap()
    }

    #[test]
    fn an_image_that_fits_is_copied_unchanged() {
        let img = ramp(4, 3);
        assert_eq!(downscale(&img, 4).unwrap(), img);
    }

    #[test]
    fn downscaling_averages_blocks_and_keeps_partial_edge_blocks() {
        // 5×1 → factor 2 → 3 pixels: avg(0,1), avg(2,3), 4.
        let img = ramp(5, 1);
        let small = downscale(&img, 3).unwrap();
        assert_eq!((small.width(), small.height()), (3, 1));
        let r: Vec<f32> = small.pixels().iter().map(|p| p[0]).collect();
        assert_eq!(r, vec![0.5, 2.5, 4.0]);
        assert!(small.pixels().iter().all(|p| p[1] == 1.0 && p[2] == 0.5));
    }

    #[test]
    fn the_longer_side_never_exceeds_the_limit() {
        for (w, h, max) in [(1500, 1000, 512), (1000, 1500, 512), (1201, 801, 1200), (101, 7, 10), (7, 1, 1)] {
            let img = ImageF32::new(w, h, WorkingSpace::LinearRec2020).unwrap();
            let small = downscale(&img, max).unwrap();
            assert!(small.width().max(small.height()) <= max, "{w}×{h} → {max}");
            assert!(small.width() >= 1 && small.height() >= 1);
        }
        assert!(downscale(&ramp(2, 2), 0).is_err());
    }

    #[test]
    fn display_pixels_are_upright_and_three_bytes_each() {
        let img = ramp(4, 2);
        let d = to_display(&img, Orientation::Rotate90, 4096).unwrap();
        assert_eq!((d.width, d.height), (2, 4));
        assert_eq!(d.pixels.len(), 2 * 4 * 3);
    }

    #[test]
    fn the_cpu_engine_renders_and_says_so() {
        let engine = Engine::new(builtin_tools().unwrap(), EngineChoice::CpuOnly);
        assert_eq!(engine.backend(), Backend::Cpu);
        assert_eq!(engine.gpu_problem(), None);
        let graph = NodeGraph::new();
        let r = engine.render(&graph, &ramp(3, 2)).unwrap();
        assert_eq!(r.backend, Backend::Cpu);
        assert_eq!(r.image, ramp(3, 2));
        assert_eq!(r.gpu_failure, None);
    }

    #[test]
    fn export_never_overwrites_and_reports_the_name_used() {
        let dir = std::env::temp_dir().join(format!("rne-engine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wanted = dir.join("photo.jpg");
        let first = export_jpeg(&ramp(8, 4), Orientation::Normal, 90, &wanted).unwrap();
        let second = export_jpeg(&ramp(8, 4), Orientation::Normal, 90, &wanted).unwrap();
        assert_eq!(first, wanted);
        assert_eq!(second, dir.join("photo (2).jpg"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const _: () = {
        const fn shared<T: Send + Sync>() {}
        shared::<Engine>();
        shared::<Rendered>();
    };
}
