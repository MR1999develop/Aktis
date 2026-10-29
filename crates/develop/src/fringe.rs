//! Purple fringing: the violet to magenta halo that lenses leave along
//! high-contrast edges (longitudinal chromatic aberration, blooming), which
//! the lens profile's lateral correction does not remove.
//!
//! Works on the linear working space after the profile, keeping each
//! pixel's luminance. A pixel is fringe when three things hold, each as a
//! soft weight between 0 and 1:
//! - it is near a strong edge in luminance (in stops, so dark and bright
//!   scenes behave alike);
//! - its hue lies between magenta and blue-violet (not red, not sky blue);
//! - it is clearly coloured.
//!
//! Instead of only taking the colour away, the colour of the pixel is
//! replaced by the colour of the nearby pixels that are not fringe (what
//! was under the halo: the white of the highlight, the green of the
//! leaves), as Capture One does; with no such neighbour it becomes neutral.
//!
//! Everything spatial runs on a grid of cells about as large as a fringe
//! is wide, so the memory is a small fraction of the image's and the time
//! is a few passes over the pixels.

use core_model::{Rgba, WorkingSpace};

use crate::lens::parallel_bands;

/// Smallest luminance considered (linear); darker pixels are left alone.
const DARK: f32 = 1e-4;
/// Edge strength (largest log2-luminance step between neighbours, in stops)
/// from which a pixel starts being near an edge…
const EDGE_FROM: f32 = 0.25;
/// …and from which it fully is.
const EDGE_AT: f32 = 0.75;
/// Chroma (relative to luminance) from which a pixel counts as coloured…
const CHROMA_FROM: f32 = 0.06;
/// …and from which it fully does.
const CHROMA_AT: f32 = 0.25;

/// Purple fringing removed from `pixels` (`width × height`, linear `space`),
/// `amount` from 0 (nothing) to 1 (the fringe takes its neighbours' colour
/// fully).
pub(crate) fn defringe(pixels: &mut [Rgba], width: usize, height: usize, space: WorkingSpace, amount: f32) {
    let amount = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
    if amount == 0.0 || width < 3 || height < 3 || pixels.len() != width * height {
        return;
    }
    let grid = Grid::new(width, height);
    let lum = |px: &Rgba| core_color::luminance([px[0], px[1], px[2]], space);
    let src: &[Rgba] = pixels;

    // 1. Strongest edge in each cell, then spread to the neighbouring cells:
    //    a fringe lies within about one cell of its edge.
    let mut edges = vec![0.0_f32; grid.cells()];
    parallel_bands(&mut edges, grid.cols, 1, |gy, row| {
        for (gx, cell) in row.iter_mut().enumerate() {
            let mut strongest = 0.0_f32;
            for (x, y) in grid.pixels_of(gx, gy) {
                let at = |xx: usize, yy: usize| stops(lum(&src[yy * width + xx]));
                let (l, r) = (at(x.saturating_sub(1), y), at((x + 1).min(width - 1), y));
                let (u, d) = (at(x, y.saturating_sub(1)), at(x, (y + 1).min(height - 1)));
                strongest = strongest.max((r - l).abs() * 0.5).max((d - u).abs() * 0.5);
            }
            *cell = strongest;
        }
    });
    let edges = grid.spread_max(&edges);

    // 2. Colour of the pixels of each cell that are not fringe (their
    //    chromaticity: RGB over luminance), weighted by how much they are not.
    let mut sums = vec![[0.0_f32; 4]; grid.cells()];
    parallel_bands(&mut sums, grid.cols, 1, |gy, row| {
        for (gx, cell) in row.iter_mut().enumerate() {
            for (x, y) in grid.pixels_of(gx, gy) {
                let px = &src[y * width + x];
                let y_lum = lum(px);
                if y_lum.is_nan() || y_lum <= DARK {
                    continue;
                }
                let ratio = [px[0] / y_lum, px[1] / y_lum, px[2] / y_lum];
                let keep = 1.0 - fringe(ratio, grid.sample(&edges, x, y), space);
                for c in 0..3 {
                    cell[c] += keep * ratio[c];
                }
                cell[3] += keep;
            }
        }
    });
    let colours: Vec<[f32; 3]> = grid
        .spread_sum(&sums)
        .into_iter()
        .map(|s| if s[3] > 1e-3 { [s[0] / s[3], s[1] / s[3], s[2] / s[3]] } else { [1.0; 3] })
        .collect();

    // 3. Each fringe pixel takes the colour of its surroundings, at its own luminance.
    parallel_bands(pixels, width, 16, |first, rows| {
        for (i, px) in rows.iter_mut().enumerate() {
            let (x, y) = (i % width, first + i / width);
            let y_lum = lum(px);
            if y_lum.is_nan() || y_lum <= DARK {
                continue;
            }
            let ratio = [px[0] / y_lum, px[1] / y_lum, px[2] / y_lum];
            let t = amount * fringe(ratio, grid.sample(&edges, x, y), space);
            if t <= 0.0 {
                continue;
            }
            let around = grid.sample3(&colours, x, y);
            // The mix of two chromaticities of luminance 1 has luminance 1.
            for c in 0..3 {
                px[c] = y_lum * (ratio[c] + (around[c] - ratio[c]) * t);
            }
        }
    });
}

/// Luminance in stops (log2), floored at [`DARK`].
fn stops(y: f32) -> f32 {
    if y > DARK { y.log2() } else { DARK.log2() }
}

/// How much a pixel of chromaticity `ratio` (RGB over luminance) next to an
/// edge of strength `edge` (stops) is fringe, from 0 to 1.
fn fringe(ratio: [f32; 3], edge: f32, space: WorkingSpace) -> f32 {
    let near = smoothstep(EDGE_FROM, EDGE_AT, edge);
    if near <= 0.0 {
        return 0.0;
    }
    // Opponent axes on the chromaticity: magenta against green, blue against red.
    let neutral = core_color::luminance([1.0, 1.0, 1.0], space).max(f32::MIN_POSITIVE);
    let [r, g, b] = ratio.map(|v| v * neutral);
    let magenta = 0.5 * (r + b) - g;
    let blue = b - r;
    let chroma = magenta.hypot(blue);
    let coloured = smoothstep(CHROMA_FROM, CHROMA_AT, chroma);
    if coloured <= 0.0 || magenta <= 0.0 {
        return 0.0;
    }
    // Hue from magenta (0°) towards blue (about 63° for a pure blue);
    // reds are below 0°, sky blues above 70°.
    let hue = blue.atan2(magenta).to_degrees();
    let in_hue = smoothstep(-25.0, -10.0, hue) * (1.0 - smoothstep(50.0, 65.0, hue));
    near * coloured * in_hue
}

/// Hermite step from 0 at `edge0` to 1 at `edge1`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Cells of `size × size` pixels over the image.
struct Grid {
    width: usize,
    height: usize,
    size: usize,
    cols: usize,
    rows: usize,
}

impl Grid {
    /// Cells about as large as a fringe is wide: 2 pixels up to 1200 pixels
    /// on the long side, then one more pixel per 1200 (5 for a 26 MP file).
    fn new(width: usize, height: usize) -> Self {
        let size = (width.max(height) / 1200).clamp(2, 12);
        Self { width, height, size, cols: width.div_ceil(size), rows: height.div_ceil(size) }
    }

    fn cells(&self) -> usize {
        self.cols * self.rows
    }

    /// The pixels of cell `(gx, gy)`.
    fn pixels_of(&self, gx: usize, gy: usize) -> impl Iterator<Item = (usize, usize)> + use<> {
        let (x0, y0) = (gx * self.size, gy * self.size);
        let (x1, y1) = ((x0 + self.size).min(self.width), (y0 + self.size).min(self.height));
        (y0..y1).flat_map(move |y| (x0..x1).map(move |x| (x, y)))
    }

    /// Each cell's largest value among itself and its 8 neighbours.
    fn spread_max(&self, values: &[f32]) -> Vec<f32> {
        self.spread(values, 0.0_f32, f32::max)
    }

    /// Each cell's sum with its 8 neighbours.
    fn spread_sum(&self, values: &[[f32; 4]]) -> Vec<[f32; 4]> {
        self.spread(values, [0.0; 4], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]])
    }

    fn spread<T: Copy>(&self, values: &[T], zero: T, join: impl Fn(T, T) -> T) -> Vec<T> {
        let mut out = vec![zero; values.len()];
        for gy in 0..self.rows {
            for gx in 0..self.cols {
                let mut acc = zero;
                for ny in gy.saturating_sub(1)..(gy + 2).min(self.rows) {
                    for nx in gx.saturating_sub(1)..(gx + 2).min(self.cols) {
                        acc = join(acc, values[ny * self.cols + nx]);
                    }
                }
                out[gy * self.cols + gx] = acc;
            }
        }
        out
    }

    /// Cell coordinates and weights for bilinear interpolation between cell
    /// centres at pixel `(x, y)`.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn corners(&self, x: usize, y: usize) -> ([usize; 2], [usize; 2], f32, f32) {
        let s = self.size as f32;
        let fx = ((x as f32 + 0.5) / s - 0.5).clamp(0.0, (self.cols - 1) as f32);
        let fy = ((y as f32 + 0.5) / s - 0.5).clamp(0.0, (self.rows - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.cols - 1), (y0 + 1).min(self.rows - 1));
        ([x0, x1], [y0, y1], fx - x0 as f32, fy - y0 as f32)
    }

    /// `values` (one per cell) interpolated at pixel `(x, y)`.
    fn sample(&self, values: &[f32], x: usize, y: usize) -> f32 {
        let ([x0, x1], [y0, y1], tx, ty) = self.corners(x, y);
        let at = |cx: usize, cy: usize| values[cy * self.cols + cx];
        let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
        top + (bottom - top) * ty
    }

    /// [`Self::sample`] for three values per cell.
    fn sample3(&self, values: &[[f32; 3]], x: usize, y: usize) -> [f32; 3] {
        let ([x0, x1], [y0, y1], tx, ty) = self.corners(x, y);
        let at = |cx: usize, cy: usize| values[cy * self.cols + cx];
        std::array::from_fn(|c| {
            let top = at(x0, y0)[c] + (at(x1, y0)[c] - at(x0, y0)[c]) * tx;
            let bottom = at(x0, y1)[c] + (at(x1, y1)[c] - at(x0, y1)[c]) * tx;
            top + (bottom - top) * ty
        })
    }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;

    const SPACE: WorkingSpace = WorkingSpace::LinearRec2020;

    fn lum(px: &Rgba) -> f32 {
        core_color::luminance([px[0], px[1], px[2]], SPACE)
    }

    /// Chroma of a pixel relative to its luminance (0 for a grey).
    fn chroma(px: &Rgba) -> f32 {
        let y = lum(px);
        let [r, g, b] = [px[0] / y, px[1] / y, px[2] / y];
        (r - g).abs().max((b - g).abs()).max((r - b).abs())
    }

    /// A bright white square on a dark grey ground with a purple band two
    /// pixels wide around it, as a lens draws it.
    fn halo(size: usize, ground: [f32; 3]) -> Vec<Rgba> {
        let (lo, hi) = (size / 3, 2 * size / 3);
        (0..size * size)
            .map(|i| {
                let (x, y) = (i % size, i / size);
                let inside = (lo..hi).contains(&x) && (lo..hi).contains(&y);
                let near = (lo - 2..hi + 2).contains(&x) && (lo - 2..hi + 2).contains(&y);
                if inside {
                    [4.0, 4.0, 4.0, 1.0]
                } else if near {
                    [0.30, 0.04, 0.45, 1.0]
                } else {
                    [ground[0], ground[1], ground[2], 1.0]
                }
            })
            .collect()
    }

    #[test]
    fn the_purple_halo_of_a_highlight_loses_its_colour_and_keeps_its_brightness() {
        let size = 60;
        let before = halo(size, [0.02, 0.02, 0.02]);
        let mut after = before.clone();
        defringe(&mut after, size, size, SPACE, 1.0);
        let i = (size / 3 - 1) * size + size / 2; // in the band, above the square
        assert!(chroma(&before[i]) > 0.5);
        assert!(chroma(&after[i]) < 0.15, "still coloured: {:?}", after[i]);
        for (a, b) in before.iter().zip(&after) {
            assert!((lum(a) - lum(b)).abs() <= 1e-4 * lum(a).max(1.0), "{a:?} → {b:?}");
        }
    }

    #[test]
    fn the_halo_takes_the_colour_of_the_ground_around_it() {
        let size = 60;
        let green = [0.02, 0.06, 0.02];
        let mut img = halo(size, green);
        defringe(&mut img, size, size, SPACE, 1.0);
        let px = img[(size / 3 - 1) * size + size / 2];
        assert!(px[1] > px[0] && px[1] > px[2], "not green: {px:?}");
    }

    #[test]
    fn nothing_changes_at_zero_or_away_from_edges() {
        let size = 60;
        let before = halo(size, [0.02, 0.02, 0.02]);
        let mut zero = before.clone();
        defringe(&mut zero, size, size, SPACE, 0.0);
        assert_eq!(zero, before);

        // A purple flower: flat colour, no strong edge.
        let flat: Vec<Rgba> = (0..size * size)
            .map(|i| if (i % size) < size / 2 { [0.30, 0.04, 0.45, 1.0] } else { [0.25, 0.05, 0.40, 1.0] })
            .collect();
        let mut out = flat.clone();
        defringe(&mut out, size, size, SPACE, 1.0);
        assert_eq!(out, flat);
    }

    #[test]
    fn blue_sky_and_red_edges_are_not_fringe() {
        let size = 60;
        for colour in [[0.20, 0.35, 0.60], [0.50, 0.03, 0.03]] {
            let before: Vec<Rgba> =
                (0..size * size)
                    .map(|i| {
                        if (i % size) < size / 2 {
                            [4.0, 4.0, 4.0, 1.0]
                        } else {
                            [colour[0], colour[1], colour[2], 1.0]
                        }
                    })
                    .collect();
            let mut after = before.clone();
            defringe(&mut after, size, size, SPACE, 1.0);
            let i = 10 * size + size / 2 + 1;
            for c in 0..3 {
                assert!(
                    (after[i][c] - before[i][c]).abs() < 1e-3 * before[i][c].max(1.0),
                    "{colour:?}: {:?}",
                    after[i]
                );
            }
        }
    }

    #[test]
    fn tiny_dark_and_odd_images_are_safe() {
        for (w, h) in [(1, 1), (2, 5), (3, 3), (7, 1201)] {
            let mut img = vec![[0.0, 0.0, 0.0, 1.0]; w * h];
            if let Some(p) = img.get_mut(0) {
                *p = [f32::NAN, 1.0, -1.0, 1.0];
            }
            defringe(&mut img, w, h, SPACE, 1.0);
        }
    }

    #[test]
    fn grid_cells_cover_the_image_once() {
        let g = Grid::new(2500, 1300);
        assert_eq!(g.size, 2);
        let mut seen = vec![0_u8; 2500 * 1300];
        for gy in 0..g.rows {
            for gx in 0..g.cols {
                for (x, y) in g.pixels_of(gx, gy) {
                    seen[y * 2500 + x] += 1;
                }
            }
        }
        assert!(seen.iter().all(|&n| n == 1));
        assert_eq!(Grid::new(6240, 4160).size, 5);
    }
}
