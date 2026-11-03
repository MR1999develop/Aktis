//! Vignetting, as in Capture One: darkens (or lightens) the image towards
//! its edges.
//!
//! The distance from the centre is measured so that the corners are at 1
//! (an ellipse following the frame); with Roundness at 100 it becomes a
//! circle. The change starts at Midpoint and blends over Feather, reaching
//! Amount × 2 stops at the corners for an Amount of ±100.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{choice, float, slider, smoothstep};

/// Id of the amount (−100 darker to 100 lighter).
pub const AMOUNT: &str = "amount";
/// Id of where the change starts, 0 (centre) to 100 (corners).
pub const MIDPOINT: &str = "midpoint";
/// Id of how gradually it starts, 0–100.
pub const FEATHER: &str = "feather";
/// Id of the shape, 0 (follows the frame) to 100 (a circle), for the elliptic method.
pub const ROUNDNESS: &str = "roundness";
/// Id of the method (an index into [`METHODS`]).
pub const METHOD: &str = "method";

/// The methods, as Capture One names them: an ellipse following the frame
/// (its roundness adjustable), a circle reaching the corners, or a circle
/// touching the frame's shorter sides (the corners fully changed).
pub const METHODS: [&str; 3] = ["Elliptic", "Circular", "Circular crop"];

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("vignetting").expect("constant id is valid"),
    version: 2,
    name: "Vignetting",
    category: Category::Lens,
    order: 150,
    params: vec![
        slider(AMOUNT, "Amount"),
        float(MIDPOINT, "Midpoint", [0.0, 100.0], 50.0, 1.0, Unit::None),
        float(FEATHER, "Feather", [0.0, 100.0], 50.0, 1.0, Unit::None),
        float(ROUNDNESS, "Roundness", [0.0, 100.0], 0.0, 1.0, Unit::None),
        choice(METHOD, "Method", &METHODS, 0),
    ],
});

/// The gain in stops at `uv` (0–1 from the top-left corner) of an image
/// `aspect` = width / height.
#[must_use]
pub fn gain_stops(uv: [f32; 2], aspect: f32, amount: f32, midpoint: f32, feather: f32, roundness: f32) -> f32 {
    gain_stops_with(uv, aspect, [amount, midpoint, feather, roundness], 0)
}

/// [`gain_stops`] for a method (an index into [`METHODS`]); `p` holds
/// amount, midpoint, feather and roundness.
#[must_use]
pub fn gain_stops_with(uv: [f32; 2], aspect: f32, p: [f32; 4], method: u32) -> f32 {
    let [amount, midpoint, feather, roundness] = p;
    let mut q = [(uv[0] - 0.5) * 2.0, (uv[1] - 0.5) * 2.0];
    let r = if method == 0 { roundness / 100.0 } else { 1.0 };
    let reach = if method == 2 { 1.0 } else { std::f32::consts::SQRT_2 };
    if aspect >= 1.0 {
        q[0] *= 1.0 + (aspect - 1.0) * r;
    } else {
        q[1] *= 1.0 + (1.0 / aspect - 1.0) * r;
    }
    let d = q[0].hypot(q[1]) / reach;
    let mid = 0.2 + 0.8 * midpoint / 100.0;
    let f = (feather / 100.0).max(0.01) * 0.5;
    amount / 100.0 * 2.0 * smoothstep(mid - f, mid + f, d)
}

/// The vignetting tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Vignetting;

impl Tool for Vignetting {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    #[allow(clippy::cast_precision_loss)] // image sizes
    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let amount = params.float(AMOUNT);
        if amount == 0.0 {
            return;
        }
        let (mid, feather, round) = (params.float(MIDPOINT), params.float(FEATHER), params.float(ROUNDNESS));
        let method = params.choice(METHOD);
        let (w, h) = (image.width() as usize, image.height() as usize);
        let aspect = w as f32 / h.max(1) as f32;
        for (i, p) in image.pixels_mut().iter_mut().enumerate() {
            let uv = [((i % w) as f32 + 0.5) / w as f32, ((i / w) as f32 + 0.5) / h as f32];
            let k = gain_stops_with(uv, aspect, [amount, mid, feather, round], method).exp2();
            for v in &mut p[..3] {
                *v *= k;
            }
        }
    }

    /// Version 1 had no method (it was Elliptic): its parameters are valid as they are.
    fn migrate(&self, from_version: u32, params: tool_api::Params) -> Result<tool_api::Params, tool_api::ToolError> {
        match from_version {
            1 | 2 => Ok(params),
            found => Err(tool_api::ToolError::UnsupportedVersion { found, current: 2 }),
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel {
            wgsl: "
    let p = params.v[0];
    if (p.x == 0.0) { return c; }
    let aspect = f32(params.n.z) / max(f32(params.n.w), 1.0);
    var q = (uv - vec2<f32>(0.5)) * 2.0;
    let m = u32(params.v[1].x + 0.5);
    var r = p.w / 100.0;
    if (m != 0u) { r = 1.0; }
    if (aspect >= 1.0) { q.x = q.x * (1.0 + (aspect - 1.0) * r); } else { q.y = q.y * (1.0 + (1.0 / aspect - 1.0) * r); }
    var reach = 1.41421356;
    if (m == 2u) { reach = 1.0; }
    let d = length(q) / reach;
    let mid = 0.2 + 0.8 * p.y / 100.0;
    let f = max(p.z / 100.0, 0.01) * 0.5;
    let t = clamp((d - (mid - f)) / (2.0 * f), 0.0, 1.0);
    let s = p.x / 100.0 * 2.0 * (t * t * (3.0 - 2.0 * t));
    return vec4<f32>(c.rgb * exp2(s), c.a);",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_centre_is_untouched_and_the_corners_get_the_amount() {
        assert!(gain_stops([0.5, 0.5], 1.5, -100.0, 50.0, 50.0, 0.0).abs() < 1e-6);
        assert!((gain_stops([0.0, 0.0], 1.5, -100.0, 50.0, 50.0, 0.0) + 2.0).abs() < 1e-5);
        assert!((gain_stops([1.0, 1.0], 1.5, 50.0, 50.0, 50.0, 0.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn circular_crop_changes_the_corners_fully_and_earlier() {
        let p = [-100.0, 50.0, 50.0, 0.0];
        assert!((gain_stops_with([0.0, 0.0], 1.5, p, 2) + 2.0).abs() < 1e-5);
        let side = |m| gain_stops_with([0.25, 0.5], 1.5, p, m);
        assert!(side(2) < side(1), "{} {}", side(2), side(1));
    }

    #[test]
    fn roundness_makes_a_circle_on_a_wide_frame() {
        // The middle of the long edge is nearer the centre than the middle
        // of the short edge in a circle, but not when following the frame.
        let follow = gain_stops([0.0, 0.5], 2.0, -100.0, 50.0, 50.0, 0.0);
        let top = gain_stops([0.5, 0.0], 2.0, -100.0, 50.0, 50.0, 0.0);
        assert!((follow - top).abs() < 1e-6);
        let round_side = gain_stops([0.0, 0.5], 2.0, -100.0, 50.0, 50.0, 100.0);
        let round_top = gain_stops([0.5, 0.0], 2.0, -100.0, 50.0, 50.0, 100.0);
        assert!(round_side < round_top, "{round_side} {round_top}");
    }
}
