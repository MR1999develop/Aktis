//! The complete, non-destructive edit of one image version.

use crate::graph::NodeGraph;

/// White balance of the development stage.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum WhiteBalance {
    /// The multipliers chosen by the camera.
    #[default]
    AsShot,
    /// Explicit multipliers for R, G, B, normalised to green = 1.
    Multipliers([f32; 3]),
}

/// Which colour profile turns camera RGB into the working space.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProfileChoice {
    /// The camera's own calibration matrices (built into the editor for
    /// about 740 cameras, or stored in the DNG), interpolated between their
    /// illuminants by the white balance.
    #[default]
    Builtin,
    /// A DCP camera profile, by its name, looked up among the profiles in
    /// the editor's own `profiles` folder for the camera.
    /// When it is missing on this computer the built-in matrices are used
    /// and a warning says so.
    Named(String),
}

/// Lens corrections of the development stage (lensfun data).
#[derive(Debug, Clone, PartialEq)]
pub struct LensSettings {
    /// Master switch.
    pub enabled: bool,
    /// The lens chosen by hand (full database name); `None` identifies the
    /// lens from the file.
    pub lens: Option<String>,
    /// Correct geometric distortion.
    pub distortion: bool,
    /// Correct transverse (lateral) chromatic aberration.
    pub tca: bool,
    /// Correct vignetting.
    pub vignetting: bool,
    /// Magnification after correcting the geometry; `None` chooses the
    /// smallest that leaves no empty borders.
    pub zoom: Option<f32>,
}

impl Default for LensSettings {
    fn default() -> Self {
        Self { enabled: true, lens: None, distortion: true, tca: true, vignetting: true, zoom: None }
    }
}

impl LensSettings {
    /// All corrections switched off.
    #[must_use]
    pub fn off() -> Self {
        Self { enabled: false, ..Self::default() }
    }
}

/// Settings of the fixed development stage (RAW → linear working space).
/// The order of operations is fixed by the sensor's physics, not by the user:
/// white balance, demosaic, lens corrections (in camera RGB), colour profile.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DevelopSettings {
    /// White balance applied to the sensor data before demosaicing.
    pub white_balance: WhiteBalance,
    /// Camera colour profile.
    pub profile: ProfileChoice,
    /// Lens corrections.
    pub lens: LensSettings,
}

/// Everything needed to rebuild an edited image from its RAW file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recipe {
    /// The fixed development stage.
    pub develop: DevelopSettings,
    /// The free node graph.
    pub graph: NodeGraph,
    /// Perspective, straightening and crop of the finished image.
    pub geometry: crate::Geometry,
}
