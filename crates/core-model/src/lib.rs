//! Domain model of the RAW node editor.
//!
//! Pure data and rules, no I/O, no GPU, no UI. Everything that can change an
//! image's edit is expressed here as a [`Recipe`]: a fixed development stage
//! (RAW → linear working space) followed by a free [`NodeGraph`].
//!
//! Pixel data inside the editor is always linear, scene-referred and in
//! `f32`; quantisation happens only when a result leaves the editor.

mod graph;
mod ids;
mod image;
mod math;
mod orientation;
mod params;
mod recipe;
mod sensor;

pub use graph::{GraphError, Link, Node, NodeGraph, NodeKind, NodeMut, PortKind};
pub use ids::{IdError, NodeId, ParamId, ToolId};
pub use image::{ImageError, ImageF32, MAX_PIXELS, Rgba, WorkingSpace};
pub use math::Matrix3;
pub use orientation::Orientation;
pub use params::{ParamValue, Params};
pub use recipe::{DevelopSettings, Recipe, WhiteBalance};
pub use sensor::{BayerPattern, CfaPattern, SensorError, SensorImage};
