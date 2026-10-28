//! Exposure, as in Capture One's Exposure panel: exposure, contrast,
//! brightness and saturation.
//!
//! - **Exposure** multiplies linear light by `2^stops`, like opening the
//!   aperture: every tone moves by the same number of stops.
//! - **Contrast** spreads the tones around middle grey (0.18) evenly in
//!   stops: luminance `Y → 0.18·(Y/0.18)^k`, `k = 1 + contrast/200`.
//! - **Brightness** lifts or lowers the midtones and keeps black and white:
//!   `Y → Y^γ` up to white, `γ = 2^(−0.8·brightness/100)`, continued in a
//!   straight line above.
//! - **Saturation** moves colours away from (or towards) their grey.
//!
//! Contrast and brightness change luminance only, scaling R, G and B
//! together, so hues stay as they are. Alpha is untouched. Because the data
//! are scene-referred, values above 1 are kept; highlight roll-off belongs
//! to the output render, not here.

use std::sync::{Arc, LazyLock};

use core_model::ImageF32;
use tool_api::{
    Category, GpuKernel, ParamDescriptor, ParamId, ParamKind, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId, Unit,
};

/// Id of the exposure parameter, in stops.
pub const EXPOSURE: &str = "exposure";
/// Id of the contrast parameter, −100 to 100.
pub const CONTRAST: &str = "contrast";
/// Id of the brightness parameter, −100 to 100.
pub const BRIGHTNESS: &str = "brightness";
/// Id of the saturation parameter, −100 to 100.
pub const SATURATION: &str = "saturation";

/// Rec. 2020 luminance weights (the working space).
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

fn slider(id: &str, label: &'static str) -> ParamDescriptor {
    ParamDescriptor {
        id: ParamId::new(id).expect("constant id is valid"),
        label,
        kind: ParamKind::Float { min: -100.0, max: 100.0, default: 0.0, step: 1.0, unit: Unit::None },
    }
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("exposure").expect("constant id is valid"),
    version: 2,
    name: "Exposure",
    category: Category::Exposure,
    stage: Stage::Graph,
    params: vec![
        ParamDescriptor {
            id: ParamId::new(EXPOSURE).expect("constant id is valid"),
            label: "Exposure",
            kind: ParamKind::Float { min: -5.0, max: 5.0, default: 0.0, step: 0.01, unit: Unit::Stops },
        },
        slider(CONTRAST, "Contrast"),
        slider(BRIGHTNESS, "Brightness"),
        slider(SATURATION, "Saturation"),
    ],
});

/// One pixel: what the CPU reference and the WGSL kernel both compute.
fn pixel(rgb: [f32; 3], stops: f32, contrast: f32, brightness: f32, saturation: f32) -> [f32; 3] {
    let gain = stops.exp2();
    let mut rgb = rgb.map(|v| v * gain);
    let luma = |c: [f32; 3]| c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
    if contrast != 0.0 || brightness != 0.0 {
        let y = luma(rgb);
        if y > 1e-6 {
            let mut y2 = 0.18 * ((y / 0.18).log2() * (1.0 + contrast / 200.0)).exp2();
            let g = (-0.8 * brightness / 100.0).exp2();
            y2 = if y2 <= 1.0 { y2.powf(g) } else { 1.0 + g * (y2 - 1.0) };
            let k = y2 / y;
            rgb = rgb.map(|v| v * k);
        }
    }
    if saturation != 0.0 {
        let y = luma(rgb);
        let s = 1.0 + saturation / 100.0;
        rgb = rgb.map(|v| y + (v - y) * s);
    }
    rgb
}

const WGSL: &str = "
    let w = vec3<f32>(0.2627, 0.6780, 0.0593);
    let contrast = params.v[0].y;
    let bright = params.v[0].z;
    let sat = params.v[0].w;
    var rgb = c.rgb * exp2(params.v[0].x);
    if (contrast != 0.0 || bright != 0.0) {
        let y = dot(rgb, w);
        if (y > 1e-6) {
            var y2 = 0.18 * exp2(log2(y / 0.18) * (1.0 + contrast / 200.0));
            let g = exp2(-0.8 * bright / 100.0);
            if (y2 <= 1.0) { y2 = pow(y2, g); } else { y2 = 1.0 + g * (y2 - 1.0); }
            rgb = rgb * (y2 / y);
        }
    }
    if (sat != 0.0) {
        let y = dot(rgb, w);
        rgb = vec3<f32>(y) + (rgb - vec3<f32>(y)) * (1.0 + sat / 100.0);
    }
    return vec4<f32>(rgb, c.a);
";

/// The exposure tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Exposure;

impl Exposure {
    /// The tool, ready to register.
    #[must_use]
    pub fn shared() -> Arc<dyn Tool> {
        Arc::new(Self)
    }
}

impl Tool for Exposure {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let (stops, contrast) = (params.float(EXPOSURE), params.float(CONTRAST));
        let (brightness, saturation) = (params.float(BRIGHTNESS), params.float(SATURATION));
        for p in image.pixels_mut() {
            let rgb = pixel([p[0], p[1], p[2]], stops, contrast, brightness, saturation);
            p[..3].copy_from_slice(&rgb);
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: WGSL }
    }

    /// Version 1 had only the exposure: its parameters are valid as they are.
    fn migrate(&self, from_version: u32, params: tool_api::Params) -> Result<tool_api::Params, tool_api::ToolError> {
        match from_version {
            1 | 2 => Ok(params),
            found => Err(tool_api::ToolError::UnsupportedVersion { found, current: 2 }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // multiplying by powers of two is exact
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    fn image(v: [f32; 4]) -> ImageF32 {
        ImageF32::from_pixels(1, 1, WorkingSpace::LinearRec2020, vec![v]).expect("1×1 image")
    }

    fn run(stops: f32, v: [f32; 4]) -> [f32; 4] {
        run_all(&[(EXPOSURE, stops)], v)
    }

    fn run_all(values: &[(&str, f32)], v: [f32; 4]) -> [f32; 4] {
        let mut p = Params::new();
        for (id, x) in values {
            p.set(ParamId::new(id).expect("valid"), ParamValue::Float(*x));
        }
        let r = ResolvedParams::resolve(Exposure.descriptor(), &p);
        let mut img = image(v);
        Exposure.apply_cpu(&mut img, &r);
        img.pixels()[0]
    }

    fn luma(p: [f32; 4]) -> f32 {
        p[0] * LUMA[0] + p[1] * LUMA[1] + p[2] * LUMA[2]
    }

    #[test]
    fn contrast_spreads_the_tones_evenly_in_stops_around_middle_grey() {
        let grey = |y: f32| [y, y, y, 1.0];
        assert!((luma(run_all(&[(CONTRAST, 100.0)], grey(0.18))) - 0.18).abs() < 1e-5);
        // +100: one stop above grey becomes 1.5 stops, one below −1.5.
        let up = luma(run_all(&[(CONTRAST, 100.0)], grey(0.36)));
        let down = luma(run_all(&[(CONTRAST, 100.0)], grey(0.09)));
        assert!(((up / 0.18).log2() - 1.5).abs() < 1e-4, "{up}");
        assert!(((down / 0.18).log2() + 1.5).abs() < 1e-4, "{down}");
        // Less contrast pulls them in.
        let up = luma(run_all(&[(CONTRAST, -100.0)], grey(0.36)));
        assert!(((up / 0.18).log2() - 0.5).abs() < 1e-4, "{up}");
    }

    #[test]
    fn brightness_moves_the_midtones_and_keeps_black_and_white() {
        let grey = |y: f32| [y, y, y, 1.0];
        let lifted = luma(run_all(&[(BRIGHTNESS, 100.0)], grey(0.18)));
        assert!(lifted > 0.3 && lifted < 0.42, "{lifted}");
        assert!(luma(run_all(&[(BRIGHTNESS, -100.0)], grey(0.18))) < 0.1);
        assert!((luma(run_all(&[(BRIGHTNESS, 100.0)], grey(1.0))) - 1.0).abs() < 1e-5);
        assert!(luma(run_all(&[(BRIGHTNESS, 100.0)], grey(0.0))).abs() < 1e-9);
        // Monotonic above white too.
        assert!(luma(run_all(&[(BRIGHTNESS, 100.0)], grey(2.0))) > 1.0);
    }

    #[test]
    fn contrast_and_brightness_keep_the_hue() {
        let c = run_all(&[(CONTRAST, 60.0), (BRIGHTNESS, 40.0)], [0.4, 0.2, 0.1, 1.0]);
        assert!((c[0] / c[1] - 2.0).abs() < 1e-4 && (c[1] / c[2] - 2.0).abs() < 1e-4, "{c:?}");
    }

    #[test]
    fn saturation_moves_colours_from_their_grey() {
        let grey = run_all(&[(SATURATION, -100.0)], [0.4, 0.2, 0.1, 1.0]);
        assert!((grey[0] - grey[1]).abs() < 1e-6 && (grey[1] - grey[2]).abs() < 1e-6, "{grey:?}");
        let more = run_all(&[(SATURATION, 50.0)], [0.4, 0.2, 0.1, 1.0]);
        assert!(more[0] > 0.4 && more[2] < 0.1, "{more:?}");
        assert!((luma(more) - luma([0.4, 0.2, 0.1, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn version_one_recipes_still_open() {
        let p = Params::new();
        assert_eq!(Exposure.migrate(1, p.clone()), Ok(p));
        assert!(Exposure.migrate(3, Params::new()).is_err());
    }

    #[test]
    fn plus_one_stop_doubles_light_and_keeps_alpha() {
        assert_eq!(run(1.0, [0.18, 0.5, 2.0, 0.75]), [0.36, 1.0, 4.0, 0.75]);
    }

    #[test]
    fn zero_stops_changes_nothing_and_negative_values_stay_negative() {
        assert_eq!(run(0.0, [-0.01, 0.0, 3.0, 1.0]), [-0.01, 0.0, 3.0, 1.0]);
        assert_eq!(run(-1.0, [-0.02, 0.5, 1.0, 1.0]), [-0.01, 0.25, 0.5, 1.0]);
    }

    #[test]
    fn values_beyond_the_range_are_clamped_to_five_stops() {
        assert_eq!(run(12.0, [1.0, 1.0, 1.0, 1.0]), [32.0, 32.0, 32.0, 1.0]);
    }

    #[test]
    fn the_descriptor_is_valid() {
        assert_eq!(Exposure.descriptor().validate(), Ok(()));
    }
}
