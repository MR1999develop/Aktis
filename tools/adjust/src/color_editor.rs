//! Colour editor, as Capture One's basic colour editor: for each of six
//! colour ranges (red, yellow, green, cyan, blue, magenta) a hue shift,
//! a saturation and a lightness.
//!
//! A pixel takes the settings of the ranges its hue falls between, blended
//! by how close it is to each; greys are left alone and pale colours are
//! changed less than strong ones. Hue shifts turn the colour around the
//! grey axis, so the luminance of greys never moves.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ParamDescriptor, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{WGSL_HUE_CHROMA, float, hue_chroma, luma, six_way, slider};

/// The six ranges, every 60° from red: id prefix and name.
pub const RANGES: [(&str, &str); 6] = [
    ("red", "Red"),
    ("yellow", "Yellow"),
    ("green", "Green"),
    ("cyan", "Cyan"),
    ("blue", "Blue"),
    ("magenta", "Magenta"),
];

/// Id of a range's hue shift (degrees, −30 to 30).
#[must_use]
pub fn hue_id(range: usize) -> String {
    format!("{}_hue", RANGES[range].0)
}

/// Id of a range's saturation (−100 to 100).
#[must_use]
pub fn saturation_id(range: usize) -> String {
    format!("{}_saturation", RANGES[range].0)
}

/// Id of a range's lightness (−100 to 100).
#[must_use]
pub fn lightness_id(range: usize) -> String {
    format!("{}_lightness", RANGES[range].0)
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("color_editor").expect("constant id is valid"),
    version: 1,
    name: "Colour editor",
    category: Category::Color,
    stage: Stage::Graph,
    params: (0..RANGES.len())
        .flat_map(|r| -> [ParamDescriptor; 3] {
            [
                float(&hue_id(r), "Hue", [-30.0, 30.0], 0.0, 0.5, Unit::Degrees),
                slider(&saturation_id(r), "Saturation"),
                slider(&lightness_id(r), "Lightness"),
            ]
        })
        .collect(),
});

/// The colour-editor tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct ColorEditor;

/// `rgb` turned by `degrees` around the grey axis (1, 1, 1).
fn rotate_hue(rgb: [f32; 3], degrees: f32) -> [f32; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    let k = 1.0 / 3.0_f32.sqrt();
    let dot = (rgb[0] + rgb[1] + rgb[2]) * k;
    // Rodrigues: v cos + (k × v) sin + k (k · v)(1 − cos), with k = (1, 1, 1)/√3.
    let cross = [k * (rgb[2] - rgb[1]), k * (rgb[0] - rgb[2]), k * (rgb[1] - rgb[0])];
    std::array::from_fn(|i| rgb[i] * c + cross[i] * s + k * dot * (1.0 - c))
}

impl Tool for ColorEditor {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let hue: [f32; 6] = std::array::from_fn(|r| params.float(&hue_id(r)));
        let sat: [f32; 6] = std::array::from_fn(|r| params.float(&saturation_id(r)));
        let light: [f32; 6] = std::array::from_fn(|r| params.float(&lightness_id(r)));
        if hue.iter().chain(&sat).chain(&light).all(|v| *v == 0.0) {
            return;
        }
        for p in image.pixels_mut() {
            let mut rgb = [p[0], p[1], p[2]];
            let (h, chroma) = hue_chroma(rgb);
            if chroma <= 0.0 {
                continue;
            }
            let (dh, ds, dl) = (six_way(hue, h), six_way(sat, h), six_way(light, h));
            if dh == 0.0 && ds == 0.0 && dl == 0.0 {
                continue;
            }
            rgb = rotate_hue(rgb, dh * chroma);
            let y = luma(rgb);
            let s = 1.0 + ds / 100.0 * chroma;
            rgb = rgb.map(|v| y + (v - y) * s);
            let k = (dl / 100.0 * chroma).exp2();
            p[..3].copy_from_slice(&rgb.map(|v| v * k));
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: WGSL.as_str() }
    }
}

static WGSL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
    var rgb = c.rgb;
    var changed = false;
    for (var i = 0u; i < 18u; i++) {{ if (params.v[i / 4u][i % 4u] != 0.0) {{ changed = true; }} }}
    if (!changed) {{ return c; }}
    {WGSL_HUE_CHROMA}
    if (chroma <= 0.0) {{ return c; }}
    let hh = hue / 60.0;
    let i0 = min(u32(floor(hh)), 5u);
    let i1 = (i0 + 1u) % 6u;
    let f = hh - floor(hh);
    let a = i0 * 3u;
    let b = i1 * 3u;
    let dh = params.v[a / 4u][a % 4u] * (1.0 - f) + params.v[b / 4u][b % 4u] * f;
    let ds = params.v[(a + 1u) / 4u][(a + 1u) % 4u] * (1.0 - f) + params.v[(b + 1u) / 4u][(b + 1u) % 4u] * f;
    let dl = params.v[(a + 2u) / 4u][(a + 2u) % 4u] * (1.0 - f) + params.v[(b + 2u) / 4u][(b + 2u) % 4u] * f;
    if (dh == 0.0 && ds == 0.0 && dl == 0.0) {{ return c; }}
    let ang = radians(dh * chroma);
    let sn = sin(ang);
    let cs = cos(ang);
    let kk = 1.0 / sqrt(3.0);
    let d = (rgb.r + rgb.g + rgb.b) * kk;
    let cr = vec3<f32>(kk * (rgb.b - rgb.g), kk * (rgb.r - rgb.b), kk * (rgb.g - rgb.r));
    rgb = rgb * cs + cr * sn + vec3<f32>(kk * d * (1.0 - cs));
    let y = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    rgb = vec3<f32>(y) + (rgb - vec3<f32>(y)) * (1.0 + ds / 100.0 * chroma);
    return vec4<f32>(rgb * exp2(dl / 100.0 * chroma), c.a);"
    )
});

#[cfg(test)]
#[allow(clippy::float_cmp)] // unchanged values are bit-identical
mod tests {
    use super::*;
    use core_model::{ParamValue, Params, WorkingSpace};

    fn run(values: &[(String, f32)], rgb: [f32; 3]) -> [f32; 3] {
        let mut p = Params::new();
        for (id, v) in values {
            p.set(crate::common::pid(id), ParamValue::Float(*v));
        }
        let r = ResolvedParams::resolve(ColorEditor.descriptor(), &p);
        let mut img =
            ImageF32::from_pixels(1, 1, WorkingSpace::LinearRec2020, vec![[rgb[0], rgb[1], rgb[2], 1.0]]).expect("1×1");
        ColorEditor.apply_cpu(&mut img, &r);
        let o = img.pixels()[0];
        [o[0], o[1], o[2]]
    }

    #[test]
    fn greys_are_never_touched() {
        let all: Vec<(String, f32)> = (0..6).flat_map(|r| [(hue_id(r), 30.0), (lightness_id(r), 100.0)]).collect();
        assert_eq!(run(&all, [0.3, 0.3, 0.3]), [0.3, 0.3, 0.3]);
    }

    #[test]
    fn only_the_chosen_range_changes() {
        let blue_darker = [(lightness_id(4), -100.0)];
        let blue = run(&blue_darker, [0.05, 0.1, 0.6]);
        assert!(blue[2] < 0.4, "{blue:?}");
        assert_eq!(run(&blue_darker, [0.6, 0.1, 0.05]), [0.6, 0.1, 0.05]);
        let less_red = run(&[(saturation_id(0), -100.0)], [0.6, 0.1, 0.1]);
        assert!(less_red[0] - less_red[1] < 0.1, "{less_red:?}");
    }

    #[test]
    fn a_hue_shift_turns_the_colour_and_keeps_grey_on_the_axis() {
        let r = rotate_hue([0.5, 0.5, 0.5], 25.0);
        assert!(r.iter().all(|v| (v - 0.5).abs() < 1e-6), "{r:?}");
        // Red turned +30° towards yellow gains green.
        let shifted = run(&[(hue_id(0), 30.0)], [0.8, 0.0, 0.0]);
        assert!(shifted[1] > shifted[2], "{shifted:?}");
    }
}
