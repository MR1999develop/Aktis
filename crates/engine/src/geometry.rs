//! Perspective, straightening and crop of the finished image
//! ([`core_model::Geometry`]), on the CPU after the layers: the upright
//! image is resampled (bilinear, on every core) and cropped.

use core_model::{Geometry, ImageError, ImageF32, Rgba};

/// `upright` with the perspective and rotation of `g` applied and, when
/// `crop` is true, cut to its crop. A geometry that changes nothing returns
/// a copy.
///
/// # Errors
/// [`ImageError`] only if the cropped size is invalid (never in practice).
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)] // pixel positions
pub fn apply(upright: &ImageF32, g: &Geometry, crop: bool) -> Result<ImageF32, ImageError> {
    let (w, h) = (upright.width() as usize, upright.height() as usize);
    let warped = if g.warps() && w > 0 && h > 0 {
        let warp = g.warp(w as f32 / h as f32);
        let src = upright.pixels();
        let mut out = vec![[0.0_f32; 4]; w * h];
        rows(&mut out, w, |y, row| {
            for (x, px) in row.iter_mut().enumerate() {
                let s = warp.source([(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32]);
                *px = bilinear(src, w, h, s[0] * w as f32 - 0.5, s[1] * h as f32 - 0.5);
            }
        });
        ImageF32::from_pixels(upright.width(), upright.height(), upright.space(), out)?
    } else {
        upright.clone()
    };
    let Some(c) = g.crop.filter(|_| crop) else { return Ok(warped) };
    let x0 = ((c.min().x() * w as f32).round() as usize).min(w.saturating_sub(1));
    let y0 = ((c.min().y() * h as f32).round() as usize).min(h.saturating_sub(1));
    let x1 = ((c.max().x() * w as f32).round() as usize).clamp(x0 + 1, w);
    let y1 = ((c.max().y() * h as f32).round() as usize).clamp(y0 + 1, h);
    let src = warped.pixels();
    let pixels: Vec<Rgba> = (y0..y1).flat_map(|y| src[y * w + x0..y * w + x1].iter().copied()).collect();
    ImageF32::from_pixels((x1 - x0) as u32, (y1 - y0) as u32, warped.space(), pixels)
}

/// The pixel at `(x, y)` (pixel centres at integers), interpolated, edges repeated.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // clamped positions
fn bilinear(src: &[Rgba], w: usize, h: usize, x: f32, y: f32) -> Rgba {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (a, b, c, d) = (src[y0 * w + x0], src[y0 * w + x1], src[y1 * w + x0], src[y1 * w + x1]);
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// Runs `f(y, row)` on every row of `out` (rows of `w` pixels), in parallel.
fn rows(out: &mut [Rgba], w: usize, f: impl Fn(usize, &mut [Rgba]) + Sync) {
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let n_rows = out.len() / w.max(1);
    let per = n_rows.div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (i, chunk) in out.chunks_mut(per * w).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (j, row) in chunk.chunks_mut(w).enumerate() {
                    f(i * per + j, row);
                }
            });
        }
    });
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unwrap_used)]
mod tests {
    use super::*;
    use core_model::{NormPoint, NormRect, WorkingSpace};

    fn ramp(w: u32, h: u32) -> ImageF32 {
        let px = (0..w * h).map(|i| [(i % w) as f32, (i / w) as f32, 0.5, 1.0]).collect();
        ImageF32::from_pixels(w, h, WorkingSpace::LinearRec2020, px).unwrap()
    }

    #[test]
    fn a_crop_keeps_its_rectangle() {
        let g = Geometry {
            crop: Some(NormRect::new(NormPoint::new(0.25, 0.5).unwrap(), NormPoint::new(0.75, 1.0).unwrap())),
            ..Geometry::default()
        };
        let out = apply(&ramp(8, 4), &g, true).unwrap();
        assert_eq!((out.width(), out.height()), (4, 2));
        assert_eq!(out.pixels()[0], [2.0, 2.0, 0.5, 1.0]);
        // Not cropped while the crop is being edited.
        assert_eq!(apply(&ramp(8, 4), &g, false).unwrap().width(), 8);
    }

    #[test]
    fn a_rotation_keeps_the_centre_and_the_size() {
        let img = ramp(41, 21);
        let out = apply(&img, &Geometry { angle: 5.0, ..Geometry::default() }, true).unwrap();
        assert_eq!((out.width(), out.height()), (41, 21));
        let c = out.pixels()[10 * 41 + 20];
        assert!((c[0] - 20.0).abs() < 1e-3 && (c[1] - 10.0).abs() < 1e-3, "{c:?}");
        // Corners are filled with image, enlarged around the centre.
        let corner = out.pixels()[0];
        assert!(corner[0] > 0.0 && corner[1] >= 0.0);
    }

    #[test]
    fn no_geometry_is_a_plain_copy() {
        let img = ramp(5, 3);
        assert_eq!(apply(&img, &Geometry::default(), true).unwrap(), img);
    }
}
