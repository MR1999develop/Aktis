//! Rotated square crops of an image, and the way back.
//!
//! Face models want the face upright, centred and filling the input; a
//! [`Square`] says where that square sits in the photo (centre, side, angle).
//! The same square maps the model's answer back onto the photo.

use crate::inference::Tensor;
use crate::prep::Planes;

/// A square region of an image, possibly rotated, in pixels of that image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Square {
    /// Centre, x.
    pub cx: f32,
    /// Centre, y.
    pub cy: f32,
    /// Side length.
    pub side: f32,
    /// Rotation in radians: the square's x axis points at this angle (y down).
    pub angle: f32,
}

impl Square {
    /// An axis-aligned square.
    pub(crate) fn upright(cx: f32, cy: f32, side: f32) -> Self {
        Self { cx, cy, side, angle: 0.0 }
    }

    /// Image position of the point at `(u, v)` in the square, both 0–1.
    pub(crate) fn to_image(self, u: f32, v: f32) -> (f32, f32) {
        let (dx, dy) = ((u - 0.5) * self.side, (v - 0.5) * self.side);
        let (s, c) = self.angle.sin_cos();
        (self.cx + c * dx - s * dy, self.cy + s * dx + c * dy)
    }

    /// Position in the square (0–1 inside it) of the image point `(x, y)`.
    pub(crate) fn to_square(self, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let (s, c) = self.angle.sin_cos();
        ((c * dx + s * dy) / self.side + 0.5, (-s * dx + c * dy) / self.side + 0.5)
    }

    /// The same square in an image scaled by `k`.
    pub(crate) fn scaled(self, k: f32) -> Self {
        Self { cx: self.cx * k, cy: self.cy * k, side: self.side * k, angle: self.angle }
    }
}

/// Bilinear sample of a plane at a pixel position (edges repeat).
pub(crate) fn sample(plane: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (y - 0.5).clamp(0.0, (h - 1) as f32);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the plane
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| plane[yy * w + xx];
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// The square `sq` of `image` resampled to `n × n`, as an NHWC tensor
/// `[1, n, n, 3]` of values `v · scale + offset`.
pub(crate) fn nhwc(image: &Planes, sq: Square, n: usize, scale: f32, offset: f32) -> Tensor {
    let mut data = Vec::with_capacity(n * n * 3);
    for row in 0..n {
        for col in 0..n {
            let (x, y) = sq.to_image((col as f32 + 0.5) / n as f32, (row as f32 + 0.5) / n as f32);
            for plane in &image.rgb {
                data.push(sample(plane, image.w, image.h, x, y) * scale + offset);
            }
        }
    }
    Tensor { shape: vec![1, n, n, 3], data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_goes_to_the_square_and_back() {
        let sq = Square { cx: 50.0, cy: 40.0, side: 20.0, angle: 0.4 };
        let (x, y) = sq.to_image(0.25, 0.8);
        let (u, v) = sq.to_square(x, y);
        assert!((u - 0.25).abs() < 1e-5 && (v - 0.8).abs() < 1e-5);
        assert_eq!(sq.to_image(0.5, 0.5), (50.0, 40.0));
    }

    #[test]
    fn a_quarter_turn_maps_the_square_x_axis_downwards() {
        let sq = Square { cx: 0.0, cy: 0.0, side: 2.0, angle: std::f32::consts::FRAC_PI_2 };
        let (x, y) = sq.to_image(1.0, 0.5);
        assert!(x.abs() < 1e-5 && (y - 1.0).abs() < 1e-5, "({x}, {y})");
    }

    #[test]
    fn sampling_interpolates_between_pixel_centres() {
        let plane = [0.0, 1.0];
        assert!((sample(&plane, 2, 1, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert!((sample(&plane, 2, 1, -5.0, 0.5)).abs() < 1e-6);
    }

    #[test]
    fn crops_come_out_in_channel_last_order() {
        let img = Planes { w: 2, h: 2, rgb: [vec![1.0; 4], vec![0.5; 4], vec![0.0; 4]] };
        let t = nhwc(&img, Square::upright(1.0, 1.0, 2.0), 3, 2.0, -1.0);
        assert_eq!(t.shape, vec![1, 3, 3, 3]);
        assert_eq!(&t.data[..3], &[1.0, 0.0, -1.0]);
    }
}
