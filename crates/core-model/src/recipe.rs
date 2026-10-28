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

/// The tone curve of the output render (scene light → display), as Capture
/// One's Base Characteristics ▸ Curve: the photo's basic look, under every
/// adjustment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BaseCurve {
    /// The camera's default; for now [`BaseCurve::FilmStandard`] for every camera.
    #[default]
    Auto,
    /// Like slide film: deep blacks, bright midtones and highlights.
    FilmStandard,
    /// Film Standard with less contrast in the deep shadows (more detail there).
    FilmExtraShadow,
    /// More contrast than Film Standard: deeper shadows, brighter highlights.
    FilmHighContrast,
    /// Very low contrast with a gentle roll-off near white, to build the look
    /// with the Curve and Levels nodes.
    LinearResponse,
}

impl BaseCurve {
    /// Every curve, in menu order.
    pub const ALL: [Self; 5] =
        [Self::Auto, Self::FilmStandard, Self::FilmExtraShadow, Self::FilmHighContrast, Self::LinearResponse];

    /// Name in menus.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::FilmStandard => "Film Standard",
            Self::FilmExtraShadow => "Film Extra Shadow",
            Self::FilmHighContrast => "Film High Contrast",
            Self::LinearResponse => "Linear Response",
        }
    }

    /// Stable name in saved recipes.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::FilmStandard => "film_standard",
            Self::FilmExtraShadow => "film_extra_shadow",
            Self::FilmHighContrast => "film_high_contrast",
            Self::LinearResponse => "linear_response",
        }
    }

    /// The curve saved as `id`.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The curve actually applied: [`BaseCurve::Auto`] becomes the camera's
    /// default (Film Standard until per-camera defaults exist).
    #[must_use]
    pub fn resolved(self) -> Self {
        match self {
            Self::Auto => Self::FilmStandard,
            other => other,
        }
    }
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
    /// Tone curve of the output render.
    pub curve: BaseCurve,
}
