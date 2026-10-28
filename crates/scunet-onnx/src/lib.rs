//! Converts the original SCUNet checkpoint into the ONNX model of the AI
//! denoise tool, so the editor can download the weights from their source
//! (KAIR's `scunet_color_real_psnr.pth`) and users need no Python.
//!
//! This is a port of `scripts/scunet/` (`pth_weights.py`, `scunet_graph.py`,
//! `onnxproto.py`): for the same checkpoint and tile size the output is
//! byte-identical to `export_onnx_model.py`'s, which stays the reference.
//!
//! The checkpoint is read safely: the zip reader accepts only stored
//! entries, and the pickle interpreter only rebuilds tensors in
//! dictionaries; any other object or opcode is an error, never code run.
//! Only `std` is used.

mod graph;
mod pickle;
mod proto;
mod zip;

use std::collections::BTreeMap;
use std::fmt;

/// Smallest tile side accepted by [`convert`]: below it the deepest stage
/// has a single 8×8 window, where KAIR's model behaves differently (it does
/// not shift such windows).
pub const MIN_TILE: u32 = 192;

/// A weight of the checkpoint as a contiguous row-major f32 array.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor {
    /// Size of each axis; empty for a scalar.
    pub shape: Vec<usize>,
    /// The elements in row-major (C) order, as many as the product of `shape`.
    pub data: Vec<f32>,
}

/// Why a checkpoint could not be read or converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    /// The tile side is not a multiple of 64 or is below [`MIN_TILE`].
    InvalidTile(u32),
    /// The file is not a zip this reader understands (damaged, compressed,
    /// encrypted, or not a PyTorch checkpoint).
    Archive(String),
    /// `data.pkl` is malformed or contains something other than tensors in
    /// dictionaries.
    Pickle(String),
    /// A weight the network needs is missing: not a SCUNet checkpoint.
    MissingWeight(String),
    /// A weight has a shape the network cannot use.
    BadWeight(String),
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTile(t) => {
                write!(f, "tile side {t} is not a multiple of 64 of at least {MIN_TILE}")
            }
            Self::Archive(m) => write!(f, "unreadable checkpoint archive: {m}"),
            Self::Pickle(m) => write!(f, "unsupported checkpoint content: {m}"),
            Self::MissingWeight(w) => write!(f, "not a SCUNet checkpoint: weight {w} is missing"),
            Self::BadWeight(m) => write!(f, "unexpected SCUNet weight: {m}"),
        }
    }
}

impl std::error::Error for ConvertError {}

/// The tensors of a PyTorch zip checkpoint, by name, as f32 (integer
/// tensors are converted too). Like `pth_weights.load_state_dict`, a
/// dictionary wrapped under `params`, `state_dict` or `model` is unwrapped.
///
/// # Errors
/// [`ConvertError::Archive`] or [`ConvertError::Pickle`] when `pth` is not
/// a checkpoint made only of tensors in dictionaries.
pub fn load_state_dict(pth: &[u8]) -> Result<BTreeMap<String, Tensor>, ConvertError> {
    pickle::load(pth)
}

/// The ONNX model (opset 13, input `input` and output `output`, both
/// 1×3×`tile`×`tile`) of the SCUNet checkpoint `pth`
/// (`scunet_color_real_psnr.pth`), the same bytes as
/// `export_onnx_model.py --tile <tile>` writes.
///
/// # Errors
/// [`ConvertError::InvalidTile`] for a tile side that is not a multiple of
/// 64 of at least [`MIN_TILE`]; otherwise the errors of
/// [`load_state_dict`], or [`ConvertError::MissingWeight`] /
/// [`ConvertError::BadWeight`] when the checkpoint is not SCUNet's.
pub fn convert(pth: &[u8], tile: u32) -> Result<Vec<u8>, ConvertError> {
    if !tile.is_multiple_of(64) || tile < MIN_TILE {
        return Err(ConvertError::InvalidTile(tile));
    }
    let weights = pickle::load_views(pth)?;
    let tile = usize::try_from(tile).map_err(|_| ConvertError::InvalidTile(tile))?;
    graph::build(weights, tile)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn refuses_bad_tiles_before_reading() {
        for t in [0, 64, 128, 200, 449] {
            assert_eq!(convert(b"", t), Err(ConvertError::InvalidTile(t)));
        }
        assert!(matches!(convert(b"", 192), Err(ConvertError::Archive(_))));
    }
}
