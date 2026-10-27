//! AI selection: masks for the subject, for objects and people chosen with
//! clicks or rectangles, and for every person (or car, dog …) in a photo.
//!
//! Three models do the work, each from the [`catalog`]:
//!
//! - **Subject** — BiRefNet finds the main subject without any prompt.
//! - **Objects** — Segment Anything 2.1 segments what is under the clicks or
//!   inside a rectangle. Its image encoder runs once per photo; every
//!   further click takes milliseconds.
//! - **People and objects by kind** — YOLOX-S finds each person (or any of
//!   80 COCO classes) with a box; each box becomes a Segment Anything
//!   component, so every person is a separate part of the mask that can be
//!   refined or removed.
//!
//! The engine returns *rasters*: the model's answer at about one megapixel.
//! They are cached under a key of image, model and prompt (crate `masks`),
//! and recipes refer to them by that key, so a model never re-runs unless
//! the prompt changes. Scaling to full resolution and snapping to the
//! image's edges happen at render time in crate `masks`, the same way on CPU
//! and GPU.
//!
//! Inference goes through the [`Inference`] trait; with the default `onnx`
//! feature, [`OnnxRuntime`] loads ONNX Runtime at run time and places each
//! model on the best GPU provider the library offers (DirectML, CUDA,
//! ROCm/MIGraphX, OpenVINO, Core ML), or the CPU.

pub mod catalog;
mod detect;
mod engine;
mod face;
mod inference;
#[cfg(feature = "onnx")]
mod onnx;
mod parts;
mod prep;
mod region;
mod sam;
mod selection;
mod sha256;
mod subject;
mod warp;

pub use detect::{COCO_CLASSES, Detection, PERSON, class_index};
pub use engine::{RASTER_LONG_SIDE, Segmenter, default_model};
pub use inference::{AiError, Inference, Model, Tensor};
#[cfg(feature = "onnx")]
pub use onnx::{OnnxRuntime, find_library, library_name};
pub use selection::{compute_pending, from_detections, object, person_part, subject};
pub use sha256::{Sha256, sha256_hex};
