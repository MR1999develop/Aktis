//! Small linear algebra needed by colour conversions.

use std::ops::Mul;

/// A 3×3 matrix in row-major order, applied to column vectors: `m * v`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix3(pub [[f32; 3]; 3]);

impl Matrix3 {
    /// The identity matrix.
    pub const IDENTITY: Self = Self([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// Builds a matrix from nine values in row-major order.
    #[must_use]
    pub const fn from_rows(rows: [[f32; 3]; 3]) -> Self {
        Self(rows)
    }

    /// Multiplies the matrix by a column vector.
    #[must_use]
    pub fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    /// The inverse, or `None` when the matrix is (nearly) singular.
    ///
    /// Computed in `f64` so that chains of colour conversions do not lose precision.
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let m = self.0.map(|row| row.map(f64::from));
        let c00 = m[1][1] * m[2][2] - m[1][2] * m[2][1];
        let c01 = m[1][2] * m[2][0] - m[1][0] * m[2][2];
        let c02 = m[1][0] * m[2][1] - m[1][1] * m[2][0];
        let det = m[0][0] * c00 + m[0][1] * c01 + m[0][2] * c02;
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let r = [
            [c00, m[0][2] * m[2][1] - m[0][1] * m[2][2], m[0][1] * m[1][2] - m[0][2] * m[1][1]],
            [c01, m[0][0] * m[2][2] - m[0][2] * m[2][0], m[0][2] * m[1][0] - m[0][0] * m[1][2]],
            [c02, m[0][1] * m[2][0] - m[0][0] * m[2][1], m[0][0] * m[1][1] - m[0][1] * m[1][0]],
        ];
        #[allow(clippy::cast_possible_truncation)] // f64 → f32 is the intended precision
        Some(Self(r.map(|row| row.map(|v| (v * inv) as f32))))
    }

    /// Scales each row so that it sums to 1, which makes `(1, 1, 1)` map to
    /// `(1, 1, 1)`. Rows that sum to (nearly) zero are left untouched.
    #[must_use]
    pub fn normalize_rows(&self) -> Self {
        Self(self.0.map(|row| {
            let sum: f32 = row.iter().sum();
            if sum.abs() < 1e-9 { row } else { row.map(|v| v / sum) }
        }))
    }
}

impl Mul for Matrix3 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        let a = &self.0;
        let b = &rhs.0;
        let mut out = [[0.0_f32; 3]; 3];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Self(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Matrix3, b: &Matrix3, tol: f32) -> bool {
        a.0.iter().flatten().zip(b.0.iter().flatten()).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn a_matrix_times_its_inverse_is_the_identity() {
        let m = Matrix3::from_rows([[0.6, 0.3, 0.1], [0.2, 0.7, 0.1], [0.05, 0.15, 0.8]]);
        let inv = m.inverse().expect("matrix is invertible");
        assert!(close(&(m * inv), &Matrix3::IDENTITY, 1e-5));
    }

    #[test]
    fn a_singular_matrix_has_no_inverse() {
        let m = Matrix3::from_rows([[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 1.0, 0.0]]);
        assert_eq!(m.inverse(), None);
    }

    #[test]
    fn normalised_rows_map_white_to_white() {
        let m = Matrix3::from_rows([[2.0, 1.0, 1.0], [0.5, 0.5, 1.0], [0.0, 0.0, 3.0]]).normalize_rows();
        let w = m.apply([1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-6), "{w:?}");
    }
}
