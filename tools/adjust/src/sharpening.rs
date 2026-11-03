//! Sharpening, as in Capture One: an unsharp mask on luminance.
//!
//! The luminance, encoded with a 2.2 gamma (so shadows and highlights get
//! halos of similar visibility), is compared with a Gaussian blur of
//! `radius` pixels; the difference, the edge detail, is added back
//! `amount` / 25 times (up to 4 times at 100). Differences smaller than `threshold` (in 8-bit
//! levels of the encoded luminance) are left alone, which keeps noise
//! from being sharpened. Halo suppression limits how far a sharpened pixel
//! may go above the brightest or below the darkest of its neighbours,
//! which is what makes bright and dark lines along edges. R, G and B are
//! scaled together, so colours keep their hue.
//!
//! Radius is in pixels of the full-size photo: on a reduced preview it is
//! made smaller in proportion ([`core_model::ImageF32::pixel_size`]), so the
//! effect is judged at 100 %, as in Capture One.
//!
//! It needs the neighbourhood of each pixel, so it runs on the CPU.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{float, luma, smoothstep};
use crate::filters::{gaussian, reflect};

/// Id of the amount (0 to 100; 25 adds the edge detail once, 100 four times).
pub const AMOUNT: &str = "amount";
/// Id of the radius (pixels of the full-size photo).
pub const RADIUS: &str = "radius";
/// Id of the threshold (8-bit levels).
pub const THRESHOLD: &str = "threshold";
/// Id of the halo suppression (0 to 100).
pub const HALO: &str = "halo_suppression";

/// Amount that adds the edge detail once.
const AMOUNT_PER_DETAIL: f32 = 25.0;

/// Gamma of the luminance the detail is taken from.
const GAMMA: f32 = 2.2;

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("sharpening").expect("constant id is valid"),
    version: 1,
    name: "Sharpening",
    category: Category::Details,
    order: 140,
    // Capture One's usual starting values.
    params: vec![
        float(AMOUNT, "Amount", [0.0, 100.0], 45.0, 1.0, Unit::None),
        float(RADIUS, "Radius", [0.2, 2.5], 0.8, 0.1, Unit::Pixels),
        float(THRESHOLD, "Threshold", [0.0, 10.0], 1.0, 0.1, Unit::None),
        float(HALO, "Halo suppression", [0.0, 100.0], 0.0, 1.0, Unit::None),
    ],
});

/// The sharpening tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Sharpening;

impl Tool for Sharpening {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let amount = params.float(AMOUNT) / AMOUNT_PER_DETAIL;
        let sigma = params.float(RADIUS) / image.pixel_size();
        if amount <= 0.0 || sigma.is_nan() || sigma < 0.2 {
            return;
        }
        let threshold = params.float(THRESHOLD) / 255.0;
        let keep_halo = 1.0 - params.float(HALO) / 100.0;
        let (w, h) = (image.width() as usize, image.height() as usize);
        let encoded: Vec<f32> =
            image.pixels().iter().map(|p| luma([p[0], p[1], p[2]]).max(0.0).powf(1.0 / GAMMA)).collect();
        let blurred = gaussian(&encoded, w, h, sigma);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a small positive radius
        let reach = sigma.round().max(1.0) as usize;
        let bounds = (keep_halo < 1.0).then(|| local_range(&encoded, w, h, reach));
        for (i, p) in image.pixels_mut().iter_mut().enumerate() {
            let e = encoded[i];
            let y = luma([p[0], p[1], p[2]]);
            if y.is_nan() || y <= 1e-7 {
                continue;
            }
            let detail = e - blurred[i];
            let weight = if threshold > 0.0 { smoothstep(0.5 * threshold, 1.5 * threshold, detail.abs()) } else { 1.0 };
            let mut sharp = e + amount * weight * detail;
            if let Some((lo, hi)) = &bounds {
                if sharp > hi[i] {
                    sharp = hi[i] + (sharp - hi[i]) * keep_halo;
                } else if sharp < lo[i] {
                    sharp = lo[i] - (lo[i] - sharp) * keep_halo;
                }
            }
            let k = sharp.max(0.0).powf(GAMMA) / y;
            for v in &mut p[..3] {
                *v *= k;
            }
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::Host
    }
}

/// The smallest and largest value within `reach` pixels (a square) of each pixel.
#[allow(clippy::cast_possible_wrap)] // image sizes
fn local_range(plane: &[f32], w: usize, h: usize, reach: usize) -> (Vec<f32>, Vec<f32>) {
    let r = reach as isize;
    let pass = |src: &[f32], horizontal: bool, pick: fn(f32, f32) -> f32| -> Vec<f32> {
        let mut out = vec![0.0_f32; src.len()];
        crate::filters::parallel_rows(&mut out, w, |first, rows| {
            for (row, line) in rows.chunks_mut(w).enumerate() {
                let y = first + row;
                for (x, o) in line.iter_mut().enumerate() {
                    let mut acc = src[y * w + x];
                    for d in -r..=r {
                        let v = if horizontal {
                            src[y * w + reflect(x as isize + d, w)]
                        } else {
                            src[reflect(y as isize + d, h) * w + x]
                        };
                        acc = pick(acc, v);
                    }
                    *o = acc;
                }
            }
        });
        out
    };
    let lo = pass(&pass(plane, true, f32::min), false, f32::min);
    let hi = pass(&pass(plane, true, f32::max), false, f32::max);
    (lo, hi)
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    /// A soft vertical edge from 0.1 to 0.4, coloured or grey.
    fn edge(colour: [f32; 3]) -> ImageF32 {
        let px = (0..64 * 16)
            .map(|i| {
                let x = (i % 64) as f32;
                let t = ((x - 31.5) / 2.0).clamp(-1.0, 1.0) * 0.5 + 0.5;
                let v = 0.1 + 0.3 * t;
                [v * colour[0], v * colour[1], v * colour[2], 1.0]
            })
            .collect();
        ImageF32::from_pixels(64, 16, WorkingSpace::LinearRec2020, px).expect("64×16")
    }

    fn sharpen(image: &mut ImageF32, values: &[(&str, f32)]) {
        let mut p = Params::new();
        for (id, v) in values {
            p.set(crate::common::pid(id), ParamValue::Float(*v));
        }
        Sharpening.apply_cpu(image, &ResolvedParams::resolve(Sharpening.descriptor(), &p));
    }

    fn row(image: &ImageF32) -> Vec<f32> {
        image.pixels()[8 * 64..9 * 64].iter().map(|p| luma([p[0], p[1], p[2]])).collect()
    }

    #[test]
    fn an_edge_gets_steeper_with_overshoot_on_both_sides() {
        let before = edge([1.0; 3]);
        let mut after = before.clone();
        sharpen(&mut after, &[(AMOUNT, 75.0), (RADIUS, 1.5), (THRESHOLD, 0.0)]);
        let (b, a) = (row(&before), row(&after));
        assert!(a[30] < b[30] && a[33] > b[33], "{} {} / {} {}", b[30], a[30], b[33], a[33]);
        assert!(a.iter().copied().fold(f32::MIN, f32::max) > 0.4, "no bright overshoot");
        assert!(a.iter().copied().fold(f32::MAX, f32::min) < 0.1, "no dark overshoot");
        // Flat areas far from the edge do not change.
        assert!((a[3] - b[3]).abs() < 1e-5 && (a[60] - b[60]).abs() < 1e-5);
    }

    #[test]
    fn halo_suppression_keeps_the_edge_within_its_neighbours() {
        let mut full = edge([1.0; 3]);
        sharpen(&mut full, &[(AMOUNT, 100.0), (RADIUS, 1.5), (THRESHOLD, 0.0), (HALO, 100.0)]);
        let a = row(&full);
        assert!(a.iter().all(|v| (0.1 - 1e-4..=0.4 + 1e-4).contains(v)), "{a:?}");
    }

    #[test]
    fn colours_keep_their_hue_and_a_tiny_radius_on_a_small_copy_does_nothing() {
        let mut img = edge([1.0, 0.5, 0.25]);
        sharpen(&mut img, &[(AMOUNT, 75.0), (THRESHOLD, 0.0)]);
        for p in img.pixels() {
            assert!((p[1] / p[0] - 0.5).abs() < 1e-4 && (p[2] / p[0] - 0.25).abs() < 1e-4);
        }
        // A preview at a fifth of the photo: 0.8 px become 0.16 px, nothing to sharpen.
        let before = edge([1.0; 3]).with_pixel_size(5.0);
        let mut after = before.clone();
        sharpen(&mut after, &[(AMOUNT, 75.0)]);
        assert_eq!(after, before);
    }

    #[test]
    fn the_threshold_leaves_small_ripples_alone() {
        let px = (0..32 * 8)
            .map(|i| {
                let v = 0.2 + if i % 2 == 0 { 0.001 } else { -0.001 };
                [v, v, v, 1.0]
            })
            .collect();
        let before = ImageF32::from_pixels(32, 8, WorkingSpace::LinearRec2020, px).expect("32×8");
        let mut after = before.clone();
        sharpen(&mut after, &[(AMOUNT, 100.0), (THRESHOLD, 3.0)]);
        for (a, b) in after.pixels().iter().zip(before.pixels()) {
            assert!((a[0] - b[0]).abs() < 1e-4, "{a:?} {b:?}");
        }
    }
}
