//! Demosaic of Fujifilm X-Trans sensors (6×6 colour filter), on the CPU.
//!
//! Two passes over a white-balanced mosaic:
//!
//! 1. Green everywhere. X-Trans is more than half green, so every red or
//!    blue photosite has green photosites at most two steps away along
//!    several directions. Each direction (horizontal, vertical, the two
//!    diagonals) with green on both sides gives an estimate, linearly
//!    interpolated between the two greens; estimates are mixed with weights
//!    that fall with the green difference along that direction, so an edge
//!    is followed rather than crossed.
//! 2. Red and blue from colour differences. The difference between a
//!    colour and green changes slowly even where brightness changes fast,
//!    so each missing red (blue) is the local green plus the mean of
//!    `red − green` at the red photosites of the 3×3 neighbourhood, widened
//!    to 5×5 where the 3×3 has none.
//!
//! Borders are mirrored like the Bayer path ("reflect-101"). Mirroring does
//! not preserve the X-Trans pattern, so the channel used is always that of
//! the photosite actually read.

use crate::lens::parallel_rows;
use crate::reflect;

const RED: usize = 0;
const GREEN: usize = 1;
const BLUE: usize = 2;

/// Directions for the green estimate: horizontal, vertical, both diagonals.
const DIRECTIONS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];

/// A white-balanced X-Trans mosaic with mirrored borders.
struct Mosaic<'a> {
    values: &'a [f32],
    width: usize,
    height: usize,
    pattern: &'a [[u8; 6]; 6],
}

impl Mosaic<'_> {
    /// Channel of the photosite at `(x, y)`; the pattern starts at the image's top-left corner.
    fn channel(&self, x: usize, y: usize) -> usize {
        usize::from(self.pattern[y % 6][x % 6])
    }

    /// Index of the photosite read for the position `(dx, dy)` away from `(x, y)`.
    fn site(&self, x: usize, y: usize, dx: isize, dy: isize) -> usize {
        reflect(y as isize + dy, self.height) * self.width + reflect(x as isize + dx, self.width)
    }

    /// Channel of the photosite at index `i`.
    fn channel_of(&self, i: usize) -> usize {
        self.channel(i % self.width, i / self.width)
    }

    /// The nearest green photosite (value, steps) within two steps along `(ux, uy)`.
    fn nearest_green(&self, x: usize, y: usize, ux: isize, uy: isize) -> Option<(f32, f32)> {
        [(1_isize, 1.0_f32), (2, 2.0)].into_iter().find_map(|(step, distance)| {
            let i = self.site(x, y, ux * step, uy * step);
            (self.channel_of(i) == GREEN).then(|| (self.values[i], distance))
        })
    }

    /// Green at `(x, y)`: the photosite itself, or the edge-aware estimate.
    fn green_at(&self, x: usize, y: usize) -> f32 {
        let centre = self.values[y * self.width + x];
        if self.channel(x, y) == GREEN {
            return centre;
        }
        let mut estimates = [(0.0_f32, 0.0_f32); 4];
        let mut n = 0;
        for (ux, uy) in DIRECTIONS {
            if let (Some((ga, da)), Some((gb, db))) =
                (self.nearest_green(x, y, ux, uy), self.nearest_green(x, y, -ux, -uy))
            {
                // Linear interpolation between the two greens: exact on a
                // ramp even when they are at different distances.
                estimates[n] = ((ga * db + gb * da) / (da + db), (ga - gb).abs());
                n += 1;
            }
        }
        if n == 0 {
            return self.mean_green_around(x, y).unwrap_or(centre);
        }
        let estimates = &estimates[..n];
        // Differences smaller than about 1 % of the local level are treated
        // as noise, so flat areas mix all directions instead of flipping
        // between them from one photosite to the next.
        #[allow(clippy::cast_precision_loss)] // n ≤ 4
        let level = estimates.iter().map(|(e, _)| e.abs()).sum::<f32>() / n as f32;
        let floor = 0.01 * level + 1e-5;
        let (mut sum, mut weights) = (0.0_f32, 0.0_f32);
        for &(estimate, difference) in estimates {
            let w = 1.0 / ((difference + floor) * (difference + floor));
            sum += w * estimate;
            weights += w;
        }
        sum / weights
    }

    /// Mean of the green photosites in the 5×5 neighbourhood, for images too
    /// small to have green on both sides in any direction.
    fn mean_green_around(&self, x: usize, y: usize) -> Option<f32> {
        let (mut sum, mut count) = (0.0_f32, 0_u16);
        for dy in -2_isize..=2 {
            for dx in -2_isize..=2 {
                let i = self.site(x, y, dx, dy);
                if self.channel_of(i) == GREEN {
                    sum += self.values[i];
                    count += 1;
                }
            }
        }
        (count > 0).then(|| sum / f32::from(count))
    }

    /// Red, green and blue at `(x, y)`, given the green plane.
    fn rgb_at(&self, green: &[f32], x: usize, y: usize) -> [f32; 3] {
        let i = y * self.width + x;
        let own = self.channel(x, y);
        let g = green[i];
        let mut out = [0.0, g, 0.0];
        // Distance-weighted means of (colour − green) for red and blue together.
        let mut sums = [(0.0_f32, 0.0_f32); 3];
        self.add_differences(green, x, y, 1, &mut sums);
        for c in [RED, BLUE] {
            if c == own {
                out[c] = self.values[i];
                continue;
            }
            if sums[c].1 <= 0.0 {
                // No photosite of this colour in the 3×3: look in the 5×5.
                let mut wide = [(0.0_f32, 0.0_f32); 3];
                self.add_differences(green, x, y, 2, &mut wide);
                sums[c] = wide[c];
            }
            let (sum, weights) = sums[c];
            // Without any photosite of the colour nearby, assume neutral.
            out[c] = if weights > 0.0 { g + sum / weights } else { g };
        }
        out
    }

    /// Adds `value − green`, weighted by inverse squared distance, of the
    /// red and blue photosites within `radius` of `(x, y)` to `sums[channel]`.
    fn add_differences(&self, green: &[f32], x: usize, y: usize, radius: isize, sums: &mut [(f32, f32); 3]) {
        let centre = y * self.width + x;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let j = self.site(x, y, dx, dy);
                // Mirroring can land on the centre itself in tiny images.
                let c = self.channel_of(j);
                if j == centre || c == GREEN {
                    continue;
                }
                #[allow(clippy::cast_precision_loss)] // at most 8
                let w = 1.0 / (dx * dx + dy * dy) as f32;
                sums[c].0 += w * (self.values[j] - green[j]);
                sums[c].1 += w;
            }
        }
    }
}

/// Camera RGB from a white-balanced X-Trans mosaic of `width × height`
/// photosites; `pattern[row][column]` is the channel of each photosite of
/// the 6×6 tile starting at the image's top-left corner.
pub(crate) fn demosaic(values: &[f32], width: usize, height: usize, pattern: &[[u8; 6]; 6]) -> Vec<[f32; 3]> {
    let mosaic = Mosaic { values, width, height, pattern };
    let mut green = vec![0.0_f32; width * height];
    parallel_rows(&mut green, width, |first, rows| {
        for (i, g) in rows.iter_mut().enumerate() {
            *g = mosaic.green_at(i % width, first + i / width);
        }
    });
    let mut out = vec![[0.0_f32; 3]; width * height];
    parallel_rows(&mut out, width, |first, rows| {
        for (i, px) in rows.iter_mut().enumerate() {
            *px = mosaic.rgb_at(&green, i % width, first + i / width);
        }
    });
    out
}

#[cfg(test)]
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

    /// The tile as seen from a crop starting at column `ox`, row `oy`.
    fn shifted(ox: usize, oy: usize) -> [[u8; 6]; 6] {
        let mut p = [[0; 6]; 6];
        for (r, row) in p.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = TILE[(r + oy) % 6][(c + ox) % 6];
            }
        }
        p
    }

    /// A mosaic sampling `scene(x, y)` through `pattern`.
    fn mosaic_of(w: usize, h: usize, pattern: &[[u8; 6]; 6], scene: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
        (0..w * h).map(|i| scene(i % w, i / w)[usize::from(pattern[(i / w) % 6][(i % w) % 6])]).collect()
    }

    #[test]
    fn every_3x3_neighbourhood_of_the_tile_has_all_three_colours() {
        // The property the demosaic relies on away from the borders.
        let m = Mosaic { values: &[0.0; 144], width: 12, height: 12, pattern: &TILE };
        for y in 0..6 {
            for x in 0..6 {
                let mut seen = [false; 3];
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        seen[m.channel_of(m.site(x + 3, y + 3, dx, dy))] = true;
                    }
                }
                assert_eq!(seen, [true; 3], "at {x},{y}");
            }
        }
    }

    #[test]
    fn a_flat_colour_is_reconstructed_exactly_everywhere() {
        let colour = [0.2, 0.55, 0.8];
        let (w, h) = (17, 13);
        let values = mosaic_of(w, h, &TILE, |_, _| colour);
        for (i, px) in demosaic(&values, w, h, &TILE).iter().enumerate() {
            for c in 0..3 {
                assert!((px[c] - colour[c]).abs() < 1e-6, "pixel {i}: {px:?}");
            }
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // the same arithmetic on the same values must give the same bits
    fn the_phase_of_the_tile_is_respected_at_any_crop_origin() {
        // Demosaicing a crop gives the same pixels as demosaicing the whole
        // frame (away from the crop's own borders), whatever its origin.
        let scene = |x: usize, y: usize| {
            #[allow(clippy::cast_precision_loss)]
            let (x, y) = (x as f32, y as f32);
            [0.3 + 0.2 * (x * 0.7).sin(), 0.4 + 0.1 * (y * 0.5).cos(), 0.5 + 0.004 * x * y]
        };
        let (w, h) = (30, 30);
        let full = demosaic(&mosaic_of(w, h, &TILE, scene), w, h, &TILE);
        for oy in 0..6 {
            for ox in 0..6 {
                let (cw, ch) = (18, 17);
                let pattern = shifted(ox, oy);
                let crop = demosaic(&mosaic_of(cw, ch, &pattern, |x, y| scene(x + ox, y + oy)), cw, ch, &pattern);
                // Green looks two photosites away, red and blue two more.
                for y in 4..ch - 4 {
                    for x in 4..cw - 4 {
                        assert_eq!(crop[y * cw + x], full[(y + oy) * w + x + ox], "origin {ox},{oy} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_smooth_gradient_stays_smooth() {
        let slope = 0.002_f32;
        let scene = |x: usize, y: usize| {
            #[allow(clippy::cast_precision_loss)]
            let (x, y) = (x as f32, y as f32);
            [0.2 + slope * x, 0.3 + slope * y, 0.6 - slope * (x + y) / 2.0]
        };
        let (w, h) = (36, 30);
        let out = demosaic(&mosaic_of(w, h, &TILE, scene), w, h, &TILE);
        for y in 0..h {
            for x in 0..w {
                let px = out[y * w + x];
                let want = scene(x, y);
                // Interior: within a few slopes (the colour differences are
                // averaged over neighbours on one side); borders, where
                // mirroring folds the ramp: a few more.
                let interior = (2..w - 2).contains(&x) && (2..h - 2).contains(&y);
                let tol = if interior { 3.0 * slope } else { 6.0 * slope };
                for c in 0..3 {
                    assert!((px[c] - want[c]).abs() <= tol, "{x},{y} channel {c}: {px:?}, want {want:?}");
                }
                if x > 0 {
                    let left = out[y * w + x - 1];
                    for c in 0..3 {
                        assert!((px[c] - left[c]).abs() <= 8.0 * slope, "step at {x},{y}: {left:?} → {px:?}");
                    }
                }
            }
        }
        // Green, interpolated along the rows and columns, is exact inside.
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                assert!((out[y * w + x][1] - scene(x, y)[1]).abs() < 1e-5, "green at {x},{y}");
            }
        }
    }

    #[test]
    fn an_edge_is_followed_not_blurred_across() {
        // A vertical edge in a grey scene: green next to it must stay on its own side.
        let scene = |x: usize, _y: usize| if x < 9 { [0.1; 3] } else { [0.8; 3] };
        let (w, h) = (18, 12);
        let out = demosaic(&mosaic_of(w, h, &TILE, scene), w, h, &TILE);
        for y in 2..h - 2 {
            assert!((out[y * w + 8][1] - 0.1).abs() < 0.02, "left of the edge, row {y}: {:?}", out[y * w + 8]);
            assert!((out[y * w + 9][1] - 0.8).abs() < 0.02, "right of the edge, row {y}: {:?}", out[y * w + 9]);
        }
    }

    #[test]
    fn tiny_images_do_not_panic_and_stay_neutral() {
        for (w, h) in [(1, 1), (2, 1), (1, 3), (2, 2), (3, 3), (5, 4), (6, 6)] {
            for (ox, oy) in [(0, 0), (2, 0), (5, 3)] {
                let pattern = shifted(ox, oy);
                let out = demosaic(&vec![0.4; w * h], w, h, &pattern);
                assert_eq!(out.len(), w * h);
                for px in out {
                    assert!(px.iter().all(|v| (v - 0.4).abs() < 1e-6), "{w}×{h} at {ox},{oy}: {px:?}");
                }
            }
        }
    }
}
