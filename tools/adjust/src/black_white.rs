//! Black & white, as in Capture One: the image becomes grey, and six
//! sliders (red, yellow, green, cyan, blue, magenta) make the parts of
//! each colour lighter or darker in the conversion; a split tone can tint
//! the result.
//!
//! The grey is the colour's luminance, moved by up to ±1.5 stops by the
//! slider of its hue, in proportion to how strong the colour was (a grey
//! stays as it is). The toning hue and amount tint the result like
//! [`crate::color_balance`] does, by up to ±0.5 stop.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{WGSL_HUE_CHROMA, float, hue_chroma, hue_direction, luma, six_way, slider};

/// The six colour sliders, every 60° from red: id and label.
pub const COLORS: [(&str, &str); 6] = [
    ("red", "Red"),
    ("yellow", "Yellow"),
    ("green", "Green"),
    ("cyan", "Cyan"),
    ("blue", "Blue"),
    ("magenta", "Magenta"),
];
/// Id of the toning hue (degrees).
pub const TONE_HUE: &str = "tone_hue";
/// Id of the toning amount (0–100).
pub const TONE_AMOUNT: &str = "tone_amount";

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| {
    let mut params: Vec<_> = COLORS.iter().map(|(id, label)| slider(id, label)).collect();
    params.push(float(TONE_HUE, "Toning hue", [0.0, 360.0], 40.0, 1.0, Unit::Degrees));
    params.push(float(TONE_AMOUNT, "Toning amount", [0.0, 100.0], 0.0, 1.0, Unit::None));
    ToolDescriptor {
        id: ToolId::new("black_white").expect("constant id is valid"),
        version: 1,
        name: "Black & white",
        category: Category::Color,
        stage: Stage::Graph,
        params,
    }
});

/// The black-and-white tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct BlackWhite;

impl Tool for BlackWhite {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let mix: [f32; 6] = std::array::from_fn(|i| params.float(COLORS[i].0));
        let dir = hue_direction(params.float(TONE_HUE));
        let amount = params.float(TONE_AMOUNT);
        for p in image.pixels_mut() {
            let rgb = [p[0], p[1], p[2]];
            let (h, chroma) = hue_chroma(rgb);
            let grey = luma(rgb) * (six_way(mix, h) / 100.0 * 1.5 * chroma).exp2();
            for (ch, v) in p[..3].iter_mut().enumerate() {
                *v = grey * (amount / 100.0 * 0.5 * dir[ch]).exp2();
            }
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: WGSL.as_str() }
    }
}

static WGSL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
    let rgb = c.rgb;
    {WGSL_HUE_CHROMA}
    let hh = hue / 60.0;
    let i0 = min(u32(floor(hh)), 5u);
    let i1 = (i0 + 1u) % 6u;
    let f = hh - floor(hh);
    let w = params.v[i0 / 4u][i0 % 4u] * (1.0 - f) + params.v[i1 / 4u][i1 % 4u] * f;
    let grey = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593)) * exp2(w / 100.0 * 1.5 * chroma);
    let th = radians(params.v[1].z);
    let third = 6.2831853 / 3.0;
    let dir = vec3<f32>(cos(th), cos(th - third), cos(th - 2.0 * third));
    return vec4<f32>(grey * exp2(params.v[1].w / 100.0 * 0.5 * dir), c.a);"
    )
});

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    fn run(values: &[(&str, f32)], rgb: [f32; 3]) -> [f32; 3] {
        let mut p = Params::new();
        for (id, v) in values {
            p.set(crate::common::pid(id), ParamValue::Float(*v));
        }
        let r = ResolvedParams::resolve(BlackWhite.descriptor(), &p);
        let mut img =
            ImageF32::from_pixels(1, 1, WorkingSpace::LinearRec2020, vec![[rgb[0], rgb[1], rgb[2], 1.0]]).expect("1×1");
        BlackWhite.apply_cpu(&mut img, &r);
        let o = img.pixels()[0];
        [o[0], o[1], o[2]]
    }

    #[test]
    fn the_image_becomes_its_luminance() {
        let o = run(&[], [0.6, 0.2, 0.1]);
        let y = luma([0.6, 0.2, 0.1]);
        assert!(o.iter().all(|v| (v - y).abs() < 1e-6), "{o:?}");
    }

    #[test]
    fn a_colour_slider_lightens_only_its_colour() {
        let red = run(&[("red", 100.0)], [0.6, 0.05, 0.05]);
        assert!(red[0] > luma([0.6, 0.05, 0.05]) * 2.0, "{red:?}");
        let blue = run(&[("red", 100.0)], [0.05, 0.05, 0.6]);
        assert!((blue[0] - luma([0.05, 0.05, 0.6])).abs() < 1e-6, "{blue:?}");
    }

    #[test]
    fn toning_tints_the_grey() {
        let warm = run(&[(TONE_HUE, 30.0), (TONE_AMOUNT, 100.0)], [0.3, 0.3, 0.3]);
        assert!(warm[0] > warm[1] && warm[1] > warm[2], "{warm:?}");
    }
}
