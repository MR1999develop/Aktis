//! Demosaic of Fujifilm X-Trans sensors with Frank Markesteijn's algorithm
//! (three passes), the one dcraw, RawTherapee and darktable use.
//!
//! X-Trans is more than half green and its greens repeat every three
//! photosites, so every photosite has a "hexagon" of neighbours of the other
//! kind (green around red and blue, red and blue around green). On a
//! white-balanced mosaic, in tiles:
//!
//! 1. Green at red and blue photosites, four ways (horizontal, vertical, the
//!    two diagonals), each weighted from the hexagon and kept between the
//!    smallest and largest neighbouring green, so it cannot overshoot.
//! 2. For each of the four, red and blue from colour differences: at the
//!    solitary greens, at the other colour's photosites, then in the 2×2
//!    blocks of green.
//! 3. Two more passes recompute green from the interpolated neighbours and
//!    then red and blue again, giving four more refined candidates.
//! 4. The eight candidates are turned into CIELab and differentiated along
//!    their own direction; at each pixel the candidates whose neighbourhood
//!    is most homogeneous (smallest change) are averaged. A direction that
//!    follows an edge changes least along it, so the edge is followed and
//!    the maze patterns and false colours of simpler methods stay away.
//!
//! The arithmetic follows dcraw's `xtrans_interpolate` (in floating point);
//! the tiles carry a margin wide enough that the steps' reach (33
//! photosites) stays inside, mirrored at the image's borders with the 6×6
//! pattern kept, so borders need no special case and tiles leave no seams.

use crate::lens::parallel_bands;

const GREEN: usize = 1;

/// Margin read around each tile: more than the steps' reach, a multiple of
/// six so a tile has the colours of the image.
const PAD: usize = 36;
/// Size of a tile without its margin (multiples of six).
const TILE_W: usize = 510;
const TILE_H: usize = 192;
/// Candidates: four directions, from the first pass and from the last.
const DIRS: usize = 8;

/// Smallest image the algorithm handles (the mirrored margins need a whole
/// repeat of the pattern on each side).
pub(crate) const MIN_SIDE: usize = 12;

/// Red, green and blue at every photosite of a white-balanced X-Trans
/// `mosaic`, or `None` when the image is smaller than [`MIN_SIDE`] or the
/// pattern is not an X-Trans layout.
pub(crate) fn demosaic(mosaic: &[f32], width: usize, height: usize, pattern: &[[u8; 6]; 6]) -> Option<Vec<[f32; 3]>> {
    demosaic_in_tiles(mosaic, width, height, pattern, TILE_W, TILE_H)
}

fn demosaic_in_tiles(
    mosaic: &[f32],
    width: usize,
    height: usize,
    pattern: &[[u8; 6]; 6],
    tile_w: usize,
    tile_h: usize,
) -> Option<Vec<[f32; 3]>> {
    assert_eq!(mosaic.len(), width * height, "mosaic size");
    if width < MIN_SIDE || height < MIN_SIDE {
        return None;
    }
    let layout = Layout::of(pattern)?;
    let mut out = vec![[0.0_f32; 3]; width * height];
    parallel_bands(&mut out, width, tile_h, |first, rows| {
        let h = rows.len() / width;
        let mut tile = Tile::default();
        let mut x0 = 0;
        while x0 < width {
            let region = Region { x0, y0: first, w: tile_w.min(width - x0), h };
            tile.develop(mosaic, width, height, &layout, region);
            tile.write(rows, width, region);
            x0 += region.w;
        }
    });
    Some(out)
}

/// The pattern and what the algorithm derives from it.
struct Layout {
    pattern: [[u8; 6]; 6],
    /// For each position modulo 3, the hexagon of neighbours `(dy, dx)`.
    hex: [[[(isize, isize); 8]; 3]; 3],
    /// Position modulo 3 of the solitary greens.
    sg_row: usize,
    sg_col: usize,
}

impl Layout {
    /// The hexagons as dcraw maps them, or `None` for a pattern without
    /// solitary greens (not X-Trans).
    #[allow(clippy::cast_sign_loss)] // positions 0..6 of the pattern
    fn of(pattern: &[[u8; 6]; 6]) -> Option<Self> {
        const ORTH: [isize; 12] = [1, 0, 0, 1, -1, 0, 0, -1, 1, 0, 0, 1];
        const PATT: [[isize; 16]; 2] = [
            [0, 1, 0, -1, 2, 0, -1, 0, 1, 1, 1, -1, 0, 0, 0, 0],
            [0, 1, 0, -2, 1, 0, -2, 0, 1, 1, -2, -2, 1, -1, -1, 1],
        ];
        let fcol =
            |r: isize, c: isize| usize::from(pattern[(r + 6).rem_euclid(6) as usize][(c + 6).rem_euclid(6) as usize]);
        let mut hex = [[[(0, 0); 8]; 3]; 3];
        let mut mapped = [[false; 3]; 3];
        let mut solitary = None;
        for row in 0..3_isize {
            for col in 0..3_isize {
                let g = usize::from(fcol(row, col) == GREEN);
                let mut ng = 0;
                for d in (0..10).step_by(2) {
                    if fcol(row + ORTH[d], col + ORTH[d + 2]) == GREEN {
                        ng = 0;
                    } else {
                        ng += 1;
                    }
                    if ng == 4 {
                        solitary = Some((row as usize, col as usize));
                    }
                    if ng == g + 1 {
                        for c in 0..8 {
                            let (py, px) = (PATT[g][c * 2], PATT[g][c * 2 + 1]);
                            let v = ORTH[d] * py + ORTH[d + 1] * px;
                            let h = ORTH[d + 2] * py + ORTH[d + 3] * px;
                            hex[row as usize][col as usize][c ^ ((g * 2) & d)] = (v, h);
                        }
                        mapped[row as usize][col as usize] = true;
                    }
                }
            }
        }
        let (sg_row, sg_col) = solitary?;
        mapped.iter().flatten().all(|m| *m).then_some(Self { pattern: *pattern, hex, sg_row, sg_col })
    }
}

#[derive(Debug, Clone, Copy)]
struct Region {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

/// One tile with its margin and the planes of the steps.
#[derive(Debug, Default)]
struct Tile {
    /// Padded width (the row stride) and height.
    w: usize,
    h: usize,
    cfa: Vec<f32>,
    channel: Vec<u8>,
    /// Green as read: the photosite's at green, the smallest neighbouring
    /// green elsewhere; and the largest neighbouring green.
    g_min: Vec<f32>,
    g_max: Vec<f32>,
    rgb: Vec<Vec<[f32; 3]>>,
    lab: Vec<[f32; 3]>,
    drv: Vec<Vec<f32>>,
    /// Eight times the smallest change at each pixel.
    tr: Vec<f32>,
    /// Scratch planes of the homogeneity counts.
    mask: Vec<u16>,
    count: Vec<u16>,
    tmp: Vec<u16>,
    /// Per candidate, how many of the 5×5 pixels around are homogeneous
    /// (counted over 3×3 each).
    homo: Vec<Vec<u16>>,
}

impl Tile {
    fn develop(&mut self, mosaic: &[f32], width: usize, height: usize, l: &Layout, r: Region) {
        self.load(mosaic, width, height, l, r);
        self.green_bounds(l);
        for k in 0..4 {
            for (i, px) in self.rgb[k].iter_mut().enumerate() {
                *px = initial(&self.cfa, &self.channel, &self.g_min, i);
            }
        }
        self.green(l);
        for pass in 0..3 {
            let base = if pass == 0 { 0 } else { 4 };
            if pass == 1 {
                let (first, rest) = self.rgb.split_at_mut(4);
                for (to, from) in rest.iter_mut().zip(first.iter()) {
                    to.copy_from_slice(from);
                }
            }
            if pass > 0 {
                self.refine_green(l, base);
            }
            self.solitary_greens(l, base);
            self.red_at_blue(l, base);
            self.green_blocks(l, base);
        }
        self.homogeneity();
    }

    fn at(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    /// Offset of a step `(dy, dx)` times `k` in the tile.
    fn off(&self, (dy, dx): (isize, isize), k: isize) -> isize {
        k * (dy * self.w as isize + dx)
    }

    fn load(&mut self, mosaic: &[f32], width: usize, height: usize, l: &Layout, r: Region) {
        self.w = r.w + 2 * PAD;
        self.h = r.h + 2 * PAD;
        let n = self.w * self.h;
        // Every step writes what the next ones read, so planes of the same
        // size are reused as they are.
        for plane in [&mut self.cfa, &mut self.g_min, &mut self.g_max] {
            plane.resize(n, 0.0);
        }
        self.channel.resize(n, 0);
        self.rgb.resize_with(DIRS, Vec::new);
        self.drv.resize_with(DIRS, Vec::new);
        self.homo.resize_with(DIRS, Vec::new);
        for d in 0..DIRS {
            self.rgb[d].resize(n, [0.0; 3]);
            self.drv[d].resize(n, 0.0);
            self.homo[d].resize(n, 0);
        }
        self.lab.resize(n, [0.0; 3]);
        self.tr.resize(n, 0.0);
        for plane in [&mut self.mask, &mut self.count, &mut self.tmp] {
            plane.resize(n, 0);
        }
        // The tile starts PAD (a multiple of six) before the region, which
        // starts at a multiple of six: tile and image share the pattern.
        let columns: Vec<usize> =
            (0..self.w).map(|tx| mirror6(r.x0 as isize + tx as isize - PAD as isize, width)).collect();
        for ty in 0..self.h {
            let gy = mirror6(r.y0 as isize + ty as isize - PAD as isize, height);
            let line = &mosaic[gy * width..(gy + 1) * width];
            for (tx, &gx) in columns.iter().enumerate() {
                let i = ty * self.w + tx;
                self.cfa[i] = line[gx].max(0.0);
                self.channel[i] = l.pattern[ty % 6][tx % 6];
            }
        }
    }

    /// The smallest and largest green of each red or blue photosite's hexagon.
    fn green_bounds(&mut self, l: &Layout) {
        for y in 2..self.h - 2 {
            for x in 2..self.w - 2 {
                let i = self.at(x, y);
                if usize::from(self.channel[i]) == GREEN {
                    self.g_min[i] = self.cfa[i];
                    self.g_max[i] = self.cfa[i];
                    continue;
                }
                let hex = &l.hex[y % 3][x % 3];
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for &step in &hex[..6] {
                    let v = self.cfa[shift(i, self.off(step, 1))];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                self.g_min[i] = lo;
                self.g_max[i] = hi;
            }
        }
    }

    /// Whether row `y` is not one of the solitary greens' rows (dcraw's
    /// `(row - sgrow) % 3`).
    fn off_sg_row(&self, l: &Layout, y: usize) -> bool {
        !(y + 3 - l.sg_row).is_multiple_of(3)
    }

    /// Step 1: green at red and blue, four ways.
    fn green(&mut self, l: &Layout) {
        for y in 6..self.h - 6 {
            let swap = usize::from(!self.off_sg_row(l, y));
            for x in 6..self.w - 6 {
                let i = self.at(x, y);
                let f = usize::from(self.channel[i]);
                if f == GREEN {
                    continue;
                }
                let hex = &l.hex[y % 3][x % 3];
                let o = |k: usize, m: isize| shift(i, self.off(hex[k], m));
                let g = |j: usize| self.g_min[j]; // green as read
                let p = |j: usize| image(&self.cfa, &self.channel, &self.g_min, j, f);
                let own = p(i);
                let mut color = [0.0_f32; 4];
                color[0] = 174.0 * (g(o(1, 1)) + g(o(0, 1))) - 46.0 * (g(o(1, 2)) + g(o(0, 2)));
                color[1] = 223.0 * g(o(3, 1)) + 33.0 * g(o(2, 1)) + 92.0 * (own - p(o(2, -1)));
                for c in 0..2 {
                    color[2 + c] = 164.0 * g(o(4 + c, 1))
                        + 92.0 * g(o(4 + c, -2))
                        + 33.0 * (2.0 * own - p(o(4 + c, 3)) - p(o(4 + c, -3)));
                }
                let (lo, hi) = (self.g_min[i], self.g_max[i]);
                for (c, v) in color.iter().enumerate() {
                    self.rgb[c ^ swap][i][1] = (v / 256.0).clamp(lo, hi);
                }
            }
        }
    }

    /// Passes 2 and 3: green again from the interpolated neighbours.
    fn refine_green(&mut self, l: &Layout, base: usize) {
        for y in 4..self.h - 4 {
            let swap = usize::from(!self.off_sg_row(l, y));
            for x in 4..self.w - 4 {
                let i = self.at(x, y);
                let f = usize::from(self.channel[i]);
                if f == GREEN {
                    continue;
                }
                let hex = l.hex[y % 3][x % 3];
                for (d, &step) in hex.iter().enumerate().take(6).skip(3) {
                    let b = base + ((d - 2) ^ swap);
                    let (near, far) = (shift(i, self.off(step, 1)), shift(i, self.off(step, -2)));
                    let px = &self.rgb[b];
                    let val = px[far][1] + 2.0 * px[near][1] - px[far][f] - 2.0 * px[near][f] + 3.0 * px[i][f];
                    self.rgb[b][i][1] = (val / 3.0).clamp(self.g_min[i], self.g_max[i]);
                }
            }
        }
    }

    /// Step 2a: red and blue at the solitary greens.
    fn solitary_greens(&mut self, l: &Layout, base: usize) {
        let s = self.w as isize;
        let first = |start: usize, sg: usize| start + (sg + 3 - start % 3) % 3; // first ≥ start ≡ sg (mod 3)
        let mut y = first(2, l.sg_row);
        while y < self.h - 2 {
            let mut x = first(2, l.sg_col);
            while x < self.w - 2 {
                let i = self.at(x, y);
                let right = usize::from(self.channel[i + 1]);
                // Horizontal and vertical estimates from one candidate's greens,
                // with how far each is from a smooth colour difference.
                let estimate = |px: &[[f32; 3]], step: isize, mut h: usize| {
                    let mut color = [0.0_f32; 3];
                    let mut diff = 0.0_f32;
                    for c in 0..2 {
                        let k = step << c;
                        let (a, b) = (shift(i, k), shift(i, -k));
                        let g = 2.0 * px[i][1] - px[a][1] - px[b][1];
                        color[h] = g + px[a][h] + px[b][h];
                        diff += sq(px[a][1] - px[b][1] - px[a][h] + px[b][h]) + sq(g);
                        h ^= 2;
                    }
                    (color, diff)
                };
                for k in 0..4 {
                    let b = base + k;
                    let (hz, hd) = estimate(&self.rgb[b], 1, right);
                    let (vt, vd) = estimate(&self.rgb[b], s, right ^ 2);
                    let color = match k {
                        0 => hz,
                        1 => vt,
                        _ if hd < vd => hz,
                        _ => vt,
                    };
                    let px = &mut self.rgb[b][i];
                    px[0] = (color[0] / 2.0).max(0.0);
                    px[2] = (color[2] / 2.0).max(0.0);
                }
                x += 3;
            }
            y += 3;
        }
    }

    /// Step 2b: red at blue photosites and blue at red ones.
    fn red_at_blue(&mut self, l: &Layout, base: usize) {
        let s = self.w as isize;
        for y in 3..self.h - 3 {
            let vertical = self.off_sg_row(l, y);
            let (c_off, h_off) = if vertical { (s, 3) } else { (1, 3 * s) };
            for x in 3..self.w - 3 {
                let i = self.at(x, y);
                let own = usize::from(self.channel[i]);
                if own == GREEN {
                    continue;
                }
                let f = 2 - own;
                for d in 0..4 {
                    let px = &self.rgb[base + d];
                    let g0 = px[i][1];
                    let grad = |o: isize| (g0 - px[shift(i, o)][1]).abs() + (g0 - px[shift(i, -o)][1]).abs();
                    let forced = d > 1 || if vertical { d & 1 == 1 } else { d & 1 == 0 };
                    let k = if forced || grad(c_off) < 2.0 * grad(h_off) { c_off } else { h_off };
                    let (a, b) = (shift(i, k), shift(i, -k));
                    let v = (px[a][f] + px[b][f] + 2.0 * g0 - px[a][1] - px[b][1]) / 2.0;
                    self.rgb[base + d][i][f] = v.max(0.0);
                }
            }
        }
    }

    /// Step 2c: red and blue in the 2×2 blocks of green.
    fn green_blocks(&mut self, l: &Layout, base: usize) {
        for y in 2..self.h - 2 {
            if !self.off_sg_row(l, y) {
                continue;
            }
            for x in 2..self.w - 2 {
                if (x + 3 - l.sg_col).is_multiple_of(3) {
                    continue;
                }
                let i = self.at(x, y);
                let hex = l.hex[y % 3][x % 3];
                for k in 0..4 {
                    let (h1, h2) = (hex[2 * k], hex[2 * k + 1]);
                    let (a, b) = (shift(i, self.off(h1, 1)), shift(i, self.off(h2, 1)));
                    let px = &self.rgb[base + k];
                    let g0 = px[i][1];
                    let mut out = px[i];
                    let opposite = h1.0 + h2.0 == 0 && h1.1 + h2.1 == 0;
                    for c in [0, 2] {
                        out[c] = if opposite {
                            (2.0 * g0 - px[a][1] - px[b][1] + px[a][c] + px[b][c]) / 2.0
                        } else {
                            (3.0 * g0 - 2.0 * px[a][1] - px[b][1] + 2.0 * px[a][c] + px[b][c]) / 3.0
                        }
                        .max(0.0);
                    }
                    self.rgb[base + k][i] = out;
                }
            }
        }
    }

    /// Steps 3 and 4: derivatives in CIELab and the homogeneity of each candidate.
    fn homogeneity(&mut self) {
        let s = self.w as isize;
        let steps = [1, s, s + 1, s - 1];
        for d in 0..DIRS {
            for y in 2..self.h - 2 {
                for x in 2..self.w - 2 {
                    let i = self.at(x, y);
                    self.lab[i] = cielab(self.rgb[d][i]);
                }
            }
            let f = steps[d & 3];
            for y in 3..self.h - 3 {
                for x in 3..self.w - 3 {
                    let i = self.at(x, y);
                    let (l0, la, lb) = (self.lab[i], self.lab[shift(i, f)], self.lab[shift(i, -f)]);
                    let g = 2.0 * l0[0] - la[0] - lb[0];
                    self.drv[d][i] = sq(g)
                        + sq(2.0 * l0[1] - la[1] - lb[1] + g * 500.0 / 232.0)
                        + sq(2.0 * l0[2] - la[2] - lb[2] - g * 500.0 / 580.0);
                }
            }
        }
        // A candidate is homogeneous at a pixel when its change there is
        // within eight times the smallest of all candidates; count them over
        // 3×3, then sum the counts over 5×5 (separable sums).
        for y in 3..self.h - 3 {
            for x in 3..self.w - 3 {
                let i = self.at(x, y);
                self.tr[i] = 8.0 * (0..DIRS).map(|d| self.drv[d][i]).fold(f32::MAX, f32::min);
            }
        }
        for d in 0..DIRS {
            for y in 3..self.h - 3 {
                for x in 3..self.w - 3 {
                    let i = self.at(x, y);
                    self.mask[i] = u16::from(self.drv[d][i] <= self.tr[i]);
                }
            }
            box_sum(&self.mask, &mut self.tmp, &mut self.count, self.w, self.h, 1, 4);
            box_sum(&self.count, &mut self.tmp, &mut self.homo[d], self.w, self.h, 2, 6);
        }
    }

    /// The average of the most homogeneous candidates, for the region.
    fn write(&self, rows: &mut [[f32; 3]], width: usize, r: Region) {
        for ty in 0..r.h {
            for tx in 0..r.w {
                let i = self.at(tx + PAD, ty + PAD);
                let mut hm: [u16; DIRS] = std::array::from_fn(|d| self.homo[d][i]);
                // Of a direction's first and refined candidates, the more homogeneous one.
                for d in 0..DIRS - 4 {
                    if hm[d] < hm[d + 4] {
                        hm[d] = 0;
                    } else if hm[d] > hm[d + 4] {
                        hm[d + 4] = 0;
                    }
                }
                let mut best = hm.iter().copied().max().unwrap_or(0);
                best -= best >> 3;
                let (mut sum, mut n) = ([0.0_f32; 3], 0_u16);
                for (d, &m) in hm.iter().enumerate() {
                    if m >= best {
                        for (a, v) in sum.iter_mut().zip(self.rgb[d][i]) {
                            *a += v;
                        }
                        n += 1;
                    }
                }
                let n = f32::from(n.max(1));
                rows[ty * width + r.x0 + tx] = sum.map(|v| v / n);
            }
        }
    }
}

/// The photosite's channel `c` as dcraw's image holds it before
/// interpolation: its own value in its colour, the smallest neighbouring
/// green as green, zero otherwise.
fn image(cfa: &[f32], channel: &[u8], g_min: &[f32], i: usize, c: usize) -> f32 {
    if c == GREEN {
        g_min[i]
    } else if usize::from(channel[i]) == c {
        cfa[i]
    } else {
        0.0
    }
}

fn initial(cfa: &[f32], channel: &[u8], g_min: &[f32], i: usize) -> [f32; 3] {
    [0, 1, 2].map(|c| image(cfa, channel, g_min, i, c))
}

/// Sums of `src` over `(2r + 1)²` squares into `dst`, where they lie at
/// least `margin` (≥ `r`) inside the `w × h` plane.
fn box_sum(src: &[u16], tmp: &mut [u16], dst: &mut [u16], w: usize, h: usize, r: usize, margin: usize) {
    for y in margin - r..h - (margin - r) {
        for x in margin..w - margin {
            let i = y * w + x;
            tmp[i] = src[i - r..=i + r].iter().sum();
        }
    }
    for y in margin..h - margin {
        for x in margin..w - margin {
            let i = y * w + x;
            dst[i] = (0..=2 * r).map(|k| tmp[i + k * w - r * w]).sum();
        }
    }
}

fn shift(i: usize, o: isize) -> usize {
    i.wrapping_add_signed(o)
}

fn sq(v: f32) -> f32 {
    v * v
}

/// CIELab of a white-balanced camera colour, read as linear sRGB (only the
/// differences between neighbours matter here; a neutral stays neutral).
fn cielab(rgb: [f32; 3]) -> [f32; 3] {
    const M: [[f32; 3]; 3] = [
        [0.412_456_4 / 0.950_47, 0.357_576_1 / 0.950_47, 0.180_437_5 / 0.950_47],
        [0.212_672_9, 0.715_152_2, 0.072_175],
        [0.019_333_9 / 1.088_83, 0.119_192 / 1.088_83, 0.950_304_1 / 1.088_83],
    ];
    let f = |t: f32| {
        let t = t.max(0.0);
        if t > 0.008_856 { cbrt(t) } else { 7.787 * t + 16.0 / 116.0 }
    };
    let [x, y, z] = M.map(|row| f(row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]));
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

/// Cube root of a positive number: a first guess from the float's bits,
/// then two Newton steps (relative error below 1e-6, faster than
/// `f32::cbrt`; this runs 24 times per photosite).
fn cbrt(x: f32) -> f32 {
    let y = f32::from_bits(x.to_bits() / 3 + 709_921_077);
    let y = (2.0 * y + x / (y * y)) / 3.0;
    (2.0 * y + x / (y * y)) / 3.0
}

/// Mirrors a coordinate at the borders and moves it to the nearest place
/// with the same position in the 6×6 pattern; `n ≥ 12`.
fn mirror6(i: isize, n: usize) -> usize {
    let n = n as isize;
    let mut r = i;
    while r < 0 || r >= n {
        r = if r < 0 { -r } else { 2 * n - 2 - r };
    }
    let mut c = r + (i - r).rem_euclid(6);
    if c >= n {
        c -= 6;
    }
    usize::try_from(c).unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::cast_precision_loss, clippy::cast_sign_loss)]
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

    fn shifted(ox: usize, oy: usize) -> [[u8; 6]; 6] {
        let mut p = [[0; 6]; 6];
        for (r, row) in p.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = TILE[(r + oy) % 6][(c + ox) % 6];
            }
        }
        p
    }

    fn mosaic_of(w: usize, h: usize, pattern: &[[u8; 6]; 6], scene: impl Fn(usize, usize) -> [f32; 3]) -> Vec<f32> {
        (0..w * h).map(|i| scene(i % w, i / w)[usize::from(pattern[(i / w) % 6][(i % w) % 6])]).collect()
    }

    #[test]
    fn the_quick_cube_root_is_accurate() {
        for k in 0..2000 {
            let x = 0.008_856 + k as f32 * 0.002;
            assert!((cbrt(x) - x.cbrt()).abs() < 1e-5 * x.cbrt(), "{x}");
        }
    }

    #[test]
    fn mirroring_keeps_the_place_in_the_pattern() {
        for n in 12..20 {
            for i in -40_isize..(n as isize + 40) {
                let r = mirror6(i, n);
                assert!(r < n, "i {i}, n {n}");
                assert_eq!((r as isize - i).rem_euclid(6), 0, "i {i}, n {n}");
            }
        }
    }

    #[test]
    fn every_phase_of_the_tile_has_hexagons_and_solitary_greens() {
        for oy in 0..6 {
            for ox in 0..6 {
                let l = Layout::of(&shifted(ox, oy)).expect("an X-Trans layout");
                let p = shifted(ox, oy);
                assert_eq!(p[l.sg_row][l.sg_col], 1, "the solitary photosite is green");
                // The hexagon of a red or blue photosite is all green.
                for y in 0..3 {
                    for x in 0..3 {
                        if p[y + 3][x + 3] != 1 {
                            for &(dy, dx) in &l.hex[y][x][..6] {
                                let (yy, xx) = ((y as isize + 3 + dy) as usize, (x as isize + 3 + dx) as usize);
                                assert_eq!(p[yy % 6][xx % 6], 1, "phase {ox},{oy} at {x},{y}");
                            }
                        }
                    }
                }
            }
        }
        let bayerish = [
            [0, 1, 0, 1, 0, 1],
            [1, 2, 1, 2, 1, 2],
            [0, 1, 0, 1, 0, 1],
            [1, 2, 1, 2, 1, 2],
            [0, 1, 0, 1, 0, 1],
            [1, 2, 1, 2, 1, 2],
        ];
        assert!(Layout::of(&bayerish).is_none());
    }

    #[test]
    fn a_flat_colour_comes_out_exact_at_any_phase() {
        for (ox, oy) in [(0, 0), (1, 4), (3, 3), (5, 2)] {
            let p = shifted(ox, oy);
            let (w, h) = (31, 25);
            let m = mosaic_of(w, h, &p, |_, _| [0.2, 0.5, 0.8]);
            let out = demosaic(&m, w, h, &p).unwrap();
            for (i, px) in out.iter().enumerate() {
                for c in 0..3 {
                    assert!((px[c] - [0.2, 0.5, 0.8][c]).abs() < 1e-4, "phase {ox},{oy} pixel {i}: {px:?}");
                }
            }
        }
        assert!(demosaic(&[0.0; 11 * 20], 11, 20, &TILE).is_none(), "too small");
    }

    #[test]
    fn tiles_leave_no_seams() {
        let (w, h) = (100, 70);
        let stripe = |t: usize| if t % 7 < 3 { 0.8 } else { 0.1 };
        let small = |m: &[f32]| demosaic_in_tiles(m, w, h, &TILE, 24, 18).unwrap();
        let whole = |m: &[f32]| demosaic_in_tiles(m, w, h, &TILE, 240, 120).unwrap();
        for m in [mosaic_of(w, h, &TILE, |x, _| [stripe(x); 3]), mosaic_of(w, h, &TILE, |x, y| [stripe(x + 2 * y); 3])]
        {
            let (a, b) = (small(&m), whole(&m));
            for (i, (p, q)) in a.iter().zip(&b).enumerate() {
                for c in 0..3 {
                    assert!((p[c] - q[c]).abs() < 1e-5, "pixel {i}: {p:?} vs {q:?}");
                }
            }
        }
    }

    /// Mean error against the grey scene, and the largest colour error.
    fn errors(out: &[[f32; 3]], truth: impl Fn(usize, usize) -> f32, w: usize) -> (f32, f32) {
        let (mut sum, mut worst) = (0.0_f32, 0.0_f32);
        for (i, px) in out.iter().enumerate() {
            let t = truth(i % w, i / w);
            for v in px {
                sum += (v - t).abs();
                worst = worst.max((v - px[1]).abs());
            }
        }
        (sum / (out.len() * 3) as f32, worst)
    }

    #[test]
    fn fine_grey_detail_is_resolved_better_than_the_simple_demosaic() {
        let (w, h) = (96, 96);
        let truth = |x: usize, y: usize| {
            let (dx, dy) = (x as f32 - 48.0, y as f32 - 48.0);
            0.5 + 0.4 * ((dx * dx + dy * dy) * 0.004).cos()
        };
        let m = mosaic_of(w, h, &TILE, |x, y| [truth(x, y); 3]);
        let (mk_error, mk_colour) = errors(&demosaic(&m, w, h, &TILE).unwrap(), truth, w);
        let (simple_error, simple_colour) = errors(&crate::xtrans::demosaic(&m, w, h, &TILE), truth, w);
        assert!(mk_error < simple_error, "error: Markesteijn {mk_error}, simple {simple_error}");
        assert!(mk_colour < simple_colour, "false colour: Markesteijn {mk_colour}, simple {simple_colour}");
    }

    #[test]
    fn a_sharp_edge_stays_sharp_and_neutral() {
        let (w, h) = (60, 60);
        let truth = |x: usize, y: usize| if 3 * x > 2 * y + 50 { 0.9 } else { 0.1 };
        let m = mosaic_of(w, h, &TILE, |x, y| [truth(x, y); 3]);
        let (mk_error, mk_colour) = errors(&demosaic(&m, w, h, &TILE).unwrap(), truth, w);
        let (simple_error, simple_colour) = errors(&crate::xtrans::demosaic(&m, w, h, &TILE), truth, w);
        assert!(mk_error <= simple_error * 1.05, "error: Markesteijn {mk_error}, simple {simple_error}");
        assert!(mk_colour < simple_colour, "false colour: Markesteijn {mk_colour}, simple {simple_colour}");
    }
}
