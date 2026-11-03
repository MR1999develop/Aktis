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
use tool_api::{Category, GpuKernel, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{
    GREY, WGSL_HUE_CHROMA, float, hue_chroma, hue_direction, luma, six_way, slider, smoothstep, toggle,
};

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
/// Id of the switch that makes the image black and white.
pub const ENABLE: &str = "enable";
/// Id of the shadows' split-tone hue (degrees).
pub const SHADOW_HUE: &str = "shadow_hue";
/// Id of the shadows' split-tone saturation (0–100).
pub const SHADOW_SATURATION: &str = "shadow_saturation";
/// Id of the highlights' split-tone hue (degrees).
pub const HIGHLIGHT_HUE: &str = "highlight_hue";
/// Id of the highlights' split-tone saturation (0–100).
pub const HIGHLIGHT_SATURATION: &str = "highlight_saturation";
/// Id of the balance between shadow and highlight tones (−100 to 100).
pub const BALANCE: &str = "balance";

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| {
    let mut params: Vec<_> = COLORS.iter().map(|(id, label)| slider(id, label)).collect();
    params.push(float(TONE_HUE, "Toning hue", [0.0, 360.0], 40.0, 1.0, Unit::Degrees));
    params.push(float(TONE_AMOUNT, "Toning amount", [0.0, 100.0], 0.0, 1.0, Unit::None));
    params.push(toggle(ENABLE, "Enable Black & White", true));
    params.push(float(SHADOW_HUE, "Shadow hue", [0.0, 360.0], 220.0, 1.0, Unit::Degrees));
    params.push(float(SHADOW_SATURATION, "Shadow saturation", [0.0, 100.0], 0.0, 1.0, Unit::None));
    params.push(float(HIGHLIGHT_HUE, "Highlight hue", [0.0, 360.0], 40.0, 1.0, Unit::Degrees));
    params.push(float(HIGHLIGHT_SATURATION, "Highlight saturation", [0.0, 100.0], 0.0, 1.0, Unit::None));
    params.push(slider(BALANCE, "Balance"));
    ToolDescriptor {
        id: ToolId::new("black_white").expect("constant id is valid"),
        version: 2,
        name: "Black & white",
        category: Category::Color,
        order: 110,
        params,
    }
});

/// The black-and-white tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct BlackWhite;

/// How much a grey `y` takes the highlight tone (the rest, the shadow
/// tone): half-way at middle grey moved by `balance` (−100 to 100, up to
/// two stops), blending over three stops each way.
#[must_use]
pub fn split_weight(y: f32, balance: f32) -> f32 {
    let e = if y > 1e-6 { (y / GREY).log2() } else { -20.0 };
    smoothstep(-3.0, 3.0, e - balance / 100.0 * 2.0)
}

impl Tool for BlackWhite {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        if !params.bool(ENABLE) {
            return;
        }
        let mix: [f32; 6] = std::array::from_fn(|i| params.float(COLORS[i].0));
        let dir = hue_direction(params.float(TONE_HUE));
        let amount = params.float(TONE_AMOUNT);
        let shadow = (hue_direction(params.float(SHADOW_HUE)), params.float(SHADOW_SATURATION));
        let high = (hue_direction(params.float(HIGHLIGHT_HUE)), params.float(HIGHLIGHT_SATURATION));
        let balance = params.float(BALANCE);
        for p in image.pixels_mut() {
            let rgb = [p[0], p[1], p[2]];
            let (h, chroma) = hue_chroma(rgb);
            let grey = luma(rgb) * (six_way(mix, h) / 100.0 * 1.5 * chroma).exp2();
            let wh = split_weight(grey, balance);
            for (ch, v) in p[..3].iter_mut().enumerate() {
                let stops = amount / 100.0 * 0.5 * dir[ch]
                    + 0.5 * (shadow.1 / 100.0 * (1.0 - wh) * shadow.0[ch] + high.1 / 100.0 * wh * high.0[ch]);
                *v = grey * stops.exp2();
            }
        }
    }

    /// Version 1 had only the overall tint: its parameters are valid as they are.
    fn migrate(&self, from_version: u32, params: tool_api::Params) -> Result<tool_api::Params, tool_api::ToolError> {
        match from_version {
            1 | 2 => Ok(params),
            found => Err(tool_api::ToolError::UnsupportedVersion { found, current: 2 }),
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: WGSL.as_str() }
    }
}

static WGSL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "
    if (params.v[2].x < 0.5) {{ return c; }}
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
    var e = -20.0;
    if (grey > 1e-6) {{ e = log2(grey / 0.18); }}
    let tt = clamp((e - params.v[3].y / 100.0 * 2.0 + 3.0) / 6.0, 0.0, 1.0);
    let wh = tt * tt * (3.0 - 2.0 * tt);
    let sh = radians(params.v[2].y);
    let hi = radians(params.v[2].w);
    let sdir = vec3<f32>(cos(sh), cos(sh - third), cos(sh - 2.0 * third));
    let hdir = vec3<f32>(cos(hi), cos(hi - third), cos(hi - 2.0 * third));
    let stops = params.v[1].w / 100.0 * 0.5 * dir
        + 0.5 * (params.v[2].z / 100.0 * (1.0 - wh) * sdir + params.v[3].x / 100.0 * wh * hdir);
    return vec4<f32>(grey * exp2(stops), c.a);"
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
    fn split_tones_colour_shadows_and_highlights_apart_and_the_switch_turns_it_off() {
        let split = [(SHADOW_SATURATION, 100.0), (HIGHLIGHT_SATURATION, 100.0)];
        let dark = run(&split, [0.01, 0.01, 0.01]);
        assert!(dark[2] > dark[0], "cool shadows: {dark:?}");
        let bright = run(&split, [2.0, 2.0, 2.0]);
        assert!(bright[0] > bright[2], "warm highlights: {bright:?}");
        assert!(split_weight(0.18, 0.0) > 0.49 && split_weight(0.18, 0.0) < 0.51);
        assert!(split_weight(0.18, 100.0) < 0.3);
    }

    #[test]
    fn toning_tints_the_grey() {
        let warm = run(&[(TONE_HUE, 30.0), (TONE_AMOUNT, 100.0)], [0.3, 0.3, 0.3]);
        assert!(warm[0] > warm[1] && warm[1] > warm[2], "{warm:?}");
    }
}
