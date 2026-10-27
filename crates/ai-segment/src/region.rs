//! Planes and rectangles: cropping an image to a region, finding the box of
//! a mask, and pasting a region's result back into a full-frame raster.
//!
//! Models work at a fixed, small size (256 to 1024 pixels). Running them on a
//! crop around the object instead of on the whole frame gives the object many
//! more of those pixels, which is what makes fine structure (spokes, hair,
//! fingers) come out.

use core_model::{ImageF32, NormPoint, NormRect};

use crate::inference::AiError;

/// A single-channel plane of `w × h` values, rows top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Plane {
    /// A plane of `w × h` zeros.
    pub(crate) fn zeros(w: usize, h: usize) -> Self {
        Self { w, h, data: vec![0.0; w * h] }
    }

    /// The plane resized to `w × h`.
    pub(crate) fn resized(&self, w: usize, h: usize) -> Self {
        Self { w, h, data: masks::resize_plane(&self.data, self.w, self.h, w, h) }
    }

    /// The box of the values above `threshold`, in normalised coordinates, or
    /// `None` when there are none.
    pub(crate) fn bbox_above(&self, threshold: f32) -> Option<NormRect> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for (i, v) in self.data.iter().enumerate() {
            if *v > threshold {
                let (x, y) = (i % self.w, i / self.w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        if x0 == usize::MAX {
            return None;
        }
        let (w, h) = (self.w as f32, self.h as f32);
        Some(NormRect::new(
            NormPoint::new(x0 as f32 / w, y0 as f32 / h)?,
            NormPoint::new(x1 as f32 / w, y1 as f32 / h)?,
        ))
    }

    /// Writes `src`, which covers `rect` of this plane, into the plane
    /// (resampled to the pixels `rect` spans). Values outside `rect` are kept.
    pub(crate) fn paste(&mut self, src: &Plane, rect: NormRect) {
        let px = pixel_rect(rect, self.w, self.h);
        if px.w == 0 || px.h == 0 {
            return;
        }
        let part = src.resized(px.w, px.h);
        for y in 0..px.h {
            let row = &part.data[y * px.w..(y + 1) * px.w];
            let start = (px.y + y) * self.w + px.x;
            self.data[start..start + px.w].copy_from_slice(row);
        }
    }

    /// Applies `f` to every value.
    pub(crate) fn map(mut self, f: impl Fn(f32) -> f32) -> Self {
        for v in &mut self.data {
            *v = f(*v);
        }
        self
    }
}

/// A rectangle in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PixelRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// The pixels of a `w × h` image that `rect` covers (at least one pixel when
/// the image has any).
pub(crate) fn pixel_rect(rect: NormRect, w: usize, h: usize) -> PixelRect {
    let to = |v: f32, n: usize| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // v in [0, 1], n an image side
        let p = (v * n as f32).round() as usize;
        p.min(n)
    };
    let (x0, y0) = (to(rect.min().x(), w), to(rect.min().y(), h));
    let (x1, y1) = (to(rect.max().x(), w).max(x0 + 1).min(w), to(rect.max().y(), h).max(y0 + 1).min(h));
    let (x0, y0) = (x0.min(x1.saturating_sub(1)), y0.min(y1.saturating_sub(1)));
    PixelRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
}

/// `rect` grown by `fraction` of its size on every side (and at least `min`
/// of the image), clamped to the image.
pub(crate) fn expand(rect: NormRect, fraction: f32, min: f32) -> NormRect {
    let (w, h) = (rect.max().x() - rect.min().x(), rect.max().y() - rect.min().y());
    let (dx, dy) = ((w * fraction).max(min), (h * fraction).max(min));
    let p = |x: f32, y: f32| NormPoint::new(x, y).unwrap_or(rect.min());
    NormRect::new(p(rect.min().x() - dx, rect.min().y() - dy), p(rect.max().x() + dx, rect.max().y() + dy))
}

/// Fraction of the image `rect` covers.
pub(crate) fn area(rect: NormRect) -> f32 {
    (rect.max().x() - rect.min().x()) * (rect.max().y() - rect.min().y())
}

/// `p` in the coordinates of `rect` (0–1 across the rectangle), or `None` outside it.
pub(crate) fn into_rect(p: NormPoint, rect: NormRect) -> Option<NormPoint> {
    let (w, h) = (rect.max().x() - rect.min().x(), rect.max().y() - rect.min().y());
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let (x, y) = ((p.x() - rect.min().x()) / w, (p.y() - rect.min().y()) / h);
    ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then(|| NormPoint::new(x, y)).flatten()
}

/// The pixels of `image` inside `rect`.
pub(crate) fn crop(image: &ImageF32, rect: NormRect) -> Result<ImageF32, AiError> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let px = pixel_rect(rect, w, h);
    let mut pixels = Vec::with_capacity(px.w * px.h);
    for y in px.y..px.y + px.h {
        pixels.extend_from_slice(&image.pixels()[y * w + px.x..y * w + px.x + px.w]);
    }
    let (cw, ch) = (u32::try_from(px.w), u32::try_from(px.h));
    match (cw, ch) {
        (Ok(cw), Ok(ch)) => {
            ImageF32::from_pixels(cw, ch, image.space(), pixels).map_err(|e| AiError::Image(e.to_string()))
        }
        _ => Err(AiError::Image("crop too large".to_owned())),
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact values on small grids
mod tests {
    use super::*;
    use core_model::WorkingSpace;

    fn p(x: f32, y: f32) -> NormPoint {
        NormPoint::new(x, y).expect("finite")
    }

    #[test]
    fn the_box_of_a_blob_is_found_in_normalised_coordinates() {
        let mut plane = Plane::zeros(10, 4);
        plane.data[10 + 2] = 1.0;
        plane.data[2 * 10 + 5] = 1.0;
        assert_eq!(plane.bbox_above(0.5), Some(NormRect::new(p(0.2, 0.25), p(0.6, 0.75))));
        assert_eq!(Plane::zeros(3, 3).bbox_above(0.5), None);
    }

    #[test]
    fn pasting_resamples_into_the_rectangle_only() {
        let mut dst = Plane::zeros(8, 4);
        let src = Plane { w: 1, h: 1, data: vec![1.0] };
        dst.paste(&src, NormRect::new(p(0.5, 0.5), p(1.0, 1.0)));
        let ones: Vec<usize> = (0..32).filter(|i| dst.data[*i] == 1.0).collect();
        assert_eq!(ones, vec![20, 21, 22, 23, 28, 29, 30, 31]);
    }

    #[test]
    fn rectangles_grow_and_stay_inside_the_image() {
        let r = expand(NormRect::new(p(0.4, 0.4), p(0.6, 0.6)), 0.5, 0.0);
        assert!((r.min().x() - 0.3).abs() < 1e-6 && (r.max().y() - 0.7).abs() < 1e-6);
        let edge = expand(NormRect::new(p(0.0, 0.0), p(0.2, 0.2)), 1.0, 0.0);
        assert_eq!(edge.min(), p(0.0, 0.0));
        let px = pixel_rect(NormRect::new(p(0.4, 0.2), p(0.5, 0.8)), 200, 100);
        assert_eq!((px.x, px.y, px.w, px.h), (80, 20, 20, 60));
    }

    #[test]
    fn points_map_into_a_rectangle() {
        let r = NormRect::new(p(0.5, 0.0), p(1.0, 0.5));
        assert_eq!(into_rect(p(0.75, 0.25), r), Some(p(0.5, 0.5)));
        assert_eq!(into_rect(p(0.25, 0.25), r), None);
    }

    #[test]
    fn crops_take_the_pixels_of_the_rectangle() {
        let px: Vec<[f32; 4]> = (0..12).map(|i| [i as f32, 0.0, 0.0, 1.0]).collect();
        let img = ImageF32::from_pixels(4, 3, WorkingSpace::LinearSrgb, px).expect("4×3");
        let c = crop(&img, NormRect::new(p(0.5, 1.0 / 3.0), p(1.0, 1.0))).expect("crop");
        assert_eq!((c.width(), c.height()), (2, 2));
        assert_eq!(c.pixels().iter().map(|q| q[0]).collect::<Vec<_>>(), vec![6.0, 7.0, 10.0, 11.0]);
    }
}
