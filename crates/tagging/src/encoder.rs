//! The model, seen from this crate: something that turns images and texts
//! into embeddings of the same space.

use std::fmt;

use core_model::ImageF32;

use crate::{Embedding, ModelId};

/// Why a model could not encode its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeError {
    message: String,
}

impl EncodeError {
    /// An error described by `message`.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EncodeError {}

/// Turns photos into embeddings. Takes the editor's own images (linear,
/// any working space, already oriented): the encoder does its own resizing
/// and colour conversion, see [`crate::to_model_input`].
pub trait ImageEncoder {
    /// The model, with its version.
    fn model(&self) -> &ModelId;

    /// One embedding per image, in order.
    ///
    /// # Errors
    /// [`EncodeError`] when the model fails to run.
    fn encode_images(&mut self, images: &[&ImageF32]) -> Result<Vec<Embedding>, EncodeError>;
}

/// Turns keyword prompts into embeddings comparable with the image ones.
pub trait TextEncoder {
    /// The model, with its version. Must equal the image encoder's.
    fn model(&self) -> &ModelId;

    /// One embedding per text, in order.
    ///
    /// # Errors
    /// [`EncodeError`] when the model fails to run.
    fn encode_texts(&mut self, texts: &[&str]) -> Result<Vec<Embedding>, EncodeError>;
}
