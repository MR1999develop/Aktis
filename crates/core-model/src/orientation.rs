//! Image orientation as recorded by the camera (EXIF tag 0x0112).

use crate::image::{ImageError, ImageF32};

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

    /// Source coordinates of output pixel `(x, y)` in an input of `w × h`.
    fn source(self, x: usize, y: usize, w: usize, h: usize) -> (usize, usize) {
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
        let (w, h) = (self.width() as usize, self.height() as usize);
        let (ow, oh) = if o.swaps_axes() { (h, w) } else { (w, h) };
        let src = self.pixels();
        let mut out = Vec::with_capacity(src.len());
        for y in 0..oh {
            for x in 0..ow {
                let (sx, sy) = o.source(x, y, w, h);
                out.push(src[sy * w + sx]);
            }
        }
        let (ow, oh) = if o.swaps_axes() { (self.height(), self.width()) } else { (self.width(), self.height()) };
        Self::from_pixels(ow, oh, self.space(), out)
    }
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

    #[test]
    fn rotating_90_then_270_returns_the_original() {
        let img = numbered();
        let back = img.oriented(Orientation::Rotate90).and_then(|r| r.oriented(Orientation::Rotate270));
        assert_eq!(back, Ok(img));
    }
}
