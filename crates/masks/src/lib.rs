//! Builds mask images from mask definitions.
//!
//! A mask node's [`core_model::MaskDef`] says *what* to select; this crate
//! turns it into values at the resolution being rendered: AI rasters scaled
//! up, their uncertain parts decided by colour and their edges snapped to the
//! image's, components combined, feather,
//! invert and opacity applied. It has no GPU or model code, so both
//! renderers use it and get identical masks, and the AI crate can be left
//! out of a build without losing masks already computed.

mod brush;
mod evaluate;
mod filter;
mod key;
mod refine;
mod resample;
mod store;

pub use brush::rasterize;
pub use evaluate::{EvaluatedMask, evaluate, guide_of};
pub use filter::{box_mean, soft_blur};
pub use key::{fingerprint, raster_key};
pub use refine::{Guide, refine};
pub use resample::{fit_within, resize_plane};
pub use store::{DiskRasters, MemoryRasters, NoRasters, RasterSource, RasterStore};
