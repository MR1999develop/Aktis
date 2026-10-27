//! Edge refinement: from the model's coarse mask to a mask that follows the
//! photo's own edges.
//!
//! Two steps, both driven by the colours of the image:
//!
//! 1. **Uncertain areas by colour.** Where the model is unsure (0.2–0.8) —
//!    the gaps between spokes, strands of hair, the edge band — each pixel is
//!    judged by how much its colour looks like the object's sure part or like
//!    the background around it (colour histograms, as in GrabCut). This is
//!    what empties the inside of a wheel that the model filled.
//! 2. **Edges by colour.** A colour guided filter (He, Sun & Tang, 2013)
//!    snaps the mask to the image's edges. A colour guide separates a red frame
//!    from a yellow wall even when both have the same brightness.
//!
//! The work is done on the image reduced to at most 2048 pixels; the filter's
//! local linear models are then applied with the full-resolution image, so
//! the edge is as sharp as the photo.

use crate::filter::box_mean;
use crate::resample::{fit_within, resize_plane};

/// The image the mask must follow: sRGB-encoded colour, 0–1, one plane per channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    w: usize,
    h: usize,
    rgb: [Vec<f32>; 3],
}

impl Guide {
    /// A guide from `w × h` pixels of sRGB-encoded colour in [0, 1].
    ///
    /// # Panics
    /// When a plane does not hold `w × h` values (a caller bug).
    #[must_use]
    pub fn new(w: usize, h: usize, rgb: [Vec<f32>; 3]) -> Self {
        assert!(rgb.iter().all(|p| p.len() == w * h), "guide planes must hold w × h values");
        Self { w, h, rgb }
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> usize {
        self.w
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> usize {
        self.h
    }

    fn resized(&self, w: usize, h: usize) -> Self {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let r = |c: usize| resize_plane(&self.rgb[c], self.w, self.h, w, h);
        Self { w, h, rgb: [r(0), r(1), r(2)] }
    }
}

/// Long side of the working resolution.
const WORK_LONG_SIDE: u32 = 2048;
/// Above this the model is sure the pixel is selected; below [`SURE_OUT`], sure it is not.
/// Large models are confident even where they are wrong (0.7–0.9 inside a
/// wheel), so only values very close to 1 count as sure.
const SURE_IN: f32 = 0.97;
/// See [`SURE_IN`].
const SURE_OUT: f32 = 0.2;
/// Bins per colour channel of the histograms.
const BINS: usize = 16;
/// Regularisation of the guided filter for colours in [0, 1]: smaller values
/// follow the image's edges more closely.
const EPS: f32 = 1e-4;
/// Contrast applied around 0.5 after filtering: keeps edges crisp without
/// making them jagged.
const CONTRAST: f32 = 1.4;

/// The refined mask, at the guide's size, values in [0, 1].
///
/// # Panics
/// When `coarse` does not hold `guide.width() × guide.height()` values.
#[must_use]
pub fn refine(coarse: &[f32], guide: &Guide) -> Vec<f32> {
    let (w, h) = (guide.w, guide.h);
    assert_eq!(coarse.len(), w * h, "mask and guide must have the same size");
    let (wu, hu) = (u32::try_from(w).unwrap_or(u32::MAX), u32::try_from(h).unwrap_or(u32::MAX));
    let (lw, lh) = fit_within(wu, hu, WORK_LONG_SIDE);
    let (lw, lh) = (lw as usize, lh as usize);
    let low = guide.resized(lw, lh);
    let mut p = resize_plane(coarse, w, h, lw, lh);

    decide_uncertain_by_colour(&mut p, &low);

    // Window radius: about 3 pixels at 768, growing with the working size.
    let r = (lw.max(lh) / 256).max(2);
    let (a, b) = guided_coefficients(&p, &low, r);
    let up = |plane: &[f32]| resize_plane(plane, lw, lh, w, h);
    let (a0, a1, a2, b) = (up(&a[0]), up(&a[1]), up(&a[2]), up(&b));
    (0..w * h)
        .map(|k| {
            let q = a0[k] * guide.rgb[0][k] + a1[k] * guide.rgb[1][k] + a2[k] * guide.rgb[2][k] + b[k];
            ((q - 0.5) * CONTRAST + 0.5).clamp(0.0, 1.0)
        })
        .collect()
}

/// Histogram bin of a colour.
fn bin(c: [f32; 3]) -> usize {
    let q = |v: f32| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..BINS
        let b = (v.clamp(0.0, 1.0) * BINS as f32) as usize;
        b.min(BINS - 1)
    };
    (q(c[0]) * BINS + q(c[1])) * BINS + q(c[2])
}

/// Normalised, slightly smoothed colour histogram of the pixels where `take` holds.
fn histogram(g: &Guide, take: impl Fn(usize) -> bool) -> Option<Vec<f32>> {
    let mut hist = vec![0.0_f32; BINS * BINS * BINS];
    let mut n = 0_usize;
    for k in (0..g.w * g.h).filter(|k| take(*k)) {
        hist[bin([g.rgb[0][k], g.rgb[1][k], g.rgb[2][k]])] += 1.0;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    // [1 2 1] along each colour axis: neighbouring shades count a little.
    for axis in 0..3 {
        let stride = [BINS * BINS, BINS, 1][axis];
        let src = hist.clone();
        for (i, v) in hist.iter_mut().enumerate() {
            let pos = (i / stride) % BINS;
            let lo = if pos > 0 { src[i - stride] } else { src[i] };
            let hi = if pos + 1 < BINS { src[i + stride] } else { src[i] };
            *v = (lo + 2.0 * src[i] + hi) / 4.0;
        }
    }
    let total: f32 = hist.iter().sum();
    let floor = 1e-3 / hist.len() as f32;
    Some(hist.iter().map(|v| v / total + floor).collect())
}

/// Weighs uncertain values against the likelihood that their colour belongs
/// to the object rather than to the background next to it (Bayes' rule, the
/// model's value as the prior): a colour typical of the background pulls a
/// half-sure pixel out, an ambiguous colour leaves the model's value alone.
fn decide_uncertain_by_colour(p: &mut [f32], g: &Guide) {
    let (w, h) = (g.w, g.h);
    // Background samples only from around the object and its unsure band, where they matter.
    let maybe_in: Vec<f32> = p.iter().map(|v| if *v >= SURE_OUT { 1.0 } else { 0.0 }).collect();
    let reach = (w.max(h) / 40).max(4);
    let near = box_mean(&maybe_in, w, h, reach);
    let Some(fg) = histogram(g, |k| p[k] > SURE_IN) else { return };
    let Some(bg) = histogram(g, |k| p[k] < SURE_OUT && near[k] > 0.0) else { return };
    for (k, v) in p.iter_mut().enumerate() {
        if *v >= SURE_OUT && *v <= SURE_IN {
            let i = bin([g.rgb[0][k], g.rgb[1][k], g.rgb[2][k]]);
            let l = fg[i] / (fg[i] + bg[i]);
            *v = *v * l / (*v * l + (1.0 - *v) * (1.0 - l));
        }
    }
}

/// The colour guided filter's local models, box-averaged: `a` per channel and `b`.
fn guided_coefficients(p: &[f32], g: &Guide, r: usize) -> ([Vec<f32>; 3], Vec<f32>) {
    let (w, h) = (g.w, g.h);
    let n = w * h;
    let mean = |v: &[f32]| box_mean(v, w, h, r);
    let product = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a * b).collect::<Vec<f32>>();
    let m: [Vec<f32>; 3] = [mean(&g.rgb[0]), mean(&g.rgb[1]), mean(&g.rgb[2])];
    let mp = mean(p);
    let cov_ip: [Vec<f32>; 3] = std::array::from_fn(|c| mean(&product(&g.rgb[c], p)));
    // Upper triangle of the colour covariance: rr, rg, rb, gg, gb, bb.
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let corr: Vec<Vec<f32>> = pairs.iter().map(|(i, j)| mean(&product(&g.rgb[*i], &g.rgb[*j]))).collect();

    let mut a: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0; n]);
    let mut b = vec![0.0; n];
    for k in 0..n {
        let s = |idx: usize, i: usize, j: usize| corr[idx][k] - m[i][k] * m[j][k];
        let (rr, rg, rb, gg, gb, bb) =
            (s(0, 0, 0) + EPS, s(1, 0, 1), s(2, 0, 2), s(3, 1, 1) + EPS, s(4, 1, 2), s(5, 2, 2) + EPS);
        let cov = [cov_ip[0][k] - m[0][k] * mp[k], cov_ip[1][k] - m[1][k] * mp[k], cov_ip[2][k] - m[2][k] * mp[k]];
        // Inverse of the symmetric 3×3 matrix by its adjugate.
        let (i00, i01, i02) = (gg * bb - gb * gb, rb * gb - rg * bb, rg * gb - rb * gg);
        let (i11, i12, i22) = (rr * bb - rb * rb, rg * rb - rr * gb, rr * gg - rg * rg);
        let det = rr * i00 + rg * i01 + rb * i02;
        if det.abs() < f32::MIN_POSITIVE {
            b[k] = mp[k];
            continue;
        }
        let ak = [
            (i00 * cov[0] + i01 * cov[1] + i02 * cov[2]) / det,
            (i01 * cov[0] + i11 * cov[1] + i12 * cov[2]) / det,
            (i02 * cov[0] + i12 * cov[1] + i22 * cov[2]) / det,
        ];
        b[k] = mp[k] - ak[0] * m[0][k] - ak[1] * m[1][k] - ak[2] * m[2][k];
        for c in 0..3 {
            a[c][k] = ak[c];
        }
    }
    (std::array::from_fn(|c| mean(&a[c])), mean(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w × h` guide whose colour is `f(x, y)`.
    fn guide(w: usize, h: usize, f: impl Fn(usize, usize) -> [f32; 3]) -> Guide {
        let px: Vec<[f32; 3]> = (0..w * h).map(|k| f(k % w, k / w)).collect();
        Guide::new(w, h, std::array::from_fn(|c| px.iter().map(|p| p[c]).collect()))
    }

    #[test]
    fn a_blurry_edge_snaps_to_a_colour_edge_of_equal_brightness() {
        // Red on the left, green of the same brightness on the right; the
        // model's edge is a wide ramp around x = 40.
        let (w, h) = (80, 12);
        let g = guide(w, h, |x, _| if x < 40 { [0.8, 0.2, 0.2] } else { [0.35, 0.55, 0.2] });
        let coarse: Vec<f32> = (0..w * h).map(|k| (1.0 - ((k % w) as f32 - 30.0) / 20.0).clamp(0.0, 1.0)).collect();
        let fine = refine(&coarse, &g);
        let row = &fine[6 * w..7 * w];
        assert!(row[36] > 0.9 && row[43] < 0.1, "edge at x = 40: {:?}", &row[34..46]);
    }

    #[test]
    fn a_hole_the_model_half_filled_is_emptied_by_its_colour() {
        // A ring of red around a yellow hole, on yellow: the model is sure of
        // the ring and unsure (0.5) of the hole.
        let (w, h) = (90, 90);
        let ring = |x: usize, y: usize| {
            let d = ((x as f32 - 45.0).powi(2) + (y as f32 - 45.0).powi(2)).sqrt();
            (20.0..30.0).contains(&d)
        };
        let inside = |x: usize, y: usize| ((x as f32 - 45.0).powi(2) + (y as f32 - 45.0).powi(2)).sqrt() < 20.0;
        let g = guide(w, h, |x, y| if ring(x, y) { [0.8, 0.1, 0.1] } else { [0.8, 0.75, 0.2] });
        let coarse: Vec<f32> = (0..w * h)
            .map(|k| {
                let (x, y) = (k % w, k / w);
                if ring(x, y) {
                    0.99
                } else if inside(x, y) {
                    0.5
                } else {
                    0.0
                }
            })
            .collect();
        let fine = refine(&coarse, &g);
        assert!(fine[45 * w + 45] < 0.1, "hole emptied: {}", fine[45 * w + 45]);
        assert!(fine[45 * w + 20] > 0.9, "ring kept: {}", fine[45 * w + 20]);
    }

    #[test]
    fn a_flat_image_keeps_the_mask_and_the_output_stays_in_range() {
        let g = guide(30, 20, |_, _| [0.5, 0.5, 0.5]);
        let coarse: Vec<f32> = (0..600).map(|k| if k % 30 < 15 { 1.0 } else { 0.0 }).collect();
        let fine = refine(&coarse, &g);
        assert!(fine.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(fine[10 * 30 + 2] > 0.95 && fine[10 * 30 + 27] < 0.05);
    }
}
