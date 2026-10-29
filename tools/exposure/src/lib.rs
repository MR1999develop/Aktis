//! Exposure, as in Capture One's Exposure panel: exposure, contrast,
//! brightness and saturation.
//!
//! - **Exposure** multiplies linear light by `2^stops`, like opening the
//!   aperture: every tone moves by the same number of stops (−4 to +4, as
//!   in Capture One).
//! - **Contrast** is an S curve in stops around middle grey (0.18): near
//!   grey the tones spread by `k` ([`contrast_slope`]: +100 doubles them,
//!   one stop above grey becomes about two; −100 halves them), the extra push levelling off over
//!   [`CONTRAST_SPAN`] stops so the far highlights and shadows move by a
//!   bounded amount. As in Capture One it also makes colours more (or
//!   less) saturated: the channels' ratios to luminance are raised to
//!   `1 + (k − 1)·`[`CONTRAST_COLOUR`].
//! - **Brightness** lifts or lowers the midtones and keeps black and white:
//!   `Y → Y^γ` up to white, `γ = 2^(−0.8·brightness/100)`, continued in a
//!   straight line above.
//! - **Saturation** moves colours away from (or towards) their grey, the
//!   same on every channel.
//!
//! Brightness changes luminance only, scaling R, G and B together, so hues
//! stay as they are; contrast keeps the luminance it sets. Alpha is
//! untouched. Because the data are scene-referred, values above 1 are
//! kept; highlight roll-off belongs to the output render, not here.

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

/// Largest exposure change, in stops (Capture One's range).
pub const MAX_STOPS: f32 = 4.0;

/// Stops around middle grey over which contrast acts fully; further out
/// its extra push levels off (at most `(k − 1)·CONTRAST_SPAN` stops).
pub const CONTRAST_SPAN: f32 = 3.0;

/// Share of the contrast that also goes into the colours.
pub const CONTRAST_COLOUR: f32 = 0.5;

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
            kind: ParamKind::Float { min: -MAX_STOPS, max: MAX_STOPS, default: 0.0, step: 0.05, unit: Unit::Stops },
        },
        slider(CONTRAST, "Contrast"),
        slider(BRIGHTNESS, "Brightness"),
        slider(SATURATION, "Saturation"),
    ],
});

/// How much contrast spreads the tones around middle grey: 2 at +100, 1 at
/// 0, 0.5 at −100 (the same number of stops either way).
#[must_use]
pub fn contrast_slope(contrast: f32) -> f32 {
    (contrast / 100.0).exp2()
}

/// One pixel: what the CPU reference and the WGSL kernel both compute.
fn pixel(rgb: [f32; 3], stops: f32, contrast: f32, brightness: f32, saturation: f32) -> [f32; 3] {
    let gain = stops.exp2();
    let mut rgb = rgb.map(|v| v * gain);
    let luma = |c: [f32; 3]| c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2];
    if contrast != 0.0 || brightness != 0.0 {
        let y = luma(rgb);
        if y > 1e-6 {
            let k = contrast_slope(contrast);
            let d = (y / 0.18).log2();
            let d2 = d + (k - 1.0) * CONTRAST_SPAN * (d / CONTRAST_SPAN).tanh();
            let mut y2 = 0.18 * d2.exp2();
            let g = (-0.8 * brightness / 100.0).exp2();
            y2 = if y2 <= 1.0 { y2.powf(g) } else { 1.0 + g * (y2 - 1.0) };
            // Contrast reaches the colours: ratios to luminance raised to kc.
            let kc = 1.0 + (k - 1.0) * CONTRAST_COLOUR;
            let ratios = rgb.map(|v| if v > 0.0 { (v / y).powf(kc) } else { v / y });
            let ly = luma(ratios);
            let scale = if ly > 1e-6 { y2 / ly } else { y2 / y };
            rgb = if ly > 1e-6 { ratios.map(|r| r * scale) } else { rgb.map(|v| v * scale) };
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
            let k = exp2(contrast / 100.0);
            let d = log2(y / 0.18);
            let d2 = d + (k - 1.0) * 3.0 * tanh(d / 3.0);
            var y2 = 0.18 * exp2(d2);
            let g = exp2(-0.8 * bright / 100.0);
            if (y2 <= 1.0) { y2 = pow(y2, g); } else { y2 = 1.0 + g * (y2 - 1.0); }
            let kc = 1.0 + (k - 1.0) * 0.5;
            let q = rgb / y;
            let ratios = select(q, pow(max(q, vec3<f32>(1e-30)), vec3<f32>(kc)), rgb > vec3<f32>(0.0));
            let ly = dot(ratios, w);
            if (ly > 1e-6) { rgb = ratios * (y2 / ly); } else { rgb = rgb * (y2 / y); }
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
    fn contrast_is_an_s_curve_in_stops_around_middle_grey() {
        let grey = |y: f32| [y, y, y, 1.0];
        let stops = |c: f32, y: f32| (luma(run_all(&[(CONTRAST, c)], grey(y))) / 0.18).log2();
        assert!(stops(100.0, 0.18).abs() < 1e-4, "middle grey stays");
        // +100: one stop above grey becomes about two, one below about −2.
        assert!((1.9..2.0).contains(&stops(100.0, 0.36)), "{}", stops(100.0, 0.36));
        assert!((-2.0..-1.9).contains(&stops(100.0, 0.09)), "{}", stops(100.0, 0.09));
        // Far from grey the push levels off: 6 stops up gain at most 3.
        let far = stops(100.0, 0.18 * 64.0);
        assert!(far > 8.5 && far < 9.0, "{far}");
        // Less contrast pulls the tones in.
        assert!((0.5..0.55).contains(&stops(-100.0, 0.36)), "{}", stops(-100.0, 0.36));
        // Always in order.
        let mut last = f32::NEG_INFINITY;
        for i in 1..200 {
            #[allow(clippy::cast_precision_loss)]
            let s = stops(100.0, 0.001 * (i * i) as f32);
            assert!(s > last, "monotonic");
            last = s;
        }
    }

    #[test]
    fn contrast_also_saturates_colours_and_keeps_their_order() {
        let c = run_all(&[(CONTRAST, 100.0)], [0.4, 0.2, 0.1, 1.0]);
        assert!(c[0] / c[1] > 2.0 && c[1] / c[2] > 2.0, "more saturated: {c:?}");
        assert!((c[0] / c[1] - c[1] / c[2]).abs() < 1e-3, "evenly, in stops: {c:?}");
        let less = run_all(&[(CONTRAST, -100.0)], [0.4, 0.2, 0.1, 1.0]);
        assert!(less[0] / less[1] < 2.0 && less[0] > less[1] && less[1] > less[2], "{less:?}");
        // A grey stays grey.
        let g = run_all(&[(CONTRAST, 80.0)], [0.3, 0.3, 0.3, 1.0]);
        assert!((g[0] - g[1]).abs() < 1e-6 && (g[1] - g[2]).abs() < 1e-6);
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
    fn brightness_keeps_the_hue() {
        let c = run_all(&[(BRIGHTNESS, 40.0)], [0.4, 0.2, 0.1, 1.0]);
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
    fn values_beyond_the_range_are_clamped_to_four_stops() {
        assert_eq!(run(12.0, [1.0, 1.0, 1.0, 1.0]), [16.0, 16.0, 16.0, 1.0]);
    }

    #[test]
    fn the_descriptor_is_valid() {
        assert_eq!(Exposure.descriptor().validate(), Ok(()));
    }
}
