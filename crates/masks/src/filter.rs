//! Box mean and feather.
//!
//! All filters run in O(pixels), independent of the radius, so a large
//! feather on a 60-megapixel image costs the same as a small one.

/// Mean over the `(2r+1)²` window around each pixel. Near the borders the
/// window shrinks to the pixels inside the plane (no padding colour leaks in).
#[must_use]
pub fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    debug_assert_eq!(src.len(), w * h);
    if r == 0 || src.is_empty() {
        return src.to_vec();
    }
    // Horizontal: running sum per row, in f64 so long rows do not drift.
    let mut tmp = vec![0.0_f32; src.len()];
    for (row_in, row_out) in src.chunks_exact(w).zip(tmp.chunks_exact_mut(w)) {
        let mut sum = 0.0_f64;
        let mut lo = 0; // first index in the window
        let mut hi = 0; // one past the last index in the window
        for (x, out) in row_out.iter_mut().enumerate() {
            let want_hi = (x + r + 1).min(w);
            while hi < want_hi {
                sum += f64::from(row_in[hi]);
                hi += 1;
            }
            let want_lo = x.saturating_sub(r);
            while lo < want_lo {
                sum -= f64::from(row_in[lo]);
                lo += 1;
            }
            *out = narrow(sum / (hi - lo) as f64);
        }
    }
    // Vertical: running sums per column, adding and removing whole rows.
    let mut out = vec![0.0_f32; src.len()];
    let mut sums = vec![0.0_f64; w];
    let (mut lo, mut hi) = (0, 0);
    for y in 0..h {
        let want_hi = (y + r + 1).min(h);
        while hi < want_hi {
            for (s, v) in sums.iter_mut().zip(&tmp[hi * w..(hi + 1) * w]) {
                *s += f64::from(*v);
            }
            hi += 1;
        }
        let want_lo = y.saturating_sub(r);
        while lo < want_lo {
            for (s, v) in sums.iter_mut().zip(&tmp[lo * w..(lo + 1) * w]) {
                *s -= f64::from(*v);
            }
            lo += 1;
        }
        let n = (hi - lo) as f64;
        for (o, s) in out[y * w..(y + 1) * w].iter_mut().zip(&sums) {
            *o = narrow(s / n);
        }
    }
    out
}

/// A mean in `f32` (values are mask or luminance levels, far inside f32 range).
fn narrow(v: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)] // a mean of f32 values
    let f = v as f32;
    f
}

/// A soft blur close to a Gaussian of standard deviation ≈ `sigma` pixels
/// (three box passes).
#[must_use]
pub fn soft_blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a small positive radius
    let r = sigma.max(0.0).round() as usize;
    if r == 0 {
        return src.to_vec();
    }
    let once = box_mean(src, w, h, r);
    let twice = box_mean(&once, w, h, r);
    box_mean(&twice, w, h, r)
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact arithmetic on small integers
mod tests {
    use super::*;

    #[test]
    fn box_mean_shrinks_the_window_at_the_borders() {
        let src = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(box_mean(&src, 4, 1, 1), vec![1.5, 2.0, 3.0, 3.5]);
        let col = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(box_mean(&col, 1, 4, 1), vec![1.5, 2.0, 3.0, 3.5]);
    }

    #[test]
    fn box_mean_matches_a_direct_sum() {
        let (w, h, r) = (9, 7, 2);
        let src: Vec<f32> = (0..w * h).map(|i| ((i * 37) % 11) as f32).collect();
        let fast = box_mean(&src, w, h, r);
        for y in 0..h {
            for x in 0..w {
                let (mut s, mut n) = (0.0, 0.0);
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        s += src[yy * w + xx];
                        n += 1.0;
                    }
                }
                assert!((fast[y * w + x] - s / n).abs() < 1e-4, "at {x},{y}");
            }
        }
    }

    #[test]
    fn blurring_a_constant_changes_nothing() {
        let src = vec![0.7; 20 * 10];
        assert!(soft_blur(&src, 20, 10, 4.0).iter().all(|v| (v - 0.7).abs() < 1e-6));
    }
}
