//! Lens corrections from the lensfun database.
//!
//! [Lensfun](https://github.com/lensfun/lensfun) describes cameras (mount,
//! crop factor) and lenses (distortion, transverse chromatic aberration and
//! vignetting measured at several focal lengths, apertures and distances).
//! This crate reads that XML format directly, so newer lensfun files can be
//! dropped in without a conversion step, and reproduces lensfun's lookup,
//! interpolation and correction maths in plain Rust:
//!
//! 1. [`LensDatabase::find_camera`] and [`LensDatabase::find_lenses`] turn the
//!    names in the EXIF data into database entries;
//! 2. [`LensCorrection::new`] interpolates the calibration for the shot's
//!    focal length, aperture and distance and brings it into the image's
//!    pixel coordinates;
//! 3. the development stage evaluates [`LensCorrection::vignetting_gain`] and
//!    [`LensCorrection::source`] per pixel.
//!
//! The database files for the major camera and lens makers are built in
//! (`data/lensfun/`, CC BY-SA 3.0, see its README). No I/O happens except in
//! [`LensDatabase::add_dir`].

mod correction;
mod data;
mod db;
mod interp;
mod matching;
mod model;
mod xml;

pub use correction::{Applied, CorrectionError, CorrectionRequest, LensCorrection};
pub use db::{DbError, LensDatabase, LensMatch, MAX_FILE_BYTES};
pub use model::{
    CalibrationSet, Camera, Distortion, DistortionModel, Lens, LensType, Mount, Name, Tca, TcaModel, Vignetting,
    loose_eq,
};
pub use xml::XmlError;
