//! Semi-automatic keyword tagging.
//!
//! An image model turns each photo into an [`Embedding`] once; a text model
//! turns each keyword of the user's [`Vocabulary`] into embeddings of its
//! name and synonyms. Comparing the two gives a zero-shot probability for
//! every keyword, with no training. The user then confirms or rejects the
//! [`Suggestion`]s; those [`Decision`]s are the only ground truth, and the
//! [`Tagger`] learns from them a small per-keyword correction on top of the
//! zero-shot score, so suggestions get better with use and keywords with no
//! visual meaning (a client, a trip) can be learned from examples alone.
//!
//! This crate is pure: no model runtime, no files. The model lives behind
//! the [`ImageEncoder`] and [`TextEncoder`] traits (implemented in
//! `tagging-onnx`); embeddings and decisions are stored by the catalog.
//! Decisions are model-independent: when the model changes, embeddings and
//! learned corrections are recomputed from them.

mod embedding;
mod encoder;
mod learn;
mod preprocess;
mod search;
mod suggest;
mod tagger;
mod vocabulary;

pub use embedding::{Embedding, EmbeddingError, ModelId};
pub use encoder::{EncodeError, ImageEncoder, TextEncoder};
pub use learn::{Adapter, LearnSettings};
pub use preprocess::{PixelLayout, to_model_input};
pub use search::{Ranked, nearest};
pub use suggest::{Decision, ImageTags, SuggestPolicy, Suggestion};
pub use tagger::{Tagger, TaggerError, ZeroShotHead};
pub use vocabulary::{Keyword, KeywordId, Vocabulary, VocabularyError};
