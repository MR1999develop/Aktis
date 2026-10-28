//! Dehaze, as in Capture One: removes (positive) or adds (negative) haze.
//!
//! Haze lifts the darkest channel of every colour towards the colour of
//! the haze. Following the dark-channel prior (He, Sun and Tang), the haze
//! colour is the average of the pixels whose darkest channel is in the top
//! 0.1 %, and the haze in each place is the darkest channel smoothed over
//! 1 % of the image diagonal (a mean rather than a minimum, which avoids
//! halos at edges). Removing it divides out the estimated transmission,
//! never by less than 0.15; adding haze mixes the haze colour in.
//!
//! It needs the whole image, so it runs on the CPU.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId};

use crate::common::{blur, slider};

/// Id of the amount (−100 adds haze, 100 removes the most).
pub const AMOUNT: &str = "amount";

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("dehaze").expect("constant id is valid"),
    version: 1,
    name: "Dehaze",
    category: Category::Details,
    stage: Stage::Graph,
    params: vec![slider(AMOUNT, "Amount")],
});

/// The dehaze tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Dehaze;

/// The haze colour: the mean of the pixels with the brightest darkest channel.
fn airlight(pixels: &[[f32; 4]], dark: &[f32]) -> [f32; 3] {
    let mut sorted: Vec<f32> = dark.to_vec();
    sorted.sort_by(f32::total_cmp);
    let threshold = sorted[sorted.len() - 1 - sorted.len() / 1000];
    let (mut sum, mut n) = ([0.0_f64; 3], 0_u32);
    for (p, d) in pixels.iter().zip(dark) {
        if *d >= threshold {
            for c in 0..3 {
                sum[c] += f64::from(p[c].max(0.0));
            }
            n += 1;
        }
    }
    #[allow(clippy::cast_possible_truncation)] // a colour
    sum.map(|s| (s / f64::from(n.max(1))) as f32)
}

impl Tool for Dehaze {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    #[allow(clippy::cast_precision_loss)] // image sizes
    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let amount = params.float(AMOUNT);
        if amount == 0.0 || image.pixels().is_empty() {
            return;
        }
        let (w, h) = (image.width() as usize, image.height() as usize);
        let dark: Vec<f32> = image.pixels().iter().map(|p| p[0].min(p[1]).min(p[2]).max(0.0)).collect();
        let air = airlight(image.pixels(), &dark);
        if amount < 0.0 {
            let mix = -amount / 100.0 * 0.6;
            for p in image.pixels_mut() {
                for c in 0..3 {
                    p[c] = p[c] * (1.0 - mix) + air[c] * mix;
                }
            }
            return;
        }
        let mut haze = dark;
        blur(&mut haze, w, h, (0.01 * (w as f32).hypot(h as f32)).max(1.0));
        let air_dark = air[0].min(air[1]).min(air[2]).max(1e-4);
        let strength = 0.95 * amount / 100.0;
        for (p, d) in image.pixels_mut().iter_mut().zip(haze) {
            let t = (1.0 - strength * (d / air_dark).min(1.0)).max(0.15);
            for c in 0..3 {
                p[c] = (p[c] - air[c]) / t + air[c];
            }
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::Host
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    /// A hazy scene: a dark subject and bright sky, both lifted towards grey-blue haze.
    fn hazy() -> ImageF32 {
        let haze = [0.6, 0.65, 0.7];
        let px = (0..40 * 40)
            .map(|i| {
                let scene = if i % 40 < 20 { [0.05, 0.04, 0.03] } else { [0.3, 0.2, 0.1] };
                let t = if i / 40 < 4 { 0.0 } else { 0.5 }; // the top rows are pure haze (sky)
                let c: [f32; 3] = std::array::from_fn(|k| scene[k] * t + haze[k] * (1.0 - t));
                [c[0], c[1], c[2], 1.0]
            })
            .collect();
        ImageF32::from_pixels(40, 40, WorkingSpace::LinearRec2020, px).expect("40×40")
    }

    fn run(amount: f32) -> ImageF32 {
        let mut p = Params::new();
        p.set(crate::common::pid(AMOUNT), ParamValue::Float(amount));
        let r = ResolvedParams::resolve(Dehaze.descriptor(), &p);
        let mut img = hazy();
        Dehaze.apply_cpu(&mut img, &r);
        img
    }

    #[test]
    fn the_haze_colour_is_found_and_removing_haze_adds_contrast() {
        let img = hazy();
        let dark: Vec<f32> = img.pixels().iter().map(|p| p[0].min(p[1]).min(p[2])).collect();
        let air = airlight(img.pixels(), &dark);
        assert!((air[2] - 0.7).abs() < 1e-5, "{air:?}");
        let before = hazy();
        let after = run(100.0);
        let spread = |img: &ImageF32| img.pixels()[30 * 40 + 30][0] - img.pixels()[30 * 40 + 5][0];
        assert!(spread(&after) > spread(&before) * 1.2, "{} {}", spread(&after), spread(&before));
    }

    #[test]
    fn negative_amounts_add_haze() {
        let after = run(-100.0);
        let p = after.pixels()[30 * 40 + 5];
        assert!(p[0] > hazy().pixels()[30 * 40 + 5][0]);
    }
}
