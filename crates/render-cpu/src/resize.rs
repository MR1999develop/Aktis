//! Shrinking an image by area averaging: each output pixel is the mean of
//! the source area it covers, fractional pixels at its edges weighted by how
//! much of them it covers. No detail aliases, and any size can be reached
//! (not only whole fractions), so a picture can be made exactly as large as
//! the screen shows it.

/// The size of a `width × height` image fitted in `max_side` (the longer
/// side becomes `max_side`, the other keeps the proportions, at least 1);
/// the size itself when it already fits.
#[must_use]
pub fn fitted_size(width: u32, height: u32, max_side: u32) -> (u32, u32) {
    let long = width.max(height);
    if long <= max_side || long == 0 {
        return (width, height);
    }
    let side = |n: u32| {
        let v = (u64::from(n) * u64::from(max_side) + u64::from(long) / 2) / u64::from(long);
        u32::try_from(v).unwrap_or(max_side).clamp(1, max_side)
    };
    (side(width), side(height))
}

/// `src` (`width × height`, rows top to bottom) shrunk to `out_w × out_h`
/// by area averaging. Sizes larger than the source are clamped to it.
#[must_use]
pub fn shrink<const N: usize>(src: &[[f32; N]], width: u32, height: u32, out_w: u32, out_h: u32) -> Vec<[f32; N]> {
    let (w, h) = (width as usize, height as usize);
    assert_eq!(src.len(), w * h, "image size");
    let (ow, oh) = ((out_w as usize).clamp(1, w.max(1)), (out_h as usize).clamp(1, h.max(1)));
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let columns = weights(w, ow);
    let rows = weights(h, oh);
    // Rows first: each source row shrunk to `ow` pixels.
    let mut narrow = vec![[0.0_f32; N]; ow * h];
    for (y, out) in narrow.chunks_exact_mut(ow).enumerate() {
        let line = &src[y * w..(y + 1) * w];
        for (px, taps) in out.iter_mut().zip(&columns) {
            *px = mix(taps.iter().map(|&(i, k)| (&line[i], k)));
        }
    }
    let mut out = vec![[0.0_f32; N]; ow * oh];
    for (oy, taps) in rows.iter().enumerate() {
        for ox in 0..ow {
            out[oy * ow + ox] = mix(taps.iter().map(|&(y, k)| (&narrow[y * ow + ox], k)));
        }
    }
    out
}

/// For each of `n_out` output positions, the source positions it covers
/// among `n_in` and their weights (summing to 1).
#[allow(clippy::cast_precision_loss)] // image sides are far below 2^24
fn weights(n_in: usize, n_out: usize) -> Vec<Vec<(usize, f32)>> {
    let step = n_in as f64 / n_out as f64;
    (0..n_out)
        .map(|o| {
            let (start, end) = (o as f64 * step, (o + 1) as f64 * step);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // in 0..n_in
            let (first, last) = (start.floor() as usize, (end.ceil() as usize).min(n_in));
            (first..last)
                .filter_map(|i| {
                    let cover = (end.min((i + 1) as f64) - start.max(i as f64)) / step;
                    #[allow(clippy::cast_possible_truncation)] // a weight in 0..=1
                    (cover > 1e-9).then_some((i, cover as f32))
                })
                .collect()
        })
        .collect()
}

fn mix<'a, const N: usize>(taps: impl Iterator<Item = (&'a [f32; N], f32)>) -> [f32; N] {
    let mut acc = [0.0_f32; N];
    for (px, k) in taps {
        for (a, v) in acc.iter_mut().zip(px) {
            *a += v * k;
        }
    }
    acc
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    #[test]
    fn the_longer_side_becomes_the_limit_exactly() {
        assert_eq!(fitted_size(8256, 6192, 1920), (1920, 1440));
        assert_eq!(fitted_size(6192, 8256, 1920), (1440, 1920));
        assert_eq!(fitted_size(1000, 3, 10), (10, 1));
        assert_eq!(fitted_size(800, 600, 1920), (800, 600));
    }

    #[test]
    fn whole_factors_average_blocks() {
        let src: Vec<[f32; 1]> = (0..4).map(|v| [v as f32]).collect();
        assert_eq!(shrink(&src, 4, 1, 2, 1), [[0.5], [2.5]]);
    }

    #[test]
    fn fractional_factors_weight_the_pixels_they_cover() {
        // 3 → 2: the first output covers 1.5 pixels: (0 + 0.5·1) / 1.5.
        let src: Vec<[f32; 1]> = (0..3).map(|v| [v as f32]).collect();
        let out = shrink(&src, 3, 1, 2, 1);
        assert!((out[0][0] - 1.0 / 3.0).abs() < 1e-6 && (out[1][0] - 5.0 / 3.0).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn a_flat_image_stays_flat_and_the_mean_is_kept() {
        let src = vec![[0.25_f32, 0.5, 0.75]; 37 * 23];
        let out = shrink(&src, 37, 23, 10, 6);
        assert_eq!(out.len(), 60);
        assert!(out.iter().all(|p| (p[0] - 0.25).abs() < 1e-5 && (p[2] - 0.75).abs() < 1e-5));
        let ramp: Vec<[f32; 1]> = (0..37 * 23).map(|i| [(i % 37) as f32]).collect();
        let mean = |v: &[[f32; 1]]| v.iter().map(|p| p[0]).sum::<f32>() / v.len() as f32;
        assert!((mean(&shrink(&ramp, 37, 23, 10, 6)) - mean(&ramp)).abs() < 1e-3);
    }

    #[test]
    fn sizes_beyond_the_source_are_clamped() {
        let src = vec![[1.0_f32]; 6];
        assert_eq!(shrink(&src, 3, 2, 10, 10).len(), 6);
    }
}
