//! Fast 8-bit sRGB output for the screen and for JPEG export.
//!
//! The transfer curve is not evaluated per value: a table indexed by the
//! linear value gives the code, and the exact decision thresholds between
//! codes correct the cells that straddle one. The result is identical to
//! [`crate::encode_reference`] for every input. Rows are shared among the
//! processor's cores.

use std::sync::OnceLock;
use std::thread;

use core_model::{ImageF32, Orientation, WorkingSpace};

/// Cells of the table over linear [0, 1). Narrower than the gap between two
/// codes anywhere on the curve, so a cell holds at most one threshold.
const CELLS: usize = 1 << 16;

/// Rows below which splitting an image among threads costs more than it saves.
const ROWS_PER_THREAD: usize = 32;

/// Table-driven, exact sRGB encoder.
struct Encoder {
    /// Code at the start of each cell.
    cells: Vec<u8>,
    /// `thresholds[k]`: the smallest linear value whose code is at least `k` (k ≥ 1).
    thresholds: [f32; 256],
}

impl Encoder {
    fn new() -> Self {
        let mut thresholds = [0.0_f32; 256];
        for (k, t) in thresholds.iter_mut().enumerate().skip(1) {
            // Bisection on the bit patterns of positive floats, which sort like the values.
            let (mut lo, mut hi) = (0_u32, 1.0_f32.to_bits());
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if usize::from(crate::encode_reference(f32::from_bits(mid))) >= k {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            *t = f32::from_bits(hi);
        }
        #[allow(clippy::cast_precision_loss)] // cell indices are below 2^24
        let cells = (0..CELLS).map(|i| crate::encode_reference(i as f32 / CELLS as f32)).collect();
        Self { cells, thresholds }
    }

    /// The 8-bit sRGB code of a linear value (clipped to [0, 1]).
    fn encode(&self, v: f32) -> u8 {
        if v.is_nan() || v <= 0.0 {
            return 0;
        }
        if v >= 1.0 {
            return 255;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let cell = ((v * CELLS as f32) as usize).min(CELLS - 1);
        let mut code = self.cells[cell];
        while code > 0 && v < self.thresholds[usize::from(code)] {
            code -= 1;
        }
        while code < 255 && v >= self.thresholds[usize::from(code) + 1] {
            code += 1;
        }
        code
    }
}

fn encoder() -> &'static Encoder {
    static ENCODER: OnceLock<Encoder> = OnceLock::new();
    ENCODER.get_or_init(Encoder::new)
}

/// Runs `row(y, bytes)` for each of `rows` rows of `row_bytes` bytes of
/// `out` (the last may be shorter), on several threads when each gets at
/// least `min_per_thread` rows.
fn for_each_row(
    out: &mut [u8],
    rows: usize,
    row_bytes: usize,
    min_per_thread: usize,
    row: &(dyn Fn(usize, &mut [u8]) + Sync),
) {
    if rows == 0 || row_bytes == 0 {
        return;
    }
    let cores = thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let threads = cores.min(rows.div_ceil(min_per_thread.max(1))).max(1);
    let rows_per_chunk = rows.div_ceil(threads);
    if threads == 1 {
        for (y, bytes) in out.chunks_mut(row_bytes).enumerate() {
            row(y, bytes);
        }
        return;
    }
    thread::scope(|scope| {
        for (chunk, part) in out.chunks_mut(rows_per_chunk * row_bytes).enumerate() {
            scope.spawn(move || {
                for (i, bytes) in part.chunks_mut(row_bytes).enumerate() {
                    row(chunk * rows_per_chunk + i, bytes);
                }
            });
        }
    });
}

/// 8-bit sRGB pixels of `image`, row by row.
pub fn to_srgb8(image: &ImageF32) -> Vec<u8> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let m = core_color::rgb_to_rgb(image.space(), WorkingSpace::LinearSrgb);
    let enc = encoder();
    let src = image.pixels();
    let mut out = vec![0_u8; w * h * 3];
    for_each_row(&mut out, h, w * 3, ROWS_PER_THREAD, &|y, bytes| {
        for (p, o) in src[y * w..(y + 1) * w].iter().zip(bytes.chunks_exact_mut(3)) {
            let rgb = crate::tone::output_rgb([p[0], p[1], p[2]], &m);
            o[0] = enc.encode(rgb[0]);
            o[1] = enc.encode(rgb[1]);
            o[2] = enc.encode(rgb[2]);
        }
    });
    out
}

/// Output rows and columns handled together when turning an image: the
/// source pixels they read then stay in the processor's cache.
const TILE: usize = 64;

/// 8-bit sRGB pixels of `image` turned upright by `o`: width, height, pixels.
pub fn to_srgb8_oriented(image: &ImageF32, o: Orientation) -> (u32, u32, Vec<u8>) {
    if o == Orientation::Normal {
        return (image.width(), image.height(), to_srgb8(image));
    }
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (ow, oh) = if o.swaps_axes() { (h, w) } else { (w, h) };
    let m = core_color::rgb_to_rgb(image.space(), WorkingSpace::LinearSrgb);
    let enc = encoder();
    let src = image.pixels();
    let mut out = vec![0_u8; ow * oh * 3];
    // One "row" here is a band of TILE output rows, walked tile by tile.
    let band_bytes = TILE * ow * 3;
    let bands = oh.div_ceil(TILE);
    for_each_row(&mut out, bands, band_bytes, 1, &|band, bytes| {
        let y0 = band * TILE;
        let rows = bytes.len() / (ow * 3);
        for x0 in (0..ow).step_by(TILE) {
            let x1 = (x0 + TILE).min(ow);
            for dy in 0..rows {
                let y = y0 + dy;
                let line = &mut bytes[dy * ow * 3..(dy + 1) * ow * 3];
                for x in x0..x1 {
                    let (sx, sy) = o.source(x, y, w, h);
                    let p = src[sy * w + sx];
                    let rgb = crate::tone::output_rgb([p[0], p[1], p[2]], &m);
                    let o3 = &mut line[x * 3..x * 3 + 3];
                    o3[0] = enc.encode(rgb[0]);
                    o3[1] = enc.encode(rgb[1]);
                    o3[2] = enc.encode(rgb[2]);
                }
            }
        }
    });
    #[allow(clippy::cast_possible_truncation)] // the sides of an existing image
    let size = (ow as u32, oh as u32);
    (size.0, size.1, out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::encode_reference;

    #[test]
    fn the_table_gives_exactly_the_reference_codes() {
        let enc = encoder();
        // Every cell start and middle, a stride through all floats of [0, 1],
        // each threshold and the float just below it, and special values.
        let mut values: Vec<f32> = Vec::new();
        for i in 0..CELLS {
            #[allow(clippy::cast_precision_loss)]
            let v = i as f32 / CELLS as f32;
            values.extend([v, v + 0.5 / CELLS as f32]);
        }
        values.extend((0..1.0_f32.to_bits()).step_by(9973).map(f32::from_bits));
        for t in &enc.thresholds[1..] {
            values.extend([*t, f32::from_bits(t.to_bits() - 1)]);
        }
        values.extend([-1.0, -0.0, 0.0, 1.0, 1.5, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 1e-30]);
        for v in values {
            assert_eq!(enc.encode(v), encode_reference(v), "{v:e}");
        }
    }

    fn sample(w: u32, h: u32, space: WorkingSpace) -> ImageF32 {
        let px = (0..w * h)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let f = i as f32;
                [(f * 0.37).sin().abs(), (f * 0.11).cos().abs() * 1.2, f / (w * h) as f32 - 0.1, 1.0]
            })
            .collect();
        ImageF32::from_pixels(w, h, space, px).unwrap()
    }

    fn slow(image: &ImageF32) -> Vec<u8> {
        let m = core_color::rgb_to_rgb(image.space(), WorkingSpace::LinearSrgb);
        image
            .pixels()
            .iter()
            .flat_map(|p| crate::tone::output_rgb([p[0], p[1], p[2]], &m).map(encode_reference))
            .collect()
    }

    #[test]
    fn large_images_split_among_threads_match_the_direct_conversion() {
        for space in [WorkingSpace::LinearSrgb, WorkingSpace::LinearRec2020] {
            let img = sample(97, 301, space);
            assert_eq!(to_srgb8(&img), slow(&img));
        }
    }

    #[test]
    fn orienting_while_converting_matches_orienting_first() {
        let img = sample(37, 129, WorkingSpace::LinearRec2020);
        for o in [
            Orientation::Normal,
            Orientation::FlipHorizontal,
            Orientation::Rotate180,
            Orientation::FlipVertical,
            Orientation::Transpose,
            Orientation::Rotate90,
            Orientation::Transverse,
            Orientation::Rotate270,
        ] {
            let upright = img.oriented(o).unwrap();
            let (w, h, px) = to_srgb8_oriented(&img, o);
            assert_eq!((w, h), (upright.width(), upright.height()), "{o:?}");
            assert_eq!(px, slow(&upright), "{o:?}");
        }
    }

    #[test]
    fn empty_rows_are_harmless() {
        let mut out: Vec<u8> = Vec::new();
        for_each_row(&mut out, 0, 0, 1, &|_, _| {});
        assert!(out.is_empty());
    }
}
