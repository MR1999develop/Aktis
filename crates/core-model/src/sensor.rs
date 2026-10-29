//! Sensor data as decoded from a RAW file, before demosaicing.

use std::fmt;

use crate::math::Matrix3;

/// Position of red, green and blue in the 2×2 Bayer tile, read left to right,
/// top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BayerPattern {
    /// R G / G B
    Rggb,
    /// B G / G R
    Bggr,
    /// G R / B G
    Grbg,
    /// G B / R G
    Gbrg,
}

impl BayerPattern {
    /// Colour channel (0 = R, 1 = G, 2 = B) of the photosite at `(x, y)`.
    #[must_use]
    pub fn channel_at(self, x: usize, y: usize) -> usize {
        let tile = match self {
            Self::Rggb => [[0, 1], [1, 2]],
            Self::Bggr => [[2, 1], [1, 0]],
            Self::Grbg => [[1, 0], [2, 1]],
            Self::Gbrg => [[1, 2], [0, 1]],
        };
        tile[y % 2][x % 2]
    }
}

/// Layout of the colour filter array over the sensor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfaPattern {
    /// The common 2×2 Bayer layout.
    Bayer(BayerPattern),
    /// Fujifilm X-Trans 6×6 layout; each entry is a channel (0 = R, 1 = G, 2 = B).
    XTrans([[u8; 6]; 6]),
    /// Already demosaiced: three values (camera R, G, B) per pixel, as in
    /// Linear DNG and Apple ProRAW.
    LinearRgb,
}

impl CfaPattern {
    /// Values stored per pixel: 1 for a mosaic, 3 for linear RGB.
    #[must_use]
    pub fn values_per_pixel(&self) -> usize {
        match self {
            Self::Bayer(_) | Self::XTrans(_) => 1,
            Self::LinearRgb => 3,
        }
    }

    /// The kind of filter, for choosing a demosaic method.
    #[must_use]
    pub fn kind(&self) -> crate::SensorKind {
        match self {
            Self::Bayer(_) => crate::SensorKind::Bayer,
            Self::XTrans(_) => crate::SensorKind::XTrans,
            Self::LinearRgb => crate::SensorKind::Rgb,
        }
    }
}

/// Why sensor data could not be accepted.
#[derive(Debug, Clone, PartialEq)]
pub enum SensorError {
    /// Width or height is zero, or the data length does not match.
    BadDimensions,
    /// A white-balance multiplier is not finite and positive.
    BadWhiteBalance([f32; 3]),
}

impl fmt::Display for SensorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadDimensions => write!(f, "sensor data does not match its dimensions"),
            Self::BadWhiteBalance(m) => write!(f, "white balance multipliers {m:?} are not all positive"),
        }
    }
}

impl std::error::Error for SensorError {}

/// Sensor values with the black level removed and scaled so that the white
/// level is 1.0; values below 0 (noise under black) are kept. One value per
/// photosite for a mosaic, three per pixel for [`CfaPattern::LinearRgb`].
#[derive(Clone, PartialEq)]
pub struct SensorImage {
    width: u32,
    height: u32,
    data: Vec<f32>,
    cfa: CfaPattern,
    as_shot_wb: [f32; 3],
    xyz_to_camera: Matrix3,
}

impl fmt::Debug for SensorImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SensorImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("cfa", &self.cfa)
            .field("as_shot_wb", &self.as_shot_wb)
            .finish_non_exhaustive()
    }
}

impl SensorImage {
    /// Builds sensor data.
    ///
    /// `as_shot_wb` are the camera's multipliers for R, G, B (any scale; they
    /// are normalised to green = 1). `xyz_to_camera` maps CIE XYZ (D65) to the
    /// camera's native RGB, as stored in the RAW file's colour matrix.
    ///
    /// # Errors
    /// [`SensorError`] when dimensions and data disagree or a multiplier is not positive.
    pub fn new(
        width: u32,
        height: u32,
        data: Vec<f32>,
        cfa: CfaPattern,
        as_shot_wb: [f32; 3],
        xyz_to_camera: Matrix3,
    ) -> Result<Self, SensorError> {
        let expected = u64::from(width) * u64::from(height) * cfa.values_per_pixel() as u64;
        if width == 0 || height == 0 || u64::try_from(data.len()).ok() != Some(expected) {
            return Err(SensorError::BadDimensions);
        }
        if !as_shot_wb.iter().all(|m| m.is_finite() && *m > 0.0) {
            return Err(SensorError::BadWhiteBalance(as_shot_wb));
        }
        let g = as_shot_wb[1];
        let as_shot_wb = as_shot_wb.map(|m| m / g);
        Ok(Self { width, height, data, cfa, as_shot_wb, xyz_to_camera })
    }

    /// Width in photosites.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in photosites.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Normalised values, row by row ([`CfaPattern::values_per_pixel`] per pixel).
    #[must_use]
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// The colour filter layout.
    #[must_use]
    pub fn cfa(&self) -> &CfaPattern {
        &self.cfa
    }

    /// White balance chosen by the camera, normalised to green = 1.
    #[must_use]
    pub fn as_shot_wb(&self) -> [f32; 3] {
        self.as_shot_wb
    }

    /// CIE XYZ (D65) → camera RGB.
    #[must_use]
    pub fn xyz_to_camera(&self) -> Matrix3 {
        self.xyz_to_camera
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // the expected values are exact
mod tests {
    use super::*;

    #[test]
    fn bayer_patterns_place_channels_as_named() {
        assert_eq!([(0, 0), (1, 0), (0, 1), (1, 1)].map(|(x, y)| BayerPattern::Rggb.channel_at(x, y)), [0, 1, 1, 2]);
        assert_eq!([(0, 0), (1, 0), (0, 1), (1, 1)].map(|(x, y)| BayerPattern::Gbrg.channel_at(x, y)), [1, 2, 0, 1]);
        assert_eq!(BayerPattern::Bggr.channel_at(2, 2), 2);
    }

    #[test]
    fn as_shot_white_balance_is_normalised_to_green() {
        let s = SensorImage::new(
            2,
            2,
            vec![0.0; 4],
            CfaPattern::Bayer(BayerPattern::Rggb),
            [2.0, 1.0 * 2.0, 3.0],
            Matrix3::IDENTITY,
        )
        .expect("valid sensor data");
        assert_eq!(s.as_shot_wb(), [1.0, 1.0, 1.5]);
    }

    #[test]
    fn mismatched_data_and_bad_multipliers_are_refused() {
        let cfa = CfaPattern::Bayer(BayerPattern::Rggb);
        let r = SensorImage::new(2, 2, vec![0.0; 3], cfa.clone(), [1.0; 3], Matrix3::IDENTITY);
        assert_eq!(r.map(|_| ()), Err(SensorError::BadDimensions));
        let r = SensorImage::new(2, 2, vec![0.0; 4], cfa, [1.0, 0.0, 1.0], Matrix3::IDENTITY);
        assert_eq!(r.map(|_| ()), Err(SensorError::BadWhiteBalance([1.0, 0.0, 1.0])));
    }

    #[test]
    fn linear_rgb_data_needs_three_values_per_pixel() {
        let ok = SensorImage::new(2, 1, vec![0.0; 6], CfaPattern::LinearRgb, [1.0; 3], Matrix3::IDENTITY);
        assert!(ok.is_ok());
        let short = SensorImage::new(2, 1, vec![0.0; 2], CfaPattern::LinearRgb, [1.0; 3], Matrix3::IDENTITY);
        assert_eq!(short.map(|_| ()), Err(SensorError::BadDimensions));
    }
}
