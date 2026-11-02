//! Camera colour profiles: from white-balanced camera RGB to the working space.
//!
//! A profile holds one or two calibrations (usually under standard illuminant
//! A and D65), each with a colour matrix (XYZ → camera) and optionally a
//! forward matrix (white-balanced camera → XYZ D50) and a hue/saturation map.
//! The image's white balance decides how the two are mixed, as the DNG
//! specification (chapter 6) describes:
//!
//! 1. the camera neutral (the inverse of the white-balance multipliers) is
//!    turned into a white chromaticity, iterating because the matrix used
//!    depends on that white's temperature;
//! 2. the calibrations are weighted by inverse temperature between their
//!    illuminants;
//! 3. with forward matrices, balanced camera RGB goes straight to XYZ D50;
//!    otherwise the inverse colour matrix gives XYZ under the scene white,
//!    which a Bradford adaptation brings to D50;
//! 4. hue/saturation maps and the look table, if any, act in linear ProPhoto
//!    RGB; then a Bradford adaptation to D65 and the working space's primaries.
//!
//! A profile's tone curve is read but not applied: the pipeline stays
//! scene-referred until tone mapping (phase 4).

use std::fmt;

use core_model::{CalibratedMatrix, Illuminant, Matrix3, WorkingSpace};

use crate::temperature::{self, D50_XY, D50_XYZ, D65_XYZ};

/// Linear ProPhoto (ROMM) RGB → XYZ D50.
const PROPHOTO_TO_XYZ_D50: Matrix3 = Matrix3::from_rows([
    [0.797_674_9, 0.135_191_7, 0.031_353_4],
    [0.288_040_2, 0.711_874_1, 0.000_085_7],
    [0.0, 0.0, 0.825_21],
]);

/// Largest table accepted (Adobe's largest are 90 × 30 × 1 and 36 × 8 × 16).
pub const MAX_TABLE_ENTRIES: usize = 360 * 100 * 64;

/// A three-dimensional table of hue shift (degrees), saturation scale and
/// value scale, indexed by hue, saturation and value (DNG `ProfileHueSatMap`
/// and `ProfileLookTable`).
#[derive(Clone, PartialEq)]
pub struct HsvTable {
    hue_divisions: usize,
    sat_divisions: usize,
    val_divisions: usize,
    /// The value axis is sRGB-encoded (encoding 1) instead of linear.
    srgb_value: bool,
    /// Entries with value outermost, then hue, then saturation.
    data: Vec<[f32; 3]>,
}

impl fmt::Debug for HsvTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HsvTable")
            .field("hue_divisions", &self.hue_divisions)
            .field("sat_divisions", &self.sat_divisions)
            .field("val_divisions", &self.val_divisions)
            .field("srgb_value", &self.srgb_value)
            .finish_non_exhaustive()
    }
}

impl HsvTable {
    /// Builds a table from its dimensions and `hue × sat × val × 3` floats.
    /// `None` when the sizes disagree, a dimension is out of range, or an
    /// entry is not finite.
    #[must_use]
    pub fn new(dims: [u32; 3], values: &[f32], srgb_value: bool) -> Option<Self> {
        let [h, s, v] = dims.map(|d| d as usize);
        // The DNG specification requires at least two saturation divisions.
        if h == 0 || s < 2 || v == 0 {
            return None;
        }
        let entries = h.checked_mul(s)?.checked_mul(v)?;
        if entries > MAX_TABLE_ENTRIES || values.len() != entries.checked_mul(3)? {
            return None;
        }
        if values.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let data = values.as_chunks::<3>().0.to_vec();
        Some(Self { hue_divisions: h, sat_divisions: s, val_divisions: v, srgb_value, data })
    }

    /// Dimensions: hue, saturation, value divisions.
    #[must_use]
    pub fn dims(&self) -> [usize; 3] {
        [self.hue_divisions, self.sat_divisions, self.val_divisions]
    }

    /// `w · self + (1 − w) · other` entry by entry; `self` when the shapes differ.
    #[must_use]
    pub fn blend(&self, other: &Self, w: f32) -> Self {
        if self.dims() != other.dims() {
            return self.clone();
        }
        let data = self
            .data
            .iter()
            .zip(&other.data)
            .map(|(a, b)| std::array::from_fn(|i| w * a[i] + (1.0 - w) * b[i]))
            .collect();
        Self { data, ..self.clone() }
    }

    fn entry(&self, v: usize, h: usize, s: usize) -> [f32; 3] {
        self.data[(v * self.hue_divisions + h) * self.sat_divisions + s]
    }

    /// Applies the table to an HSV colour (hue in [0, 6), as the DNG SDK).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    fn apply(&self, hsv: [f32; 3]) -> [f32; 3] {
        let [h, s, v] = hsv;
        let h_scaled = h * self.hue_divisions as f32 / 6.0;
        let s_scaled = (s * (self.sat_divisions - 1) as f32).clamp(0.0, (self.sat_divisions - 1) as f32);
        let v_encoded = if self.srgb_value { crate::srgb_encode(v) } else { v };
        let v_scaled = if self.val_divisions < 2 {
            0.0
        } else {
            (v_encoded * (self.val_divisions - 1) as f32).clamp(0.0, (self.val_divisions - 1) as f32)
        };

        let mut h0 = (h_scaled.max(0.0) as usize).min(self.hue_divisions - 1);
        let mut h1 = h0 + 1;
        if h0 >= self.hue_divisions - 1 {
            h0 = self.hue_divisions - 1;
            h1 = 0; // hue wraps around
        }
        let s0 = (s_scaled as usize).min(self.sat_divisions - 2);
        let v0 = if self.val_divisions < 2 { 0 } else { (v_scaled as usize).min(self.val_divisions - 2) };
        let v1 = if self.val_divisions < 2 { 0 } else { v0 + 1 };
        let hf = (h_scaled - h0 as f32).clamp(0.0, 1.0);
        let sf = s_scaled - s0 as f32;
        let vf = v_scaled - v0 as f32;

        let mut out = [0.0_f32; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let at = |v: usize, s: usize| (1.0 - hf) * self.entry(v, h0, s)[c] + hf * self.entry(v, h1, s)[c];
            let lo = (1.0 - sf) * at(v0, s0) + sf * at(v0, s0 + 1);
            let hi = (1.0 - sf) * at(v1, s0) + sf * at(v1, s0 + 1);
            *o = (1.0 - vf) * lo + vf * hi;
        }
        let [hue_shift, sat_scale, val_scale] = out;

        let mut h = h + hue_shift * 6.0 / 360.0;
        h = h.rem_euclid(6.0);
        let s = (s * sat_scale).clamp(0.0, 1.0);
        let v = if self.srgb_value { crate::srgb_decode(v_encoded * val_scale) } else { v * val_scale };
        [h, s, v]
    }
}

/// Linear RGB → HSV with hue in [0, 6); `None` for colours with a negative
/// component or black, which tables leave alone.
#[allow(clippy::float_cmp)] // `max` is one of the three values, compared exactly on purpose
fn rgb_to_hsv(rgb: [f32; 3]) -> Option<[f32; 3]> {
    let [r, g, b] = rgb;
    if r < 0.0 || g < 0.0 || b < 0.0 {
        return None;
    }
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max <= 0.0 {
        return None;
    }
    let delta = max - min;
    let s = delta / max;
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        let h = (g - b) / delta;
        if h < 0.0 { h + 6.0 } else { h }
    } else if max == g {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    Some([h, s, max])
}

/// HSV (hue in [0, 6)) → linear RGB.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // h is in [0, 6)
fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    if s <= 0.0 {
        return [v, v, v];
    }
    let h = h.rem_euclid(6.0);
    let i = (h as usize).min(5);
    let f = h - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// One calibration of a profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    /// Light of the calibration.
    pub illuminant: Illuminant,
    /// XYZ → camera native RGB.
    pub color_matrix: Matrix3,
    /// White-balanced camera RGB → XYZ D50, if the profile has one.
    pub forward_matrix: Option<Matrix3>,
    /// Hue/saturation map, if the profile has one.
    pub hue_sat_map: Option<HsvTable>,
}

/// Where a profile comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSource {
    /// The camera's matrices, from the editor's camera database or the DNG file.
    Builtin,
    /// A DCP file.
    Dcp(std::path::PathBuf),
}

/// A camera colour profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorProfile {
    /// Profile name ("Adobe Standard"); "Built-in" for the camera's matrices.
    pub name: String,
    /// The camera the profile is for, as the profile names it.
    pub camera: String,
    /// Where it comes from.
    pub source: ProfileSource,
    /// Copyright notice of the profile, if any.
    pub copyright: Option<String>,
    /// Exposure offset in stops the profile asks for (applied with tone mapping).
    pub baseline_exposure_offset: f32,
    calibrations: Vec<Calibration>,
    look_table: Option<HsvTable>,
    tone_curve: Option<Vec<[f32; 2]>>,
}

/// Why a profile cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    /// The profile has no usable colour matrix.
    NoColorMatrix,
    /// A matrix cannot be inverted.
    DegenerateMatrix,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoColorMatrix => write!(f, "the profile has no colour matrix"),
            Self::DegenerateMatrix => write!(f, "a matrix of the profile cannot be inverted"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl ColorProfile {
    /// A profile with the given calibrations (one or two are used, ordered by
    /// temperature).
    ///
    /// # Errors
    /// [`ProfileError::NoColorMatrix`] without calibrations.
    pub fn new(
        name: impl Into<String>,
        camera: impl Into<String>,
        source: ProfileSource,
        mut calibrations: Vec<Calibration>,
        look_table: Option<HsvTable>,
        tone_curve: Option<Vec<[f32; 2]>>,
    ) -> Result<Self, ProfileError> {
        calibrations.retain(|c| c.color_matrix.inverse().is_some());
        if calibrations.is_empty() {
            return Err(ProfileError::NoColorMatrix);
        }
        // Keep the two calibrations farthest apart in temperature, lower first.
        calibrations.sort_by(|a, b| {
            let t = |c: &Calibration| c.illuminant.temperature().unwrap_or(f32::MAX);
            t(a).total_cmp(&t(b))
        });
        if calibrations.len() > 2 {
            let last = calibrations.pop().expect("more than two");
            calibrations.truncate(1);
            calibrations.push(last);
        }
        Ok(Self {
            name: name.into(),
            camera: camera.into(),
            source,
            copyright: None,
            baseline_exposure_offset: 0.0,
            calibrations,
            look_table,
            tone_curve,
        })
    }

    /// The camera's own matrices as a profile.
    ///
    /// # Errors
    /// [`ProfileError::NoColorMatrix`] when no matrix is usable.
    pub fn from_matrices(camera: impl Into<String>, matrices: &[CalibratedMatrix]) -> Result<Self, ProfileError> {
        let calibrations = matrices
            .iter()
            .map(|m| Calibration {
                illuminant: m.illuminant,
                color_matrix: m.xyz_to_camera,
                forward_matrix: None,
                hue_sat_map: None,
            })
            .collect();
        Self::new("Built-in", camera, ProfileSource::Builtin, calibrations, None, None)
    }

    /// The calibrations, lower temperature first.
    #[must_use]
    pub fn calibrations(&self) -> &[Calibration] {
        &self.calibrations
    }

    /// The look table, if any.
    #[must_use]
    pub fn look_table(&self) -> Option<&HsvTable> {
        self.look_table.as_ref()
    }

    /// The profile's tone curve (input, output pairs in [0, 1]), if any.
    #[must_use]
    pub fn tone_curve(&self) -> Option<&[[f32; 2]]> {
        self.tone_curve.as_deref()
    }

    /// Weight of the first (lower-temperature) calibration for a white of
    /// `temperature` kelvin: 1 at or below its illuminant, 0 at or above the
    /// second's, inverse-temperature interpolation between.
    fn weight(&self, temperature: f64) -> f64 {
        let [a, b] = match self.calibrations.as_slice() {
            [a, b] => [a, b],
            _ => return 1.0,
        };
        let (Some(t1), Some(t2)) = (a.illuminant.temperature(), b.illuminant.temperature()) else {
            return 1.0;
        };
        let (t1, t2) = (f64::from(t1), f64::from(t2));
        if t1 >= t2 || temperature <= t1 {
            1.0
        } else if temperature >= t2 {
            0.0
        } else {
            (1.0 / temperature - 1.0 / t2) / (1.0 / t1 - 1.0 / t2)
        }
    }

    fn mix(&self, w: f64, pick: impl Fn(&Calibration) -> Option<Matrix3>) -> Option<Matrix3> {
        #[allow(clippy::cast_possible_truncation)] // a weight in [0, 1]
        let w = w as f32;
        match self.calibrations.as_slice() {
            [a, b] => match (pick(a), pick(b)) {
                (Some(x), Some(y)) => Some(Matrix3(std::array::from_fn(|i| {
                    std::array::from_fn(|j| w * x.0[i][j] + (1.0 - w) * y.0[i][j])
                }))),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
            [a] => pick(a),
            _ => None,
        }
    }

    /// White chromaticity of the camera `neutral`, by the DNG SDK's fixed
    /// point iteration (the matrix depends on the white it is looking for).
    fn neutral_to_xy(&self, neutral: [f32; 3]) -> [f64; 2] {
        let mut xy = D50_XY;
        for pass in 0..30 {
            let w = self.weight(temperature::xy_to_temperature(xy));
            let Some(inv) = self.mix(w, |c| Some(c.color_matrix)).and_then(|m| m.inverse()) else { return xy };
            let next = temperature::xyz_to_xy(inv.apply(neutral).map(f64::from));
            if (next[0] - xy[0]).abs() + (next[1] - xy[1]).abs() < 1e-7 {
                return next;
            }
            // Not converging: most likely oscillating between two values.
            xy = if pass == 29 { [(xy[0] + next[0]) / 2.0, (xy[1] + next[1]) / 2.0] } else { next };
        }
        xy
    }

    /// The conversion for an image white-balanced with `multipliers`
    /// (R, G, B, any scale), into `space`.
    ///
    /// # Errors
    /// [`ProfileError::DegenerateMatrix`] when a matrix cannot be inverted.
    pub fn transform(&self, multipliers: [f32; 3], space: WorkingSpace) -> Result<ProfileTransform, ProfileError> {
        let g = multipliers[1];
        let neutral = multipliers.map(|m| if m > 0.0 && m.is_finite() { g / m } else { 1.0 });
        let white_xy = self.neutral_to_xy(neutral);
        let temperature = temperature::xy_to_temperature(white_xy);
        let w = self.weight(temperature);
        let color_matrix = self.mix(w, |c| Some(c.color_matrix)).ok_or(ProfileError::NoColorMatrix)?;

        // White-balanced camera RGB → XYZ D50.
        let camera_to_pcs = match self.mix(w, |c| c.forward_matrix) {
            Some(fm) => fm,
            None => {
                let white_xyz = temperature::xy_to_xyz(white_xy);
                let inv = color_matrix.inverse().ok_or(ProfileError::DegenerateMatrix)?;
                let diag = Matrix3::from_rows([[neutral[0], 0.0, 0.0], [0.0, neutral[1], 0.0], [0.0, 0.0, neutral[2]]]);
                temperature::bradford(white_xyz, D50_XYZ) * inv * diag
            }
        };
        // Keep exposure: balanced (1, 1, 1) has luminance 1, as without a profile.
        let y = camera_to_pcs.apply([1.0; 3])[1];
        if !(y.is_finite() && y > 1e-6) {
            return Err(ProfileError::DegenerateMatrix);
        }
        let camera_to_pcs = Matrix3(camera_to_pcs.0.map(|r| r.map(|v| v / y)));

        let xyz_to_working = crate::to_xyz(space).inverse().ok_or(ProfileError::DegenerateMatrix)?;
        let pcs_to_working = xyz_to_working * temperature::bradford(D50_XYZ, D65_XYZ);

        #[allow(clippy::cast_possible_truncation)] // a weight in [0, 1]
        let wf = w as f32;
        let hue_sat_map = match self.calibrations.as_slice() {
            [a, b] => match (&a.hue_sat_map, &b.hue_sat_map) {
                (Some(x), Some(y)) => Some(x.blend(y, wf)),
                (Some(x), None) | (None, Some(x)) => Some(x.clone()),
                (None, None) => None,
            },
            [a] => a.hue_sat_map.clone(),
            _ => None,
        };
        let tables: Vec<HsvTable> = hue_sat_map.into_iter().chain(self.look_table.clone()).collect();

        let kind = if tables.is_empty() {
            TransformKind::Matrix(pcs_to_working * camera_to_pcs)
        } else {
            let pcs_to_prophoto = PROPHOTO_TO_XYZ_D50.inverse().ok_or(ProfileError::DegenerateMatrix)?;
            TransformKind::Tables {
                to_prophoto: pcs_to_prophoto * camera_to_pcs,
                tables,
                to_working: pcs_to_working * PROPHOTO_TO_XYZ_D50,
            }
        };
        Ok(ProfileTransform { kind, white_xy, temperature })
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TransformKind {
    Matrix(Matrix3),
    Tables { to_prophoto: Matrix3, tables: Vec<HsvTable>, to_working: Matrix3 },
}

/// A profile prepared for one white balance: white-balanced camera RGB →
/// working space, per pixel.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileTransform {
    kind: TransformKind,
    white_xy: [f64; 2],
    temperature: f64,
}

impl ProfileTransform {
    /// Converts one pixel.
    #[must_use]
    pub fn apply(&self, camera: [f32; 3]) -> [f32; 3] {
        match &self.kind {
            TransformKind::Matrix(m) => m.apply(camera),
            TransformKind::Tables { to_prophoto, tables, to_working } => {
                let mut rgb = to_prophoto.apply(camera);
                for t in tables {
                    if let Some(hsv) = rgb_to_hsv(rgb) {
                        rgb = hsv_to_rgb(t.apply(hsv));
                    }
                }
                to_working.apply(rgb)
            }
        }
    }

    /// `true` when the profile uses tables, i.e. is more than a matrix.
    #[must_use]
    pub fn has_tables(&self) -> bool {
        matches!(self.kind, TransformKind::Tables { .. })
    }

    /// Chromaticity of the white the image was balanced to.
    #[must_use]
    pub fn white_xy(&self) -> [f64; 2] {
        self.white_xy
    }

    /// Correlated colour temperature of that white, in kelvin.
    #[must_use]
    pub fn temperature(&self) -> f64 {
        self.temperature
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sony ILCE-7RM5 matrices (as in rawler's camera data).
    fn a7r5() -> Vec<CalibratedMatrix> {
        vec![
            CalibratedMatrix {
                illuminant: Illuminant::StandardA,
                xyz_to_camera: Matrix3::from_rows([
                    [0.9185, -0.4857, 0.0505],
                    [-0.3651, 1.1061, 0.2982],
                    [-0.0161, 0.0698, 0.6769],
                ]),
            },
            CalibratedMatrix {
                illuminant: Illuminant::D65,
                xyz_to_camera: Matrix3::from_rows([
                    [0.82, -0.2976, -0.0719],
                    [-0.4296, 1.2053, 0.2532],
                    [-0.0429, 0.1282, 0.5774],
                ]),
            },
        ]
    }

    /// Multipliers that make the camera see `white_xy` as neutral, for the given matrix.
    fn multipliers_for(m: Matrix3, white_xy: [f64; 2]) -> [f32; 3] {
        #[allow(clippy::cast_possible_truncation)]
        let n = m.apply(temperature::xy_to_xyz(white_xy).map(|v| v as f32));
        [n[1] / n[0], 1.0, n[1] / n[2]]
    }

    #[test]
    fn a_white_balanced_neutral_stays_neutral_at_any_temperature() {
        let p = ColorProfile::from_matrices("Sony ILCE-7RM5", &a7r5()).expect("profile");
        for (xy, mat) in [([0.3127, 0.3290], 1), ([0.4476, 0.4074], 0), ([0.3457, 0.3585], 1)] {
            let mult = multipliers_for(p.calibrations()[mat].color_matrix, xy);
            let t = p.transform(mult, WorkingSpace::LinearRec2020).expect("transform");
            let grey = t.apply([0.4, 0.4, 0.4]);
            assert!(grey.iter().all(|v| (v - 0.4).abs() < 2e-3), "white {xy:?}: {grey:?}");
        }
    }

    #[test]
    fn the_white_temperature_picks_the_calibration() {
        let p = ColorProfile::from_matrices("Sony ILCE-7RM5", &a7r5()).expect("profile");
        // Under illuminant A the A matrix alone is used, so its white is found again.
        let mult = multipliers_for(p.calibrations()[0].color_matrix, [0.4476, 0.4074]);
        let t = p.transform(mult, WorkingSpace::LinearRec2020).expect("transform");
        assert!((t.temperature() - 2856.0).abs() < 40.0, "{}", t.temperature());
        let mult = multipliers_for(p.calibrations()[1].color_matrix, [0.3127, 0.3290]);
        let t = p.transform(mult, WorkingSpace::LinearRec2020).expect("transform");
        assert!((t.temperature() - 6504.0).abs() < 40.0, "{}", t.temperature());
        assert!((p.weight(2000.0) - 1.0).abs() < 1e-12 && p.weight(9000.0).abs() < 1e-12);
        let mid = p.weight(4000.0);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn hsv_round_trips() {
        for rgb in [[0.2_f32, 0.5, 0.9], [0.9, 0.1, 0.1], [0.3, 0.3, 0.3], [0.0, 0.6, 0.2]] {
            let back = hsv_to_rgb(rgb_to_hsv(rgb).expect("positive"));
            assert!(rgb.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-6), "{rgb:?} → {back:?}");
        }
        assert!(rgb_to_hsv([-0.1, 0.5, 0.5]).is_none());
    }

    #[test]
    fn an_identity_table_changes_nothing_and_a_hue_table_rotates_hue() {
        let dims = [6_u32, 2, 1];
        let identity: Vec<f32> = (0..12).flat_map(|_| [0.0, 1.0, 1.0]).collect();
        let t = HsvTable::new(dims, &identity, false).expect("table");
        let hsv = [1.3, 0.4, 0.7];
        let out = t.apply(hsv);
        assert!(hsv.iter().zip(out).all(|(a, b)| (a - b).abs() < 1e-6), "{out:?}");

        let shift: Vec<f32> = (0..12).flat_map(|_| [60.0, 0.5, 1.0]).collect();
        let t = HsvTable::new(dims, &shift, false).expect("table");
        let out = t.apply([5.5, 0.8, 0.7]);
        assert!((out[0] - 0.5).abs() < 1e-5 && (out[1] - 0.4).abs() < 1e-6, "{out:?}");
        assert!(HsvTable::new([6, 1, 1], &[0.0; 18], false).is_none());
        assert!(HsvTable::new(dims, &[0.0; 35], false).is_none());
    }

    #[test]
    fn forward_matrices_take_balanced_camera_rgb_to_d50() {
        // A forward matrix mapping balanced (1, 1, 1) to D50 must give grey in the working space.
        let fm = Matrix3::from_rows([[0.7, 0.2, 0.06422], [0.3, 0.6, 0.1], [0.0, 0.1, 0.72521]]);
        let mut cal = vec![Calibration {
            illuminant: Illuminant::D65,
            color_matrix: a7r5()[1].xyz_to_camera,
            forward_matrix: Some(fm),
            hue_sat_map: None,
        }];
        cal[0].forward_matrix = Some(fm);
        let p = ColorProfile::new("Test", "Cam", ProfileSource::Builtin, cal, None, None).expect("profile");
        let t = p.transform([2.0, 1.0, 1.5], WorkingSpace::LinearSrgb).expect("transform");
        let grey = t.apply([0.25; 3]);
        assert!(grey.iter().all(|v| (v - 0.25).abs() < 1e-3), "{grey:?}");
    }
}
