//! Lens corrections on camera RGB: vignetting gain, then resampling for
//! distortion and lateral chromatic aberration (each channel at its own
//! position), with a Catmull-Rom bicubic filter.

use lens_db::LensCorrection;

/// Rows per work item when the image is split across threads.
const BAND: usize = 32;

/// Runs `f(first_row, rows)` over bands of `rows` (each `width` long) on all cores.
pub(crate) fn parallel_rows<T: Send>(rows: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if width == 0 || rows.is_empty() {
        return;
    }
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get).max(1);
    let total_rows = rows.len() / width;
    if threads == 1 || total_rows <= BAND {
        f(0, rows);
        return;
    }
    let bands: Vec<(usize, &mut [T])> =
        rows.chunks_mut(width * BAND).enumerate().map(|(i, chunk)| (i * BAND, chunk)).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let bands = std::sync::Mutex::new(bands.into_iter().map(Some).collect::<Vec<_>>());
    std::thread::scope(|s| {
        for _ in 0..threads.min(total_rows.div_ceil(BAND)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let item = match bands.lock() {
                        Ok(mut b) => b.get_mut(i).and_then(Option::take),
                        Err(_) => None,
                    };
                    let Some((first, chunk)) = item else { break };
                    f(first, chunk);
                }
            });
        }
    });
}

/// Multiplies each pixel by the vignetting gain at its position.
#[allow(clippy::cast_precision_loss)] // pixel coordinates are far below 2^24
pub(crate) fn devignette(pixels: &mut [[f32; 3]], width: usize, correction: &LensCorrection) {
    parallel_rows(pixels, width, |first, rows| {
        for (i, px) in rows.iter_mut().enumerate() {
            let (x, y) = (i % width, first + i / width);
            let g = correction.vignetting_gain(x as f32, y as f32);
            for c in px.iter_mut() {
                *c *= g;
            }
        }
    });
}

/// The corrected image: each output pixel reads each channel where the
/// correction says, from `src` (`width × height`).
#[allow(clippy::cast_precision_loss)] // pixel coordinates are far below 2^24
pub(crate) fn resample(src: &[[f32; 3]], width: usize, height: usize, correction: &LensCorrection) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0_f32; 3]; src.len()];
    parallel_rows(&mut out, width, |first, rows| {
        for (i, px) in rows.iter_mut().enumerate() {
            let (x, y) = (i % width, first + i / width);
            let positions = correction.source(x as f32, y as f32);
            for (c, [sx, sy]) in positions.into_iter().enumerate() {
                px[c] = bicubic(src, width, height, c, sx, sy);
            }
        }
    });
    out
}

/// Catmull-Rom weights for the four samples around a fractional position `t`.
fn weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [0.5 * (-t3 + 2.0 * t2 - t), 0.5 * (3.0 * t3 - 5.0 * t2 + 2.0), 0.5 * (-3.0 * t3 + 4.0 * t2 + t), 0.5 * (t3 - t2)]
}

/// Channel `c` at `(x, y)`; positions outside the image read the nearest edge.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]
fn bicubic(src: &[[f32; 3]], width: usize, height: usize, c: usize, x: f32, y: f32) -> f32 {
    let (wmax, hmax) = ((width - 1) as f32, (height - 1) as f32);
    let x = if x.is_finite() { x.clamp(0.0, wmax) } else { 0.0 };
    let y = if y.is_finite() { y.clamp(0.0, hmax) } else { 0.0 };
    let (xf, yf) = (x.floor(), y.floor());
    let (wx, wy) = (weights(x - xf), weights(y - yf));
    let (x0, y0) = (xf as isize - 1, yf as isize - 1);
    let clamp = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;
    let mut sum = 0.0;
    for (j, wyj) in wy.iter().enumerate() {
        let row = clamp(y0 + j as isize, height) * width;
        let mut acc = 0.0;
        for (i, wxi) in wx.iter().enumerate() {
            acc += wxi * src[row + clamp(x0 + i as isize, width)][c];
        }
        sum += wyj * acc;
    }
    sum
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact weights at t = 0
mod tests {
    use super::*;

    #[test]
    fn catmull_rom_weights_sum_to_one_and_interpolate_samples() {
        for t in [0.0_f32, 0.25, 0.5, 0.9] {
            let w = weights(t);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6, "{t}: {w:?}");
        }
        assert_eq!(weights(0.0), [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn bicubic_reproduces_a_linear_ramp_and_clamps_outside() {
        let (w, h) = (8, 5);
        #[allow(clippy::cast_precision_loss)]
        let src: Vec<[f32; 3]> = (0..w * h).map(|i| [(i % w) as f32, (i / w) as f32, 1.0]).collect();
        assert!((bicubic(&src, w, h, 0, 3.3, 2.0) - 3.3).abs() < 1e-5);
        assert!((bicubic(&src, w, h, 1, 4.0, 1.7) - 1.7).abs() < 1e-5);
        assert!((bicubic(&src, w, h, 0, -5.0, 2.0)).abs() < 1e-6);
        assert!((bicubic(&src, w, h, 0, 100.0, f32::NAN) - 7.0).abs() < 1e-6);
    }

    #[test]
    fn parallel_rows_visits_every_row_once_in_place() {
        let width = 3;
        let mut rows = vec![0_usize; width * 1000];
        parallel_rows(&mut rows, width, |first, chunk| {
            for (i, v) in chunk.iter_mut().enumerate() {
                *v += first + i / width;
            }
        });
        for (i, v) in rows.iter().enumerate() {
            assert_eq!(*v, i / width);
        }
    }
}
