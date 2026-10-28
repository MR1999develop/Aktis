//! Chromaticity, colour temperature and chromatic adaptation.

use core_model::Matrix3;

/// White point of CIE D50 (the DNG profile connection space), XYZ with Y = 1.
pub const D50_XYZ: [f64; 3] = [0.964_22, 1.0, 0.825_21];

/// White point of CIE D65 (the working space's white), XYZ with Y = 1.
pub const D65_XYZ: [f64; 3] = [0.950_47, 1.0, 1.088_83];

/// D50 as an xy chromaticity, as the DNG SDK writes it.
pub const D50_XY: [f64; 2] = [0.3457, 0.3585];

/// XYZ → xy; D50 for black or negative sums.
#[must_use]
pub fn xyz_to_xy(xyz: [f64; 3]) -> [f64; 2] {
    let sum = xyz[0] + xyz[1] + xyz[2];
    if sum > 0.0 && sum.is_finite() { [xyz[0] / sum, xyz[1] / sum] } else { D50_XY }
}

/// xy → XYZ with Y = 1, keeping xy inside the valid triangle.
#[must_use]
pub fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    let mut x = xy[0].clamp(0.000_001, 0.999_999);
    let mut y = xy[1].clamp(0.000_001, 0.999_999);
    let sum = x + y;
    if sum > 0.999_999 {
        let scale = 0.999_999 / sum;
        x *= scale;
        y *= scale;
    }
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Robertson's isotemperature lines: (mired, u, v, slope). The values are the
/// DNG SDK's (including its 325-mired entry), so dual-illuminant profiles are
/// weighted exactly as Adobe's reference code weights them.
const ROBERTSON: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24702, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

/// Correlated colour temperature (kelvin) of chromaticity `xy`, by
/// Robertson's method. Valid from about 1667 K to infinity.
#[must_use]
pub fn xy_to_temperature(xy: [f64; 2]) -> f64 {
    let denom = 1.5 - xy[0] + 6.0 * xy[1];
    let u = 2.0 * xy[0] / denom;
    let v = 3.0 * xy[1] / denom;
    let mut last_dt = 0.0;
    for i in 1..ROBERTSON.len() {
        let (_, ui, vi, ti) = ROBERTSON[i];
        let len = (1.0 + ti * ti).sqrt();
        let (du, dv) = (1.0 / len, ti / len);
        // Signed distance of (u, v) from isotemperature line i.
        let mut dt = -(u - ui) * dv + (v - vi) * du;
        if dt <= 0.0 || i == ROBERTSON.len() - 1 {
            dt = (-dt).max(0.0);
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            let mired = ROBERTSON[i - 1].0 * f + ROBERTSON[i].0 * (1.0 - f);
            return if mired > 0.0 { 1.0e6 / mired } else { f64::INFINITY };
        }
        last_dt = dt;
    }
    f64::INFINITY
}

/// Matrix adapting XYZ under white `from` to XYZ under white `to`, with the
/// linear Bradford cone response (the DNG SDK's `MapWhiteMatrix`).
#[must_use]
pub fn bradford(from: [f64; 3], to: [f64; 3]) -> Matrix3 {
    const MB: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let cone = |w: [f64; 3]| MB.map(|r| (r[0] * w[0] + r[1] * w[1] + r[2] * w[2]).max(0.0));
    let (a, b) = (cone(from), cone(to));
    // Scale each cone response, limited to something reasonable.
    let scale: [f64; 3] = std::array::from_fn(|i| if a[i] > 0.0 { (b[i] / a[i]).clamp(0.1, 10.0) } else { 10.0 });
    let mb = mat(MB);
    let inv = mb.inverse().unwrap_or(Matrix3::IDENTITY);
    #[allow(clippy::cast_possible_truncation)] // f64 → f32 is the matrix precision
    let diag =
        Matrix3::from_rows([[scale[0] as f32, 0.0, 0.0], [0.0, scale[1] as f32, 0.0], [0.0, 0.0, scale[2] as f32]]);
    inv * diag * mb
}

/// Tint units per unit of Duv (distance from the Planckian locus in CIE
/// 1960 uv): the scale photo editors use, about ±150 for strong casts.
pub const TINT_PER_DUV: f64 = 3000.0;

/// Lowest and highest temperature the Planckian approximation covers.
pub const KELVIN_RANGE: [f64; 2] = [1500.0, 25000.0];

/// CIE 1960 uv of a black body at `kelvin` (Krystek's rational
/// approximation, within 1e-4 of the exact locus from 1000 to 15000 K).
#[must_use]
pub fn planck_uv(kelvin: f64) -> [f64; 2] {
    let t = kelvin.clamp(KELVIN_RANGE[0], KELVIN_RANGE[1]);
    let t2 = t * t;
    let u = (0.860_117_757 + 1.541_182_54e-4 * t + 1.286_412_12e-7 * t2)
        / (1.0 + 8.424_202_35e-4 * t + 7.081_451_63e-7 * t2);
    let v = (0.317_398_726 + 4.228_062_45e-5 * t + 4.204_816_91e-8 * t2)
        / (1.0 - 2.897_418_16e-5 * t + 1.614_560_53e-7 * t2);
    [u, v]
}

/// Unit normal to the locus at `kelvin`, pointing to the green side.
fn green_normal(kelvin: f64) -> [f64; 2] {
    let h = kelvin * 1e-3;
    let (a, b) = (planck_uv(kelvin - h), planck_uv(kelvin + h));
    let (du, dv) = (b[0] - a[0], b[1] - a[1]);
    let len = du.hypot(dv).max(1e-12);
    // The locus runs towards lower u and v as the temperature rises; this
    // normal has a positive v component, the green side.
    let n = [dv / len, -du / len];
    if n[1] < 0.0 { [-n[0], -n[1]] } else { n }
}

/// xy → CIE 1960 uv.
#[must_use]
pub fn xy_to_uv(xy: [f64; 2]) -> [f64; 2] {
    let d = -2.0 * xy[0] + 12.0 * xy[1] + 3.0;
    [4.0 * xy[0] / d, 6.0 * xy[1] / d]
}

/// CIE 1960 uv → xy.
#[must_use]
pub fn uv_to_xy(uv: [f64; 2]) -> [f64; 2] {
    let d = 2.0 * uv[0] - 8.0 * uv[1] + 4.0;
    [3.0 * uv[0] / d, 2.0 * uv[1] / d]
}

/// The white of a light at `kelvin` with `tint` (positive = greener light,
/// which a photo editor corrects towards magenta), as xy.
#[must_use]
pub fn kelvin_tint_to_xy(kelvin: f64, tint: f64) -> [f64; 2] {
    let p = planck_uv(kelvin);
    let n = green_normal(kelvin);
    let duv = tint / TINT_PER_DUV;
    uv_to_xy([p[0] + n[0] * duv, p[1] + n[1] * duv])
}

/// Temperature and tint of the white `xy`: the nearest point of the
/// locus, and the signed distance from it ([`kelvin_tint_to_xy`] inverted).
#[must_use]
pub fn xy_to_kelvin_tint(xy: [f64; 2]) -> (f64, f64) {
    let uv = xy_to_uv(xy);
    let dist = |mired: f64| {
        let p = planck_uv(1.0e6 / mired);
        (p[0] - uv[0]).powi(2) + (p[1] - uv[1]).powi(2)
    };
    // The distance has one minimum along the locus: golden-section search in mired.
    let (mut lo, mut hi) = (1.0e6 / KELVIN_RANGE[1], 1.0e6 / KELVIN_RANGE[0]);
    let g = (5.0_f64.sqrt() - 1.0) / 2.0;
    for _ in 0..80 {
        let a = hi - g * (hi - lo);
        let b = lo + g * (hi - lo);
        if dist(a) < dist(b) { hi = b } else { lo = a }
    }
    let kelvin = 1.0e6 / ((lo + hi) / 2.0);
    let p = planck_uv(kelvin);
    let n = green_normal(kelvin);
    let duv = (uv[0] - p[0]) * n[0] + (uv[1] - p[1]) * n[1];
    (kelvin, duv * TINT_PER_DUV)
}

/// The matrix that re-balances an image developed for the light `shot`
/// (whose white became the working space's D65 white) as if it had been
/// lit by `target`: scene colours are recovered under `shot`, then
/// adapted from `target` to D65 (Bradford). `to_xyz` is the working
/// space's RGB → XYZ matrix.
#[must_use]
pub fn rebalance(to_xyz: Matrix3, shot: [f64; 2], target: [f64; 2]) -> Matrix3 {
    let Some(from_xyz) = to_xyz.inverse() else { return Matrix3::IDENTITY };
    let (shot, target) = (xy_to_xyz(shot), xy_to_xyz(target));
    from_xyz * bradford(target, D65_XYZ) * bradford(D65_XYZ, shot) * to_xyz
}

/// The light that makes `rgb` (a colour of an image developed for the
/// light `shot`) neutral: what a white-balance picker or a grey-world
/// estimate looks for. `None` for black or colours without a white.
#[must_use]
pub fn white_of(to_xyz: Matrix3, rgb: [f32; 3], shot: [f64; 2]) -> Option<[f64; 2]> {
    let xyz = to_xyz.apply(rgb).map(f64::from);
    if !(xyz.iter().all(|v| v.is_finite()) && xyz[1] > 1e-9) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)] // the matrix precision
    let scene = bradford(D65_XYZ, xy_to_xyz(shot)).apply(xyz.map(|v| v as f32)).map(f64::from);
    let xy = xyz_to_xy(scene);
    (xy[0] > 0.0 && xy[1] > 0.0 && xy[0] + xy[1] < 1.0).then_some(xy)
}

#[allow(clippy::cast_possible_truncation)] // f64 → f32 is the matrix precision
fn mat(m: [[f64; 3]; 3]) -> Matrix3 {
    Matrix3::from_rows(m.map(|r| r.map(|v| v as f32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_whites_have_their_temperature() {
        // D65 ≈ 6504 K, D50 ≈ 5003 K, illuminant A ≈ 2856 K.
        let t65 = xy_to_temperature([0.312_71, 0.329_02]);
        let t50 = xy_to_temperature([0.345_67, 0.358_50]);
        let ta = xy_to_temperature([0.447_57, 0.407_45]);
        assert!((t65 - 6504.0).abs() < 15.0, "{t65}");
        assert!((t50 - 5003.0).abs() < 15.0, "{t50}");
        assert!((ta - 2856.0).abs() < 15.0, "{ta}");
    }

    #[test]
    fn xy_and_xyz_round_trip() {
        let xy = [0.3127, 0.329];
        let back = xyz_to_xy(xy_to_xyz(xy));
        assert!((back[0] - xy[0]).abs() < 1e-12 && (back[1] - xy[1]).abs() < 1e-12);
    }

    #[test]
    fn the_locus_passes_near_the_standard_whites() {
        // D65 sits 0.0032 above the locus at about 6504 K; illuminant A on it at 2856 K.
        let (k, tint) = xy_to_kelvin_tint([0.312_71, 0.329_02]);
        assert!((k - 6504.0).abs() < 30.0, "{k}");
        assert!((tint - 0.0032 * TINT_PER_DUV).abs() < 1.5, "{tint}");
        let (k, tint) = xy_to_kelvin_tint([0.447_57, 0.407_45]);
        assert!((k - 2856.0).abs() < 10.0, "{k}");
        assert!(tint.abs() < 1.0, "{tint}");
    }

    #[test]
    fn kelvin_and_tint_round_trip() {
        for k in [2000.0, 3200.0, 5500.0, 7500.0, 12000.0] {
            for tint in [-100.0, -10.0, 0.0, 25.0, 120.0] {
                let (k2, t2) = xy_to_kelvin_tint(kelvin_tint_to_xy(k, tint));
                assert!((k2 - k).abs() / k < 2e-3, "{k} {tint} → {k2}");
                assert!((t2 - tint).abs() < 0.2, "{k} {tint} → {t2}");
            }
        }
        // A positive tint is a greener light (larger v).
        assert!(xy_to_uv(kelvin_tint_to_xy(5000.0, 50.0))[1] > planck_uv(5000.0)[1]);
    }

    #[test]
    fn rebalancing_to_the_same_light_changes_nothing_and_a_warmer_target_cools_the_image() {
        let to_xyz = crate::REC2020_TO_XYZ;
        let shot = kelvin_tint_to_xy(5500.0, 0.0);
        let same = rebalance(to_xyz, shot, shot);
        for (i, row) in same.0.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-4, "{:?}", same.0);
            }
        }
        // Saying the light was warmer (3000 K) makes the image bluer.
        let grey = rebalance(to_xyz, shot, kelvin_tint_to_xy(3000.0, 0.0)).apply([0.5, 0.5, 0.5]);
        assert!(grey[2] > grey[0], "{grey:?}");
        // Saying it was greener makes it more magenta.
        let grey = rebalance(to_xyz, shot, kelvin_tint_to_xy(5500.0, 60.0)).apply([0.5, 0.5, 0.5]);
        assert!(grey[1] < grey[0] && grey[1] < grey[2], "{grey:?}");
    }

    #[test]
    fn the_white_of_a_colour_makes_it_neutral() {
        let to_xyz = crate::REC2020_TO_XYZ;
        let shot = kelvin_tint_to_xy(5000.0, 5.0);
        let warm = [0.6_f32, 0.45, 0.3];
        let white = white_of(to_xyz, warm, shot).expect("a white");
        let fixed = rebalance(to_xyz, shot, white).apply(warm);
        assert!((fixed[0] - fixed[1]).abs() < 2e-3 && (fixed[1] - fixed[2]).abs() < 2e-3, "{fixed:?}");
        assert_eq!(white_of(to_xyz, [0.0; 3], shot), None);
    }

    #[test]
    fn bradford_maps_white_to_white_and_matches_the_published_matrix() {
        let m = bradford(D50_XYZ, D65_XYZ);
        #[allow(clippy::cast_possible_truncation)]
        let w = m.apply(D50_XYZ.map(|v| v as f32));
        for (got, want) in w.iter().zip(D65_XYZ) {
            assert!((f64::from(*got) - want).abs() < 1e-4, "{w:?}");
        }
        // Lindbloom's D50 → D65 Bradford matrix.
        let want = [
            [0.955_576_6, -0.023_039_3, 0.063_163_6],
            [-0.028_289_5, 1.009_941_6, 0.021_007_7],
            [0.012_298_2, -0.020_483_0, 1.329_909_8],
        ];
        for (row, wrow) in m.0.iter().zip(want) {
            for (a, b) in row.iter().zip(wrow) {
                assert!((f64::from(*a) - b).abs() < 2e-4, "{:?}", m.0);
            }
        }
    }
}
