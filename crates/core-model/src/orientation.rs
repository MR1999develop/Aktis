//! Image orientation as recorded by the camera (EXIF tag 0x0112).

use crate::image::{ImageError, ImageF32};
use crate::mask::{MaskImage, NormPoint};

/// How an image must be turned to be viewed upright. Values follow EXIF 1–8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// 1: already upright.
    #[default]
    Normal,
    /// 2: mirror left–right.
    FlipHorizontal,
    /// 3: rotate 180°.
    Rotate180,
    /// 4: mirror top–bottom.
    FlipVertical,
    /// 5: mirror along the main diagonal.
    Transpose,
    /// 6: rotate 90° clockwise.
    Rotate90,
    /// 7: mirror along the anti-diagonal.
    Transverse,
    /// 8: rotate 90° counter-clockwise.
    Rotate270,
}

impl Orientation {
    /// Whether width and height swap.
    #[must_use]
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::Transpose | Self::Rotate90 | Self::Transverse | Self::Rotate270)
    }

    /// The orientation that undoes this one.
    #[must_use]
    pub fn inverse(self) -> Self {
        match self {
            Self::Rotate90 => Self::Rotate270,
            Self::Rotate270 => Self::Rotate90,
            other => other,
        }
    }

    /// Where a point of the stored (sensor-oriented) image lands in the
    /// upright image, in normalised coordinates.
    #[must_use]
    pub fn to_upright(self, p: NormPoint) -> NormPoint {
        let (x, y) = (p.x(), p.y());
        let (u, v) = match self {
            Self::Normal => (x, y),
            Self::FlipHorizontal => (1.0 - x, y),
            Self::Rotate180 => (1.0 - x, 1.0 - y),
            Self::FlipVertical => (x, 1.0 - y),
            Self::Transpose => (y, x),
            Self::Rotate90 => (1.0 - y, x),
            Self::Transverse => (1.0 - y, 1.0 - x),
            Self::Rotate270 => (y, 1.0 - x),
        };
        // Inputs are in [0, 1], so are the results.
        NormPoint::new(u, v).unwrap_or(p)
    }

    /// Source coordinates of output pixel `(x, y)` in an input of `w × h`.
    #[must_use]
    pub fn source(self, x: usize, y: usize, w: usize, h: usize) -> (usize, usize) {
        match self {
            Self::Normal => (x, y),
            Self::FlipHorizontal => (w - 1 - x, y),
            Self::Rotate180 => (w - 1 - x, h - 1 - y),
            Self::FlipVertical => (x, h - 1 - y),
            Self::Transpose => (y, x),
            Self::Rotate90 => (y, h - 1 - x),
            Self::Transverse => (w - 1 - y, h - 1 - x),
            Self::Rotate270 => (w - 1 - y, x),
        }
    }
}

impl ImageF32 {
    /// The image turned upright. Exact: pixels are moved, never interpolated.
    ///
    /// # Errors
    /// Never in practice; the result has the same pixel count as `self`.
    pub fn oriented(&self, o: Orientation) -> Result<Self, ImageError> {
        if o == Orientation::Normal {
            return Ok(self.clone());
        }
        let out = reorient(self.pixels(), self.width() as usize, self.height() as usize, o);
        let (ow, oh) = if o.swaps_axes() { (self.height(), self.width()) } else { (self.width(), self.height()) };
        Self::from_pixels(ow, oh, self.space(), out)
    }
}

impl MaskImage {
    /// The mask turned by `o`, exactly like [`ImageF32::oriented`].
    ///
    /// # Errors
    /// Never in practice; the result has the same value count as `self`.
    pub fn oriented(&self, o: Orientation) -> Result<Self, ImageError> {
        if o == Orientation::Normal {
            return Ok(self.clone());
        }
        let out = reorient(self.values(), self.width() as usize, self.height() as usize, o);
        let (ow, oh) = if o.swaps_axes() { (self.height(), self.width()) } else { (self.width(), self.height()) };
        Self::from_values(ow, oh, out)
    }
}

/// Moves the `w × h` values of `src` as orientation `o` says.
fn reorient<T: Copy>(src: &[T], w: usize, h: usize, o: Orientation) -> Vec<T> {
    let (ow, oh) = if o.swaps_axes() { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(src.len());
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = o.source(x, y, w, h);
            out.push(src[sy * w + sx]);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // pixels are moved, not computed
mod tests {
    use super::*;
    use crate::image::WorkingSpace;

    /// A 3×2 image whose red channel numbers the pixels:
    /// 1 2 3
    /// 4 5 6
    fn numbered() -> ImageF32 {
        let px = (1..=6).map(|v| [v as f32, 0.0, 0.0, 1.0]).collect();
        ImageF32::from_pixels(3, 2, WorkingSpace::LinearSrgb, px).expect("3×2 image")
    }

    fn reds(img: &ImageF32) -> (u32, u32, Vec<f32>) {
        (img.width(), img.height(), img.pixels().iter().map(|p| p[0]).collect())
    }

    #[test]
    fn every_exif_orientation_moves_pixels_as_specified() {
        let cases = [
            (Orientation::Normal, 3, 2, vec![1., 2., 3., 4., 5., 6.]),
            (Orientation::FlipHorizontal, 3, 2, vec![3., 2., 1., 6., 5., 4.]),
            (Orientation::Rotate180, 3, 2, vec![6., 5., 4., 3., 2., 1.]),
            (Orientation::FlipVertical, 3, 2, vec![4., 5., 6., 1., 2., 3.]),
            (Orientation::Transpose, 2, 3, vec![1., 4., 2., 5., 3., 6.]),
            (Orientation::Rotate90, 2, 3, vec![4., 1., 5., 2., 6., 3.]),
            (Orientation::Transverse, 2, 3, vec![6., 3., 5., 2., 4., 1.]),
            (Orientation::Rotate270, 2, 3, vec![3., 6., 2., 5., 1., 4.]),
        ];
        for (o, w, h, want) in cases {
            let got = numbered().oriented(o).expect("oriented");
            assert_eq!(reds(&got), (w, h, want), "{o:?}");
        }
    }

    const ALL: [Orientation; 8] = [
        Orientation::Normal,
        Orientation::FlipHorizontal,
        Orientation::Rotate180,
        Orientation::FlipVertical,
        Orientation::Transpose,
        Orientation::Rotate90,
        Orientation::Transverse,
        Orientation::Rotate270,
    ];

    #[test]
    fn points_follow_the_pixels_they_sit_on() {
        let img = numbered();
        for o in ALL {
            let up = img.oriented(o).expect("oriented");
            for (i, value) in (1..=6).enumerate() {
                let (sx, sy) = (i % 3, i / 3);
                let p = NormPoint::new((sx as f32 + 0.5) / 3.0, (sy as f32 + 0.5) / 2.0).expect("finite");
                let q = o.to_upright(p);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // small positive coordinates
                let (ux, uy) = ((q.x() * up.width() as f32) as u32, (q.y() * up.height() as f32) as u32);
                assert_eq!(up.get(ux, uy).map(|px| px[0]), Some(value as f32), "{o:?} pixel {value}");
            }
        }
    }

    #[test]
    fn the_inverse_undoes_every_orientation_for_images_and_masks() {
        let img = numbered();
        let mask = MaskImage::from_values(3, 2, (1..=6).map(|v| v as f32).collect()).expect("3×2 mask");
        for o in ALL {
            let back = img.oriented(o).and_then(|r| r.oriented(o.inverse()));
            assert_eq!(back, Ok(img.clone()), "{o:?}");
            let back = mask.oriented(o).and_then(|m| m.oriented(o.inverse()));
            assert_eq!(back, Ok(mask.clone()), "{o:?}");
        }
    }

    #[test]
    fn rotating_90_then_270_returns_the_original() {
        let img = numbered();
        let back = img.oriented(Orientation::Rotate90).and_then(|r| r.oriented(Orientation::Rotate270));
        assert_eq!(back, Ok(img));
    }
}
