//! The seam between the segmentation pipelines and an inference runtime.
//!
//! Pre- and post-processing (resizing, normalising, decoding boxes and
//! masks) only see [`Tensor`]s and the [`Model`] trait. The ONNX Runtime
//! implementation lives in its own module behind the `onnx` feature, and the
//! tests drive the pipelines with fake models, so all of that logic is
//! checked without a model file or a GPU.

use std::fmt;
use std::path::{Path, PathBuf};

use core_model::ModelId;

/// A dense `f32` tensor, row-major.
#[derive(Clone, PartialEq)]
pub struct Tensor {
    /// Dimensions, outermost first.
    pub shape: Vec<usize>,
    /// `shape.iter().product()` values.
    pub data: Vec<f32>,
}

impl fmt::Debug for Tensor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tensor").field("shape", &self.shape).finish_non_exhaustive()
    }
}

impl Tensor {
    /// A tensor of `shape` filled with `value`.
    #[must_use]
    pub fn filled(shape: &[usize], value: f32) -> Self {
        Self { shape: shape.to_vec(), data: vec![value; shape.iter().product()] }
    }

    /// Wraps `data`; `None` when its length does not match `shape`.
    #[must_use]
    pub fn new(shape: &[usize], data: Vec<f32>) -> Option<Self> {
        (shape.iter().product::<usize>() == data.len()).then(|| Self { shape: shape.to_vec(), data })
    }
}

/// Why segmentation could not run.
#[derive(Debug, Clone, PartialEq)]
pub enum AiError {
    /// The ONNX Runtime library could not be loaded or initialised.
    Runtime(String),
    /// This build has no inference runtime (built without the `onnx` feature).
    NotBuilt,
    /// The model is not in the catalog of this build.
    UnknownModel(ModelId),
    /// A model file is missing or damaged.
    ModelFile {
        /// The model.
        model: &'static str,
        /// The file expected.
        path: PathBuf,
        /// Where it can be downloaded.
        url: &'static str,
    },
    /// The model was loaded but running it failed.
    Inference(String),
    /// The model answered with something the pipeline does not understand
    /// (a different export of the model, for example).
    UnexpectedOutput(String),
    /// The prompt does not say what to select (an object with no including
    /// click and no rectangle).
    IncompletePrompt,
    /// The image could not be prepared (empty or too large).
    Image(String),
    /// A facial feature was asked for, but no face is visible on that person.
    NoFace,
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(why) => write!(f, "the AI runtime (ONNX Runtime) is not available: {why}"),
            Self::NotBuilt => write!(f, "this build has no AI support"),
            Self::UnknownModel(id) => write!(f, "model '{id}' is not known to this build"),
            Self::ModelFile { model, path, url } => {
                write!(
                    f,
                    "model '{model}' is not installed: {} is missing or damaged (download: {url})",
                    path.display()
                )
            }
            Self::Inference(why) => write!(f, "the model failed: {why}"),
            Self::UnexpectedOutput(why) => write!(f, "the model gave an unexpected result: {why}"),
            Self::IncompletePrompt => write!(f, "click on the object or draw a rectangle around it first"),
            Self::Image(why) => write!(f, "the image cannot be analysed: {why}"),
            Self::NoFace => write!(f, "no face is visible on this person"),
        }
    }
}

impl std::error::Error for AiError {}

/// A loaded model, ready to run.
pub trait Model: Send {
    /// Runs the model on named inputs; returns every output, by name.
    ///
    /// # Errors
    /// [`AiError::Inference`] when the runtime fails.
    fn run(&mut self, inputs: Vec<(&'static str, Tensor)>) -> Result<Vec<(String, Tensor)>, AiError>;

    /// The device the model runs on ("CUDA", "DirectML", "CPU" …), for the status bar.
    fn device(&self) -> String;
}

/// Something that loads model files.
pub trait Inference: Send + Sync {
    /// Loads the model in `path` on the fastest device available.
    ///
    /// # Errors
    /// [`AiError`] when the file cannot be loaded.
    fn load(&self, path: &Path) -> Result<Box<dyn Model>, AiError>;
}

/// The output called `name`, or, when the model names its outputs
/// differently, the output at `index` (the order of the reference export).
pub(crate) fn output<'a>(outputs: &'a [(String, Tensor)], name: &str, index: usize) -> Result<&'a Tensor, AiError> {
    outputs
        .iter()
        .find(|(n, _)| n == name)
        .or_else(|| outputs.get(index))
        .map(|(_, t)| t)
        .ok_or_else(|| AiError::UnexpectedOutput(format!("no output '{name}'")))
}
