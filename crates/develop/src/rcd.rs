//! Demosaic of Bayer sensors with RCD ("ratio corrected demosaicing", Luis
//! Sanz Rodríguez, 2017; the default of RawTherapee and darktable), followed
//! by a median on the colour differences against false colours.
//!
//! Over a white-balanced mosaic:
//!
//! 1. Direction. Squared high-pass filters along columns and along rows,
//!    summed over three photosites, say how much the image changes in each
//!    direction; their ratio (`vh`, 0 = vertical structure, 1 = horizontal)
//!    decides how much each interpolation counts.
//! 2. A low-pass of the mosaic at the red and blue photosites, which mixes
//!    all three channels and so follows brightness, not colour.
//! 3. Green at red and blue photosites. Each neighbouring green is corrected
//!    by the ratio of the low-pass here and two photosites away (where the
//!    image gets brighter, so does green); north and south, east and west
//!    are mixed with weights that fall with the gradient on their side, and
//!    the vertical and horizontal estimates by the direction of step 1
//!    (refined with the four diagonal neighbours where they are surer).
//! 4. Red at blue photosites and blue at red ones from the colour
//!    differences along the two diagonals, weighted the same way; then red
//!    and blue at green photosites along rows and columns.
//! 5. False colours: the 3×3 median of red − green and of blue − green
//!    ([`crate::chroma`]), inside the tile.
//!
//! The image is processed in tiles (in parallel), each with a margin read
//! from the mosaic and mirrored at the image's borders with the Bayer phase
//! kept, so borders need no special case and tiles leave no seams.

use core_model::BayerPattern;

use crate::chroma::{median_at, sort_columns};
use crate::lens::parallel_bands;

const RED: usize = 0;
const GREEN: usize = 1;
const BLUE: usize = 2;

/// Margin read around each tile: the steps above reach 11 photosites. Even,
/// so a tile's photosites have the colours of the image's.
const PAD: usize = 12;
/// Size of a tile without its margin.
const TILE_W: usize = 512;
const TILE_H: usize = 96;

const EPS: f32 = 1e-5;
const EPS_SQ: f32 = 1e-10;

/// Red, green and blue at every photosite of a white-balanced Bayer
/// `mosaic` (`width × height`, at least 2 × 2).
pub(crate) fn demosaic(mosaic: &[f32], width: usize, height: usize, pattern: BayerPattern) -> Vec<[f32; 3]> {
    assert!(width >= 2 && height >= 2, "RCD needs at least a 2 × 2 mosaic");
    assert_eq!(mosaic.len(), width * height, "mosaic size");
    let source = Source { mosaic, width, height, pattern };
    let mut out = vec![[0.0_f32; 3]; width * height];
    parallel_bands(&mut out, width, TILE_H, |first, rows| {
        let h = rows.len() / width;
        let mut tile = Tile::default(); // buffers reused across the band's tiles
        let mut x0 = 0;
        while x0 < width {
            let region = Region { x0, y0: first, w: TILE_W.min(width - x0), h };
            tile.develop(&source, region);
            tile.write(rows, width, region);
            x0 += region.w;
        }
    });
    out
}

struct Source<'a> {
    mosaic: &'a [f32],
    width: usize,
    height: usize,
    pattern: BayerPattern,
}

/// Part of the image a tile produces.
#[derive(Debug, Clone, Copy)]
struct Region {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

/// One tile with its margin, and the planes of the steps.
#[derive(Debug, Default)]
struct Tile {
    /// Padded width (the row stride) and height.
    w: usize,
    h: usize,
    /// Channel of the photosite at `(x % 2, y % 2)` of the tile.
    cells: [[usize; 2]; 2],
    cfa: Vec<f32>,
    /// Two high-pass planes (columns/rows, then the diagonals), later the colour differences.
    a: Vec<f32>,
    b: Vec<f32>,
    vh: Vec<f32>,
    pq: Vec<f32>,
    lpf: Vec<f32>,
    rgb: [Vec<f32>; 3],
    /// The sorted columns of three of one row, for the median.
    columns: Vec<[f32; 3]>,
}

impl Tile {
    fn channel(&self, x: usize, y: usize) -> usize {
        self.cells[y % 2][x % 2]
    }

    fn develop(&mut self, src: &Source<'_>, r: Region) {
        self.load(src, r);
        self.direction();
        self.low_pass();
        self.green();
        self.diagonal_direction();
        self.red_blue_at_red_blue();
        self.red_blue_at_green();
        self.suppress_false_colours();
    }

    /// Reads the region and its margin from the mosaic.
    fn load(&mut self, src: &Source<'_>, r: Region) {
        self.w = r.w + 2 * PAD;
        self.h = r.h + 2 * PAD;
        let n = self.w * self.h;
        // Every step writes what the next ones read, so a tile of the same
        // size as the last one reuses the planes as they are.
        let [red, green, blue] = &mut self.rgb;
        for plane in
            [&mut self.cfa, &mut self.a, &mut self.b, &mut self.vh, &mut self.pq, &mut self.lpf, red, green, blue]
        {
            if plane.len() != n {
                plane.clear();
                plane.resize(n, 0.0);
            }
        }
        // The tile starts PAD (even) photosites before the region.
        let (px, py) = (r.x0 % 2, r.y0 % 2);
        for (dy, row) in self.cells.iter_mut().enumerate() {
            for (dx, cell) in row.iter_mut().enumerate() {
                *cell = src.pattern.channel_at(px + dx, py + dy);
            }
        }
        // Columns read for each tile column: mirrored only near the image's sides.
        let columns: Vec<usize> =
            (0..self.w).map(|tx| mirror(r.x0 as isize + tx as isize - PAD as isize, src.width)).collect();
        let inside = r.x0 >= PAD && r.x0 + r.w + PAD <= src.width;
        for (ty, row) in self.cfa.chunks_exact_mut(self.w).enumerate() {
            let gy = mirror(r.y0 as isize + ty as isize - PAD as isize, src.height);
            let line = &src.mosaic[gy * src.width..(gy + 1) * src.width];
            // Negative levels (noise below black) would break the ratios.
            if inside {
                let first = r.x0 - PAD;
                for (v, m) in row.iter_mut().zip(&line[first..first + self.w]) {
                    *v = m.max(0.0);
                }
            } else {
                for (v, &gx) in row.iter_mut().zip(&columns) {
                    *v = line[gx].max(0.0);
                }
            }
        }
    }

    /// Step 1: `vh`, from the squared high-pass along columns and rows.
    fn direction(&mut self) {
        let (s, w, h) = (self.w, self.w, self.h);
        let c = &self.cfa;
        for y in 3..h - 3 {
            for x in 3..w - 3 {
                let i = y * s + x;
                self.a[i] = high_pass(c, i, s);
                self.b[i] = high_pass(c, i, 1);
            }
        }
        for y in 4..h - 4 {
            for x in 4..w - 4 {
                let i = y * s + x;
                let v = (self.a[i - s] + self.a[i] + self.a[i + s]).max(EPS_SQ);
                let hz = (self.b[i - 1] + self.b[i] + self.b[i + 1]).max(EPS_SQ);
                self.vh[i] = v / (v + hz);
            }
        }
    }

    /// Step 2: the low-pass at the red and blue photosites.
    fn low_pass(&mut self) {
        let s = self.w;
        for y in 1..self.h - 1 {
            for x in 1..self.w - 1 {
                if self.channel(x, y) == GREEN {
                    continue;
                }
                let c = &self.cfa;
                let i = y * s + x;
                self.lpf[i] = c[i]
                    + 0.5 * (c[i - s] + c[i + s] + c[i - 1] + c[i + 1])
                    + 0.25 * (c[i - s - 1] + c[i - s + 1] + c[i + s - 1] + c[i + s + 1]);
            }
        }
    }

    /// Step 3: green everywhere.
    fn green(&mut self) {
        let s = self.w;
        self.rgb[GREEN].copy_from_slice(&self.cfa); // right at the green photosites
        for y in 5..self.h - 5 {
            for x in 5..self.w - 5 {
                if self.channel(x, y) == GREEN {
                    continue;
                }
                let (c, lpf) = (&self.cfa, &self.lpf);
                let i = y * s + x;
                let grad = |d: usize, up: bool| {
                    let at = |k: usize| if up { c[i - k * d] } else { c[i + k * d] };
                    EPS + (c[i - d] - c[i + d]).abs()
                        + (c[i] - at(2)).abs()
                        + (at(1) - at(3)).abs()
                        + (at(2) - at(4)).abs()
                };
                let estimate =
                    |near: usize, far: usize| c[near] * (1.0 + (lpf[i] - lpf[far]) / (EPS + lpf[i] + lpf[far]));
                let (n_grad, s_grad, w_grad, e_grad) = (grad(s, true), grad(s, false), grad(1, true), grad(1, false));
                let (n_est, s_est) = (estimate(i - s, i - 2 * s), estimate(i + s, i + 2 * s));
                let (w_est, e_est) = (estimate(i - 1, i - 2), estimate(i + 1, i + 2));
                let v_est = (s_grad * n_est + n_grad * s_est) / (n_grad + s_grad);
                let h_est = (w_grad * e_est + e_grad * w_est) / (e_grad + w_grad);
                let disc = refined(&self.vh, i, s);
                self.rgb[GREEN][i] = mix(disc, h_est, v_est);
            }
        }
    }

    /// Step 4.1: `pq`, the direction along the two diagonals, at red and blue.
    fn diagonal_direction(&mut self) {
        let s = self.w;
        let c = &self.cfa;
        for y in 3..self.h - 3 {
            for x in 3..self.w - 3 {
                let i = y * s + x;
                self.a[i] = high_pass(c, i, s + 1);
                self.b[i] = high_pass(c, i, s - 1);
            }
        }
        for y in 4..self.h - 4 {
            for x in 4..self.w - 4 {
                if self.channel(x, y) == GREEN {
                    continue;
                }
                let i = y * s + x;
                let p = (self.a[i - s - 1] + self.a[i] + self.a[i + s + 1]).max(EPS_SQ);
                let q = (self.b[i - s + 1] + self.b[i] + self.b[i + s - 1]).max(EPS_SQ);
                self.pq[i] = p / (p + q);
            }
        }
    }

    /// Step 4.2: red at blue photosites, blue at red ones, along the diagonals.
    fn red_blue_at_red_blue(&mut self) {
        let s = self.w;
        for y in 7..self.h - 7 {
            for x in 7..self.w - 7 {
                let own = self.channel(x, y);
                if own == GREEN {
                    continue;
                }
                let (c, g) = (&self.cfa, &self.rgb[GREEN]);
                let i = y * s + x;
                // Up-left/down-right is `p = s + 1`, up-right/down-left `q = s - 1`.
                let (p, q) = (s + 1, s - 1);
                let nw =
                    EPS + (c[i - p] - c[i + p]).abs() + (c[i - p] - c[i - 3 * p]).abs() + (g[i] - g[i - 2 * p]).abs();
                let ne =
                    EPS + (c[i - q] - c[i + q]).abs() + (c[i - q] - c[i - 3 * q]).abs() + (g[i] - g[i - 2 * q]).abs();
                let sw =
                    EPS + (c[i - q] - c[i + q]).abs() + (c[i + q] - c[i + 3 * q]).abs() + (g[i] - g[i + 2 * q]).abs();
                let se =
                    EPS + (c[i - p] - c[i + p]).abs() + (c[i + p] - c[i + 3 * p]).abs() + (g[i] - g[i + 2 * p]).abs();
                let diff = |k: usize| c[k] - g[k];
                let p_est = (nw * diff(i + p) + se * diff(i - p)) / (nw + se);
                let q_est = (ne * diff(i + q) + sw * diff(i - q)) / (ne + sw);
                let disc = refined(&self.pq, i, s);
                let value = g[i] + mix(disc, q_est, p_est);
                self.rgb[own][i] = c[i];
                self.rgb[2 - own][i] = value;
            }
        }
    }

    /// Step 4.3: red and blue at green photosites, along rows and columns.
    fn red_blue_at_green(&mut self) {
        let s = self.w;
        let [red, green, blue] = &mut self.rgb;
        for y in 10..self.h - 10 {
            for x in 10..self.w - 10 {
                if self.cells[y % 2][x % 2] != GREEN {
                    continue;
                }
                let i = y * s + x;
                let disc = refined(&self.vh, i, s);
                for plane in [&mut *red, &mut *blue] {
                    let (g, rc) = (&*green, &*plane);
                    let grad = |d: usize, up: bool| {
                        let (near, far, far2) =
                            if up { (i - d, i - 3 * d, i - 2 * d) } else { (i + d, i + 3 * d, i + 2 * d) };
                        let other = if up { i + d } else { i - d };
                        EPS + (g[i] - g[far2]).abs() + (rc[near] - rc[other]).abs() + (rc[near] - rc[far]).abs()
                    };
                    let (n_grad, s_grad, w_grad, e_grad) =
                        (grad(s, true), grad(s, false), grad(1, true), grad(1, false));
                    let diff = |k: usize| rc[k] - g[k];
                    let v_est = (n_grad * diff(i + s) + s_grad * diff(i - s)) / (n_grad + s_grad);
                    let h_est = (e_grad * diff(i - 1) + w_grad * diff(i + 1)) / (e_grad + w_grad);
                    let value = g[i] + mix(disc, h_est, v_est);
                    plane[i] = value;
                }
            }
        }
    }

    /// Step 5: 3×3 median of red − green and blue − green.
    fn suppress_false_colours(&mut self) {
        let s = self.w;
        let [red, green, blue] = &mut self.rgb;
        for y in 10..self.h - 10 {
            for x in 10..self.w - 10 {
                let i = y * s + x;
                self.a[i] = red[i] - green[i];
                self.b[i] = blue[i] - green[i];
            }
        }
        let mut columns = std::mem::take(&mut self.columns);
        columns.resize(s, [0.0; 3]);
        for (diff, plane) in [(&self.a, &mut *red), (&self.b, &mut *blue)] {
            for y in 11..self.h - 11 {
                let row = |k: usize| &diff[k * s..(k + 1) * s];
                sort_columns(row(y - 1), row(y), row(y + 1), &mut columns);
                for x in 11..s - 11 {
                    let i = y * s + x;
                    plane[i] = green[i] + median_at(&columns, x);
                }
            }
        }
        self.columns = columns;
    }

    /// Copies the region (without the margin) into the band's rows.
    fn write(&self, rows: &mut [[f32; 3]], width: usize, r: Region) {
        for ty in 0..r.h {
            for tx in 0..r.w {
                let i = (ty + PAD) * self.w + tx + PAD;
                rows[ty * width + r.x0 + tx] = [self.rgb[RED][i], self.rgb[GREEN][i], self.rgb[BLUE][i]];
            }
        }
    }
}

/// Squared high-pass of the mosaic at `i` along the step `d` (a row, a
/// column or a diagonal); zero on flat areas and ramps.
fn high_pass(c: &[f32], i: usize, d: usize) -> f32 {
    let v = (c[i - 3 * d] - c[i - d] - c[i + d] + c[i + 3 * d]) - 3.0 * (c[i - 2 * d] + c[i + 2 * d]) + 6.0 * c[i];
    v * v
}

/// The direction at `i`, or the mean of its four diagonal neighbours where
/// they are further from undecided (0.5).
fn refined(dir: &[f32], i: usize, s: usize) -> f32 {
    let centre = dir[i];
    let around = 0.25 * (dir[i - s - 1] + dir[i - s + 1] + dir[i + s - 1] + dir[i + s + 1]);
    if (0.5 - centre).abs() < (0.5 - around).abs() { around } else { centre }
}

/// `a` of `one` and the rest of `other`.
fn mix(a: f32, one: f32, other: f32) -> f32 {
    a * one + (1.0 - a) * other
}

/// Mirrors a coordinate at the borders ("reflect-101"), keeping its parity
/// so the Bayer colour stays the same; `n ≥ 2`.
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
    fn mirroring_keeps_the_bayer_colour() {
        for n in 2..9 {
            for i in -14_isize..(n as isize + 14) {
                let r = mirror(i, n);
                assert!(r < n);
                assert_eq!((r as isize - i).rem_euclid(2), 0, "i {i}, n {n}");
            }
        }
        assert_eq!(mirror(-1, 10), 1);
        assert_eq!(mirror(10, 10), 8);
    }

    #[test]
    fn flat_colours_come_out_exact_at_any_size_and_phase() {
        for pattern in PATTERNS {
            for (w, h) in [(2, 2), (3, 5), (7, 5), (300, 70)] {
                let m = mosaic(w, h, pattern, |_, _| [0.2, 0.5, 0.8]);
                for (i, px) in demosaic(&m, w, h, pattern).iter().enumerate() {
                    for c in 0..3 {
                        assert!((px[c] - [0.2, 0.5, 0.8][c]).abs() < 1e-5, "{pattern:?} {w}×{h} pixel {i}: {px:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn tiles_leave_no_seams() {
        // Stripes along one axis: every row (column) must come out the same,
        // across bands of rows and tiles of columns alike.
        let (w, h) = (600, 150);
        let stripe = |t: usize| if t % 5 < 2 { 0.8 } else { 0.1 };
        let m = mosaic(w, h, BayerPattern::Rggb, |x, _| [stripe(x); 3]);
        let out = demosaic(&m, w, h, BayerPattern::Rggb);
        for y in 0..h {
            for x in 0..w {
                // The colour phase repeats every two rows.
                let same = out[(y % 2) * w + x];
                for c in 0..3 {
                    assert!((out[y * w + x][c] - same[c]).abs() < 1e-5, "row {y} column {x}");
                }
            }
        }
        let m = mosaic(w, h, BayerPattern::Rggb, |_, y| [stripe(y); 3]);
        let out = demosaic(&m, w, h, BayerPattern::Rggb);
        for y in 0..h {
            for x in 0..w {
                let same = out[y * w + x % 2];
                for c in 0..3 {
                    assert!((out[y * w + x][c] - same[c]).abs() < 1e-5, "row {y} column {x}");
                }
            }
        }
    }

    /// The largest colour error (channel minus grey) and the mean error of
    /// a demosaiced grey scene.
    fn errors(out: &[[f32; 3]], truth: impl Fn(usize, usize) -> f32, w: usize) -> (f32, f32) {
        let (mut worst, mut sum) = (0.0_f32, 0.0_f32);
        for (i, px) in out.iter().enumerate() {
            let t = truth(i % w, i / w);
            for v in px {
                worst = worst.max((v - px[1]).abs());
                sum += (v - t).abs();
            }
        }
        (worst, sum / (out.len() * 3) as f32)
    }

    fn bilinear(m: &[f32], w: usize, h: usize, pattern: BayerPattern) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| crate::demosaic_bilinear_at(m, w, h, i % w, i / w, |x, y| pattern.channel_at(x, y)))
            .collect()
    }

    #[test]
    fn a_sharp_grey_edge_gets_far_less_false_colour_than_bilinear() {
        let (w, h) = (64, 64);
        // A slanted edge between dark and light grey.
        let truth = |x: usize, y: usize| if 3 * x > 2 * y + 60 { 0.9 } else { 0.1 };
        let m = mosaic(w, h, BayerPattern::Rggb, |x, y| [truth(x, y); 3]);
        let (rcd_colour, rcd_error) = errors(&demosaic(&m, w, h, BayerPattern::Rggb), truth, w);
        let (bil_colour, bil_error) = errors(&bilinear(&m, w, h, BayerPattern::Rggb), truth, w);
        assert!(rcd_colour < 0.5 * bil_colour, "false colour: RCD {rcd_colour}, bilinear {bil_colour}");
        assert!(rcd_error < bil_error, "error: RCD {rcd_error}, bilinear {bil_error}");
    }

    #[test]
    fn fine_grey_detail_is_resolved_better_than_bilinear() {
        let (w, h) = (96, 96);
        // Concentric rings getting finer outwards (a zone plate).
        let truth = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 48.0, y as f32 - 48.0);
            0.5 + 0.4 * ((dx * dx + dy * dy) * 0.004).cos()
        };
        let m = mosaic(w, h, BayerPattern::Grbg, |x, y| [truth(x, y); 3]);
        let (rcd_colour, rcd_error) = errors(&demosaic(&m, w, h, BayerPattern::Grbg), truth, w);
        let (bil_colour, bil_error) = errors(&bilinear(&m, w, h, BayerPattern::Grbg), truth, w);
        assert!(rcd_error < 0.7 * bil_error, "error: RCD {rcd_error}, bilinear {bil_error}");
        assert!(rcd_colour < bil_colour, "false colour: RCD {rcd_colour}, bilinear {bil_colour}");
    }
}
