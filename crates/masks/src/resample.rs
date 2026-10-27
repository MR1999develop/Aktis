//! Resizing single-channel planes.
//!
//! Each axis is resized on its own: shrinking averages the source pixels
//! each output pixel covers (weighted by overlap, so nothing aliases);
//! enlarging interpolates linearly between pixel centres.

/// For each output index, the source indices and weights (summing to 1).
fn axis_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|i| {
            if scale > 1.0 {
                // Box of width `scale` in source coordinates.
                let start = i as f64 * scale;
                let end = start + scale;
                let first = floor_index(start);
                let last = floor_index(end.ceil()).min(src).max(first + 1);
                let mut w: Vec<(usize, f32)> = (first..last)
                    .filter_map(|j| {
                        let overlap = (end.min(j as f64 + 1.0) - start.max(j as f64)).max(0.0);
                        (overlap > 0.0).then_some((j, narrow(overlap / scale)))
                    })
                    .collect();
                normalise(&mut w);
                w
            } else {
                // Linear between the two nearest source centres.
                let x = ((i as f64 + 0.5) * scale - 0.5).clamp(0.0, (src - 1) as f64);
                let j = floor_index(x).min(src - 1);
                let t = narrow(x - j as f64);
                if t > 0.0 && j + 1 < src { vec![(j, 1.0 - t), (j + 1, t)] } else { vec![(j, 1.0)] }
            }
        })
        .collect()
}

/// A weight in `f32`; weights are in [0, 1], so nothing is lost that matters.
fn narrow(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // a weight in [0, 1]
    let f = v as f32;
    f
}

/// `floor(v)` as an index; `v` is never negative here.
fn floor_index(v: f64) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // v ≥ 0 and below the image size
    let i = v.max(0.0).floor() as usize;
    i
}

fn normalise(w: &mut [(usize, f32)]) {
    let sum: f32 = w.iter().map(|(_, v)| v).sum();
    if sum > 0.0 {
        for (_, v) in w.iter_mut() {
            *v /= sum;
        }
    }
}

/// Resizes a `sw × sh` plane (rows top to bottom) to `dw × dh`.
///
/// # Panics
/// When a size is zero or `src` does not hold `sw × sh` values (a caller bug).
#[must_use]
pub fn resize_plane(src: &[f32], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<f32> {
    assert!(sw > 0 && sh > 0 && dw > 0 && dh > 0, "empty plane");
    assert_eq!(src.len(), sw * sh, "plane length does not match its size");
    if sw == dw && sh == dh {
        return src.to_vec();
    }
    // Horizontal pass: sh rows of dw values.
    let wx = axis_weights(sw, dw);
    let mut tmp = vec![0.0_f32; dw * sh];
    for (row_in, row_out) in src.chunks_exact(sw).zip(tmp.chunks_exact_mut(dw)) {
        for (out, weights) in row_out.iter_mut().zip(&wx) {
            *out = weights.iter().map(|(j, w)| row_in[*j] * w).sum();
        }
    }
    // Vertical pass, row by row for cache friendliness.
    let wy = axis_weights(sh, dh);
    let mut out = vec![0.0_f32; dw * dh];
    for (row_out, weights) in out.chunks_exact_mut(dw).zip(&wy) {
        for (j, w) in weights {
            let row_in = &tmp[j * dw..(j + 1) * dw];
            for (o, v) in row_out.iter_mut().zip(row_in) {
                *o += v * w;
            }
        }
    }
    out
}

/// Size of an image with the same aspect ratio whose long side is at most
/// `long` pixels (never enlarged). Both sides are at least 1.
#[must_use]
pub fn fit_within(width: u32, height: u32, long: u32) -> (u32, u32) {
    let current = width.max(height);
    if current <= long {
        return (width, height);
    }
    let scale = f64::from(long) / f64::from(current);
    let side = |v: u32| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // ≤ the original size
        let s = (f64::from(v) * scale).round().max(1.0) as u32;
        s
    };
    (side(width), side(height))
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact arithmetic on small integers
mod tests {
    use super::*;

    #[test]
    fn halving_averages_pairs_of_pixels() {
        let src = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        assert_eq!(resize_plane(&src, 4, 2, 2, 1), vec![2.5, 4.5]);
    }

    #[test]
    fn shrinking_by_a_fraction_keeps_the_mean() {
        let src: Vec<f32> = (0..30).map(|v| (v % 7) as f32).collect();
        let out = resize_plane(&src, 10, 3, 4, 3);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        assert!((mean(&src) - mean(&out)).abs() < 1e-4, "{} vs {}", mean(&src), mean(&out));
    }

    #[test]
    fn enlarging_interpolates_between_centres_and_keeps_the_edges() {
        let out = resize_plane(&[0.0, 1.0], 2, 1, 4, 1);
        assert_eq!(out, vec![0.0, 0.25, 0.75, 1.0]);
    }

    #[test]
    fn a_constant_plane_stays_constant_at_any_size() {
        let src = vec![0.3; 7 * 5];
        for (w, h) in [(3, 2), (14, 10), (7, 1), (1, 9)] {
            assert!(resize_plane(&src, 7, 5, w, h).iter().all(|v| (v - 0.3).abs() < 1e-6));
        }
    }

    #[test]
    fn fit_within_keeps_the_aspect_and_never_enlarges() {
        assert_eq!(fit_within(6000, 4000, 1024), (1024, 683));
        assert_eq!(fit_within(4000, 6000, 1024), (683, 1024));
        assert_eq!(fit_within(800, 600, 1024), (800, 600));
        assert_eq!(fit_within(10_000, 1, 100), (100, 1));
    }
}
