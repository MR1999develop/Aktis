//! Levels, as in Capture One: for the RGB master and for red, green and
//! blue, an input black and white point, a midtone and an output black and
//! white point, on the 0–255 scale of the histogram.
//!
//! Values are encoded with a 2.2 gamma first (as the histogram shows them).
//! Each channel's own levels apply, then the master to all three. Below the
//! input black a value becomes the output black; above the input white it
//! continues in a straight line, so highlights above white (scene-referred
//! data) are not clipped here.

use std::sync::LazyLock;

use core_model::ImageF32;
use tool_api::{Category, GpuKernel, ParamDescriptor, ResolvedParams, Tool, ToolDescriptor, ToolId, Unit};

use crate::common::{float, slider};

/// The four sets of levels, in parameter order: their id prefix and name.
pub const CHANNELS: [(&str, &str); 4] = [("rgb", "RGB"), ("red", "Red"), ("green", "Green"), ("blue", "Blue")];

/// The five values of one set, in parameter order: id suffix and label.
pub const VALUES: [(&str, &str); 5] = [
    ("in_black", "Input black"),
    ("in_white", "Input white"),
    ("midtone", "Midtone"),
    ("out_black", "Output black"),
    ("out_white", "Output white"),
];

/// Id of value `value` (an index into [`VALUES`]) of channel `channel` (into [`CHANNELS`]).
#[must_use]
pub fn param_id(channel: usize, value: usize) -> String {
    format!("{}_{}", CHANNELS[channel].0, VALUES[value].0)
}

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("levels").expect("constant id is valid"),
    version: 1,
    name: "Levels",
    category: Category::Exposure,
    order: 70,
    starts_on: true,
    params: (0..CHANNELS.len())
        .flat_map(|c| (0..VALUES.len()).map(move |v| (c, v)))
        .map(|(c, v)| -> ParamDescriptor {
            let id = param_id(c, v);
            let label = VALUES[v].1;
            match v {
                2 => slider(&id, label),
                1 | 4 => float(&id, label, [0.0, 255.0], 255.0, 1.0, Unit::None),
                _ => float(&id, label, [0.0, 255.0], 0.0, 1.0, Unit::None),
            }
        })
        .collect(),
});

/// One set of levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Set {
    /// Input black point, 0–255.
    pub in_black: f32,
    /// Input white point, 0–255.
    pub in_white: f32,
    /// Midtone, −100 (darker) to 100 (brighter).
    pub midtone: f32,
    /// Output black point, 0–255.
    pub out_black: f32,
    /// Output white point, 0–255.
    pub out_white: f32,
}

impl Set {
    /// The set of channel `channel` (an index into [`CHANNELS`]).
    #[must_use]
    pub fn of(params: &ResolvedParams, channel: usize) -> Self {
        let v = |i: usize| params.float(&param_id(channel, i));
        Self { in_black: v(0), in_white: v(1), midtone: v(2), out_black: v(3), out_white: v(4) }
    }

    /// Whether the set changes nothing.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.in_black == 0.0
            && (self.in_white - 255.0).abs() < 1e-6
            && self.midtone == 0.0
            && self.out_black == 0.0
            && (self.out_white - 255.0).abs() < 1e-6
    }

    /// An encoded value (0–1 for black to white) through these levels.
    #[must_use]
    pub fn apply(&self, x: f32) -> f32 {
        let t = (x * 255.0 - self.in_black) / (self.in_white - self.in_black).max(1.0);
        let g = (-self.midtone / 100.0).exp2();
        let span = self.out_white - self.out_black;
        if t <= 0.0 {
            self.out_black / 255.0
        } else if t <= 1.0 {
            (self.out_black + t.powf(g) * span) / 255.0
        } else {
            (self.out_white + (t - 1.0) * g * span) / 255.0
        }
    }
}

/// The levels tool.
#[derive(Debug, Default, Clone, Copy)]
pub struct Levels;

impl Tool for Levels {
    fn descriptor(&self) -> &ToolDescriptor {
        &DESCRIPTOR
    }

    fn apply_cpu(&self, image: &mut ImageF32, params: &ResolvedParams) {
        let sets: [Set; 4] = std::array::from_fn(|c| Set::of(params, c));
        if sets.iter().all(Set::is_identity) {
            return;
        }
        for p in image.pixels_mut() {
            for (k, v) in p[..3].iter_mut().enumerate() {
                let e = v.max(0.0).powf(1.0 / 2.2);
                let e = sets[0].apply(sets[k + 1].apply(e));
                *v = e.max(0.0).powf(2.2);
            }
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel {
            wgsl: "
    var changed = false;
    for (var i = 0u; i < 20u; i++) {
        let d = select(0.0, 255.0, i % 5u == 1u || i % 5u == 4u);
        if (abs(params.v[i / 4u][i % 4u] - d) > 1e-6) { changed = true; }
    }
    if (!changed) { return c; }
    var e = pow(max(c.rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    for (var step_i = 0u; step_i < 6u; step_i++) {
        let k = step_i % 3u;
        var b = (k + 1u) * 5u;
        if (step_i >= 3u) { b = 0u; }
        let ib = params.v[b / 4u][b % 4u];
        let iw = params.v[(b + 1u) / 4u][(b + 1u) % 4u];
        let mid = params.v[(b + 2u) / 4u][(b + 2u) % 4u];
        let ob = params.v[(b + 3u) / 4u][(b + 3u) % 4u];
        let ow = params.v[(b + 4u) / 4u][(b + 4u) % 4u];
        let t = (e[k] * 255.0 - ib) / max(iw - ib, 1.0);
        let g = exp2(-mid / 100.0);
        var y = ob / 255.0;
        if (t > 1.0) {
            y = (ow + (t - 1.0) * g * (ow - ob)) / 255.0;
        } else if (t > 0.0) {
            y = (ob + pow(t, g) * (ow - ob)) / 255.0;
        }
        e[k] = y;
    }
    return vec4<f32>(pow(max(e, vec3<f32>(0.0)), vec3<f32>(2.2)), c.a);",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(in_black: f32, in_white: f32, midtone: f32, out_black: f32, out_white: f32) -> Set {
        Set { in_black, in_white, midtone, out_black, out_white }
    }

    #[test]
    fn the_default_set_changes_nothing() {
        let s = set(0.0, 255.0, 0.0, 0.0, 255.0);
        assert!(s.is_identity());
        for x in [0.0, 0.25, 0.5, 1.0, 1.7] {
            assert!((s.apply(x) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn input_points_stretch_and_output_points_compress() {
        let s = set(51.0, 204.0, 0.0, 0.0, 255.0);
        assert!(s.apply(0.1).abs() < 1e-6);
        assert!((s.apply(0.5) - 0.5).abs() < 1e-6);
        assert!((s.apply(0.8) - 1.0).abs() < 1e-6);
        let s = set(0.0, 255.0, 0.0, 25.5, 229.5);
        assert!((s.apply(0.0) - 0.1).abs() < 1e-6);
        assert!((s.apply(1.0) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn the_midtone_bends_the_middle_and_keeps_the_ends() {
        let up = set(0.0, 255.0, 100.0, 0.0, 255.0);
        assert!(up.apply(0.5) > 0.7);
        assert!((up.apply(1.0) - 1.0).abs() < 1e-6 && up.apply(0.0).abs() < 1e-6);
        assert!(set(0.0, 255.0, -100.0, 0.0, 255.0).apply(0.5) < 0.3);
    }
}
