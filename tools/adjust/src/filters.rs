//! Filters for the tools that work at the scale of the pixel (sharpening,
//! noise reduction): an exact small Gaussian and the "à trous" wavelet,
//! both separable, with mirrored borders ("reflect-101": the edge pixel is
//! not repeated) and rows spread over every core.

/// Mirrors index `i` into `0..n` without repeating the edge: −1 → 1, n → n−2.
pub fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    if n <= 1 {
        return 0;
    }
    let period = 2 * (n - 1);
    let mut r = i.rem_euclid(period);
    if r >= n {
        r = period - r;
    }
    #[allow(clippy::cast_sign_loss)] // 0 ≤ r < n
    let r = r as usize;
    r
}

/// Runs `f(first_row, rows)` over bands of rows (`w` values each) on every core.
pub fn parallel_rows(plane: &mut [f32], w: usize, f: impl Fn(usize, &mut [f32]) + Sync) {
    if w == 0 || plane.is_empty() {
        return;
    }
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let n_rows = plane.len() / w;
    let per = n_rows.div_ceil(threads).max(1);
    if threads == 1 || n_rows <= per {
        f(0, plane);
        return;
    }
    std::thread::scope(|s| {
        for (i, chunk) in plane.chunks_mut(per * w).enumerate() {
            let f = &f;
            s.spawn(move || f(i * per, chunk));
        }
    });
}

/// `plane` (`w` × `h`) convolved with the separable kernel `taps`, whose
/// tap `k` sits `spacing × (k − centre)` pixels away. Borders are mirrored.
fn convolve(plane: &[f32], w: usize, h: usize, taps: &[f32], spacing: usize) -> Vec<f32> {
    let mut across = vec![0.0_f32; plane.len()];
    let mut out = vec![0.0_f32; plane.len()];
    convolve_into(plane, &mut out, &mut across, w, h, taps, spacing);
    out
}

/// [`convolve`] into `out`, with `across` (same size) as scratch: no
/// allocation, so repeated passes reuse the same memory.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)] // image sizes and offsets are far below isize::MAX
fn convolve_into(plane: &[f32], out: &mut [f32], across: &mut [f32], w: usize, h: usize, taps: &[f32], spacing: usize) {
    let centre = (taps.len() / 2) as isize;
    let step = spacing as isize;
    // Rows.
    parallel_rows(across, w, |first, rows| {
        let reach = (centre * step) as usize;
        for (r, out) in rows.chunks_mut(w).enumerate() {
            let src = &plane[(first + r) * w..(first + r + 1) * w];
            if w > 2 * reach {
                // Inside, tap by tap over whole slices (vectorises well).
                let inner = &mut out[reach..w - reach];
                let len = inner.len();
                inner.fill(0.0);
                for (k, t) in taps.iter().enumerate() {
                    let from = k * spacing;
                    for (o, v) in inner.iter_mut().zip(&src[from..from + len]) {
                        *o += t * v;
                    }
                }
            }
            // The borders, mirrored.
            let edge = |x: usize| {
                taps.iter()
                    .enumerate()
                    .map(|(k, t)| t * src[reflect(x as isize + (k as isize - centre) * step, w)])
                    .sum::<f32>()
            };
            for x in (0..reach.min(w)).chain(w.saturating_sub(reach).max(reach.min(w))..w) {
                out[x] = edge(x);
            }
        }
    });
    // Columns.
    let across: &[f32] = across;
    parallel_rows(out, w, |first, rows| {
        for (r, row) in rows.chunks_mut(w).enumerate() {
            let y = (first + r) as isize;
            row.fill(0.0);
            for (k, t) in taps.iter().enumerate() {
                let sy = reflect(y + (k as isize - centre) * step, h);
                let src = &across[sy * w..(sy + 1) * w];
                for (o, v) in row.iter_mut().zip(src) {
                    *o += t * v;
                }
            }
        }
    });
}

/// `plane` blurred by a Gaussian of standard deviation `sigma` pixels,
/// sampled exactly (taps out to 3σ); `sigma` below 0.2 leaves it as is.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
pub fn gaussian(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma.is_nan() || sigma < 0.2 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let radius = (3.0 * sigma).ceil().max(1.0) as usize;
    let mut taps: Vec<f32> =
        (0..=2 * radius).map(|k| (-((k as f32 - radius as f32).powi(2)) / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = taps.iter().sum();
    taps.iter_mut().for_each(|t| *t /= sum);
    convolve(plane, w, h, &taps, 1)
}

/// B3-spline taps of the "à trous" wavelet.
const B3: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Standard deviation of white noise of σ = 1 in each detail level of the
/// B3 "à trous" wavelet, finest first (Starck and Murtagh).
pub const LEVEL_NOISE: [f32; 5] = [0.8907, 0.2007, 0.0856, 0.0413, 0.0205];

/// The "à trous" wavelet: `plane` split into `levels` detail levels
/// (finest first), each handed to `each(level, smooth, current)` where the
/// detail is `current − smooth`; returns the smooth rest. The rest plus
/// every detail is `plane` exactly. One level is in memory at a time.
pub fn wavelet(
    plane: &[f32],
    w: usize,
    h: usize,
    levels: usize,
    mut each: impl FnMut(usize, &[f32], &[f32]),
) -> Vec<f32> {
    let mut current = plane.to_vec();
    let mut smooth = vec![0.0_f32; plane.len()];
    let mut scratch = vec![0.0_f32; plane.len()];
    for level in 0..levels {
        convolve_into(&current, &mut smooth, &mut scratch, w, h, &B3, 1 << level);
        each(level, &smooth, &current);
        std::mem::swap(&mut current, &mut smooth);
    }
    current
}

/// Noise standard deviation of `plane`, estimated from the finest diagonal
/// differences (median absolute deviation, robust to edges and texture):
/// for white noise of σ, `(a − b − c + d) / 2` over 2×2 blocks has σ too.
#[allow(clippy::cast_precision_loss)]
pub fn noise_sigma(plane: &[f32], w: usize, h: usize) -> f32 {
    if w < 2 || h < 2 {
        return 0.0;
    }
    // A grid of at most about a million blocks is plenty.
    let step = ((w / 2) * (h / 2) / 1_000_000).max(1);
    let mut d: Vec<f32> = Vec::new();
    for (n, (bx, by)) in (0..h / 2).flat_map(|by| (0..w / 2).map(move |bx| (bx, by))).enumerate() {
        if n % step != 0 {
            continue;
        }
        let (x, y) = (2 * bx, 2 * by);
        let at = |xx: usize, yy: usize| plane[yy * w + xx];
        let v = (at(x, y) - at(x + 1, y) - at(x, y + 1) + at(x + 1, y + 1)) * 0.5;
        if v.is_finite() {
            d.push(v.abs());
        }
    }
    if d.is_empty() {
        return 0.0;
    }
    let mid = d.len() / 2;
    let (_, median, _) = d.select_nth_unstable_by(mid, f32::total_cmp);
    *median / 0.6745
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;

    #[test]
    fn reflect_mirrors_without_repeating_the_edge() {
        let got: Vec<usize> = (-3..8).map(|i| reflect(i, 5)).collect();
        assert_eq!(got, vec![3, 2, 1, 0, 1, 2, 3, 4, 3, 2, 1]);
        assert_eq!(reflect(-7, 1), 0);
        assert_eq!(reflect(40, 3), 0);
    }

    #[test]
    fn the_gaussian_keeps_a_constant_and_spreads_an_impulse_as_its_kernel() {
        let (w, h) = (31, 21);
        let flat = vec![0.7_f32; w * h];
        assert!(gaussian(&flat, w, h, 1.3).iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut impulse = vec![0.0_f32; w * h];
        impulse[10 * w + 15] = 1.0;
        let out = gaussian(&impulse, w, h, 1.0);
        let total: f32 = out.iter().sum();
        assert!((total - 1.0).abs() < 1e-4);
        // Symmetric, largest in the centre, e^(−1/2) of it one sigma away.
        let c = out[10 * w + 15];
        assert!((out[10 * w + 14] - out[10 * w + 16]).abs() < 1e-7);
        assert!((out[10 * w + 16] / c - (-0.5_f32).exp()).abs() < 1e-3);
    }

    #[test]
    fn wavelet_levels_add_up_to_the_plane() {
        let (w, h) = (37, 19);
        let plane: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 101) as f32 / 101.0).collect();
        let mut sum = vec![0.0_f32; w * h];
        let mut count = 0;
        let rest = wavelet(&plane, w, h, 4, |level, smooth, current| {
            assert_eq!(level, count);
            count += 1;
            for (s, (c, m)) in sum.iter_mut().zip(current.iter().zip(smooth)) {
                *s += c - m;
            }
        });
        assert_eq!(count, 4);
        for (i, v) in plane.iter().enumerate() {
            assert!((sum[i] + rest[i] - v).abs() < 1e-5);
        }
    }

    #[test]
    fn noise_is_measured_on_noise_and_not_on_smooth_ramps() {
        let (w, h) = (256, 256);
        let mut state = 12345_u32;
        let mut gauss = || {
            // Sum of 12 uniforms minus 6: close to a unit normal.
            (0..12)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    (state % 1_000_000) as f32 / 1_000_000.0
                })
                .sum::<f32>()
                - 6.0
        };
        let noisy: Vec<f32> = (0..w * h).map(|i| (i % w) as f32 / w as f32 + 0.05 * gauss()).collect();
        let sigma = noise_sigma(&noisy, w, h);
        assert!((sigma - 0.05).abs() < 0.005, "{sigma}");
        let ramp: Vec<f32> = (0..w * h).map(|i| (i % w) as f32 / w as f32).collect();
        assert!(noise_sigma(&ramp, w, h) < 1e-4);
        assert!(noise_sigma(&[1.0], 1, 1).abs() < f32::EPSILON);
    }
}
