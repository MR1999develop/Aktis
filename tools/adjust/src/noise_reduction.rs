//! Noise reduction, as in Capture One: luminance grain, colour blotches and
//! single hot pixels.
//!
//! The pixels are first given a square-root encoding, which makes the
//! photon noise of a sensor about equally strong in shadows and highlights,
//! then split into a luminance and two colour-difference planes. The noise
//! of each plane is measured on the image itself (the median of the finest
//! diagonal differences), so the same settings fit a photo at ISO 100 and
//! one at ISO 6400, and a reduced preview as well as the full photo.
//!
//! Each plane is split into detail levels with the "à trous" wavelet; in
//! each level the coefficients smaller than the noise expected there are
//! taken away and the larger ones shrunk less and less (non-negative
//! garrote), which keeps edges and texture that stand above the noise.
//!
//! * **Luminance**: how strongly the grain is removed (50 removes it at
//!   about 1.5 times the measured noise).
//! * **Details**: below 50 smooths more, above 50 gives back part of the
//!   finest detail removed.
//! * **Colour**: the same for the colour planes, over coarser levels, as
//!   colour noise forms blotches.
//! * **Single pixel**: pixels much brighter than all their neighbours (hot
//!   pixels of long exposures) take the median of the neighbours; it works
//!   on the full-size photo only, where such pixels exist.
//!
//! It needs the neighbourhood of each pixel, so it runs on the CPU.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{LUMA, float};
use crate::filters::{LEVEL_NOISE, noise_sigma, reflect, wavelet};

/// Id of the luminance noise reduction (0 to 100).
pub const LUMINANCE: &str = "luminance";
/// Id of the detail kept (0 to 100, 50 neutral).
pub const DETAILS: &str = "details";
/// Id of the colour noise reduction (0 to 100).
pub const COLOR: &str = "color";
/// Id of the single-pixel (hot pixel) removal (0 to 100).
pub const SINGLE_PIXEL: &str = "single_pixel";

/// Threshold at Luminance 50, in units of the noise expected in a level.
const LUMA_K: f32 = 1.5;
/// Threshold at Colour 50.
const COLOR_K: f32 = 2.5;
/// Detail levels for luminance and for colour.
const LUMA_LEVELS: usize = 4;
const COLOR_LEVELS: usize = 5;

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("noise_reduction").expect("constant id is valid"),
    version: 1,
    name: "Noise Reduction",
    category: Category::Details,
    order: 30,
    params: vec![
        float(LUMINANCE, "Luminance", [0.0, 100.0], 50.0, 1.0, Unit::None),
        float(DETAILS, "Details", [0.0, 100.0], 50.0, 1.0, Unit::None),
        float(COLOR, "Color", [0.0, 100.0], 50.0, 1.0, Unit::None),
        float(SINGLE_PIXEL, "Single pixel", [0.0, 100.0], 0.0, 1.0, Unit::None),
    ],
});

/// The noise reduction tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoiseReduction;

impl Tool for NoiseReduction {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let luminance = params.float(LUMINANCE) / 50.0;
        let details = params.float(DETAILS);
        let color = params.float(COLOR) / 50.0;
        let single = params.float(SINGLE_PIXEL) / 100.0;
        let hot_pixels = single > 0.0 && image.pixel_size() < 1.5;
        if luminance <= 0.0 && color <= 0.0 && !hot_pixels {
            return;
        }
        let (w, h) = (image.width() as usize, image.height() as usize);
        let mut planes: [Vec<f32>; 3] = std::array::from_fn(|c| image.pixels().iter().map(|p| encode(p[c])).collect());
        if hot_pixels {
            for plane in &mut planes {
                remove_hot_pixels(plane, w, h, single);
            }
        }
        // Luminance and colour differences of the encoded values.
        let mut y: Vec<f32> =
            (0..w * h).map(|i| LUMA[0] * planes[0][i] + LUMA[1] * planes[1][i] + LUMA[2] * planes[2][i]).collect();
        let mut a: Vec<f32> = planes[0].iter().zip(&y).map(|(r, y)| r - y).collect();
        let mut b: Vec<f32> = planes[2].iter().zip(&y).map(|(b, y)| b - y).collect();

        if luminance > 0.0 {
            // Details below 50 smooth more; above 50 give back fine detail.
            let smoother = 1.0 + ((50.0 - details) / 50.0).max(0.0);
            let give_back = ((details - 50.0) / 50.0).max(0.0) * 0.6;
            let k = luminance * LUMA_K * smoother;
            y = shrink_noise(&y, w, h, LUMA_LEVELS, k, give_back);
        }
        if color > 0.0 {
            a = shrink_noise(&a, w, h, COLOR_LEVELS, color * COLOR_K, 0.0);
            b = shrink_noise(&b, w, h, COLOR_LEVELS, color * COLOR_K, 0.0);
        }

        let [wr, wg, wb] = LUMA;
        for (i, p) in image.pixels_mut().iter_mut().enumerate() {
            let r = y[i] + a[i];
            let bl = y[i] + b[i];
            let g = (y[i] - wr * r - wb * bl) / wg;
            p[0] = decode(r);
            p[1] = decode(g);
            p[2] = decode(bl);
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::Host
    }
}

/// Square-root encoding, sign kept (values below black stay below).
fn encode(v: f32) -> f32 {
    v.signum() * v.abs().sqrt()
}

/// Inverse of [`encode`].
fn decode(s: f32) -> f32 {
    s * s.abs()
}

/// `plane` with the wavelet coefficients of each of `levels` levels below
/// `k` times the noise expected there removed and the others shrunk by the
/// non-negative garrote (`c − t²/c`); `give_back`
/// returns that share of what was taken from the finest level.
fn shrink_noise(plane: &[f32], w: usize, h: usize, levels: usize, k: f32, give_back: f32) -> Vec<f32> {
    let sigma = noise_sigma(plane, w, h);
    if sigma.is_nan() || sigma <= 0.0 || k <= 0.0 {
        return plane.to_vec();
    }
    let mut kept_detail = vec![0.0_f32; plane.len()];
    let rest = wavelet(plane, w, h, levels, |level, smooth, current| {
        let t = k * sigma * LEVEL_NOISE[level.min(LEVEL_NOISE.len() - 1)] / LEVEL_NOISE[0];
        let back = if level == 0 { give_back } else { 0.0 };
        for (o, (&cur, &s)) in kept_detail.iter_mut().zip(current.iter().zip(smooth)) {
            let c = cur - s;
            // Non-negative garrote: small coefficients go, large ones (edges) nearly keep.
            let kept = if c.abs() > t { c - t * t / c } else { 0.0 };
            *o += kept + (c - kept) * back;
        }
    });
    rest.iter().zip(&kept_detail).map(|(r, d)| r + d).collect()
}

/// Pixels of `plane` far above all eight neighbours take their median.
/// `strength` (0 to 1) lowers how far above they must be: from 8 times
/// the plane's noise down to 2 times.
#[allow(clippy::cast_possible_wrap)] // image sizes
fn remove_hot_pixels(plane: &mut [f32], w: usize, h: usize, strength: f32) {
    if w < 3 || h < 3 {
        return;
    }
    let sigma = noise_sigma(plane, w, h).max(1e-4);
    let margin = sigma * (8.0 - 6.0 * strength.clamp(0.0, 1.0));
    let src = plane.to_vec();
    for y in 0..h {
        for x in 0..w {
            let mut n = [0.0_f32; 8];
            let mut k = 0;
            for dy in -1_isize..=1 {
                for dx in -1_isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    n[k] = src[reflect(y as isize + dy, h) * w + reflect(x as isize + dx, w)];
                    k += 1;
                }
            }
            let highest = n.iter().copied().fold(f32::MIN, f32::max);
            let v = src[y * w + x];
            if v > highest + margin {
                n.sort_unstable_by(f32::total_cmp);
                plane[y * w + x] = 0.5 * (n[3] + n[4]);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    struct Noise(u32);
    impl Noise {
        fn uniform(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 % 1_000_000) as f32 / 1_000_000.0
        }
        fn gauss(&mut self) -> f32 {
            (0..12).map(|_| self.uniform()).sum::<f32>() - 6.0
        }
    }

    fn reduce(image: &mut ImageF32, values: &[(&str, f32)]) {
        let mut p = Params::new();
        for (id, v) in values {
            p.set(crate::common::pid(id), ParamValue::Float(*v));
        }
        NoiseReduction.apply_cpu(image, &ResolvedParams::resolve(NoiseReduction.descriptor(), &p));
    }

    fn stats(values: impl Iterator<Item = f32>) -> (f32, f32) {
        let v: Vec<f32> = values.collect();
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / v.len() as f32;
        (mean, var.sqrt())
    }

    /// Grey 0.18 with grain (the same in R, G, B) and colour noise.
    fn noisy(size: usize, grain: f32, colour: f32) -> ImageF32 {
        let mut n = Noise(2024);
        let px = (0..size * size)
            .map(|_| {
                let g = 0.18 + grain * n.gauss();
                let (dr, db) = (colour * n.gauss(), colour * n.gauss());
                [g + dr, g, g + db, 1.0]
            })
            .collect();
        ImageF32::from_pixels(size as u32, size as u32, WorkingSpace::LinearRec2020, px).expect("square")
    }

    #[test]
    fn grain_is_reduced_and_the_mean_kept() {
        let before = noisy(128, 0.02, 0.0);
        let mut after = before.clone();
        reduce(&mut after, &[(LUMINANCE, 50.0), (COLOR, 0.0)]);
        let (m0, s0) = stats(before.pixels().iter().map(|p| p[1]));
        let (m1, s1) = stats(after.pixels().iter().map(|p| p[1]));
        assert!(s1 < 0.5 * s0, "{s0} → {s1}");
        assert!((m1 - m0).abs() < 0.003, "{m0} → {m1}");
        let mut stronger = before.clone();
        reduce(&mut stronger, &[(LUMINANCE, 100.0), (DETAILS, 20.0), (COLOR, 0.0)]);
        assert!(stats(stronger.pixels().iter().map(|p| p[1])).1 < s1);
    }

    #[test]
    fn colour_noise_is_reduced_without_touching_grain_when_luminance_is_off() {
        let before = noisy(128, 0.0, 0.02);
        let mut after = before.clone();
        reduce(&mut after, &[(LUMINANCE, 0.0), (COLOR, 50.0)]);
        let chroma = |img: &ImageF32| stats(img.pixels().iter().map(|p| p[0] - p[1])).1;
        assert!(chroma(&after) < 0.4 * chroma(&before), "{} → {}", chroma(&before), chroma(&after));
    }

    #[test]
    fn an_edge_above_the_noise_survives() {
        let mut n = Noise(7);
        let size = 96;
        let px = (0..size * size)
            .map(|i| {
                let v = if i % size < size / 2 { 0.05 } else { 0.5 } + 0.01 * n.gauss();
                [v, v, v, 1.0]
            })
            .collect();
        let mut img = ImageF32::from_pixels(size as u32, size as u32, WorkingSpace::LinearRec2020, px).expect("square");
        reduce(&mut img, &[(LUMINANCE, 100.0)]);
        let at = |x: usize| (0..size).map(|y| img.pixels()[y * size + x][1]).sum::<f32>() / size as f32;
        assert!(at(size / 2 - 2) < 0.08 && at(size / 2 + 1) > 0.45, "{} {}", at(size / 2 - 2), at(size / 2 + 1));
    }

    #[test]
    fn a_hot_pixel_goes_only_when_asked_and_only_at_full_size() {
        let mut base = noisy(32, 0.002, 0.0);
        base.pixels_mut()[16 * 32 + 16] = [5.0, 5.0, 5.0, 1.0];
        let mut kept = base.clone();
        reduce(&mut kept, &[(LUMINANCE, 0.0), (COLOR, 0.0), (SINGLE_PIXEL, 0.0)]);
        assert_eq!(kept, base);
        let mut gone = base.clone();
        reduce(&mut gone, &[(LUMINANCE, 0.0), (COLOR, 0.0), (SINGLE_PIXEL, 50.0)]);
        assert!(gone.pixels()[16 * 32 + 16][0] < 0.3);
        let mut preview = base.clone().with_pixel_size(3.0);
        reduce(&mut preview, &[(LUMINANCE, 0.0), (COLOR, 0.0), (SINGLE_PIXEL, 50.0)]);
        assert_eq!(preview, base.with_pixel_size(3.0));
    }

    #[test]
    fn without_noise_little_changes_and_values_below_black_survive() {
        let px: Vec<[f32; 4]> = (0..64 * 64)
            .map(|i| {
                let f = (i % 64) as f32 / 63.0;
                [f * 0.8 - 0.01, f * 0.5, f * 0.3, 1.0]
            })
            .collect();
        let before = ImageF32::from_pixels(64, 64, WorkingSpace::LinearRec2020, px).expect("64×64");
        let mut after = before.clone();
        reduce(&mut after, &[]);
        for (a, b) in after.pixels().iter().zip(before.pixels()) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 2e-3, "{a:?} {b:?}");
            }
        }
    }
}
