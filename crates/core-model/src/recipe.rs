//! The complete, non-destructive edit of one image version.

use crate::layers::Layers;

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
    /// Purple fringing removed along high-contrast edges, from 0 (none) to
    /// 1; works without lens data.
    pub purple_fringing: f32,
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
            purple_fringing: 0.0,
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

    /// The settings once the lens `lens` of the database is chosen (`None`:
    /// Automatic, read from the file). They no longer come from a custom
    /// lens: after one, the corrections start again from the defaults (its
    /// amounts and manual values belonged to it); between database lenses
    /// the amounts and manual values stay.
    #[must_use]
    pub fn with_database_lens(self, lens: Option<String>) -> Self {
        let base = if self.custom.is_some() { Self { enabled: self.enabled, ..Self::default() } } else { self };
        Self { lens, custom: None, ..base }
    }

    /// The settings with every amount in its range (a value that is not a
    /// number takes the default).
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.amount = self.amount.clamped();
        self.manual = self.manual.clamped();
        self.purple_fringing =
            if self.purple_fringing.is_finite() { self.purple_fringing.clamp(0.0, 1.0) } else { 0.0 };
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

/// The kinds of colour filter, each demosaiced by its best method
/// (Markesteijn for X-Trans, RCD for Bayer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKind {
    /// 2×2 Bayer filter.
    Bayer,
    /// Fujifilm's 6×6 X-Trans filter.
    XTrans,
    /// Already three colours per pixel (Linear DNG): nothing to demosaic.
    Rgb,
}

/// The tone curve of the output render (scene light → display), as Capture
/// One's Base Characteristics ▸ Curve: the photo's basic look, under every
/// adjustment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum BaseCurve {
    /// Standard, the default for the file: for a camera's RAW file the curve
    /// fitted to the camera's own picture ([`BaseCurve::CameraMatch`]), or
    /// [`BaseCurve::FilmStandard`] when there is none; [`BaseCurve::Linear`]
    /// for a JPEG or TIFF (a finished picture already has its tone curve).
    #[default]
    Auto,
    /// No curve: the pixels shown as they are, as a JPEG or TIFF looks.
    Linear,
    /// Like slide film: deep blacks, bright midtones and highlights. What
    /// Standard becomes for a RAW file whose camera picture is unknown;
    /// never saved (recipes keep Auto) and not in the menu.
    FilmStandard,
    /// The film curve fitted to the camera's own picture of the photo (the
    /// JPEG inside the RAW file): what [`BaseCurve::Auto`] becomes for a RAW
    /// file whose picture is known ([`BaseCurve::with_camera_fit`]). Worked
    /// out each time the photo is opened, never saved: recipes keep Auto.
    CameraMatch(CurveFit),
}

/// The shape of a film curve fitted to a picture, in thousandths so that
/// it compares and hashes exactly: where middle grey shows (sRGB-encoded),
/// the displayed change per stop through the midtones, the stops above grey
/// where the shoulder starts, and how the toe reaches black (0: as Film
/// Standard; otherwise a power law, higher keeping more shadow detail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CurveFit {
    grey: u16,
    slope: u16,
    shoulder_from: u16,
    toe_power: u16,
}

impl CurveFit {
    /// A fitted shape; values are clamped to what a film curve can be.
    #[must_use]
    pub fn new(grey: f32, slope: f32, shoulder_from: f32, toe_power: Option<f32>) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=8000 first
        let milli = |v: f32, lo: f32, hi: f32| (v.clamp(lo, hi) * 1000.0).round() as u16;
        Self {
            grey: milli(grey, 0.05, 0.95),
            slope: milli(slope, 0.02, 1.0),
            shoulder_from: milli(shoulder_from, 0.1, 6.0),
            toe_power: toe_power.map_or(0, |p| milli(p, 0.5, 8.0)),
        }
    }

    /// Displayed (sRGB-encoded) value of middle grey.
    #[must_use]
    pub fn grey(self) -> f32 {
        f32::from(self.grey) / 1000.0
    }

    /// Displayed value per stop at middle grey.
    #[must_use]
    pub fn slope(self) -> f32 {
        f32::from(self.slope) / 1000.0
    }

    /// Stops above grey where the shoulder starts.
    #[must_use]
    pub fn shoulder_from(self) -> f32 {
        f32::from(self.shoulder_from) / 1000.0
    }

    /// The toe's power law, `None` for Film Standard's toe.
    #[must_use]
    pub fn toe_power(self) -> Option<f32> {
        (self.toe_power > 0).then(|| f32::from(self.toe_power) / 1000.0)
    }
}

impl BaseCurve {
    /// The curves offered in the menu, in order.
    pub const ALL: [Self; 2] = [Self::Auto, Self::Linear];

    /// Name in menus: Standard covers what Auto becomes for each file.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto | Self::FilmStandard | Self::CameraMatch(_) => "Standard",
            Self::Linear => "Linear",
        }
    }

    /// Stable name in saved recipes.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            // Never saved themselves: they are what Auto shows.
            Self::Auto | Self::FilmStandard | Self::CameraMatch(_) => "auto",
            Self::Linear => "linear",
        }
    }

    /// The curve saved as `id`. The film curves that could once be chosen
    /// (Film Standard, Extra Shadow, High Contrast, Linear Response) open
    /// as Standard.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "auto" | "film_standard" | "film_extra_shadow" | "film_high_contrast" | "linear_response" => {
                Some(Self::Auto)
            }
            "linear" => Some(Self::Linear),
            _ => None,
        }
    }

    /// The curve actually applied to a camera's RAW file: [`BaseCurve::Auto`]
    /// becomes Film Standard (until per-camera defaults exist).
    #[must_use]
    pub fn resolved(self) -> Self {
        self.for_source(false)
    }

    /// The curve actually applied to a file that is a finished picture
    /// (`rendered`: JPEG, TIFF) or not: [`BaseCurve::Auto`] becomes
    /// [`BaseCurve::Linear`] for the first, Film Standard for a RAW file.
    #[must_use]
    pub fn for_source(self, rendered: bool) -> Self {
        match (self, rendered) {
            (Self::Auto, true) => Self::Linear,
            (Self::Auto, false) => Self::FilmStandard,
            (other, _) => other,
        }
    }

    /// [`Self::for_source`], with Auto on a RAW file becoming the curve
    /// fitted to the camera's picture when there is one.
    #[must_use]
    pub fn with_camera_fit(self, rendered: bool, fit: Option<CurveFit>) -> Self {
        match (self, rendered, fit) {
            (Self::Auto, false, Some(fit)) => Self::CameraMatch(fit),
            _ => self.for_source(rendered),
        }
    }
}

/// Everything needed to rebuild an edited image from its RAW file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recipe {
    /// The fixed development stage.
    pub develop: DevelopSettings,
    /// The tools after the development stage, in layers.
    pub layers: Layers,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_becomes_the_camera_match_only_for_raw_files_with_a_fit() {
        let fit = CurveFit::new(0.47, 0.19, 1.8, Some(2.5));
        assert_eq!(BaseCurve::Auto.with_camera_fit(false, Some(fit)), BaseCurve::CameraMatch(fit));
        assert_eq!(BaseCurve::Auto.with_camera_fit(false, None), BaseCurve::FilmStandard);
        assert_eq!(BaseCurve::Auto.with_camera_fit(true, Some(fit)), BaseCurve::Linear);
        assert_eq!(BaseCurve::Linear.with_camera_fit(false, Some(fit)), BaseCurve::Linear);
        assert_eq!(BaseCurve::CameraMatch(fit).id(), "auto");
        assert!((fit.slope() - 0.19).abs() < 1e-6 && fit.toe_power() == Some(2.5));
        assert_eq!(CurveFit::new(0.5, 0.2, 2.0, None).toe_power(), None);
    }

    #[test]
    fn only_standard_and_linear_are_offered_and_the_old_film_curves_open_as_standard() {
        for c in BaseCurve::ALL {
            assert_eq!(BaseCurve::from_id(c.id()), Some(c));
        }
        assert_eq!(BaseCurve::Auto.label(), "Standard");
        for old in ["film_standard", "film_extra_shadow", "film_high_contrast", "linear_response"] {
            assert_eq!(BaseCurve::from_id(old), Some(BaseCurve::Auto), "{old}");
        }
        assert_eq!(BaseCurve::from_id("sepia"), None);
    }

    #[test]
    fn choosing_a_database_lens_leaves_the_custom_lens() {
        let custom = LensSettings {
            lens: Some("Canon EF 50mm".into()),
            manual: ManualLens { distortion: 0.3, ..ManualLens::default() },
            custom: Some("My 50".into()),
            ..LensSettings::default()
        };
        let next = custom.with_database_lens(Some("Sigma 35mm".into()));
        assert_eq!(next.custom, None, "the chooser no longer shows the custom lens");
        assert_eq!(next.lens.as_deref(), Some("Sigma 35mm"));
        assert_eq!(next.manual, ManualLens::default(), "the custom lens's own values go with it");
        // Between database lenses the hand-set values stay.
        let tuned =
            LensSettings { manual: ManualLens { distortion: 0.2, ..ManualLens::default() }, ..LensSettings::default() };
        let other = tuned.clone().with_database_lens(None);
        assert_eq!((other.lens, other.manual), (None, tuned.manual));
    }
}
