//! Clarity and structure, as in Capture One: local contrast.
//!
//! Each pixel's luminance (in stops) is compared with a blurred copy; the
//! difference, the local detail, is made stronger (positive values) or
//! weaker (negative). Clarity uses a wide blur (1.5 % of the image
//! diagonal) and acts on broad shapes; structure a narrow one (0.3 %) and
//! acts on texture. Both are faded out in deep shadows and bright
//! highlights, which keeps halos and noise down, and scale R, G and B
//! together, so hues keep. The blur sizes follow the image size, so a
//! preview and the full image look alike.
//!
//! It needs the neighbourhood of each pixel, so it runs on the CPU.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Tool, ToolDescriptor, ToolId};

use crate::common::{GREY, blur, choice, luma, slider};

/// Id of the clarity (−100 to 100).
pub const CLARITY: &str = "clarity";
/// Id of the structure (−100 to 100).
pub const STRUCTURE: &str = "structure";
/// Id of the method (an index into [`METHODS`]).
pub const METHOD: &str = "method";

/// The methods, as Capture One names them: Natural changes luminance only
/// and spares the darkest and brightest tones; Punch is stronger and adds
/// saturation where it adds contrast; Neutral is gentler and keeps colours
/// exactly; Classic works on all tones alike, as older versions did.
pub const METHODS: [&str; 4] = ["Natural", "Punch", "Neutral", "Classic"];

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("clarity").expect("constant id is valid"),
    version: 2,
    name: "Clarity",
    category: Category::Details,
    order: 130,
    params: vec![slider(CLARITY, "Clarity"), slider(STRUCTURE, "Structure"), choice(METHOD, "Method", &METHODS, 0)],
});

/// The clarity tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Clarity;

impl Tool for Clarity {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    #[allow(clippy::cast_precision_loss)] // image sizes
    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let (clarity, structure) = (params.float(CLARITY), params.float(STRUCTURE));
        if clarity == 0.0 && structure == 0.0 {
            return;
        }
        let (w, h) = (image.width() as usize, image.height() as usize);
        let diagonal = (w as f32).hypot(h as f32);
        let log: Vec<f32> = image.pixels().iter().map(|p| luma([p[0], p[1], p[2]]).max(1e-5).log2()).collect();
        let wide = blurred(&log, w, h, (0.015 * diagonal).max(2.0), clarity);
        let narrow = blurred(&log, w, h, (0.003 * diagonal).max(1.0), structure);
        let grey = GREY.log2();
        let method = params.choice(METHOD);
        // Strength, tone protection, saturation gain per stop of local contrast.
        let (strength, protect, saturate) = match method {
            1 => (1.4, true, 0.6),
            2 => (0.8, true, 0.0),
            3 => (1.0, false, 0.25),
            _ => (1.0, true, 0.0),
        };
        for (i, p) in image.pixels_mut().iter_mut().enumerate() {
            let l = log[i];
            // Full strength around grey, none 5 stops away (except Classic).
            let m = if protect { 1.0 - ((l - grey) / 5.0).clamp(-1.0, 1.0).powi(2) } else { 1.0 } * strength;
            let mut delta = 0.0;
            if let Some(b) = &wide {
                delta += clarity / 100.0 * 0.6 * (l - b[i]);
            }
            if let Some(b) = &narrow {
                delta += structure / 100.0 * 0.8 * (l - b[i]);
            }
            let k = (delta * m).exp2();
            for v in &mut p[..3] {
                *v *= k;
            }
            if saturate != 0.0 {
                let y = luma([p[0], p[1], p[2]]);
                let s = 1.0 + saturate * (delta * m).abs().min(1.0);
                for v in &mut p[..3] {
                    *v = y + (*v - y) * s;
                }
            }
        }
    }

    /// Version 1 had no method (it was Natural): its parameters are valid as they are.
    fn migrate(&self, from_version: u32, params: tool_api::Params) -> Result<tool_api::Params, tool_api::ToolError> {
        match from_version {
            1 | 2 => Ok(params),
            found => Err(tool_api::ToolError::UnsupportedVersion { found, current: 2 }),
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::Host
    }
}

/// `log` blurred with `sigma`, or `None` when `amount` is 0 (not needed).
fn blurred(log: &[f32], w: usize, h: usize, sigma: f32, amount: f32) -> Option<Vec<f32>> {
    (amount != 0.0).then(|| {
        let mut b = log.to_vec();
        blur(&mut b, w, h, sigma);
        b
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    fn image() -> ImageF32 {
        // A soft step from dark to light across the middle.
        let px = (0..64 * 32)
            .map(|i| {
                let v = if i % 64 < 32 { 0.1 } else { 0.3 };
                [v, v, v, 1.0]
            })
            .collect();
        ImageF32::from_pixels(64, 32, WorkingSpace::LinearRec2020, px).expect("64×32")
    }

    fn run(clarity: f32) -> ImageF32 {
        let mut p = Params::new();
        p.set(crate::common::pid(CLARITY), ParamValue::Float(clarity));
        let r = ResolvedParams::resolve(Clarity.descriptor(), &p);
        let mut img = image();
        Clarity.apply_cpu(&mut img, &r);
        img
    }

    #[test]
    fn punch_is_stronger_than_neutral() {
        let at = |img: &ImageF32| img.pixels()[16 * 64 + 33][0];
        let with = |method: u32| {
            let mut p = Params::new();
            p.set(crate::common::pid(CLARITY), ParamValue::Float(100.0));
            p.set(crate::common::pid(METHOD), ParamValue::Choice(method));
            let r = ResolvedParams::resolve(Clarity.descriptor(), &p);
            let mut img = image();
            Clarity.apply_cpu(&mut img, &r);
            img
        };
        assert!(at(&with(1)) > at(&with(2)), "{} {}", at(&with(1)), at(&with(2)));
    }

    #[test]
    fn clarity_raises_contrast_across_an_edge_and_less_lowers_it() {
        let at = |img: &ImageF32, x: usize| img.pixels()[16 * 64 + x][0];
        let more = run(100.0);
        assert!(at(&more, 30) < 0.1 && at(&more, 33) > 0.3, "{} {}", at(&more, 30), at(&more, 33));
        let less = run(-100.0);
        assert!(at(&less, 30) > 0.1 && at(&less, 33) < 0.3);
        // Far from the edge nothing much happens; hue (here grey) is kept.
        let p = more.pixels()[16 * 64 + 2];
        assert!((p[0] - 0.1).abs() < 0.01 && (p[0] - p[2]).abs() < 1e-6);
    }
}
