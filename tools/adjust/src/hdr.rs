//! High dynamic range, as in Capture One: Highlight, Shadow, White and
//! Black, each −100 to 100.
//!
//! Each slider brightens (positive) or darkens (negative) one band of tones,
//! measured in stops from middle grey, by up to 1.5 stops for Highlight and
//! Shadow and 1 stop for White and Black. The bands blend smoothly and are
//! gentle enough that a brighter tone always stays brighter: tones never
//! swap places. The gain multiplies R, G and B alike, so hues are kept.
//!
//! The gain depends on each pixel's own luminance; Capture One also looks
//! at the neighbourhood (local tone mapping), which keeps more contrast in
//! the recovered areas. That is planned as a later version of this tool.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Tool, ToolDescriptor, ToolId};

use crate::common::{luma, slider, smoothstep, stops_from_grey};

/// Id of the highlight slider (tones from about grey to 2.5 stops above).
pub const HIGHLIGHT: &str = "highlight";
/// Id of the shadow slider (tones from about grey to 3 stops below).
pub const SHADOW: &str = "shadow";
/// Id of the white slider (the brightest tones, 1.5 stops above grey and up).
pub const WHITE: &str = "white";
/// Id of the black slider (the darkest tones, 3 stops below grey and down).
pub const BLACK: &str = "black";

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("hdr").expect("constant id is valid"),
    version: 1,
    name: "High dynamic range",
    category: Category::Exposure,
    order: 60,
    starts_on: true,
    params: vec![
        slider(HIGHLIGHT, "Highlight"),
        slider(SHADOW, "Shadow"),
        slider(WHITE, "White"),
        slider(BLACK, "Black"),
    ],
});

/// The gain in stops for a tone `e` stops from grey.
#[must_use]
pub fn gain_stops(e: f32, highlight: f32, shadow: f32, white: f32, black: f32) -> f32 {
    highlight / 100.0 * 1.5 * smoothstep(-0.5, 2.5, e)
        + shadow / 100.0 * 1.5 * (1.0 - smoothstep(-3.0, 0.5, e))
        + white / 100.0 * 0.8 * smoothstep(1.5, 3.5, e)
        + black / 100.0 * (1.0 - smoothstep(-6.0, -3.0, e))
}

/// The high-dynamic-range tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hdr;

impl Tool for Hdr {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let (h, s, w, b) = (params.float(HIGHLIGHT), params.float(SHADOW), params.float(WHITE), params.float(BLACK));
        if [h, s, w, b].iter().all(|v| *v == 0.0) {
            return;
        }
        for p in image.pixels_mut() {
            let Some(e) = stops_from_grey(luma([p[0], p[1], p[2]])) else { continue };
            let k = gain_stops(e, h, s, w, b).exp2();
            for v in &mut p[..3] {
                *v *= k;
            }
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel {
            wgsl: "
    let p = params.v[0];
    let y = dot(c.rgb, vec3<f32>(0.2627, 0.6780, 0.0593));
    if (y <= 1e-6 || all(p == vec4<f32>(0.0))) { return c; }
    let e = log2(y / 0.18);
    let t_h = clamp((e + 0.5) / 3.0, 0.0, 1.0);
    let t_s = clamp((e + 3.0) / 3.5, 0.0, 1.0);
    let t_w = clamp((e - 1.5) / 2.0, 0.0, 1.0);
    let t_b = clamp((e + 6.0) / 3.0, 0.0, 1.0);
    let s = p.x / 100.0 * 1.5 * (t_h * t_h * (3.0 - 2.0 * t_h))
        + p.y / 100.0 * 1.5 * (1.0 - t_s * t_s * (3.0 - 2.0 * t_s))
        + p.z / 100.0 * 0.8 * (t_w * t_w * (3.0 - 2.0 * t_w))
        + p.w / 100.0 * (1.0 - t_b * t_b * (3.0 - 2.0 * t_b));
    return vec4<f32>(c.rgb * exp2(s), c.a);",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_slider_works_on_its_own_band() {
        // Highlight: a tone two stops up moves, a deep shadow does not.
        assert!(gain_stops(2.0, -100.0, 0.0, 0.0, 0.0) < -1.2);
        assert!(gain_stops(-4.0, -100.0, 0.0, 0.0, 0.0).abs() < 1e-6);
        // Shadow: three stops down lifts by 1.5, three up not at all.
        assert!((gain_stops(-3.0, 0.0, 100.0, 0.0, 0.0) - 1.5).abs() < 1e-6);
        assert!(gain_stops(3.0, 0.0, 100.0, 0.0, 0.0).abs() < 1e-6);
        // White only the top, black only the bottom.
        assert!(gain_stops(0.0, 0.0, 0.0, -100.0, 0.0).abs() < 1e-6);
        assert!(gain_stops(-1.0, 0.0, 0.0, 0.0, 100.0).abs() < 1e-6);
        assert!((gain_stops(-7.0, 0.0, 0.0, 0.0, 100.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tones_never_swap_places_even_at_the_extremes() {
        for (h, s, w, b) in [
            (-100.0, 100.0, -100.0, 100.0),
            (100.0, -100.0, 100.0, -100.0),
            (-100.0, -100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0, 100.0),
        ] {
            let mut last = f32::NEG_INFINITY;
            for i in 0..=1200 {
                let e = -9.0 + i as f32 * 0.01;
                let out = e + gain_stops(e, h, s, w, b);
                assert!(out > last, "{h} {s} {w} {b}: reversal at {e}");
                last = out;
            }
        }
    }
}
