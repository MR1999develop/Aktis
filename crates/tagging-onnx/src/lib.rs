//! Image and text encoders for the `tagging` crate, running a SigLIP-family
//! model exported to ONNX (separate vision and text graphs, plus the
//! `tokenizer.json`) on ONNX Runtime, entirely on this computer.
//!
//! The model folder holds `vision_model.onnx`, `text_model.onnx` (or their
//! `_quantized` variants, directly or under `onnx/`) and `tokenizer.json`,
//! as published in the `onnx-community` exports on Hugging Face.

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

use core_model::ImageF32;
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use tagging::{Embedding, EncodeError, ImageEncoder, ModelId, PixelLayout, TextEncoder, ZeroShotHead};
use tokenizers::Tokenizer;

/// What the code needs to know about a model that the ONNX files do not say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelSpec {
    /// Stable name, stored with every embedding.
    pub name: &'static str,
    /// Image size and normalisation.
    pub layout: PixelLayout,
    /// Texts are padded (or cut) to exactly this many tokens.
    pub max_tokens: usize,
    /// Lower-case texts before tokenising (SigLIP 2 was trained so).
    pub lowercase: bool,
    /// Learned logit scale, already exponentiated: `logit = scale × cos + bias`.
    pub logit_scale: f32,
    /// Learned logit bias.
    pub logit_bias: f32,
}

impl ModelSpec {
    /// The id stored with embeddings of this model.
    ///
    /// # Panics
    /// Never for the specs defined here: their names are valid ids.
    #[must_use]
    pub fn model_id(&self) -> ModelId {
        let id = ModelId::new(self.name);
        id.expect("spec names are valid model ids (checked by a test for every spec)")
    }

    /// The zero-shot head for the tagger, `None` only for a broken spec.
    #[must_use]
    pub fn head(&self) -> Option<ZeroShotHead> {
        ZeroShotHead::new(self.logit_scale, self.logit_bias)
    }
}

/// Google SigLIP 2, ViT-B/16 at 224 px (Apache-2.0). Multilingual: keywords
/// can be written in Italian. Export: `onnx-community/siglip2-base-patch16-224-ONNX`.
/// Logit scale and bias as stored in the original checkpoint
/// (`google/siglip2-base-patch16-224`, `exp(4.724453)` and `-16.7717`);
/// the CI job `tagging-model` prints them with `.github/scripts/siglip_head.py`.
pub const SIGLIP2_BASE_224: ModelSpec = ModelSpec {
    name: "siglip2-base-patch16-224",
    layout: PixelLayout::SIGLIP_224,
    max_tokens: 64,
    lowercase: true,
    logit_scale: 112.668_9,
    logit_bias: -16.771_7,
};

/// Every model this build knows.
pub const MODELS: &[ModelSpec] = &[SIGLIP2_BASE_224];

/// Why a model could not be loaded or run.
#[derive(Debug)]
pub enum OnnxError {
    /// A required file is missing from the model folder.
    MissingFile {
        /// The folder searched.
        dir: PathBuf,
        /// The file names accepted, in order of preference.
        names: Vec<String>,
    },
    /// ONNX Runtime refused the model or failed to run it.
    Runtime(String),
    /// The tokenizer file could not be read.
    Tokenizer(String),
    /// The model's inputs or outputs are not what a SigLIP export has.
    Unexpected(String),
}

impl fmt::Display for OnnxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFile { dir, names } => {
                write!(f, "none of {} found in {}", names.join(", "), dir.display())
            }
            Self::Runtime(m) => write!(f, "ONNX Runtime: {m}"),
            Self::Tokenizer(m) => write!(f, "tokenizer: {m}"),
            Self::Unexpected(m) => write!(f, "unexpected model: {m}"),
        }
    }
}

impl std::error::Error for OnnxError {}

fn runtime(e: impl fmt::Display) -> OnnxError {
    OnnxError::Runtime(e.to_string())
}

/// How to load a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoadOptions {
    /// Prefer the 8-bit quantised graphs (smaller, faster, slightly less
    /// accurate) when both are present.
    pub prefer_quantized: bool,
    /// Threads per inference; `None` lets ONNX Runtime decide. Batch tagging
    /// in the background should leave cores to the editor.
    pub threads: Option<usize>,
}

/// The first of `names` present in `dir` or `dir/onnx`.
fn find(dir: &Path, names: &[String]) -> Result<PathBuf, OnnxError> {
    for name in names {
        for candidate in [dir.join(name), dir.join("onnx").join(name)] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(OnnxError::MissingFile { dir: dir.to_owned(), names: names.to_vec() })
}

fn graph_names(base: &str, prefer_quantized: bool) -> Vec<String> {
    let (full, quant) = (format!("{base}.onnx"), format!("{base}_quantized.onnx"));
    if prefer_quantized { vec![quant, full] } else { vec![full, quant] }
}

fn session(path: &Path, opts: &LoadOptions) -> Result<Session, OnnxError> {
    let env = &onnx_env::runtime().map_err(OnnxError::Runtime)?.env;
    let mut builder = Session::builder(env).map_err(runtime)?;
    if let Some(n) = opts.threads {
        builder = builder.with_intra_threads(n.max(1)).map_err(runtime)?;
    }
    builder = builder.with_execution_providers(execution_providers()).map_err(runtime)?;
    builder.commit_from_file(path).map_err(runtime)
}

/// GPU providers compiled in, best first; ONNX Runtime falls back to the CPU
/// when none can start.
fn execution_providers() -> Vec<ort::ep::ExecutionProviderDispatch> {
    #[allow(unused_mut)]
    let mut eps = Vec::new();
    #[cfg(feature = "cuda")]
    eps.push(ort::ep::CUDA::default().build());
    #[cfg(feature = "directml")]
    eps.push(ort::ep::DirectML::default().build());
    eps
}

/// The input names of a session, for error messages and choices.
fn input_names(s: &Session) -> Vec<String> {
    s.inputs().iter().map(|i| i.name().to_owned()).collect()
}

/// The pooled embedding output: `pooler_output` or `*_embeds` by name, else
/// the only output.
fn embedding_output(s: &Session) -> Result<String, OnnxError> {
    let names: Vec<String> = s.outputs().iter().map(|o| o.name().to_owned()).collect();
    for wanted in ["image_embeds", "text_embeds", "pooler_output"] {
        if let Some(n) = names.iter().find(|n| *n == wanted) {
            return Ok(n.clone());
        }
    }
    match names.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err(OnnxError::Unexpected(format!("no embedding among the outputs {names:?}"))),
    }
}

/// Splits a `[n, dim]` output into `n` embeddings.
fn split_embeddings(shape: &[i64], data: &[f32], n: usize) -> Result<Vec<Embedding>, EncodeError> {
    let dim = match shape {
        [rows, dim] if usize::try_from(*rows).ok() == Some(n) => usize::try_from(*dim).unwrap_or(0),
        _ => return Err(EncodeError::new(format!("output shape {shape:?}, expected [{n}, dim]"))),
    };
    if dim == 0 || data.len() != n * dim {
        return Err(EncodeError::new(format!("output of {} values for shape {shape:?}", data.len())));
    }
    data.chunks(dim).map(|c| Embedding::new(c.to_vec()).map_err(|e| EncodeError::new(e.to_string()))).collect()
}

/// The vision half of the model.
#[derive(Debug)]
pub struct OnnxImageEncoder {
    model: ModelId,
    layout: PixelLayout,
    session: Session,
    input: String,
    output: String,
}

impl OnnxImageEncoder {
    /// Loads the vision graph from the model folder.
    ///
    /// # Errors
    /// [`OnnxError`] when the file is missing or not a SigLIP vision model.
    pub fn load(dir: &Path, spec: &ModelSpec, opts: &LoadOptions) -> Result<Self, OnnxError> {
        let path = find(dir, &graph_names("vision_model", opts.prefer_quantized))?;
        let session = session(&path, opts)?;
        let inputs = input_names(&session);
        let input = inputs
            .iter()
            .find(|n| *n == "pixel_values")
            .or(inputs.first())
            .cloned()
            .ok_or_else(|| OnnxError::Unexpected("vision model has no input".to_owned()))?;
        let output = embedding_output(&session)?;
        Ok(Self { model: spec.model_id(), layout: spec.layout, session, input, output })
    }
}

impl ImageEncoder for OnnxImageEncoder {
    fn model(&self) -> &ModelId {
        &self.model
    }

    fn encode_images(&mut self, images: &[&ImageF32]) -> Result<Vec<Embedding>, EncodeError> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let n = images.len();
        let mut data = Vec::with_capacity(n * self.layout.len());
        for img in images {
            data.extend(tagging::to_model_input(img, &self.layout));
        }
        let shape = [n, 3, self.layout.height() as usize, self.layout.width() as usize];
        let tensor = Tensor::from_array((shape, data)).map_err(|e| EncodeError::new(e.to_string()))?;
        let inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> =
            vec![(Cow::from(self.input.as_str()), SessionInputValue::from(tensor))];
        let outputs = self.session.run(inputs).map_err(|e| EncodeError::new(e.to_string()))?;
        let value =
            outputs.get(&self.output).ok_or_else(|| EncodeError::new(format!("output '{}' missing", self.output)))?;
        let (shape, data) = value.try_extract_tensor::<f32>().map_err(|e| EncodeError::new(e.to_string()))?;
        split_embeddings(shape, data, n)
    }
}

/// The text half of the model, with its tokenizer.
#[derive(Debug)]
pub struct OnnxTextEncoder {
    model: ModelId,
    session: Session,
    tokenizer: Tokenizer,
    max_tokens: usize,
    lowercase: bool,
    pad_id: i64,
    ids_input: String,
    mask_input: Option<String>,
    output: String,
}

impl OnnxTextEncoder {
    /// Loads the text graph and `tokenizer.json` from the model folder.
    ///
    /// # Errors
    /// [`OnnxError`] when a file is missing or unreadable.
    pub fn load(dir: &Path, spec: &ModelSpec, opts: &LoadOptions) -> Result<Self, OnnxError> {
        let path = find(dir, &graph_names("text_model", opts.prefer_quantized))?;
        let tok_path = find(dir, &["tokenizer.json".to_owned()])?;
        let mut tokenizer = Tokenizer::from_file(&tok_path).map_err(|e| OnnxError::Tokenizer(e.to_string()))?;
        // Padding and truncation are done here, to exactly max_tokens.
        tokenizer.with_padding(None);
        tokenizer.with_truncation(None).map_err(|e| OnnxError::Tokenizer(e.to_string()))?;
        let pad_id = tokenizer.token_to_id("<pad>").map_or(0, i64::from);

        let session = session(&path, opts)?;
        let inputs = input_names(&session);
        let ids_input = inputs
            .iter()
            .find(|n| *n == "input_ids")
            .cloned()
            .ok_or_else(|| OnnxError::Unexpected(format!("text model inputs {inputs:?} lack input_ids")))?;
        let mask_input = inputs.iter().find(|n| *n == "attention_mask").cloned();
        let output = embedding_output(&session)?;
        Ok(Self {
            model: spec.model_id(),
            session,
            tokenizer,
            max_tokens: spec.max_tokens,
            lowercase: spec.lowercase,
            pad_id,
            ids_input,
            mask_input,
            output,
        })
    }

    /// Token ids of one text, padded or cut to `max_tokens`.
    fn tokens(&self, text: &str) -> Result<Vec<i64>, EncodeError> {
        let text = if self.lowercase { text.to_lowercase() } else { text.to_owned() };
        let enc = self.tokenizer.encode(text.as_str(), true).map_err(|e| EncodeError::new(e.to_string()))?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i64::from(i)).take(self.max_tokens).collect();
        ids.resize(self.max_tokens, self.pad_id);
        Ok(ids)
    }
}

impl TextEncoder for OnnxTextEncoder {
    fn model(&self) -> &ModelId {
        &self.model
    }

    fn encode_texts(&mut self, texts: &[&str]) -> Result<Vec<Embedding>, EncodeError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let n = texts.len();
        let mut ids = Vec::with_capacity(n * self.max_tokens);
        for t in texts {
            ids.extend(self.tokens(t)?);
        }
        let shape = [n, self.max_tokens];
        let err = |e: ort::Error| EncodeError::new(e.to_string());
        let mut inputs: Vec<(Cow<'_, str>, SessionInputValue<'_>)> = vec![(
            Cow::from(self.ids_input.as_str()),
            SessionInputValue::from(Tensor::from_array((shape, ids)).map_err(err)?),
        )];
        if let Some(mask) = &self.mask_input {
            // SigLIP is trained without masking the padding: attend to every
            // position, exactly as during training.
            let ones = vec![1_i64; n * self.max_tokens];
            inputs.push((
                Cow::from(mask.as_str()),
                SessionInputValue::from(Tensor::from_array((shape, ones)).map_err(err)?),
            ));
        }
        let outputs = self.session.run(inputs).map_err(err)?;
        let value =
            outputs.get(&self.output).ok_or_else(|| EncodeError::new(format!("output '{}' missing", self.output)))?;
        let (shape, data) = value.try_extract_tensor::<f32>().map_err(err)?;
        split_embeddings(shape, data, n)
    }
}

/// Both encoders of a model folder.
///
/// # Errors
/// [`OnnxError`] when either half cannot be loaded.
pub fn load(
    dir: &Path,
    spec: &ModelSpec,
    opts: &LoadOptions,
) -> Result<(OnnxImageEncoder, OnnxTextEncoder), OnnxError> {
    Ok((OnnxImageEncoder::load(dir, spec, opts)?, OnnxTextEncoder::load(dir, spec, opts)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_has_a_valid_id_and_head() {
        for spec in MODELS {
            assert!(ModelId::new(spec.name).is_some(), "{}", spec.name);
            assert!(spec.head().is_some(), "{}", spec.name);
            assert!(spec.max_tokens > 0);
        }
    }

    #[test]
    fn outputs_are_split_per_row_and_checked() {
        let e = split_embeddings(&[2, 2], &[3.0, 4.0, 0.0, 2.0], 2).expect("valid");
        assert_eq!(e.len(), 2);
        assert!((e[0].as_slice()[0] - 0.6).abs() < 1e-6);
        assert!(split_embeddings(&[3, 2], &[0.0; 6], 2).is_err(), "row count");
        assert!(split_embeddings(&[2, 2], &[1.0; 3], 2).is_err(), "length");
        assert!(split_embeddings(&[2, 1, 2], &[1.0; 4], 2).is_err(), "rank");
    }

    #[test]
    fn graph_files_are_looked_for_in_order_of_preference() {
        assert_eq!(graph_names("vision_model", false), ["vision_model.onnx", "vision_model_quantized.onnx"]);
        assert_eq!(graph_names("vision_model", true), ["vision_model_quantized.onnx", "vision_model.onnx"]);
        let missing = find(Path::new("/definitely/not/here"), &graph_names("text_model", false));
        assert!(matches!(missing, Err(OnnxError::MissingFile { .. })));
    }
}
