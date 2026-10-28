//! Colour science for the editor.
//!
//! The working space is linear Rec.2020 (D65). Conversions to and from other
//! RGB spaces go through CIE XYZ. The camera's own RGB enters through a
//! camera profile ([`ColorProfile`]): the camera's calibration matrices
//! (built in for about 740 cameras, or stored in the DNG), or a DCP file
//! copied into the editor's `profiles` folder ([`read_dcp`], [`ProfileLibrary`]). ICC profiles for export are handled by
//! a colour-management engine in the export crate, not here.

mod dcp;
mod library;
mod profile;
mod temperature;

pub use dcp::{DcpError, MAX_DCP_BYTES, read_dcp, write_dcp};
pub use library::{CameraNames, LoadError, ProfileEntry, ProfileLibrary, load_dcp};
pub use profile::{
    Calibration, ColorProfile, HsvTable, MAX_TABLE_ENTRIES, ProfileError, ProfileSource, ProfileTransform,
};
pub use temperature::{
    D50_XY, D50_XYZ, D65_XYZ, KELVIN_RANGE, TINT_PER_DUV, bradford, kelvin_tint_to_xy, planck_uv, rebalance, uv_to_xy,
    white_of, xy_to_kelvin_tint, xy_to_temperature, xy_to_uv, xy_to_xyz, xyz_to_xy,
};

use core_model::{Matrix3, WorkingSpace};

/// sRGB transfer curve: linear → encoded. Inverse of [`srgb_decode`].
///
/// Piecewise as in IEC 61966-2-1 (linear toe, then power 1/2.4); the pure
/// 2.2 power is a display model, not this conversion. Negative input maps
/// to negative output so the function stays invertible.
#[must_use]
pub fn srgb_encode(linear: f32) -> f32 {
    let a = linear.abs();
    let e = if a <= 0.003_130_8 { 12.92 * a } else { 1.055 * a.powf(1.0 / 2.4) - 0.055 };
    e.copysign(linear)
}

/// sRGB transfer curve: encoded → linear. Inverse of [`srgb_encode`].
#[must_use]
pub fn srgb_decode(encoded: f32) -> f32 {
    let a = encoded.abs();
    let l = if a <= 0.040_45 { a / 12.92 } else { ((a + 0.055) / 1.055).powf(2.4) };
    l.copysign(encoded)
}

/// Linear Rec.2020 (D65) → CIE XYZ (D65). ITU-R BT.2020.
pub const REC2020_TO_XYZ: Matrix3 = Matrix3::from_rows([
    [0.636_958, 0.144_617, 0.168_881],
    [0.262_700, 0.677_998, 0.059_302],
    [0.000_000, 0.028_073, 1.060_985],
]);

/// Linear sRGB / Rec.709 (D65) → CIE XYZ (D65). IEC 61966-2-1.
pub const SRGB_TO_XYZ: Matrix3 = Matrix3::from_rows([
    [0.412_391, 0.357_584, 0.180_481],
    [0.212_639, 0.715_169, 0.072_192],
    [0.019_331, 0.119_195, 0.950_532],
]);

/// The matrix from `space` to CIE XYZ (D65).
#[must_use]
pub fn to_xyz(space: WorkingSpace) -> Matrix3 {
    match space {
        WorkingSpace::LinearRec2020 => REC2020_TO_XYZ,
        WorkingSpace::LinearSrgb => SRGB_TO_XYZ,
    }
}

/// The matrix converting linear RGB from `from` to `to` (same white point).
///
/// # Panics
/// Never for the spaces defined in [`WorkingSpace`]: their matrices are invertible.
#[must_use]
pub fn rgb_to_rgb(from: WorkingSpace, to: WorkingSpace) -> Matrix3 {
    if from == to {
        return Matrix3::IDENTITY;
    }
    let xyz_to_target = to_xyz(to).inverse().expect("standard primaries are invertible");
    xyz_to_target * to_xyz(from)
}

/// Builds the matrix from white-balanced camera RGB to `space`.
///
/// `xyz_to_camera` is the RAW file's colour matrix (XYZ D65 → camera RGB).
/// Each row of `xyz_to_camera · working_to_xyz` is normalised so that a
/// neutral surface, after white balance, stays neutral in `space`.
/// Returns `None` when the camera matrix is degenerate.
#[must_use]
pub fn camera_to_working(xyz_to_camera: Matrix3, space: WorkingSpace) -> Option<Matrix3> {
    let working_to_camera = (xyz_to_camera * to_xyz(space)).normalize_rows();
    working_to_camera.inverse()
}

/// Relative luminance (CIE Y) of a linear pixel in `space`.
#[must_use]
pub fn luminance(rgb: [f32; 3], space: WorkingSpace) -> f32 {
    let m = to_xyz(space).0[1];
    m[0] * rgb[0] + m[1] * rgb[1] + m[2] * rgb[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_encode_and_decode_are_inverse_over_the_full_range() {
        for i in -100..=1100 {
            #[allow(clippy::cast_precision_loss)] // small integers
            let x = i as f32 / 1000.0;
            let back = srgb_decode(srgb_encode(x));
            assert!((back - x).abs() < 1e-5, "x = {x}, back = {back}");
        }
    }

    #[test]
    fn srgb_curve_matches_reference_values() {
        assert!((srgb_encode(0.18) - 0.461_356).abs() < 1e-4);
        assert!((srgb_encode(1.0) - 1.0).abs() < 1e-6);
        assert!(srgb_encode(0.0).abs() < 1e-9);
        assert!((srgb_decode(0.5) - 0.214_041).abs() < 1e-4);
    }

    #[test]
    fn white_has_luminance_one_in_both_spaces() {
        for space in [WorkingSpace::LinearRec2020, WorkingSpace::LinearSrgb] {
            assert!((luminance([1.0; 3], space) - 1.0).abs() < 1e-3, "{space:?}");
        }
    }

    #[test]
    fn rec2020_to_srgb_keeps_greys_grey_and_round_trips() {
        let m = rgb_to_rgb(WorkingSpace::LinearRec2020, WorkingSpace::LinearSrgb);
        let back = rgb_to_rgb(WorkingSpace::LinearSrgb, WorkingSpace::LinearRec2020);
        for g in [0.0_f32, 0.18, 1.0, 4.0] {
            let s = m.apply([g; 3]);
            assert!(s.iter().all(|v| (v - g).abs() < 1e-3 * g.max(1.0)), "grey {g} → {s:?}");
        }
        let c = [0.2, 0.5, 0.1];
        let rt = back.apply(m.apply(c));
        assert!(c.iter().zip(rt).all(|(a, b)| (a - b).abs() < 1e-5), "{rt:?}");
    }

    #[test]
    fn a_white_balanced_neutral_stays_neutral_after_the_camera_matrix() {
        // A plausible camera matrix (XYZ → camera), not tied to a real model.
        let xyz_to_cam = Matrix3::from_rows([[0.67, -0.07, -0.06], [-0.43, 1.23, 0.22], [-0.08, 0.17, 0.62]]);
        let m = camera_to_working(xyz_to_cam, WorkingSpace::LinearRec2020).expect("invertible");
        let w = m.apply([0.5, 0.5, 0.5]);
        assert!(w.iter().all(|v| (v - 0.5).abs() < 1e-4), "{w:?}");
    }
}
