//! Demosaic of Bayer sensors with LMMSE (directional linear minimum
//! mean-square-error estimation, Zhang and Wu, 2005; as RawTherapee's
//! LMMSE), for photos shot at a high ISO: where RCD follows the noise and
//! makes mazes and colour speckles, LMMSE weighs each direction by how
//! noisy it is.
//!
//! Over a white-balanced mosaic, with a 2.2 gamma encoding (the estimate
//! is made where noise is about as strong in shadows as in highlights):
//!
//! 1. Green minus colour along rows and along columns, at every
//!    photosite, from the Hamilton–Adams interpolation (the neighbours
//!    corrected by the curvature of the photosite's own colour).
//! 2. Each of the two estimates smoothed along its direction (a Gaussian
//!    of 2 photosites): the smoothed value is the signal, what was taken
//!    away the noise.
//! 3. In a window of nine photosites along each direction, the variance of
//!    the signal and of the noise give the best mix of the raw and the
//!    smoothed estimate, and how uncertain it still is; the two directions
//!    are then mixed by the inverse of their uncertainty.
//! 4. Green at red and blue photosites is the colour plus that difference.
//! 5. Red and blue from their differences with green: at blue (red)
//!    photosites the mean of the four diagonal red (blue) ones, at green
//!    photosites the mean of the four neighbours.
//!
//! Tiles (in parallel) read a margin of the mosaic mirrored at the image's
//! borders with the Bayer phase kept, so borders need no special case and
//! tiles leave no seams. False colours are then taken away by the same
//! median as after the other methods ([`crate::chroma`]).

use core_model::BayerPattern;

use crate::lens::parallel_bands;

const RED: u8 = 0;
const GREEN: u8 = 1;
const BLUE: u8 = 2;

/// Margin read around each tile: the steps above reach 11 photosites. Even,
/// so a tile's photosites have the colours of the image's.
const PAD: usize = 12;
/// Size of a tile without its margin.
const TILE_W: usize = 256;
const TILE_H: usize = 64;

/// Gamma of the encoding the estimate is made in.
const GAMMA: f32 = 2.2;
/// Half the window of the estimate (and of the smoothing).
const REACH: usize = 4;
/// Standard deviation of the smoothing, in photosites.
const SIGMA: f32 = 2.0;
/// Keeps variances away from zero (flat areas).
const EPS: f32 = 1e-7;

/// Red, green and blue at every photosite of a white-balanced Bayer
/// `mosaic` (`width × height`, at least 2 × 2).
pub(crate) fn demosaic(mosaic: &[f32], width: usize, height: usize, pattern: BayerPattern) -> Vec<[f32; 3]> {
    assert!(width >= 2 && height >= 2, "LMMSE needs at least a 2 × 2 mosaic");
    assert_eq!(mosaic.len(), width * height, "mosaic size");
    let kernel = smoothing_kernel();
    let source = Source { mosaic, width, height, pattern };
    let mut out = vec![[0.0_f32; 3]; width * height];
    parallel_bands(&mut out, width, TILE_H, |first, rows| {
        let h = rows.len() / width;
        let mut tile = Tile::default(); // buffers reused across the band's tiles
        let mut x0 = 0;
        while x0 < width {
            let w = TILE_W.min(width - x0);
            tile.develop(&source, (x0, first), (w, h), &kernel);
            for y in 0..h {
                for x in 0..w {
                    rows[y * width + x0 + x] = tile.rgb((x + PAD, y + PAD));
                }
            }
            x0 += TILE_W;
        }
    });
    out
}

/// The smoothing Gaussian, `2 × REACH + 1` taps summing to 1.
#[allow(clippy::cast_precision_loss)] // a few taps
fn smoothing_kernel() -> [f32; 2 * REACH + 1] {
    let mut k = [0.0_f32; 2 * REACH + 1];
    for (i, v) in k.iter_mut().enumerate() {
        let d = i as f32 - REACH as f32;
        *v = (-d * d / (2.0 * SIGMA * SIGMA)).exp();
    }
    let sum: f32 = k.iter().sum();
    k.map(|v| v / sum)
}

/// Gamma encoding, sign kept (noise below black stays below).
fn encode(v: f32) -> f32 {
    v.signum() * v.abs().powf(1.0 / GAMMA)
}

/// Inverse of [`encode`].
fn decode(v: f32) -> f32 {
    v.signum() * v.abs().powf(GAMMA)
}

/// The whole mosaic.
struct Source<'a> {
    mosaic: &'a [f32],
    width: usize,
    height: usize,
    pattern: BayerPattern,
}

/// Working planes of one tile with its margin, reused between tiles.
#[derive(Default)]
struct Tile {
    w: usize,
    h: usize,
    /// Colour of each photosite.
    colour: Vec<u8>,
    /// The encoded mosaic.
    cfa: Vec<f32>,
    /// Green minus colour along rows and columns, raw and smoothed.
    along_rows: Vec<f32>,
    along_columns: Vec<f32>,
    smooth_rows: Vec<f32>,
    smooth_columns: Vec<f32>,
    /// Green everywhere.
    green: Vec<f32>,
    /// Green minus red and green minus blue everywhere.
    minus_red: Vec<f32>,
    minus_blue: Vec<f32>,
}

impl Tile {
    /// Develops the tile at `origin` (image photosites) of `size`.
    fn develop(
        &mut self,
        source: &Source<'_>,
        origin: (usize, usize),
        size: (usize, usize),
        kernel: &[f32; 2 * REACH + 1],
    ) {
        let Source { mosaic, width, height, pattern } = *source;
        self.w = size.0 + 2 * PAD;
        self.h = size.1 + 2 * PAD;
        let n = self.w * self.h;
        for plane in [
            &mut self.cfa,
            &mut self.along_rows,
            &mut self.along_columns,
            &mut self.smooth_rows,
            &mut self.smooth_columns,
            &mut self.green,
            &mut self.minus_red,
            &mut self.minus_blue,
        ] {
            plane.clear();
            plane.resize(n, 0.0);
        }
        self.colour.clear();
        self.colour.resize(n, 0);
        #[allow(clippy::cast_possible_wrap)] // image sizes are far below isize::MAX
        for ty in 0..self.h {
            let sy = mirror(origin.1 as isize + ty as isize - PAD as isize, height);
            for tx in 0..self.w {
                let sx = mirror(origin.0 as isize + tx as isize - PAD as isize, width);
                let i = ty * self.w + tx;
                self.cfa[i] = encode(mosaic[sy * width + sx]);
                #[allow(clippy::cast_possible_truncation)] // 0, 1 or 2
                let c = pattern.channel_at(sx, sy) as u8;
                self.colour[i] = c;
            }
        }
        self.differences();
        self.smooth(kernel);
        self.estimate_green();
        self.colours();
    }

    /// The value of `plane` at `(x, y)`, held at the tile's edges (only
    /// the margin, which is not kept, reads past them).
    fn at(&self, plane: &[f32], x: isize, y: isize) -> f32 {
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)] // tile sizes are small
        let (x, y) = (x.clamp(0, self.w as isize - 1) as usize, y.clamp(0, self.h as isize - 1) as usize);
        plane[y * self.w + x]
    }

    /// Step 1: green minus colour along rows and columns (Hamilton–Adams).
    #[allow(clippy::cast_possible_wrap)] // tile sizes are small
    fn differences(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let (xi, yi) = (x as isize, y as isize);
                let c = |dx: isize, dy: isize| self.at(&self.cfa, xi + dx, yi + dy);
                let here = self.cfa[i];
                let sign = if self.colour[i] == GREEN { -1.0 } else { 1.0 };
                // The other colour (or green) interpolated, minus this photosite's.
                let rows = 0.5 * (c(-1, 0) + c(1, 0)) + 0.25 * (2.0 * here - c(-2, 0) - c(2, 0)) - here;
                let columns = 0.5 * (c(0, -1) + c(0, 1)) + 0.25 * (2.0 * here - c(0, -2) - c(0, 2)) - here;
                self.along_rows[i] = sign * rows;
                self.along_columns[i] = sign * columns;
            }
        }
    }

    /// Step 2: each difference smoothed along its own direction.
    #[allow(clippy::cast_possible_wrap)] // tile sizes are small
    fn smooth(&mut self, kernel: &[f32; 2 * REACH + 1]) {
        let r = REACH as isize;
        for y in 0..self.h {
            for x in 0..self.w {
                let (xi, yi) = (x as isize, y as isize);
                let (mut rows, mut columns) = (0.0, 0.0);
                for (k, t) in kernel.iter().enumerate() {
                    let d = k as isize - r;
                    rows += t * self.at(&self.along_rows, xi + d, yi);
                    columns += t * self.at(&self.along_columns, xi, yi + d);
                }
                self.smooth_rows[y * self.w + x] = rows;
                self.smooth_columns[y * self.w + x] = columns;
            }
        }
    }

    /// Steps 3 and 4: the two directions estimated and mixed; green at red
    /// and blue photosites.
    #[allow(clippy::cast_possible_wrap, clippy::cast_precision_loss)] // tile sizes are small
    fn estimate_green(&mut self) {
        let r = REACH as isize;
        let count = (2 * REACH + 1) as f32;
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                if self.colour[i] == GREEN {
                    self.green[i] = self.cfa[i];
                    continue;
                }
                let (xi, yi) = (x as isize, y as isize);
                // One direction: (estimate, its uncertainty).
                let direction = |raw: &[f32], smooth: &[f32], along_rows: bool| {
                    let pick = |plane: &[f32], d: isize| {
                        if along_rows { self.at(plane, xi + d, yi) } else { self.at(plane, xi, yi + d) }
                    };
                    let mean = (-r..=r).map(|d| pick(smooth, d)).sum::<f32>() / count;
                    let signal = EPS + (-r..=r).map(|d| (pick(smooth, d) - mean).powi(2)).sum::<f32>();
                    let noise = EPS + (-r..=r).map(|d| (pick(raw, d) - pick(smooth, d)).powi(2)).sum::<f32>();
                    let estimate = (raw[i] * signal + smooth[i] * noise) / (signal + noise);
                    (estimate, signal * noise / (signal + noise))
                };
                let (h, vh) = direction(&self.along_rows, &self.smooth_rows, true);
                let (v, vv) = direction(&self.along_columns, &self.smooth_columns, false);
                let difference = (h * vv + v * vh) / (vh + vv);
                self.green[i] = self.cfa[i] + difference;
            }
        }
    }

    /// Step 5: green minus red and minus blue everywhere.
    #[allow(clippy::cast_possible_wrap)] // tile sizes are small
    fn colours(&mut self) {
        // At red and blue photosites themselves.
        for i in 0..self.w * self.h {
            let d = self.green[i] - self.cfa[i];
            match self.colour[i] {
                RED => self.minus_red[i] = d,
                BLUE => self.minus_blue[i] = d,
                _ => {}
            }
        }
        // At blue (red) photosites from the four diagonal red (blue) ones.
        let diagonals = [(-1, -1), (1, -1), (-1, 1), (1, 1)];
        let neighbours = [(-1, 0), (1, 0), (0, -1), (0, 1)];
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let (xi, yi) = (x as isize, y as isize);
                let mean = |plane: &[f32]| {
                    diagonals.iter().map(|(dx, dy)| self.at(plane, xi + dx, yi + dy)).sum::<f32>() / 4.0
                };
                match self.colour[i] {
                    RED => {
                        let v = mean(&self.minus_blue);
                        self.minus_blue[i] = v;
                    }
                    BLUE => {
                        let v = mean(&self.minus_red);
                        self.minus_red[i] = v;
                    }
                    _ => {}
                }
            }
        }
        // At green photosites from the four neighbours, which now have both.
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                if self.colour[i] != GREEN {
                    continue;
                }
                let (xi, yi) = (x as isize, y as isize);
                let mean = |plane: &[f32]| {
                    neighbours.iter().map(|(dx, dy)| self.at(plane, xi + dx, yi + dy)).sum::<f32>() / 4.0
                };
                let (r, b) = (mean(&self.minus_red), mean(&self.minus_blue));
                self.minus_red[i] = r;
                self.minus_blue[i] = b;
            }
        }
    }

    /// The developed colour at tile position `(x, y)`, decoded.
    fn rgb(&self, (x, y): (usize, usize)) -> [f32; 3] {
        let i = y * self.w + x;
        let g = self.green[i];
        [decode(g - self.minus_red[i]), decode(g), decode(g - self.minus_blue[i])]
    }
}

/// `i` mirrored into `0..n` without repeating the edge photosite, which
/// keeps the Bayer colour (the parity) of every position.
#[allow(clippy::cast_possible_wrap)] // image sizes are far below isize::MAX
fn mirror(i: isize, n: usize) -> usize {
    let n = n as isize;
    let mut r = i;
    // Far outside small images a single reflection is not enough.
    while r < 0 || r >= n {
        r = if r < 0 { -r } else { 2 * n - 2 - r };
    }
    usize::try_from(r).unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::cast_precision_loss)]
mod tests {
    use super::*;

    const PATTERNS: [BayerPattern; 4] =
        [BayerPattern::Rggb, BayerPattern::Bggr, BayerPattern::Grbg, BayerPattern::Gbrg];

    /// The mosaic a sensor with `pattern` records of the scene `rgb(x, y)`.
    fn mosaic(w: usize, h: usize, pattern: BayerPattern, rgb: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
        (0..w * h).map(|i| rgb(i % w, i / w)[pattern.channel_at(i % w, i / w)]).collect()
    }

    #[test]
    fn flat_colours_come_out_exact_at_any_size_and_phase() {
        for pattern in PATTERNS {
            for (w, h) in [(2, 2), (3, 5), (7, 5), (300, 70)] {
                let m = mosaic(w, h, pattern, |_, _| [0.2, 0.5, 0.8]);
                for (i, px) in demosaic(&m, w, h, pattern).iter().enumerate() {
                    for c in 0..3 {
                        assert!((px[c] - [0.2, 0.5, 0.8][c]).abs() < 1e-4, "{pattern:?} {w}×{h} pixel {i}: {px:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn tiles_leave_no_seams() {
        let (w, h) = (600, 150);
        let stripe = |t: usize| if t % 5 < 2 { 0.8 } else { 0.1 };
        let m = mosaic(w, h, BayerPattern::Rggb, |x, _| [stripe(x); 3]);
        let out = demosaic(&m, w, h, BayerPattern::Rggb);
        for y in 0..h {
            for x in 0..w {
                let same = out[(y % 2) * w + x];
                for c in 0..3 {
                    assert!((out[y * w + x][c] - same[c]).abs() < 1e-5, "row {y} column {x}");
                }
            }
        }
    }

    /// Colour noise of a demosaiced grey: the mean distance of red and blue from green.
    fn colour_noise(out: &[[f32; 3]]) -> f32 {
        out.iter().map(|p| (p[0] - p[1]).abs() + (p[2] - p[1]).abs()).sum::<f32>() / out.len() as f32
    }

    #[test]
    fn on_a_noisy_grey_it_makes_less_colour_noise_than_bilinear() {
        let (w, h) = (128, 128);
        let mut state = 7_u32;
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
        let m: Vec<f32> = (0..w * h).map(|_| 0.2 + 0.02 * noise()).collect();
        let lmmse = colour_noise(&demosaic(&m, w, h, BayerPattern::Rggb));
        let bilinear = colour_noise(
            &(0..w * h)
                .map(|i| {
                    crate::demosaic_bilinear_at(&m, w, h, i % w, i / w, |x, y| BayerPattern::Rggb.channel_at(x, y))
                })
                .collect::<Vec<_>>(),
        );
        assert!(lmmse < bilinear, "LMMSE {lmmse}, bilinear {bilinear}");
    }

    #[test]
    fn a_sharp_grey_edge_with_the_median_gets_less_false_colour_than_bilinear() {
        let (w, h) = (64, 64);
        let truth = |x: usize, y: usize| if 3 * x > 2 * y + 60 { 0.9 } else { 0.1 };
        let m = mosaic(w, h, BayerPattern::Grbg, |x, y| [truth(x, y); 3]);
        // As in development: LMMSE, then the median against false colours.
        let mut out = demosaic(&m, w, h, BayerPattern::Grbg);
        crate::chroma::suppress_false_colours(&mut out, w, h);
        let lmmse = colour_noise(&out);
        let bilinear = colour_noise(
            &(0..w * h)
                .map(|i| {
                    crate::demosaic_bilinear_at(&m, w, h, i % w, i / w, |x, y| BayerPattern::Grbg.channel_at(x, y))
                })
                .collect::<Vec<_>>(),
        );
        assert!(lmmse < 0.5 * bilinear, "false colour: LMMSE {lmmse}, bilinear {bilinear}");
    }
}
