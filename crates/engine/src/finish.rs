//! The last steps of an export, after the layers: geometry, size,
//! output pixels, watermark. [`finish_for_export`] is the one place they
//! happen, in this order.

use std::borrow::Cow;
use std::thread;

use core_model::{ImageError, ImageF32, Orientation, Rgba};
use export::{RgbImage16, Size, Watermark};

use crate::EngineError;

/// Output rows below which splitting a pass among threads costs more than it saves.
const ROWS_PER_THREAD: usize = 16;

/// Finishes a rendered image for export: the linear render of the
/// layers (in the sensor's orientation) becomes upright 16-bit sRGB pixels
/// of the requested size, with the watermark drawn in.
///
/// Steps, in order:
/// 1. the recipe's geometry (straighten, keystone, crop) on the upright
///    picture, before resizing, so the size applies to the framed picture;
/// 2. resizing in linear light ([`resample`]), the target computed on the
///    upright size;
/// 3. the output render through the base curve `curve` to 16-bit sRGB,
///    then turning upright;
/// 4. the watermark, on the final pixels.
///
/// # Errors
/// [`EngineError::Image`] if the image is empty.
pub fn finish_for_export(
    image: &ImageF32,
    orientation: Orientation,
    geometry: &core_model::Geometry,
    size: &Size,
    watermark: Option<&Watermark>,
    curve: core_model::BaseCurve,
) -> Result<RgbImage16, EngineError> {
    // 1. Geometry (straighten, keystone, crop), worked out on the upright
    //    picture as the viewer shows it; then nothing is left to turn.
    let shaped;
    let (framed, orientation): (&ImageF32, Orientation) = if geometry.is_identity() {
        (image, orientation)
    } else {
        let upright = image.oriented(orientation).map_err(EngineError::Image)?;
        shaped = crate::geometry::apply(&upright, geometry, true).map_err(EngineError::Image)?;
        (&shaped, Orientation::Normal)
    };

    // 2. Size: decided on the upright picture, applied before turning it.
    let (w, h) = (framed.width(), framed.height());
    let swaps = orientation.swaps_axes();
    let (uw, uh) = if swaps { (h, w) } else { (w, h) };
    let (tw, th) = size.target(uw, uh);
    let (rw, rh) = if swaps { (th, tw) } else { (tw, th) };
    let sized = if (rw, rh) == (w, h) { Cow::Borrowed(framed) } else { Cow::Owned(resample(framed, rw, rh)?) };

    // 3. Output pixels, upright.
    let pixels = render_cpu::to_srgb16_with(&sized, curve);
    let (sw, sh) = (sized.width(), sized.height());
    drop(sized);
    let mut out = orient_rgb16(pixels, sw, sh, orientation);

    // 4. Watermark.
    if let Some(mark) = watermark {
        export::draw_watermark(&mut out, mark);
    }
    Ok(out)
}

/// `pixels` (`R G B` per pixel, `w × h`) turned upright by `o`.
fn orient_rgb16(pixels: Vec<u16>, w: u32, h: u32, o: Orientation) -> RgbImage16 {
    if o == Orientation::Normal {
        return RgbImage16 { width: w, height: h, pixels };
    }
    let (w_us, h_us) = (w as usize, h as usize);
    let (ow, oh) = if o.swaps_axes() { (h_us, w_us) } else { (w_us, h_us) };
    let mut out = vec![0_u16; pixels.len()];
    for y in 0..oh {
        for x in 0..ow {
            let (sx, sy) = o.source(x, y, w_us, h_us);
            let s = (sy * w_us + sx) * 3;
            let d = (y * ow + x) * 3;
            out[d..d + 3].copy_from_slice(&pixels[s..s + 3]);
        }
    }
    let (width, height) = if o.swaps_axes() { (h, w) } else { (w, h) };
    RgbImage16 { width, height, pixels: out }
}

/// Source pixels and weights making one output pixel along one axis.
#[derive(Debug, Clone, PartialEq)]
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

/// Weights for resampling `from` pixels to `to`: the exact area each output
/// pixel covers when shrinking (no aliasing, no ringing around highlights),
/// linear interpolation when enlarging.
fn taps(from: usize, to: usize) -> Vec<Taps> {
    #[allow(clippy::cast_precision_loss)] // image sides are far below 2^52
    let scale = from as f64 / to as f64;
    (0..to)
        .map(|o| {
            #[allow(clippy::cast_precision_loss)]
            let o = o as f64;
            if scale >= 1.0 {
                let (a, b) = (o * scale, (o + 1.0) * scale);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // within 0..from
                let (first, last) = (a.floor() as usize, (b.ceil() as usize).min(from));
                let weights = (first..last)
                    .map(|i| {
                        #[allow(clippy::cast_precision_loss)]
                        let i = i as f64;
                        #[allow(clippy::cast_possible_truncation)] // a fraction
                        let w = ((b.min(i + 1.0) - a.max(i)) / scale) as f32;
                        w.max(0.0)
                    })
                    .collect();
                Taps { first, weights }
            } else {
                let c = ((o + 0.5) * scale - 0.5).max(0.0);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // within 0..from
                let i0 = (c.floor() as usize).min(from - 1);
                #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
                let t = (c - i0 as f64) as f32;
                if i0 + 1 < from {
                    Taps { first: i0, weights: vec![1.0 - t, t] }
                } else {
                    Taps { first: i0, weights: vec![1.0] }
                }
            }
        })
        .collect()
}

/// Runs `row(y, out_row)` for every row of `out` (rows of `row_len`
/// values), shared among the processor's cores.
fn par_rows(out: &mut [Rgba], row_len: usize, row: &(dyn Fn(usize, &mut [Rgba]) + Sync)) {
    if out.is_empty() || row_len == 0 {
        return;
    }
    let rows = out.len() / row_len;
    let cores = thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let threads = cores.min(rows.div_ceil(ROWS_PER_THREAD)).max(1);
    let per = rows.div_ceil(threads);
    thread::scope(|scope| {
        for (chunk, part) in out.chunks_mut(per * row_len).enumerate() {
            scope.spawn(move || {
                for (i, line) in part.chunks_mut(row_len).enumerate() {
                    row(chunk * per + i, line);
                }
            });
        }
    });
}

/// `image` resampled to `width × height` in linear light: an area average
/// when shrinking, linear interpolation when enlarging, each axis apart.
///
/// # Errors
/// [`ImageError`] when a side is 0.
pub fn resample(image: &ImageF32, width: u32, height: u32) -> Result<ImageF32, ImageError> {
    if width == 0 || height == 0 || image.width() == 0 || image.height() == 0 {
        return Err(ImageError::Empty);
    }
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let (ow, oh) = (width as usize, height as usize);
    let src = image.pixels();
    let blend = |acc: &mut Rgba, p: &Rgba, w: f32| {
        for (a, v) in acc.iter_mut().zip(p) {
            *a += v * w;
        }
    };

    // Across: sh rows of ow pixels.
    let across = taps(sw, ow);
    let mut mid = vec![[0.0_f32; 4]; ow * sh];
    par_rows(&mut mid, ow, &|y, line| {
        let row = &src[y * sw..(y + 1) * sw];
        for (out, t) in line.iter_mut().zip(&across) {
            let mut acc = [0.0; 4];
            for (k, w) in t.weights.iter().enumerate() {
                blend(&mut acc, &row[t.first + k], *w);
            }
            *out = acc;
        }
    });

    // Down: oh rows of ow pixels.
    let down = taps(sh, oh);
    let mut out = vec![[0.0_f32; 4]; ow * oh];
    par_rows(&mut out, ow, &|y, line| {
        let t = &down[y];
        for (k, w) in t.weights.iter().enumerate() {
            let row = &mid[(t.first + k) * ow..(t.first + k + 1) * ow];
            for (o, p) in line.iter_mut().zip(row) {
                blend(o, p, *w);
            }
        }
    });
    ImageF32::from_pixels(width, height, image.space(), out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use core_model::{Geometry, WorkingSpace};
    use export::Resize;

    fn ramp(w: u32, h: u32) -> ImageF32 {
        let pixels = (0..w * h).map(|i| [(i % w) as f32, (i / w) as f32, 0.5, 1.0]).collect();
        ImageF32::from_pixels(w, h, WorkingSpace::LinearSrgb, pixels).unwrap()
    }

    #[test]
    fn shrinking_averages_the_covered_area() {
        // 6 → 4 pixels: each output covers 1.5 source pixels.
        let img = ramp(6, 1);
        let small = resample(&img, 4, 1).unwrap();
        let r: Vec<f32> = small.pixels().iter().map(|p| p[0]).collect();
        let expected = [(0.0 + 0.5) / 1.5, (0.5 + 2.0) / 1.5, (3.0 + 2.0) / 1.5, (2.0 + 5.0 * 1.0) / 1.5];
        for (a, b) in r.iter().zip(expected) {
            assert!((a - b).abs() < 1e-5, "{r:?}");
        }
        assert!(small.pixels().iter().all(|p| (p[2] - 0.5).abs() < 1e-6 && (p[3] - 1.0).abs() < 1e-6));
    }

    #[test]
    fn flat_images_stay_flat_at_any_size() {
        let flat =
            ImageF32::from_pixels(37, 23, WorkingSpace::LinearSrgb, vec![[0.25, 0.5, 0.75, 1.0]; 37 * 23]).unwrap();
        for (w, h) in [(10, 7), (37, 23), (80, 51), (1, 1), (5, 40)] {
            let r = resample(&flat, w, h).unwrap();
            assert_eq!((r.width(), r.height()), (w, h));
            for p in r.pixels() {
                assert!((p[0] - 0.25).abs() < 1e-5 && (p[1] - 0.5).abs() < 1e-5 && (p[2] - 0.75).abs() < 1e-5, "{p:?}");
            }
        }
        assert!(resample(&flat, 0, 4).is_err());
    }

    #[test]
    fn enlarging_interpolates_between_neighbours() {
        let img = ramp(2, 1);
        let big = resample(&img, 4, 1).unwrap();
        let r: Vec<f32> = big.pixels().iter().map(|p| p[0]).collect();
        assert_eq!(r, vec![0.0, 0.25, 0.75, 1.0]);
    }

    #[test]
    fn the_size_is_decided_on_the_upright_picture() {
        // A portrait shot stored landscape (6 × 4 turned 90°): 3 pixels on the long edge.
        let img = ramp(6, 4);
        let size = Size { resize: Resize::LongEdge(3), upscale: false };
        let out = finish_for_export(
            &img,
            Orientation::Rotate90,
            &Geometry::default(),
            &size,
            None,
            core_model::BaseCurve::Auto,
        )
        .unwrap();
        assert_eq!((out.width, out.height), (2, 3));
        assert_eq!(out.pixels.len(), 2 * 3 * 3);
        let same = finish_for_export(
            &img,
            Orientation::Normal,
            &Geometry::default(),
            &Size::default(),
            None,
            core_model::BaseCurve::Auto,
        )
        .unwrap();
        assert_eq!((same.width, same.height), (6, 4));
    }

    #[test]
    fn turning_upright_moves_pixels_like_the_image_does() {
        let img = ramp(5, 3);
        for o in [Orientation::Rotate90, Orientation::Rotate180, Orientation::Transverse, Orientation::FlipHorizontal] {
            let out =
                finish_for_export(&img, o, &Geometry::default(), &Size::default(), None, core_model::BaseCurve::Auto)
                    .unwrap();
            let upright = img.oriented(o).unwrap();
            let expected = render_cpu::to_srgb16(&upright);
            assert_eq!((out.width, out.height), (upright.width(), upright.height()), "{o:?}");
            assert_eq!(out.pixels, expected, "{o:?}");
        }
    }

    #[test]
    fn the_crop_is_taken_on_the_upright_picture_before_sizing() {
        use core_model::{NormPoint, NormRect};
        // 6 × 4 stored, 4 × 6 upright; the upper half of the upright picture.
        let img = ramp(6, 4);
        let crop = NormRect::new(NormPoint::new(0.0, 0.0).unwrap(), NormPoint::new(1.0, 0.5).unwrap());
        let geometry = Geometry { crop: Some(crop), ..Geometry::default() };
        let out = finish_for_export(
            &img,
            Orientation::Rotate90,
            &geometry,
            &Size::default(),
            None,
            core_model::BaseCurve::Auto,
        )
        .unwrap();
        assert_eq!((out.width, out.height), (4, 3));
        let size = Size { resize: Resize::LongEdge(2), upscale: false };
        let small = finish_for_export(&img, Orientation::Rotate90, &geometry, &size, None, core_model::BaseCurve::Auto)
            .unwrap();
        assert_eq!(small.width.max(small.height), 2);
    }

    #[test]
    fn the_watermark_is_drawn_last() {
        let dark =
            ImageF32::from_pixels(200, 100, WorkingSpace::LinearSrgb, vec![[0.0, 0.0, 0.0, 1.0]; 20_000]).unwrap();
        let mark = Watermark { text: "X".into(), opacity: 1.0, size_percent: 20.0, ..Watermark::default() };
        let out = finish_for_export(
            &dark,
            Orientation::Normal,
            &Geometry::default(),
            &Size::default(),
            Some(&mark),
            core_model::BaseCurve::Auto,
        )
        .unwrap();
        assert!(out.pixels.iter().any(|&v| v > 60_000));
    }
}
