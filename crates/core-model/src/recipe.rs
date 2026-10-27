//! The complete, non-destructive edit of one image version.

use crate::graph::NodeGraph;

/// White balance of the development stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteBalance {
    /// The multipliers chosen by the camera.
    AsShot,
    /// Explicit multipliers for R, G, B, normalised to green = 1.
    Multipliers([f32; 3]),
}

/// Settings of the fixed development stage (RAW → linear working space).
/// The order of operations is fixed by the sensor's physics, not by the user.
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopSettings {
    /// White balance applied to the sensor data before demosaicing.
    pub white_balance: WhiteBalance,
}

impl Default for DevelopSettings {
    fn default() -> Self {
        Self { white_balance: WhiteBalance::AsShot }
    }
}

/// Everything needed to rebuild an edited image from its RAW file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recipe {
    /// The fixed development stage.
    pub develop: DevelopSettings,
    /// The free node graph.
    pub graph: NodeGraph,
}
