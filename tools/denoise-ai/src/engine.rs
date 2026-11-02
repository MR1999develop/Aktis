//! SCUNet on ONNX Runtime.
//!
//! Two GPU back ends can be compiled in (the Windows build has both):
//! * **TensorRT for RTX** (feature `nvrtx`): NVIDIA RTX 30xx and later; needs
//!   only the two TensorRT for RTX 1.4 DLLs next to the program and the NVIDIA
//!   driver; runs the `_fp16` models when present. About 4 times faster than
//!   DirectML on an RTX 3050;
//! * **DirectML** (feature `directml`): any DirectX 12 card, nothing to install.
//!
//! If the requested GPU cannot be used, the next back end is tried and
//! finally the CPU, and [`Scunet::gpu_fallback`] says why.

use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::{GraphOptimizationLevel, SessionBuilder};
use ort::value::Tensor;

use crate::models::{DEFAULT_TILE_PX, TILE_MARGIN_PX, fp16_variant, tile_px_for};
use crate::tiling::{ModelError, TileModel, TilePlan};

/// Environment variable choosing the device: `auto` (default), `rtx`, `directml` or `cpu`.
pub const DEVICE_ENV: &str = "AKTIS_DENOISE_DEVICE";

/// Timed runs of each tile size in the automatic search; the fastest counts.
const TIMED_PROBES: usize = 2;

/// Requested compute device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Device {
    /// The fastest available: TensorRT for RTX, DirectML, then the CPU.
    #[default]
    Auto,
    /// TensorRT for RTX only (then the CPU).
    TensorRtRtx,
    /// DirectML only (then the CPU).
    DirectMl,
    /// The CPU.
    Cpu,
}

impl Device {
    /// The device named by `value` (as in [`DEVICE_ENV`]); `Auto` for anything else.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "rtx" | "tensorrt" | "nvrtx" => Self::TensorRtRtx,
            "directml" | "dml" => Self::DirectMl,
            "cpu" => Self::Cpu,
            _ => Self::Auto,
        }
    }

    /// `true` if this build can use TensorRT for RTX.
    #[must_use]
    pub const fn rtx_available() -> bool {
        cfg!(feature = "nvrtx")
    }

    /// `true` if this build can use DirectML.
    #[must_use]
    pub const fn directml_available() -> bool {
        cfg!(feature = "directml")
    }

    /// `true` if this build can use any GPU.
    #[must_use]
    pub const fn gpu_available() -> bool {
        Self::rtx_available() || Self::directml_available()
    }

    /// GPU back ends to try, in order.
    fn gpu_order(self) -> Vec<Gpu> {
        let mut order = Vec::new();
        if matches!(self, Self::Auto | Self::TensorRtRtx) && Self::rtx_available() {
            order.push(Gpu::Rtx);
        }
        if matches!(self, Self::Auto | Self::DirectMl) && Self::directml_available() {
            order.push(Gpu::DirectMl);
        }
        order
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gpu {
    Rtx,
    DirectMl,
}

impl Gpu {
    fn name(self) -> &'static str {
        match self {
            Self::Rtx => "GPU (TensorRT for RTX)",
            Self::DirectMl => "GPU (DirectML)",
        }
    }
}

/// Measured speed of one tile size on the device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileSpeed {
    /// Side of the input tile.
    pub tile_px: usize,
    /// Seconds to run the network on one tile.
    pub secs_per_tile: f64,
}

impl TileSpeed {
    /// Seconds per megapixel of useful (non-overlap) output.
    #[must_use]
    pub fn secs_per_mp(&self) -> f64 {
        let core = self.tile_px.saturating_sub(2 * TILE_MARGIN_PX).max(1);
        #[allow(clippy::cast_precision_loss)] // tile sides are small
        let px = (core * core) as f64;
        self.secs_per_tile / px * 1e6
    }
}

/// What the tile-size search found on a device: how fast each size that fits
/// runs. Lets the engine pick the fastest size for each image.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TileProfile {
    /// Back end measured (as [`Scunet::device`]).
    pub device: String,
    /// Sizes that fit, smallest first.
    pub speeds: Vec<TileSpeed>,
    /// Seconds to load a model (switching tile size costs this much).
    pub load_secs: f64,
}

impl TileProfile {
    /// Seconds to process `width × height` with `tile_px` tiles, if measured.
    /// Tiles do not shrink at the edges, so large tiles waste work there.
    #[must_use]
    pub fn estimate(&self, tile_px: usize, width: usize, height: usize) -> Option<f64> {
        let speed = self.speeds.iter().find(|s| s.tile_px == tile_px)?;
        let tiles = TilePlan::new(width, height, tile_px, TILE_MARGIN_PX)?.len();
        #[allow(clippy::cast_precision_loss)] // tile counts are small
        Some(tiles as f64 * speed.secs_per_tile)
    }

    /// The fastest tile size for an image of `size` (`None`: per useful pixel).
    /// With `current`, the size already loaded, any other size is charged the
    /// time to load it.
    #[must_use]
    pub fn best(&self, size: Option<(usize, usize)>, current: Option<usize>) -> Option<usize> {
        let cost = |s: &TileSpeed| -> f64 {
            let run = match size {
                Some((w, h)) => self.estimate(s.tile_px, w, h).unwrap_or(f64::INFINITY),
                None => s.secs_per_mp(),
            };
            let switch = if current.is_some_and(|c| c != s.tile_px) && size.is_some() { self.load_secs } else { 0.0 };
            run + switch
        };
        self.speeds.iter().min_by(|a, b| cost(a).total_cmp(&cost(b))).map(|s| s.tile_px)
    }
}

/// The SCUNet model, loaded and ready.
pub struct Scunet {
    session: Session,
    tile_px: usize,
    on_gpu: bool,
    device: String,
    gpu_fallback: Option<String>,
    tile_notes: Vec<String>,
    profile: Option<TileProfile>,
    load_secs: f64,
}

impl fmt::Debug for Scunet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scunet").field("tile_px", &self.tile_px).field("device", &self.device).finish_non_exhaustive()
    }
}

impl Scunet {
    /// Loads the model at `path` on `device` if possible, else on the CPU.
    ///
    /// # Errors
    /// A message when the file is missing or is not a valid model.
    pub fn load(path: &Path, device: Device) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("the model file does not exist: {}", path.display()));
        }
        let started = Instant::now();
        let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let tile_px = tile_px_for(path);
        let mut failures = Vec::new();
        for gpu in device.gpu_order() {
            // TensorRT for RTX only runs FP16 from an FP16 model
            let fp16 = fp16_variant(path);
            let (file, precision) = match gpu {
                Gpu::Rtx if fp16.is_file() => (fp16.as_path(), ", FP16"),
                Gpu::Rtx => (path, ", FP32"),
                Gpu::DirectMl => (path, ""),
            };
            match session(file, threads, Some(gpu)) {
                Ok(session) => {
                    let name = gpu.name();
                    let device = match name.strip_suffix(')') {
                        Some(head) if !precision.is_empty() => format!("{head}{precision})"),
                        _ => name.to_owned(),
                    };
                    return Ok(Self {
                        session,
                        tile_px,
                        on_gpu: true,
                        device,
                        gpu_fallback: (!failures.is_empty()).then(|| failures.join("; ")),
                        tile_notes: Vec::new(),
                        profile: None,
                        load_secs: started.elapsed().as_secs_f64(),
                    });
                }
                Err(e) => failures.push(format!("{}: {e}", gpu.name())),
            }
        }
        if !matches!(device, Device::Auto | Device::Cpu) && failures.is_empty() {
            failures.push(format!("this build has no {device:?} support"));
        }
        let session = session(path, threads, None)
            .map_err(|e| format!("the model {} could not be loaded: {e}", path.display()))?;
        Ok(Self {
            session,
            tile_px,
            on_gpu: false,
            device: format!("CPU, {threads} threads"),
            gpu_fallback: (!failures.is_empty()).then(|| failures.join("; ")),
            tile_notes: Vec::new(),
            profile: None,
            load_secs: started.elapsed().as_secs_f64(),
        })
    }

    /// Loads the tile size best suited to an image of `size` on `device`.
    ///
    /// `models` are files of the same network with different tile sizes. On
    /// a GPU larger tiles waste less work on the overlap but need more memory,
    /// and larger is not always faster. Unless `known` already says how fast
    /// each size is, the sizes are tried from the smallest up, each timed on a
    /// blank tile, until one does not fit or is not faster per useful pixel.
    /// The measures are kept in the model ([`Scunet::profile`]). On the CPU
    /// the 448 px model is used, the fastest there.
    ///
    /// # Errors
    /// A message when no model can be loaded at all.
    pub fn load_adaptive(
        models: &[PathBuf],
        device: Device,
        known: Option<&TileProfile>,
        size: Option<(usize, usize)>,
    ) -> Result<Self, String> {
        let mut sorted = models.to_vec();
        sorted.sort_by_key(|p| tile_px_for(p));
        sorted.dedup_by_key(|p| tile_px_for(p));
        let default = sorted
            .iter()
            .find(|p| tile_px_for(p) == DEFAULT_TILE_PX)
            .or(sorted.first())
            .cloned()
            .ok_or_else(|| "no model file".to_owned())?;
        if device == Device::Cpu || !Device::gpu_available() {
            return Self::load(&default, Device::Cpu);
        }

        // fast path: the measures of an earlier search
        if let Some(profile) = known.filter(|p| !p.speeds.is_empty())
            && let Some(path) = profile.best(size, None).and_then(|t| sorted.iter().find(|p| tile_px_for(p) == t))
            && let Ok(mut m) = Self::load(path, device)
            && m.on_gpu
            && m.device == profile.device
            && m.probe().is_ok()
        {
            m.tile_notes.push(format!("{} px tiles, from the measures of an earlier search", m.tile_px));
            let mut profile = profile.clone();
            profile.load_secs = m.load_secs;
            m.profile = Some(profile);
            return Ok(m);
        }

        let mut notes = Vec::new();
        let mut profile = TileProfile::default();
        let mut load_secs = Vec::new();
        // only one model at a time in GPU memory, so a larger one gets all of it
        let mut last: Option<Self> = None;
        for path in &sorted {
            let tile = tile_px_for(path);
            drop(last.take());
            let mut model = match Self::load(path, device) {
                Ok(m) => m,
                Err(e) => {
                    notes.push(format!("{tile} px tiles: {e}"));
                    continue;
                }
            };
            if !model.on_gpu {
                // no usable GPU at all: the CPU is fastest with the default tile
                let reason = model.gpu_fallback.take();
                drop(model);
                let mut m = Self::load(&default, Device::Cpu)?;
                m.gpu_fallback = reason;
                return Ok(m);
            }
            load_secs.push(model.load_secs);
            profile.device.clone_from(&model.device);
            // first run: compiles the GPU kernels and checks that the tile fits
            if let Err(e) = model.probe() {
                notes.push(format!("{tile} px tiles do not fit: {}", e.0));
                break;
            }
            // the faster of two timed runs: one alone can be slowed by anything else
            let mut secs = f64::INFINITY;
            let mut failed = None;
            for _ in 0..TIMED_PROBES {
                let t = Instant::now();
                if let Err(e) = model.probe() {
                    failed = Some(e);
                    break;
                }
                secs = secs.min(t.elapsed().as_secs_f64());
            }
            if let Some(e) = failed {
                notes.push(format!("{tile} px tiles failed: {}", e.0));
                break;
            }
            let speed = TileSpeed { tile_px: tile, secs_per_tile: secs };
            notes.push(format!("{tile} px tiles: {:.2} s per megapixel", speed.secs_per_mp()));
            let previous = profile.speeds.last().map(TileSpeed::secs_per_mp);
            profile.speeds.push(speed);
            last = Some(model);
            if previous.is_some_and(|p| speed.secs_per_mp() >= p * 0.97) {
                break; // not faster: stop here
            }
        }
        if profile.speeds.is_empty() {
            drop(last);
            let mut m = Self::load(&default, Device::Cpu)?;
            m.gpu_fallback = Some(notes.join("; "));
            return Ok(m);
        }
        // the fastest load seen: the first load of a size can include one-off work
        profile.load_secs = load_secs.iter().copied().fold(f64::INFINITY, f64::min);
        let best = profile.best(size, None).unwrap_or(DEFAULT_TILE_PX);
        let mut m = match last {
            Some(m) if m.tile_px == best => m,
            other => {
                drop(other);
                let path = sorted.iter().find(|p| tile_px_for(p) == best).unwrap_or(&default);
                let mut m = Self::load(path, device)?;
                // warm up again, so the first real tile is not slower
                let _ = m.probe();
                m
            }
        };
        m.tile_notes = notes;
        m.profile = Some(profile);
        Ok(m)
    }

    /// Runs the network once on a blank tile: fails if the device cannot
    /// handle this tile size (typically: not enough GPU memory).
    fn probe(&mut self) -> Result<(), ModelError> {
        let n = 3 * self.tile_px * self.tile_px;
        let input = vec![0.5_f32; n];
        let mut output = vec![0.0_f32; n];
        self.run(&input, &mut output)
    }

    /// `true` if the network runs on a GPU.
    #[must_use]
    pub fn on_gpu(&self) -> bool {
        self.on_gpu
    }

    /// Device in use, for the user ("GPU (DirectML)", "CPU, 12 threads").
    #[must_use]
    pub fn device(&self) -> &str {
        &self.device
    }

    /// Why the GPU was requested but not used, if that happened.
    #[must_use]
    pub fn gpu_fallback(&self) -> Option<&str> {
        self.gpu_fallback.as_deref()
    }

    /// Tile sizes tried by [`Scunet::load_adaptive`], with their speed or why
    /// they were rejected.
    #[must_use]
    pub fn tile_notes(&self) -> &[String] {
        &self.tile_notes
    }

    /// Measured speeds of the tile sizes (on a GPU, after [`Scunet::load_adaptive`]).
    #[must_use]
    pub fn profile(&self) -> Option<&TileProfile> {
        self.profile.as_ref()
    }
}

/// ONNX Runtime errors (whose types vary between builder steps) as text.
fn text<E: fmt::Display>(e: E) -> String {
    e.to_string()
}

/// The process's ONNX Runtime environment, shared with the other AI modules
/// (`ort` allows a single one per process).
fn environment() -> Result<&'static Environment, String> {
    onnx_env::runtime().map(|r| &r.env)
}

fn session(path: &Path, threads: usize, gpu: Option<Gpu>) -> Result<Session, String> {
    let mut b = Session::builder(environment()?)
        .map_err(text)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(text)?;
    b = match gpu {
        Some(Gpu::Rtx) => with_rtx(b)?,
        Some(Gpu::DirectMl) => with_directml(b)?,
        None => b.with_intra_threads(threads).map_err(text)?,
    };
    if gpu == Some(Gpu::Rtx) {
        return commit_compiled(b, path);
    }
    b.commit_from_file(path).map_err(text)
}

/// Folder where TensorRT for RTX keeps the engines it builds and the kernels
/// it compiles on the first run, so later runs start faster.
#[must_use]
pub fn rtx_cache_dir() -> PathBuf {
    platform::cache_dir(platform::Os::current(), platform::APP_DIR, crate::models::process_env)
        .unwrap_or_else(|| std::env::temp_dir().join(platform::APP_DIR))
        .join("tensorrt-rtx-cache")
}

/// Where the engine TensorRT for RTX builds from `path` is saved. The name
/// carries the size and date of the model and the ONNX Runtime version, so a
/// changed model or library never reuses an old engine.
fn compiled_path(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let stem = path.file_stem()?.to_string_lossy();
    Some(rtx_cache_dir().join(format!("{stem}-{:x}-{modified:x}-ort{}.onnx", meta.len(), ort::MINOR_VERSION)))
}

/// TensorRT for RTX needs about 15 s to build its engine from the ONNX model;
/// the first time the engine is saved as a compiled model and later sessions
/// load that in a moment. A compiled model that no longer loads (new driver
/// or library) is rebuilt; if building fails, the original model is used.
fn commit_compiled(b: SessionBuilder, path: &Path) -> Result<Session, String> {
    if let Some(compiled) = compiled_path(path) {
        if compiled.is_file() {
            match b.clone().commit_from_file(&compiled) {
                Ok(session) => return Ok(session),
                Err(_) => {
                    let _ = std::fs::remove_file(&compiled);
                }
            }
        }
        let partial = compiled.with_extension("partial.onnx");
        let built = std::fs::create_dir_all(rtx_cache_dir()).map_err(text).and_then(|()| {
            ort::compiler::ModelCompiler::new(b.clone())
                .and_then(|c| c.with_model_from_file(path))
                .and_then(|c| c.with_embed_ep_context())
                .and_then(|c| c.compile_to_file(&partial))
                .map_err(text)
        });
        if built.is_ok() && std::fs::rename(&partial, &compiled).is_ok() {
            if let Ok(session) = b.clone().commit_from_file(&compiled) {
                return Ok(session);
            }
            let _ = std::fs::remove_file(&compiled);
        }
        let _ = std::fs::remove_file(&partial);
    }
    let mut b = b;
    b.commit_from_file(path).map_err(text)
}

// `error_on_failure` turns a GPU that cannot be used into an error (instead of
// a silent CPU fallback inside ONNX Runtime), so the user is told the truth.

#[cfg(feature = "directml")]
fn with_directml(b: SessionBuilder) -> Result<SessionBuilder, String> {
    // DirectML requires sequential execution without memory patterns
    let b = b.with_memory_pattern(false).map_err(text)?;
    let b = b.with_parallel_execution(false).map_err(text)?;
    b.with_execution_providers([ort::ep::DirectML::default().build().error_on_failure()]).map_err(text)
}

#[cfg(not(feature = "directml"))]
fn with_directml(_b: SessionBuilder) -> Result<SessionBuilder, String> {
    Err("this build has no DirectML support".into())
}

#[cfg(feature = "nvrtx")]
fn with_rtx(b: SessionBuilder) -> Result<SessionBuilder, String> {
    let cache = rtx_cache_dir();
    std::fs::create_dir_all(&cache).map_err(text)?;
    let rtx = ort::ep::NVRTX::default().with_runtime_cache_path(cache.to_string_lossy()).build().error_on_failure();
    b.with_execution_providers([rtx]).map_err(text)
}

#[cfg(not(feature = "nvrtx"))]
fn with_rtx(_b: SessionBuilder) -> Result<SessionBuilder, String> {
    Err("this build has no TensorRT for RTX support".into())
}

impl TileModel for Scunet {
    fn input_px(&self) -> usize {
        self.tile_px
    }

    fn margin_px(&self) -> usize {
        TILE_MARGIN_PX
    }

    fn run(&mut self, planar_in: &[f32], planar_out: &mut [f32]) -> Result<(), ModelError> {
        let err = |e: &dyn fmt::Display| ModelError(e.to_string());
        let n = self.tile_px;
        let input = Tensor::from_array(([1_usize, 3, n, n], planar_in.to_vec())).map_err(|e| err(&e))?;
        let outputs = self.session.run(ort::inputs![input]).map_err(|e| err(&e))?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(|e| err(&e))?;
        if data.len() != planar_out.len() {
            return Err(ModelError(format!(
                "output has {} values instead of {}: this is not the expected model",
                data.len(),
                planar_out.len()
            )));
        }
        planar_out.copy_from_slice(data);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Speeds measured with TensorRT for RTX on an RTX 3050 laptop GPU.
    fn rtx_3050() -> TileProfile {
        let speed = |tile_px: usize, s_per_mp: f64| {
            #[allow(clippy::cast_precision_loss)]
            let core = (tile_px - 2 * TILE_MARGIN_PX) as f64;
            TileSpeed { tile_px, secs_per_tile: s_per_mp * core * core / 1e6 }
        };
        TileProfile {
            device: "GPU".into(),
            speeds: vec![speed(448, 0.81), speed(704, 0.58), speed(960, 0.50), speed(1216, 0.47), speed(1600, 0.45)],
            load_secs: 15.0,
        }
    }

    #[test]
    fn the_best_tile_depends_on_the_image() {
        let p = rtx_3050();
        assert_eq!(p.best(None, None), Some(1600));
        assert_eq!(p.best(Some((6000, 4000)), None), Some(1216));
        assert_eq!(p.best(Some((1000, 800)), None), Some(1216));
        assert_eq!(p.best(Some((300, 200)), None), Some(448));
    }

    #[test]
    fn switching_tile_size_must_pay_for_the_load() {
        let p = rtx_3050();
        assert_eq!(p.best(Some((6000, 4000)), Some(1600)), Some(1600));
        let cheap = TileProfile { load_secs: 0.2, ..p };
        assert_eq!(cheap.best(Some((6000, 4000)), Some(1600)), Some(1216));
        assert_eq!(TileProfile::default().best(Some((6000, 4000)), None), None);
    }

    #[test]
    fn device_names_are_parsed_loosely() {
        assert_eq!(Device::parse("CPU"), Device::Cpu);
        assert_eq!(Device::parse(" rtx "), Device::TensorRtRtx);
        assert_eq!(Device::parse("dml"), Device::DirectMl);
        assert_eq!(Device::parse("whatever"), Device::Auto);
    }

    #[test]
    fn a_missing_model_file_is_reported() {
        let e = Scunet::load(Path::new("no/such/scunet_color_real_448.onnx"), Device::Cpu).expect_err("missing");
        assert!(e.contains("does not exist"), "{e}");
    }
}
