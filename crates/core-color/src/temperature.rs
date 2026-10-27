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
