//! False-colour suppression: the 3×3 median of red − green and of
//! blue − green, green untouched.
//!
//! Detail finer than the sensor's colour resolution (a colour filter array
//! measures each colour on a fraction of the photosites) leaves isolated
//! dots and short streaks of colour after demosaicing. Colour differences
//! change slowly in real images, so a median over the neighbourhood removes
//! the dots and keeps colour edges, while brightness, carried by green,
//! keeps all its detail.
//!
//! The median slides along each row: the three values of every column are
//! sorted once and shared by the three windows that contain them.

use crate::lens::parallel_rows;

/// Replaces red and blue of `pixels` (`width × height`) so that their
/// differences from green are the 3×3 medians of those around them.
/// Borders are mirrored.
pub(crate) fn suppress_false_colours(pixels: &mut [[f32; 3]], width: usize, height: usize) {
    assert_eq!(pixels.len(), width * height, "image size");
    if width == 0 || height == 0 {
        return;
    }
    let red: Vec<f32> = pixels.iter().map(|p| p[0] - p[1]).collect();
    let blue: Vec<f32> = pixels.iter().map(|p| p[2] - p[1]).collect();
    parallel_rows(pixels, width, |first, rows| {
        let mut columns = vec![[0.0_f32; 3]; width];
        for (k, out) in rows.chunks_exact_mut(width).enumerate() {
            let y = first + k;
            let (above, below) = (y.checked_sub(1).unwrap_or(usize::from(height > 1)), mirrored_next(y, height));
            for (c, d) in [(0, &red), (2, &blue)] {
                sort_columns(row(d, width, above), row(d, width, y), row(d, width, below), &mut columns);
                for (x, px) in out.iter_mut().enumerate() {
                    px[c] = px[1] + median_at(&columns, x);
                }
            }
        }
    });
}

fn row(d: &[f32], width: usize, y: usize) -> &[f32] {
    &d[y * width..(y + 1) * width]
}

/// The row after `y`, mirrored at the bottom ("reflect-101").
fn mirrored_next(y: usize, height: usize) -> usize {
    if y + 1 < height { y + 1 } else { y.saturating_sub(1) }
}

/// Sorts the three values of each column of three rows of equal length.
pub(crate) fn sort_columns(above: &[f32], row: &[f32], below: &[f32], columns: &mut [[f32; 3]]) {
    for (((col, &a), &b), &c) in columns.iter_mut().zip(above).zip(row).zip(below) {
        *col = sort3(a, b, c);
    }
}

/// The median of the 3×3 window centred on column `x`, from the sorted
/// columns (mirrored at the ends): the median of the largest low, the
/// median middle and the smallest high.
pub(crate) fn median_at(columns: &[[f32; 3]], x: usize) -> f32 {
    let last = columns.len() - 1;
    let left = if x == 0 { last.min(1) } else { x - 1 };
    let right = if x == last { x.saturating_sub(1) } else { x + 1 };
    let (l, c, r) = (columns[left], columns[x], columns[right]);
    let low = max(max(l[0], c[0]), r[0]);
    let mid = sort3(l[1], c[1], r[1])[1];
    let high = min(min(l[2], c[2]), r[2]);
    sort3(low, mid, high)[1]
}

fn sort3(a: f32, b: f32, c: f32) -> [f32; 3] {
    let (a, b) = (min(a, b), max(a, b));
    let (b, c) = (min(b, c), max(b, c));
    let (a, b) = (min(a, b), max(a, b));
    [a, b, c]
}

// Plain comparisons compile to single instructions (`f32::min` also handles NaN).
fn min(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

fn max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;

    #[test]
    fn the_sliding_median_is_the_median_of_each_mirrored_window() {
        let (w, h) = (12, 5);
        let mut seed = 12345_u32;
        let d: Vec<f32> = (0..w * h)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 16) as f32 % 7.0 // with repeats
            })
            .collect();
        let reflect = |i: isize, n: usize| crate::reflect(i, n);
        let mut columns = vec![[0.0; 3]; w];
        for y in 0..h {
            let (above, below) = (reflect(y as isize - 1, h), reflect(y as isize + 1, h));
            sort_columns(&d[above * w..][..w], &d[y * w..][..w], &d[below * w..][..w], &mut columns);
            for x in 0..w {
                let mut window: Vec<f32> = (0..9)
                    .map(|k| d[reflect(y as isize + k / 3 - 1, h) * w + reflect(x as isize + k % 3 - 1, w)])
                    .collect();
                window.sort_by(f32::total_cmp);
                assert_eq!(median_at(&columns, x).to_bits(), window[4].to_bits(), "row {y} column {x}");
            }
        }
    }

    #[test]
    fn an_isolated_colour_dot_goes_and_brightness_stays() {
        let (w, h) = (5, 4);
        let mut img = vec![[0.3_f32, 0.3, 0.3]; w * h];
        img[6] = [0.9, 0.5, 0.1];
        suppress_false_colours(&mut img, w, h);
        for (i, px) in img.iter().enumerate() {
            let g = if i == 6 { 0.5 } else { 0.3 };
            for v in px {
                assert!((v - g).abs() < 1e-6, "pixel {i}: {px:?}");
            }
        }
    }

    #[test]
    fn colour_areas_and_their_edges_keep_their_colour() {
        let (w, h) = (8, 6);
        let colour = |x: usize| if x < 4 { [0.6, 0.3, 0.1] } else { [0.1, 0.3, 0.6] };
        let mut img: Vec<[f32; 3]> = (0..w * h).map(|i| colour(i % w)).collect();
        suppress_false_colours(&mut img, w, h);
        for (i, px) in img.iter().enumerate() {
            let want = colour(i % w);
            assert!((0..3).all(|c| (px[c] - want[c]).abs() < 1e-6), "pixel {i}: {px:?}");
        }
    }

    #[test]
    fn tiny_images_do_not_panic() {
        for (w, h) in [(1, 1), (1, 3), (3, 1), (2, 2), (0, 0)] {
            let mut img = vec![[0.2_f32, 0.4, 0.6]; w * h];
            suppress_false_colours(&mut img, w, h);
            assert!(img.iter().all(|p| (p[0] - 0.2).abs() < 1e-6 && (p[2] - 0.6).abs() < 1e-6));
        }
    }
}
