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
    /// How much of each database correction is applied.
    pub amount: LensAmounts,
    /// Corrections set by hand, on top of the database's or without it.
    pub manual: ManualLens,
    /// The custom lens these settings came from, by name; `None` when they
    /// were set on this photo only.
    pub custom: Option<String>,
}

impl Default for LensSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            lens: None,
            distortion: true,
            tca: true,
            vignetting: true,
            zoom: None,
            amount: LensAmounts::default(),
            manual: ManualLens::default(),
            custom: None,
        }
    }
}

impl LensSettings {
    /// All corrections switched off.
    #[must_use]
    pub fn off() -> Self {
        Self { enabled: false, ..Self::default() }
    }

    /// The settings with every amount in its range (a value that is not a
    /// number takes the default).
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.amount = self.amount.clamped();
        self.manual = self.manual.clamped();
        self
    }
}

/// How much of each database correction is applied: 1 is the full
/// correction (100 %), 0 none, up to 2 (200 %).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensAmounts {
    /// Distortion.
    pub distortion: f32,
    /// Lateral chromatic aberration.
    pub tca: f32,
    /// Vignetting.
    pub vignetting: f32,
}

impl LensAmounts {
    /// Largest amount (200 %).
    pub const MAX: f32 = 2.0;

    /// Every amount between 0 and [`Self::MAX`]; not-a-number becomes 1.
    #[must_use]
    pub fn clamped(self) -> Self {
        let c = |v: f32| if v.is_finite() { v.clamp(0.0, Self::MAX) } else { 1.0 };
        Self { distortion: c(self.distortion), tca: c(self.tca), vignetting: c(self.vignetting) }
    }
}

impl Default for LensAmounts {
    fn default() -> Self {
        Self { distortion: 1.0, tca: 1.0, vignetting: 1.0 }
    }
}

/// Corrections set by hand, each from −1 to 1, 0 doing nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ManualLens {
    /// Above 0 removes barrel distortion (straightens lines bowed outwards),
    /// below 0 removes pincushion distortion.
    pub distortion: f32,
    /// Above 0 brightens the corners (up to one stop), below 0 darkens them.
    pub vignetting: f32,
}

impl ManualLens {
    /// Every value between −1 and 1; not-a-number becomes 0.
    #[must_use]
    pub fn clamped(self) -> Self {
        let c = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        Self { distortion: c(self.distortion), vignetting: c(self.vignetting) }
    }

    /// `true` when nothing is set.
    #[must_use]
    pub fn is_none(self) -> bool {
        self.distortion == 0.0 && self.vignetting == 0.0
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

/// A custom lens: lens-correction settings saved under a name for the lens
/// that took a photo. Photos taken with the same lens and never edited
/// start with these settings.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomLens {
    /// Name shown in the lens chooser, unique among the custom lenses.
    pub name: String,
    /// The lens the settings are for, as the files name it
    /// ([`crate::CaptureInfo::lens_key`]).
    pub exif_lens: String,
    /// The settings; their `custom` names this custom lens.
    pub settings: LensSettings,
}

impl CustomLens {
    /// `true` when it is for the lens named `exif_lens` in a file.
    #[must_use]
    pub fn fits(&self, exif_lens: &str) -> bool {
        crate::capture::same_lens(&self.exif_lens, exif_lens)
    }
}
