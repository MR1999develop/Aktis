//! ONNX Runtime, loaded at run time.
//!
//! The runtime library (`onnxruntime.dll`, `libonnxruntime.so`) is not linked
//! into the program: it is loaded when AI is first used. So the editor starts
//! and works without it, and the library can be swapped for a build with the
//! right GPU support for the machine: DirectML on Windows (NVIDIA, AMD and
//! Intel alike), CUDA for NVIDIA, ROCm/MIGraphX for AMD on Linux, OpenVINO
//! for Intel, Core ML on macOS. Each model is placed on the first of those
//! the library offers that accepts it, else on the CPU.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ort::environment::Environment;
use ort::ep::{self, ExecutionProvider, ExecutionProviderDispatch};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor as OrtTensor;

use crate::inference::{AiError, Inference, Model, Tensor};

/// The runtime library's file name on this system.
#[must_use]
pub fn library_name() -> String {
    format!("{}onnxruntime{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX)
}

/// Where to look for the runtime library, in order: the `ORT_DYLIB_PATH`
/// environment variable, next to the program, then each of `extra_dirs`.
#[must_use]
pub fn find_library(extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let name = library_name();
    let beside_exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(&name)));
    beside_exe.into_iter().chain(extra_dirs.iter().map(|d| d.join(&name))).find(|p| p.is_file())
}

/// ONNX Runtime with its execution providers.
pub struct OnnxRuntime {
    env: Environment,
    library: PathBuf,
}

impl std::fmt::Debug for OnnxRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnnxRuntime").field("library", &self.library).finish_non_exhaustive()
    }
}

impl OnnxRuntime {
    /// Loads the runtime library at `library`. Only one library can be loaded
    /// per process; later calls reuse the first.
    ///
    /// # Errors
    /// [`AiError::Runtime`] when the library cannot be loaded or is too old.
    pub fn load(library: &Path) -> Result<Self, AiError> {
        // The DirectML build needs a recent DirectML.dll; Windows' own copy in
        // System32 is often too old. One next to the runtime is loaded first.
        if let Some(dml) = library.parent().map(|d| d.join("DirectML.dll")).filter(|p| p.is_file()) {
            ort::util::preload_dylib(dml.as_path())
                .map_err(|e| AiError::Runtime(format!("cannot load {}: {e}", dml.display())))?;
        }
        let builder = ort::init_from(library).map_err(|e| AiError::Runtime(e.to_string()))?;
        let env = builder.with_name("rne").build().map_err(|e| AiError::Runtime(e.to_string()))?;
        Ok(Self { env, library: library.to_path_buf() })
    }

    /// The library in use.
    #[must_use]
    pub fn library(&self) -> &Path {
        &self.library
    }

    /// GPU execution providers the loaded library offers, fastest first.
    fn gpu_providers() -> Vec<(&'static str, ExecutionProviderDispatch)> {
        let mut v = Vec::new();
        add::<ep::CUDA>(&mut v, "CUDA");
        #[cfg(windows)]
        add::<ep::DirectML>(&mut v, "DirectML");
        add::<ep::MIGraphX>(&mut v, "MIGraphX");
        add::<ep::ROCm>(&mut v, "ROCm");
        add::<ep::OpenVINO>(&mut v, "OpenVINO");
        #[cfg(target_os = "macos")]
        add::<ep::CoreML>(&mut v, "Core ML");
        v
    }

    /// Names of the GPU execution providers the loaded library offers.
    #[must_use]
    pub fn available_gpu_providers(&self) -> Vec<&'static str> {
        Self::gpu_providers().into_iter().map(|(n, _)| n).collect()
    }
}

/// Adds provider `E` to `v` if the loaded library was built with it.
fn add<E>(v: &mut Vec<(&'static str, ExecutionProviderDispatch)>, name: &'static str)
where
    E: ExecutionProvider + Default,
    ExecutionProviderDispatch: From<E>,
{
    if E::is_available().unwrap_or(false) {
        v.push((name, ExecutionProviderDispatch::from(E::default()).error_on_failure()));
    }
}

/// One session on one device.
fn session(
    env: &Environment,
    path: &Path,
    provider: Option<(&str, ExecutionProviderDispatch)>,
) -> ort::Result<Session> {
    let mut builder = Session::builder(env)?;
    match provider {
        Some((name, ep)) => {
            builder = builder.with_optimization_level(GraphOptimizationLevel::Level3)?;
            if name == "DirectML" {
                // DirectML needs sequential execution without memory patterns.
                builder = builder.with_memory_pattern(false)?.with_parallel_execution(false)?;
            }
            builder = builder.with_execution_providers([ep])?;
        }
        None => {
            // On the CPU the extended (layout) optimisations of ONNX Runtime
            // 1.24 corrupt the Segment Anything encoder's output in about one
            // run in four (rows of the mask go missing, sometimes NaN);
            // basic optimisations give the same result every time.
            builder = builder.with_optimization_level(GraphOptimizationLevel::Level1)?;
        }
    }
    builder.commit_from_file(path)
}

impl Inference for OnnxRuntime {
    fn load(&self, path: &Path) -> Result<Box<dyn Model>, AiError> {
        let mut problems = Vec::new();
        for (name, ep) in Self::gpu_providers() {
            match session(&self.env, path, Some((name, ep))) {
                Ok(s) => return Ok(Box::new(OnnxModel { session: s, device: name.to_owned() })),
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
        match session(&self.env, path, None) {
            Ok(s) => Ok(Box::new(OnnxModel { session: s, device: "CPU".to_owned() })),
            Err(e) => {
                problems.push(format!("CPU: {e}"));
                Err(AiError::Inference(format!("cannot load {}: {}", path.display(), problems.join("; "))))
            }
        }
    }
}

/// A model loaded in ONNX Runtime.
struct OnnxModel {
    session: Session,
    device: String,
}

impl Model for OnnxModel {
    fn run(&mut self, inputs: Vec<(&'static str, Tensor)>) -> Result<Vec<(String, Tensor)>, AiError> {
        let mut values: Vec<(Cow<'static, str>, SessionInputValue<'static>)> = Vec::with_capacity(inputs.len());
        for (name, t) in inputs {
            let shape: Vec<i64> = t.shape.iter().map(|d| i64::try_from(*d).unwrap_or(i64::MAX)).collect();
            let tensor = OrtTensor::from_array((shape, t.data)).map_err(|e| AiError::Inference(e.to_string()))?;
            values.push((Cow::Borrowed(name), tensor.into()));
        }
        let names: Vec<String> = self.session.outputs().iter().map(|o| o.name().to_owned()).collect();
        let outputs = self.session.run(values).map_err(|e| AiError::Inference(e.to_string()))?;
        let mut result = Vec::with_capacity(names.len());
        for name in names {
            let Some(value) = outputs.get(&name) else { continue };
            let (shape, data) = value
                .try_extract_tensor::<f32>()
                .map_err(|e| AiError::UnexpectedOutput(format!("output '{name}': {e}")))?;
            let shape = shape.iter().map(|d| usize::try_from(*d).unwrap_or(0)).collect();
            result.push((name, Tensor { shape, data: data.to_vec() }));
        }
        Ok(result)
    }

    fn device(&self) -> String {
        self.device.clone()
    }
}
