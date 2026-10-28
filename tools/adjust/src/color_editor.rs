//! Colour editor, as Capture One's:
//!
//! - **Basic**: for each of six colour ranges (red, yellow, green, cyan,
//!   blue, magenta) a hue shift, a saturation and a lightness. A pixel takes
//!   the settings of the ranges its hue falls between, blended by how close
//!   it is to each.
//! - **Advanced**: up to [`ADVANCED`] ranges picked on the photo, each with
//!   its own centre hue, width and smoothness (how gradually it fades out)
//!   and its own hue, saturation and lightness changes.
//! - **Skin tone**: one range with, besides the changes, uniformity of hue,
//!   saturation and lightness, which pull the skin's colours together.
//!
//! Greys are left alone and pale colours are changed less than strong ones.
//! Hue shifts turn the colour around the grey axis, so the luminance of
//! greys never moves.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ParamDescriptor, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{WGSL_HUE_CHROMA, float, hue_chroma, hue_diff, luma, six_way, slider, smoothstep, toggle};

/// The six ranges, every 60° from red: id prefix and name.
pub const RANGES: [(&str, &str); 6] = [
    ("red", "Red"),
    ("yellow", "Yellow"),
    ("green", "Green"),
    ("cyan", "Cyan"),
    ("blue", "Blue"),
    ("magenta", "Magenta"),
];

/// Ranges of the Advanced tab.
pub const ADVANCED: usize = 3;

/// The values of an Advanced or Skin tone range, in parameter order: id suffix.
pub const RANGE_VALUES: [&str; 7] = ["on", "center", "width", "smoothness", "hue", "saturation", "lightness"];

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

/// Id of value `value` (from [`RANGE_VALUES`]) of Advanced range `n` (0-based).
#[must_use]
pub fn advanced_id(n: usize, value: &str) -> String {
    format!("adv{}_{value}", n + 1)
}

/// Id of value `value` (from [`RANGE_VALUES`], or `uniform_hue`,
/// `uniform_saturation`, `uniform_lightness`) of the skin tone range.
#[must_use]
pub fn skin_id(value: &str) -> String {
    format!("skin_{value}")
}

/// The parameters of a picked range: on, centre, width, smoothness, changes.
fn range_params(id: impl Fn(&str) -> String, center: f32, width: f32) -> Vec<ParamDescriptor> {
    vec![
        toggle(&id("on"), "On", false),
        float(&id("center"), "Centre", [0.0, 360.0], center, 1.0, Unit::Degrees),
        float(&id("width"), "Width", [5.0, 90.0], width, 1.0, Unit::Degrees),
        float(&id("smoothness"), "Smoothness", [0.0, 100.0], 50.0, 1.0, Unit::None),
        float(&id("hue"), "Hue", [-30.0, 30.0], 0.0, 0.5, Unit::Degrees),
        slider(&id("saturation"), "Saturation"),
        slider(&id("lightness"), "Lightness"),
    ]
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| {
    let mut params: Vec<ParamDescriptor> = (0..RANGES.len())
        .flat_map(|r| -> [ParamDescriptor; 3] {
            [
                float(&hue_id(r), "Hue", [-30.0, 30.0], 0.0, 0.5, Unit::Degrees),
                slider(&saturation_id(r), "Saturation"),
                slider(&lightness_id(r), "Lightness"),
            ]
        })
        .collect();
    for n in 0..ADVANCED {
        params.extend(range_params(|v| advanced_id(n, v), 0.0, 30.0));
    }
    params.extend(range_params(skin_id, 25.0, 25.0));
    params.push(float(&skin_id("uniform_hue"), "Hue uniformity", [0.0, 100.0], 0.0, 1.0, Unit::None));
    params.push(float(&skin_id("uniform_saturation"), "Saturation uniformity", [0.0, 100.0], 0.0, 1.0, Unit::None));
    params.push(float(&skin_id("uniform_lightness"), "Lightness uniformity", [0.0, 100.0], 0.0, 1.0, Unit::None));
    ToolDescriptor {
        id: ToolId::new("color_editor").expect("constant id is valid"),
        version: 2,
        name: "Colour editor",
        category: Category::Color,
        stage: Stage::Graph,
        params,
    }
});

/// Index of the first Advanced parameter, and of the skin tone's.
const ADVANCED_BASE: usize = 18;
const SKIN_BASE: usize = ADVANCED_BASE + ADVANCED * 7;

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

/// How much a pixel of hue `h` and `chroma` belongs to a picked range.
#[must_use]
pub fn range_weight(h: f32, chroma: f32, center: f32, width: f32, smoothness: f32) -> f32 {
    let d = hue_diff(h, center).abs();
    let inner = width * (1.0 - 0.5 * smoothness / 100.0);
    let outer = width * (1.0 + 0.5 * smoothness / 100.0) + 0.001;
    (1.0 - smoothstep(inner, outer, d)) * smoothstep(0.02, 0.2, chroma)
}

/// Hue shift, saturation and lightness applied with weight `w`.
fn change(rgb: [f32; 3], dh: f32, ds: f32, dl: f32, w: f32) -> [f32; 3] {
    let rgb = rotate_hue(rgb, dh * w);
    let y = luma(rgb);
    let s = 1.0 + ds / 100.0 * w;
    let k = (dl / 100.0 * w).exp2();
    rgb.map(|v| (y + (v - y) * s) * k)
}

/// One picked range, read from the parameter values `v` from index `b`.
fn picked(rgb: [f32; 3], v: &[f32], b: usize) -> [f32; 3] {
    if v[b] < 0.5 {
        return rgb;
    }
    let (h, chroma) = hue_chroma(rgb);
    let w = range_weight(h, chroma, v[b + 1], v[b + 2], v[b + 3]);
    if w <= 0.0 { rgb } else { change(rgb, v[b + 4], v[b + 5], v[b + 6], w) }
}

impl Tool for ColorEditor {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let v = params.gpu_uniforms();
        let basic_hue: [f32; 6] = std::array::from_fn(|r| v[r * 3]);
        let basic_sat: [f32; 6] = std::array::from_fn(|r| v[r * 3 + 1]);
        let basic_light: [f32; 6] = std::array::from_fn(|r| v[r * 3 + 2]);
        let basic = v[..ADVANCED_BASE].iter().any(|x| *x != 0.0);
        let any_range = (0..=ADVANCED).any(|n| {
            let b = if n == ADVANCED { SKIN_BASE } else { ADVANCED_BASE + n * 7 };
            v[b] >= 0.5
        });
        if !basic && !any_range {
            return;
        }
        for p in image.pixels_mut() {
            let mut rgb = [p[0], p[1], p[2]];
            if basic {
                let (h, chroma) = hue_chroma(rgb);
                if chroma > 0.0 {
                    let (dh, ds, dl) = (six_way(basic_hue, h), six_way(basic_sat, h), six_way(basic_light, h));
                    if dh != 0.0 || ds != 0.0 || dl != 0.0 {
                        rgb = change(rgb, dh, ds, dl, chroma);
                    }
                }
            }
            for n in 0..ADVANCED {
                rgb = picked(rgb, &v, ADVANCED_BASE + n * 7);
            }
            if v[SKIN_BASE] >= 0.5 {
                let (h, chroma) = hue_chroma(rgb);
                let w = range_weight(h, chroma, v[SKIN_BASE + 1], v[SKIN_BASE + 2], v[SKIN_BASE + 3]);
                if w > 0.0 {
                    rgb = change(rgb, v[SKIN_BASE + 4], v[SKIN_BASE + 5], v[SKIN_BASE + 6], w);
                    rgb = uniformity(rgb, &v[SKIN_BASE..SKIN_BASE + 10], w);
                }
            }
            p[..3].copy_from_slice(&rgb);
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: WGSL.as_str() }
    }

    /// Version 1 had only the Basic ranges: its parameters are valid as they are.
    fn migrate(&self, from_version: u32, params: tool_api::Params) -> Result<tool_api::Params, tool_api::ToolError> {
        match from_version {
            1 | 2 => Ok(params),
            found => Err(tool_api::ToolError::UnsupportedVersion { found, current: 2 }),
        }
    }
}

/// The skin tone's uniformity: hue pulled to the range's centre,
/// saturation towards a typical skin's, lightness towards a mid tone, each
/// by its share (0–100) times the weight `w`. `s` holds the skin values in
/// parameter order.
fn uniformity(rgb: [f32; 3], s: &[f32], w: f32) -> [f32; 3] {
    let (uh, us, ul) = (s[7] / 100.0 * w, s[8] / 100.0 * w, s[9] / 100.0 * w);
    let (h, _) = hue_chroma(rgb);
    let mut rgb = rotate_hue(rgb, -hue_diff(h, s[1]) * uh);
    let (_, chroma) = hue_chroma(rgb);
    if chroma > 0.01 {
        let y = luma(rgb);
        let k = 1.0 + (0.4 / chroma - 1.0) * us;
        rgb = rgb.map(|v| y + (v - y) * k);
    }
    let y = luma(rgb);
    if y > 1e-6 {
        let g = (0.3 / y).powf(ul * 0.5);
        rgb = rgb.map(|v| v * g);
    }
    rgb
}

static WGSL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
    var rgb = c.rgb;
    var changed = false;
    for (var i = 0u; i < 18u; i++) {{ if (params.v[i / 4u][i % 4u] != 0.0) {{ changed = true; }} }}
    for (var n = 0u; n < 4u; n++) {{ if (params.v[(18u + n * 7u) / 4u][(18u + n * 7u) % 4u] >= 0.5) {{ changed = true; }} }}
    if (!changed) {{ return c; }}
    for (var step_i = 0u; step_i < 5u; step_i++) {{
        {WGSL_HUE_CHROMA}
        var dh = 0.0;
        var ds = 0.0;
        var dl = 0.0;
        var w = 0.0;
        let b = 18u + (step_i - min(step_i, 1u)) * 7u;
        if (step_i == 0u) {{
            if (chroma <= 0.0) {{ continue; }}
            let hh = hue / 60.0;
            let i0 = min(u32(floor(hh)), 5u);
            let i1 = (i0 + 1u) % 6u;
            let f = hh - floor(hh);
            let a = i0 * 3u;
            let e = i1 * 3u;
            dh = params.v[a / 4u][a % 4u] * (1.0 - f) + params.v[e / 4u][e % 4u] * f;
            ds = params.v[(a + 1u) / 4u][(a + 1u) % 4u] * (1.0 - f) + params.v[(e + 1u) / 4u][(e + 1u) % 4u] * f;
            dl = params.v[(a + 2u) / 4u][(a + 2u) % 4u] * (1.0 - f) + params.v[(e + 2u) / 4u][(e + 2u) % 4u] * f;
            if (dh == 0.0 && ds == 0.0 && dl == 0.0) {{ continue; }}
            w = chroma;
        }} else {{
            if (params.v[b / 4u][b % 4u] < 0.5) {{ continue; }}
            let center = params.v[(b + 1u) / 4u][(b + 1u) % 4u];
            let width = params.v[(b + 2u) / 4u][(b + 2u) % 4u];
            let smooth_v = params.v[(b + 3u) / 4u][(b + 3u) % 4u];
            dh = params.v[(b + 4u) / 4u][(b + 4u) % 4u];
            ds = params.v[(b + 5u) / 4u][(b + 5u) % 4u];
            dl = params.v[(b + 6u) / 4u][(b + 6u) % 4u];
            var hd = (hue - center) - 360.0 * floor((hue - center) / 360.0);
            if (hd > 180.0) {{ hd = hd - 360.0; }}
            let d = abs(hd);
            let inner = width * (1.0 - 0.5 * smooth_v / 100.0);
            let outer = width * (1.0 + 0.5 * smooth_v / 100.0) + 0.001;
            let t = clamp((d - inner) / (outer - inner), 0.0, 1.0);
            let tc = clamp((chroma - 0.02) / 0.18, 0.0, 1.0);
            w = (1.0 - t * t * (3.0 - 2.0 * t)) * (tc * tc * (3.0 - 2.0 * tc));
            if (w <= 0.0) {{ continue; }}
        }}
        let ang = radians(dh * w);
        let sn = sin(ang);
        let cs = cos(ang);
        let kk = 1.0 / sqrt(3.0);
        let dsum = (rgb.r + rgb.g + rgb.b) * kk;
        let cr = vec3<f32>(kk * (rgb.b - rgb.g), kk * (rgb.r - rgb.b), kk * (rgb.g - rgb.r));
        rgb = rgb * cs + cr * sn + vec3<f32>(kk * dsum * (1.0 - cs));
        let y = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
        rgb = (vec3<f32>(y) + (rgb - vec3<f32>(y)) * (1.0 + ds / 100.0 * w)) * exp2(dl / 100.0 * w);
        if (step_i == 4u) {{
            let center = params.v[(b + 1u) / 4u][(b + 1u) % 4u];
            let uh = params.v[(b + 7u) / 4u][(b + 7u) % 4u] / 100.0 * w;
            let us = params.v[(b + 8u) / 4u][(b + 8u) % 4u] / 100.0 * w;
            let ul = params.v[(b + 9u) / 4u][(b + 9u) % 4u] / 100.0 * w;
            {{
                {WGSL_HUE_CHROMA}
                var hd = (hue - center) - 360.0 * floor((hue - center) / 360.0);
                if (hd > 180.0) {{ hd = hd - 360.0; }}
                let a2 = radians(-hd * uh);
                let s2 = sin(a2);
                let c2 = cos(a2);
                let d2 = (rgb.r + rgb.g + rgb.b) * kk;
                let r2 = vec3<f32>(kk * (rgb.b - rgb.g), kk * (rgb.r - rgb.b), kk * (rgb.g - rgb.r));
                rgb = rgb * c2 + r2 * s2 + vec3<f32>(kk * d2 * (1.0 - c2));
            }}
            {{
                {WGSL_HUE_CHROMA}
                if (chroma > 0.01) {{
                    let y2 = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
                    rgb = vec3<f32>(y2) + (rgb - vec3<f32>(y2)) * (1.0 + (0.4 / chroma - 1.0) * us);
                }}
            }}
            let y3 = dot(rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
            if (y3 > 1e-6) {{ rgb = rgb * pow(0.3 / y3, ul * 0.5); }}
        }}
    }}
    return vec4<f32>(rgb, c.a);"
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
            let value = if id.ends_with("_on") { ParamValue::Bool(*v > 0.5) } else { ParamValue::Float(*v) };
            p.set(crate::common::pid(id), value);
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
    fn a_picked_range_changes_only_its_hues_and_fades_out() {
        let on = [
            (advanced_id(0, "on"), 1.0),
            (advanced_id(0, "center"), 120.0),
            (advanced_id(0, "width"), 20.0),
            (advanced_id(0, "lightness"), -100.0),
        ];
        let green = run(&on, [0.1, 0.6, 0.1]);
        assert!(green[1] < 0.45, "{green:?}");
        assert_eq!(run(&on, [0.6, 0.1, 0.1]), [0.6, 0.1, 0.1], "red is far from the range");
        assert!(range_weight(120.0, 1.0, 120.0, 20.0, 50.0) > 0.99);
        assert!(range_weight(150.0, 1.0, 120.0, 20.0, 50.0) < 1e-6);
        let w = range_weight(140.0, 1.0, 120.0, 20.0, 100.0);
        assert!(w > 0.2 && w < 0.8, "{w}");
    }

    #[test]
    fn skin_uniformity_pulls_hues_together() {
        let skin = [(skin_id("on"), 1.0), (skin_id("uniform_hue"), 100.0)];
        let (h0, _) = hue_chroma([0.6, 0.3, 0.2]);
        let (h1, _) = hue_chroma(run(&skin, [0.6, 0.3, 0.2]));
        assert!((hue_diff(h1, 25.0)).abs() < (hue_diff(h0, 25.0)).abs(), "{h0} → {h1}");
    }

    #[test]
    fn version_one_recipes_still_open() {
        assert!(ColorEditor.migrate(1, tool_api::Params::new()).is_ok());
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
