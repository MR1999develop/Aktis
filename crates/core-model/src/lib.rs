//! Domain model of Aktis.
//!
//! Pure data and rules, no I/O, no GPU, no UI. Everything that can change an
//! image's edit is expressed here as a [`Recipe`]: a fixed development stage
//! (RAW → linear working space) followed by [`Layers`] of tools, as
//! Capture One's.
//!
//! Pixel data inside the editor is always linear, scene-referred and in
//! `f32`; quantisation happens only when a result leaves the editor.

mod capture;
mod geometry;
mod ids;
mod image;
mod layers;
mod mask;
mod math;
mod orientation;
mod params;
mod recipe;
mod sensor;
mod style;

pub use capture::{CalibratedMatrix, CaptureInfo, Illuminant, same_lens};
pub use geometry::{Geometry, MAX_ANGLE, Warp, full_frame, level_angle};
pub use ids::{IdError, ModelId, ParamId, ToolId};
pub use image::{ImageError, ImageF32, MAX_PIXELS, Rgba, WorkingSpace};
pub use layers::{Layer, LayerError, LayerId, Layers, ToolRef, ToolSettings, Tools};
pub use mask::{
    AiMask, AiTarget, BodyPart, BrushMask, BrushStroke, LinearGradient, MAX_BRUSH_RADIUS, MAX_FEATHER, MAX_RADIUS,
    MIN_RADIUS, MaskComponent, MaskDef, MaskImage, MaskOp, MaskSource, NormPoint, NormRect, PromptPoint,
    RadialGradient, RasterKey,
};
pub use math::Matrix3;
pub use orientation::Orientation;
pub use params::{ParamValue, Params};
pub use recipe::{
    BaseCurve, CurveFit, CustomLens, DevelopSettings, LensAmounts, LensSettings, ManualLens, ProfileChoice, Recipe,
    SensorKind, WhiteBalance,
};
pub use sensor::{BayerPattern, CfaPattern, SensorError, SensorImage};
pub use style::{CameraDefaults, Style, StyleSelection, StyleTool};
