//! What a RAW file says about how it was taken: camera, lens, settings and
//! the camera's colour calibration.

use crate::math::Matrix3;

/// A light source as coded in EXIF and DNG (`LightSource`,
/// `CalibrationIlluminant`). Only the codes colour calibrations use are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Illuminant {
    /// 1: daylight.
    Daylight,
    /// 2: fluorescent.
    Fluorescent,
    /// 3: tungsten (incandescent).
    Tungsten,
    /// 4: flash.
    Flash,
    /// 9: fine weather.
    FineWeather,
    /// 10: cloudy weather.
    CloudyWeather,
    /// 11: shade.
    Shade,
    /// 12: daylight fluorescent (D 5700–7100 K).
    DaylightFluorescent,
    /// 13: day white fluorescent (N 4600–5500 K).
    DayWhiteFluorescent,
    /// 14: cool white fluorescent (W 3800–4500 K).
    CoolWhiteFluorescent,
    /// 15: white fluorescent (WW 3250–3800 K).
    WhiteFluorescent,
    /// 16: warm white fluorescent (L 2600–3250 K).
    WarmWhiteFluorescent,
    /// 17: CIE standard illuminant A.
    StandardA,
    /// 18: CIE standard illuminant B.
    StandardB,
    /// 19: CIE standard illuminant C.
    StandardC,
    /// 20: D55.
    D55,
    /// 21: D65.
    D65,
    /// 22: D75.
    D75,
    /// 23: D50.
    D50,
    /// 24: ISO studio tungsten.
    IsoStudioTungsten,
    /// Any other code (0 "unknown", 255 "other").
    Other(u16),
}

impl Illuminant {
    /// The illuminant with EXIF/DNG code `code`.
    #[must_use]
    pub fn from_code(code: u16) -> Self {
        match code {
            1 => Self::Daylight,
            2 => Self::Fluorescent,
            3 => Self::Tungsten,
            4 => Self::Flash,
            9 => Self::FineWeather,
            10 => Self::CloudyWeather,
            11 => Self::Shade,
            12 => Self::DaylightFluorescent,
            13 => Self::DayWhiteFluorescent,
            14 => Self::CoolWhiteFluorescent,
            15 => Self::WhiteFluorescent,
            16 => Self::WarmWhiteFluorescent,
            17 => Self::StandardA,
            18 => Self::StandardB,
            19 => Self::StandardC,
            20 => Self::D55,
            21 => Self::D65,
            22 => Self::D75,
            23 => Self::D50,
            24 => Self::IsoStudioTungsten,
            other => Self::Other(other),
        }
    }

    /// Correlated colour temperature in kelvin, with the values the DNG SDK
    /// uses to weight dual-illuminant calibrations; `None` when unknown.
    #[must_use]
    pub fn temperature(self) -> Option<f32> {
        Some(match self {
            Self::StandardA | Self::Tungsten => 2850.0,
            Self::IsoStudioTungsten => 3200.0,
            Self::D50 => 5000.0,
            Self::D55 | Self::Daylight | Self::FineWeather | Self::Flash | Self::StandardB => 5500.0,
            Self::D65 | Self::StandardC | Self::CloudyWeather => 6500.0,
            Self::D75 | Self::Shade => 7500.0,
            Self::DaylightFluorescent => (5700.0 + 7100.0) * 0.5,
            Self::DayWhiteFluorescent => (4600.0 + 5500.0) * 0.5,
            Self::CoolWhiteFluorescent | Self::Fluorescent => (3800.0 + 4500.0) * 0.5,
            Self::WhiteFluorescent => (3250.0 + 3800.0) * 0.5,
            Self::WarmWhiteFluorescent => (2600.0 + 3250.0) * 0.5,
            Self::Other(_) => return None,
        })
    }
}

/// A camera colour matrix measured under one illuminant: CIE XYZ → camera
/// RGB, as in the DNG `ColorMatrix` tags.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibratedMatrix {
    /// Light of the calibration.
    pub illuminant: Illuminant,
    /// XYZ → camera native RGB.
    pub xyz_to_camera: Matrix3,
}

/// Shooting data read from the file. Strings are as the file gives them;
/// numbers are `None` when the file does not record them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptureInfo {
    /// Camera maker as written in the file ("NIKON CORPORATION").
    pub camera_make: String,
    /// Camera model as written in the file ("NIKON D3S").
    pub camera_model: String,
    /// Camera maker, cleaned up ("Nikon").
    pub clean_make: String,
    /// Camera model, cleaned up ("D3S").
    pub clean_model: String,
    /// Lens maker, if recorded or known from the lens id.
    pub lens_make: Option<String>,
    /// Lens model, if recorded or known from the lens id.
    pub lens_model: Option<String>,
    /// Focal length in mm.
    pub focal_length: Option<f32>,
    /// F-number.
    pub aperture: Option<f32>,
    /// Focus distance in metres.
    pub focus_distance: Option<f32>,
    /// The camera's colour matrices, one per calibration illuminant (usually
    /// standard illuminant A and D65).
    pub color_matrices: Vec<CalibratedMatrix>,
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // table values
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_temperatures_follow_the_dng_sdk() {
        for code in [1_u16, 17, 21, 23, 255] {
            let i = Illuminant::from_code(code);
            assert_eq!(matches!(i, Illuminant::Other(_)), code == 255, "{code}");
        }
        assert_eq!(Illuminant::StandardA.temperature(), Some(2850.0));
        assert_eq!(Illuminant::D65.temperature(), Some(6500.0));
        assert_eq!(Illuminant::Other(0).temperature(), None);
    }
}
