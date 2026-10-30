//! X-Trans "Markesteijn + VNG", as darktable's dual demosaic: Markesteijn
//! keeps the detail, but in flat areas of a noisy photo it turns the noise
//! into small mazes and worms; there VNG (variable number of gradients,
//! Chang, Cheung and Pang, 1999), which averages along the calmest
//! directions, gives a smooth, even surface. A mask of detail taken from the
//! Markesteijn image decides, pixel by pixel, how much of each is used.
//!
//! VNG at a photosite, over its 5 × 5 neighbourhood:
//!
//! 1. A gradient in each of eight directions: the mean difference between
//!    photosites of the same colour one or two steps apart along that
//!    direction, in a band three photosites wide on that side.
//! 2. The directions whose gradient is at most `1.5 × smallest + 0.5 ×
//!    (largest − smallest)` are the calm ones.
//! 3. Over the photosites of the calm directions (and the photosite
//!    itself), the mean of each colour; a missing colour is the
//!    photosite's own value plus the difference between the means.
//!
//! The 6 × 6 pattern makes the neighbourhood of each of its 36 positions
//! different, so the pairs and the photosites of each direction are listed
//! once per position. Photosites within two of the image's edge, where the
//! neighbourhood is cut, keep the Markesteijn colour.

use crate::lens::parallel_rows;

/// Half the neighbourhood.
const REACH: isize = 2;
/// The eight directions, clockwise from north.
const DIRECTIONS: [(isize, isize); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];
/// Detail (gradient of the square-root luminance per photosite) below which
/// an area is flat and takes VNG entirely…
const FLAT: f32 = 0.015;
/// …and above which it is detail and keeps Markesteijn entirely.
const DETAIL: f32 = 0.04;

/// An offset from the photosite, `(x, y)`.
type Offset = (isize, isize);

/// What VNG reads around one position of the pattern.
#[derive(Debug, Clone, Default)]
struct Position {
    /// For each direction, the pairs of offsets whose difference measures it.
    pairs: [Vec<(Offset, Offset)>; 8],
    /// For each direction, the photosites on its side with their colour.
    sides: [Vec<(Offset, u8)>; 8],
    /// Every photosite of the neighbourhood with its colour.
    all: Vec<(Offset, u8)>,
}

/// The tables of the 36 positions of `pattern`.
fn positions(pattern: &[[u8; 6]; 6]) -> Vec<Position> {
    let colour = |px: usize, py: usize, dx: isize, dy: isize| {
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)] // small positive values after rem_euclid
        let (x, y) = ((px as isize + dx).rem_euclid(6) as usize, (py as isize + dy).rem_euclid(6) as usize);
        pattern[y][x]
    };
    let inside = |(x, y): (isize, isize)| x.abs() <= REACH && y.abs() <= REACH;
    let mut out = Vec::with_capacity(36);
    for py in 0..6 {
        for px in 0..6 {
            let mut p = Position::default();
            for dy in -REACH..=REACH {
                for dx in -REACH..=REACH {
                    p.all.push(((dx, dy), colour(px, py, dx, dy)));
                }
            }
            for (k, &(ux, uy)) in DIRECTIONS.iter().enumerate() {
                let length = ux * ux + uy * uy;
                // On the direction's side, in a band about three photosites wide.
                let on_side = |(x, y): (isize, isize)| {
                    let along = x * ux + y * uy;
                    let across = (x * uy - y * ux).abs();
                    along >= 0 && across <= length
                };
                for dy in -REACH..=REACH {
                    for dx in -REACH..=REACH {
                        let a = (dx, dy);
                        if !on_side(a) {
                            continue;
                        }
                        if a != (0, 0) && (dx * ux + dy * uy) > 0 {
                            p.sides[k].push((a, colour(px, py, dx, dy)));
                        }
                        for step in 1..=2 {
                            let b = (dx + step * ux, dy + step * uy);
                            if inside(b) && on_side(b) && colour(px, py, a.0, a.1) == colour(px, py, b.0, b.1) {
                                p.pairs[k].push((a, b));
                            }
                        }
                    }
                }
            }
            out.push(p);
        }
    }
    out
}

/// VNG's colour at `(x, y)` (at least [`REACH`] inside the image).
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn vng_at(mosaic: &[f32], width: usize, x: usize, y: usize, p: &Position, own: u8) -> [f32; 3] {
    let value = |(dx, dy): (isize, isize)| mosaic[(y as isize + dy) as usize * width + (x as isize + dx) as usize];
    let mut gradients = [f32::INFINITY; 8];
    for (g, pairs) in gradients.iter_mut().zip(&p.pairs) {
        if !pairs.is_empty() {
            *g = pairs.iter().map(|&(a, b)| (value(a) - value(b)).abs()).sum::<f32>() / pairs.len() as f32;
        }
    }
    let finite = gradients.iter().copied().filter(|g| g.is_finite());
    let (low, high) = finite.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), g| (lo.min(g), hi.max(g)));
    let threshold = 1.5 * low + 0.5 * (high - low);
    let mut sums = [(0.0_f32, 0_u32); 3];
    let here = value((0, 0));
    sums[usize::from(own)] = (here, 1);
    for (k, side) in p.sides.iter().enumerate() {
        if gradients[k] <= threshold {
            for &(o, c) in side {
                sums[usize::from(c)].0 += value(o);
                sums[usize::from(c)].1 += 1;
            }
        }
    }
    // A colour missing from the calm sides: the whole neighbourhood.
    if sums.iter().any(|s| s.1 == 0) {
        sums = [(0.0, 0); 3];
        for &(o, c) in &p.all {
            sums[usize::from(c)].0 += value(o);
            sums[usize::from(c)].1 += 1;
        }
    }
    let mean = |c: usize| sums[c].0 / sums[c].1.max(1) as f32;
    let base = here - mean(usize::from(own));
    std::array::from_fn(|c| if c == usize::from(own) { here } else { base + mean(c) })
}

/// How much of a pixel is detail, 0 (flat) to 1, from the gradient of the
/// square-root luminance of `image` (Sobel), softened over 3 × 3.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn detail_mask(image: &[[f32; 3]], width: usize, height: usize) -> Vec<f32> {
    let luma: Vec<f32> = image.iter().map(|p| (0.25 * p[0] + 0.5 * p[1] + 0.25 * p[2]).max(0.0).sqrt()).collect();
    let at = |plane: &[f32], x: isize, y: isize| {
        let (x, y) = (x.clamp(0, width as isize - 1) as usize, y.clamp(0, height as isize - 1) as usize);
        plane[y * width + x]
    };
    let mut raw = vec![0.0_f32; width * height];
    parallel_rows(&mut raw, width, |first, rows| {
        for (i, m) in rows.iter_mut().enumerate() {
            let (x, y) = ((i % width) as isize, (first + i / width) as isize);
            let l = |dx: isize, dy: isize| at(&luma, x + dx, y + dy);
            let gx = (l(1, -1) + 2.0 * l(1, 0) + l(1, 1)) - (l(-1, -1) + 2.0 * l(-1, 0) + l(-1, 1));
            let gy = (l(-1, 1) + 2.0 * l(0, 1) + l(1, 1)) - (l(-1, -1) + 2.0 * l(0, -1) + l(1, -1));
            let gradient = (gx * gx + gy * gy).sqrt() / 8.0;
            let t = ((gradient - FLAT) / (DETAIL - FLAT)).clamp(0.0, 1.0);
            *m = t * t * (3.0 - 2.0 * t);
        }
    });
    let mut soft = vec![0.0_f32; width * height];
    parallel_rows(&mut soft, width, |first, rows| {
        for (i, m) in rows.iter_mut().enumerate() {
            let (x, y) = ((i % width) as isize, (first + i / width) as isize);
            let mut sum = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    sum += at(&raw, x + dx, y + dy);
                }
            }
            // The largest of the pixel and its mean: detail spreads a little, never shrinks.
            *m = (sum / 9.0).max(raw[first * width + i]);
        }
    });
    soft
}

/// `sharp` (Markesteijn's result for the white-balanced X-Trans `mosaic`)
/// with its flat areas taken from VNG.
pub(crate) fn dual(mosaic: &[f32], width: usize, height: usize, pattern: &[[u8; 6]; 6], sharp: &mut [[f32; 3]]) {
    let reach = REACH.unsigned_abs();
    if width <= 2 * reach || height <= 2 * reach {
        return;
    }
    let mask = detail_mask(sharp, width, height);
    let table = positions(pattern);
    parallel_rows(sharp, width, |first, rows| {
        for (i, px) in rows.iter_mut().enumerate() {
            let (x, y) = (i % width, first + i / width);
            let detail = mask[y * width + x];
            if detail >= 1.0 || x < reach || y < reach || x + reach >= width || y + reach >= height {
                continue;
            }
            let own = pattern[y % 6][x % 6];
            let smooth = vng_at(mosaic, width, x, y, &table[(y % 6) * 6 + x % 6], own);
            for c in 0..3 {
                px[c] = smooth[c] + detail * (px[c] - smooth[c]);
            }
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    /// The X-Trans tile of Fujifilm cameras (X-T3, X-T5 …).
    const TILE: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];

    fn mosaic(w: usize, h: usize, rgb: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
        (0..w * h).map(|i| rgb(i % w, i / w)[usize::from(TILE[(i / w) % 6][(i % w) % 6])]).collect()
    }

    #[test]
    fn every_direction_of_every_position_has_pairs_and_photosites() {
        for p in positions(&TILE) {
            for k in 0..8 {
                assert!(!p.pairs[k].is_empty(), "direction {k}");
                assert!(!p.sides[k].is_empty(), "direction {k}");
            }
            assert_eq!(p.all.len(), 25);
        }
    }

    #[test]
    fn vng_gives_a_flat_colour_back_exactly() {
        let (w, h) = (24, 18);
        let m = mosaic(w, h, |_, _| [0.2, 0.5, 0.8]);
        let table = positions(&TILE);
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                let px = vng_at(&m, w, x, y, &table[(y % 6) * 6 + x % 6], TILE[y % 6][x % 6]);
                for c in 0..3 {
                    assert!((px[c] - [0.2, 0.5, 0.8][c]).abs() < 1e-6, "{x},{y}: {px:?}");
                }
            }
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // the detail is left exactly as it was
    fn flat_noisy_areas_get_smoother_and_detail_is_kept() {
        let (w, h) = (96, 96);
        let mut state = 11_u32;
        let mut noise = || {
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
        // Left half: a flat noisy grey; right half: a strong vertical stripe pattern.
        let truth = |x: usize, _y: usize| {
            if x < 48 {
                0.2
            } else if x % 4 < 2 {
                0.8
            } else {
                0.1
            }
        };
        let m: Vec<f32> =
            (0..w * h).map(|i| truth(i % w, i / w) + if i % w < 48 { 0.01 * noise() } else { 0.0 }).collect();
        let sharp = crate::markesteijn::demosaic(&m, w, h, &TILE).unwrap();
        let mut both = sharp.clone();
        dual(&m, w, h, &TILE, &mut both);
        // Colour noise in the flat half (away from the middle).
        let colour = |img: &[[f32; 3]]| {
            let mut sum = 0.0;
            for y in 8..h - 8 {
                for x in 8..40 {
                    let p = img[y * w + x];
                    sum += (p[0] - p[1]).abs() + (p[2] - p[1]).abs() + (p[1] - 0.2).abs();
                }
            }
            sum
        };
        assert!(colour(&both) < colour(&sharp), "dual {} Markesteijn {}", colour(&both), colour(&sharp));
        // The stripes are untouched.
        for y in 8..h - 8 {
            for x in 56..w - 8 {
                assert_eq!(both[y * w + x], sharp[y * w + x], "{x},{y}");
            }
        }
    }
}
