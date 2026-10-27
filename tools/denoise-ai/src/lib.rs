//! AI denoise: noise reduction with the SCUNet neural network, ported from
//! an earlier denoiser.
//!
//! SCUNet (K. Zhang et al., "Practical Blind Image Denoising via Swin-Conv-UNet
//! and Data Synthesis", 2023, Apache-2.0) is trained on real camera noise and
//! estimates the noise level by itself. The node therefore runs the network
//! once per input image and keeps its output: the two parameters only decide
//! how much of the correction to apply, so moving them is instant.
//!
//! * **Grain (luminance)**: how much of the luma correction is applied;
//! * **Colour blotches**: how much of the colour correction is applied.
//!
//! The network runs on ONNX Runtime ([`engine`]): TensorRT for RTX or
//! DirectML on Windows, the CPU elsewhere. The model files are not in the
//! repository; [`models`] says where they are looked for. Without a model
//! the node passes the image through and reports why.
//!
//! The image is split into overlapping tiles ([`tiling`]); how linear light
//! is handed to the network and back is described in [`mapping`].

pub mod engine;
pub mod mapping;
pub mod models;
pub mod tiling;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use core_model::ImageF32;
use tool_api::{
    Category, GpuKernel, ParamDescriptor, ParamId, ParamKind, ResolvedParams, Stage, Tool, ToolDescriptor, ToolError,
    ToolId, Unit,
};

use crate::engine::{DEVICE_ENV, Device, Scunet, TileProfile};
use crate::mapping::Mixer;
use crate::tiling::{RgbImage, TileModel};

/// Id of the tool in recipes.
pub const TOOL_ID: &str = "denoise_ai";
/// Id of the grain (luminance) amount, in percent.
pub const GRAIN: &str = "grain";
/// Id of the colour-blotch amount, in percent.
pub const COLOR: &str = "color";

/// an earlier denoiser's recommended "Medium" preset.
const DEFAULT_GRAIN: f32 = 80.0;
const DEFAULT_COLOR: f32 = 100.0;

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| {
    let percent = |default| ParamKind::Float { min: 0.0, max: 100.0, default, step: 1.0, unit: Unit::Percent };
    ToolDescriptor {
        id: ToolId::new(TOOL_ID).expect("constant id is valid"),
        version: 1,
        name: "AI denoise",
        category: Category::Details,
        stage: Stage::Graph,
        params: vec![
            ParamDescriptor {
                id: ParamId::new(GRAIN).expect("constant id is valid"),
                label: "Grain (luminance)",
                kind: percent(DEFAULT_GRAIN),
            },
            ParamDescriptor {
                id: ParamId::new(COLOR).expect("constant id is valid"),
                label: "Colour blotches",
                kind: percent(DEFAULT_COLOR),
            },
        ],
    }
});

/// Where the network comes from: the real model, or a stand-in for tests.
enum Source {
    /// Model files found in these folders, loaded on first use.
    Files { dirs: Vec<PathBuf>, device: Device },
    /// A ready model (tests).
    Fixed(Box<dyn TileModel>),
}

/// Network outputs kept: the editor alternates a reduced preview and the
/// full-size image of the same recipe, and both should stay instant.
const CACHED_IMAGES: usize = 2;

/// The network output for one input image.
struct Cached {
    fingerprint: u64,
    encoded: RgbImage,
    network: RgbImage,
}

struct State {
    source: Source,
    engine: Option<Scunet>,
    profile: Option<TileProfile>,
    /// Most recently used first.
    cache: Vec<Cached>,
    status: Option<String>,
}

/// The AI denoise tool. One instance is shared by every node that uses it;
/// the model is loaded on first use and kept.
pub struct AiDenoise {
    state: Mutex<State>,
}

impl std::fmt::Debug for AiDenoise {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiDenoise").finish_non_exhaustive()
    }
}

impl AiDenoise {
    /// The tool, looking for models in the usual folders ([`models::search_dirs`],
    /// [`models::MODELS_ENV`] first)
    /// and on the device named by `RNE_DENOISE_DEVICE` (default: the fastest).
    #[must_use]
    pub fn from_environment() -> Arc<Self> {
        let exe = std::env::current_exe().ok();
        let dirs = models::search_dirs(platform::Os::current(), exe.as_deref(), crate::models::process_env);
        let device = std::env::var(DEVICE_ENV).map_or(Device::Auto, |v| Device::parse(&v));
        Self::with_model_dirs(dirs, device)
    }

    /// The tool, looking for models only in `dirs`.
    #[must_use]
    pub fn with_model_dirs(dirs: Vec<PathBuf>, device: Device) -> Arc<Self> {
        Self::from_source(Source::Files { dirs, device })
    }

    /// The tool with a ready network, for tests of everything around it.
    #[must_use]
    pub fn with_model(model: Box<dyn TileModel>) -> Arc<Self> {
        Self::from_source(Source::Fixed(model))
    }

    fn from_source(source: Source) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State { source, engine: None, profile: None, cache: Vec::new(), status: None }),
        })
    }

    /// The tool as a registry entry.
    #[must_use]
    pub fn shared() -> Arc<dyn Tool> {
        Self::from_environment()
    }

    /// What the last run used ("GPU (DirectML), 960 px tiles · …"), or why it
    /// could not run. `None` before the first run.
    #[must_use]
    pub fn status(&self) -> Option<String> {
        self.lock().status.clone()
    }

    /// Drops the loaded model and the cached result (frees GPU memory).
    pub fn release(&self) {
        let mut s = self.lock();
        s.engine = None;
        s.cache.clear();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    /// Makes sure the first entry of `cache` is the network output for `image`.
    fn ensure_network_output(&mut self, image: &ImageF32) -> Result<(), ToolError> {
        let fingerprint = fingerprint(image);
        if let Some(i) = self.cache.iter().position(|c| c.fingerprint == fingerprint) {
            self.cache[..=i].rotate_right(1);
            return Ok(());
        }
        // free the oldest result before computing a new one
        self.cache.truncate(CACHED_IMAGES - 1);
        let encoded = mapping::encode(image);
        let network = self.run(&encoded)?;
        self.cache.insert(0, Cached { fingerprint, encoded, network });
        Ok(())
    }

    /// Runs the network, loading it if needed; a GPU that fails while running
    /// is replaced by the CPU once.
    fn run(&mut self, encoded: &RgbImage) -> Result<RgbImage, ToolError> {
        let size = (encoded.width(), encoded.height());
        let (dirs, device) = match &mut self.source {
            Source::Fixed(model) => {
                let result = tiling::run_tiles(encoded, model.as_mut(), &mut |_, _| {});
                return result.map_err(|e| self.fail(&e.to_string()));
            }
            Source::Files { dirs, device } => (dirs.clone(), *device),
        };
        if self.engine.is_none() {
            let files = models::find_models(&dirs);
            if files.is_empty() {
                let looked: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
                return Err(self.fail(&format!(
                    "AI denoise: no SCUNet model found (scunet_color_real_448.onnx); looked in {}. \
                     Set {} to the folder of the models",
                    looked.join(", "),
                    models::MODELS_ENV
                )));
            }
            let loaded = Scunet::load_adaptive(&files, device, self.profile.as_ref(), Some(size));
            let engine = loaded.map_err(|e| self.fail(&format!("AI denoise: {e}")))?;
            if let Some(p) = engine.profile() {
                self.profile = Some(p.clone());
            }
            self.engine = Some(engine);
        }
        let Some(engine) = self.engine.as_mut() else {
            return Err(self.fail("AI denoise: the model could not be loaded"));
        };
        let result = tiling::run_tiles(encoded, engine, &mut |_, _| {});
        let gpu_failed = match result {
            Ok(out) => {
                self.status = Some(describe(engine));
                return Ok(out);
            }
            Err(e) if engine.on_gpu() => format!("{} failed: {}", engine.device(), e.0),
            Err(e) => return Err(self.fail(&format!("AI denoise: {e}"))),
        };
        // The GPU failed while running (lost device, out of memory): once on the CPU.
        self.engine = None;
        let files = models::find_models(&dirs);
        let loaded = Scunet::load_adaptive(&files, Device::Cpu, None, Some(size));
        let mut cpu = loaded.map_err(|e| self.fail(&format!("AI denoise: {gpu_failed}, and the CPU too: {e}")))?;
        let result = tiling::run_tiles(encoded, &mut cpu, &mut |_, _| {});
        let out = result.map_err(|e| self.fail(&format!("AI denoise: {gpu_failed}, and the CPU too: {e}")))?;
        self.status = Some(format!("{} ({gpu_failed})", describe(&cpu)));
        self.engine = Some(cpu);
        Ok(out)
    }

    fn fail(&mut self, message: &str) -> ToolError {
        self.status = Some(message.to_owned());
        ToolError::Unavailable(message.to_owned())
    }
}

/// "GPU (DirectML), 960 px tiles", plus why the GPU was not used if so.
fn describe(engine: &Scunet) -> String {
    let mut s = format!("{}, {} px tiles", engine.device(), engine.input_px());
    if let Some(why) = engine.gpu_fallback() {
        let _ = write!(s, " (GPU not used: {why})");
    }
    s
}

/// A 64-bit fingerprint of the pixels, size and colour space, to recognise
/// an image already processed. Not cryptographic: a collision would only
/// reuse a network output for a different image of the same size.
fn fingerprint(image: &ImageF32) -> u64 {
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut h = (u64::from(image.width()) << 32) | u64::from(image.height());
    h ^= u64::from(matches!(image.space(), core_model::WorkingSpace::LinearSrgb));
    for p in image.pixels() {
        let a = u64::from(p[0].to_bits()) | (u64::from(p[1].to_bits()) << 32);
        let b = u64::from(p[2].to_bits()) | (u64::from(p[3].to_bits()) << 32);
        h = (h.rotate_left(5) ^ a).wrapping_mul(K);
        h = (h.rotate_left(5) ^ b).wrapping_mul(K);
    }
    h
}

impl Tool for AiDenoise {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    /// Same as [`Tool::apply`]: there is no separate reference for a neural
    /// network. On failure the image is left unchanged.
    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let _ = self.apply(image, params);
    }

    fn apply(&self, image: &mut ImageF32, params: &ResolvedParams) -> Result<(), ToolError> {
        let mixer = Mixer::new(params.float(GRAIN) / 100.0, params.float(COLOR) / 100.0, image.space());
        if mixer.is_identity() {
            return Ok(());
        }
        let mut state = self.lock();
        state.ensure_network_output(image)?;
        if let Some(c) = state.cache.first() {
            mixer.apply_to(image, &c.encoded, &c.network);
        }
        Ok(())
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::Host
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};
    use tiling::ModelError;

    /// Stand-in network: replaces every tile with a flat mid grey, and counts runs.
    struct Flat {
        runs: Arc<AtomicUsize>,
    }

    impl TileModel for Flat {
        fn input_px(&self) -> usize {
            16
        }
        fn margin_px(&self) -> usize {
            2
        }
        fn run(&mut self, _input: &[f32], output: &mut [f32]) -> Result<(), ModelError> {
            self.runs.fetch_add(1, Ordering::Relaxed);
            output.fill(0.5);
            Ok(())
        }
    }

    fn params(grain: f32, color: f32) -> ResolvedParams {
        let mut p = Params::new();
        p.set(ParamId::new(GRAIN).expect("id"), ParamValue::Float(grain));
        p.set(ParamId::new(COLOR).expect("id"), ParamValue::Float(color));
        ResolvedParams::resolve(&DESCRIPTOR, &p)
    }

    fn image() -> ImageF32 {
        let px = (0..20 * 12_u16).map(|i| [f32::from(i % 7) / 10.0, 0.1, 0.3, 0.5]).collect();
        ImageF32::from_pixels(20, 12, WorkingSpace::LinearRec2020, px).expect("20\u{d7}12")
    }

    #[test]
    fn the_descriptor_is_valid() {
        assert_eq!(DESCRIPTOR.validate(), Ok(()));
    }

    #[test]
    fn full_strength_gives_the_network_output_and_keeps_alpha() {
        let runs = Arc::new(AtomicUsize::new(0));
        let tool = AiDenoise::with_model(Box::new(Flat { runs: Arc::clone(&runs) }));
        let mut img = image();
        tool.apply(&mut img, &params(100.0, 100.0)).expect("runs");
        let grey = core_color::srgb_decode(0.5);
        assert!(img.pixels().iter().all(|p| (p[0] - grey).abs() < 1e-5 && (p[2] - grey).abs() < 1e-5));
        assert!(img.pixels().iter().all(|p| (p[3] - 0.5).abs() < f32::EPSILON));
        assert_eq!(runs.load(Ordering::Relaxed), 2, "20\u{d7}12 in useful tiles of 12 px: 2\u{d7}1");
    }

    #[test]
    fn changing_the_amounts_reuses_the_network_output() {
        let runs = Arc::new(AtomicUsize::new(0));
        let tool = AiDenoise::with_model(Box::new(Flat { runs: Arc::clone(&runs) }));
        let source = image();
        for (g, c) in [(100.0, 100.0), (50.0, 20.0), (80.0, 100.0)] {
            let mut img = source.clone();
            tool.apply(&mut img, &params(g, c)).expect("runs");
        }
        assert_eq!(runs.load(Ordering::Relaxed), 2, "the network ran once (2 tiles)");
        // a different image runs it again
        let mut other = source.clone();
        other.pixels_mut()[0][1] = 0.9;
        tool.apply(&mut other, &params(100.0, 100.0)).expect("runs");
        assert_eq!(runs.load(Ordering::Relaxed), 4);
        // preview and full image alternating: both stay cached
        for img in [&source, &other, &source] {
            tool.apply(&mut img.clone(), &params(60.0, 100.0)).expect("runs");
        }
        assert_eq!(runs.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn zero_amounts_change_nothing_and_skip_the_network() {
        let runs = Arc::new(AtomicUsize::new(0));
        let tool = AiDenoise::with_model(Box::new(Flat { runs: Arc::clone(&runs) }));
        let mut img = image();
        tool.apply(&mut img, &params(0.0, 0.0)).expect("runs");
        assert_eq!(img, image());
        assert_eq!(runs.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn without_a_model_the_image_passes_through_and_the_reason_is_given() {
        let tool = AiDenoise::with_model_dirs(vec![PathBuf::from("no/such/folder")], Device::Cpu);
        let mut img = image();
        let err = tool.apply(&mut img, &params(80.0, 100.0)).expect_err("no model");
        assert!(err.to_string().contains("no SCUNet model found"), "{err}");
        assert!(err.to_string().contains("no/such/folder"), "{err}");
        assert_eq!(img, image());
        assert_eq!(tool.status(), Some(err.to_string()));
        assert_eq!(tool.gpu_kernel(), GpuKernel::Host);
    }
}
