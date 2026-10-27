//! Exposure: multiplies linear light by `2^stops`, like opening the aperture.
//!
//! Alpha is untouched. Because the data are scene-referred, values above 1
//! are kept; highlight roll-off belongs to the output render, not here.

use std::sync::{Arc, LazyLock};

use core_model::ImageF32;
use tool_api::{
    Category, GpuKernel, ParamDescriptor, ParamId, ParamKind, ResolvedParams, Stage, Tool, ToolDescriptor, ToolId, Unit,
};

/// Id of the exposure parameter, in stops.
pub const EXPOSURE: &str = "exposure";

static DESCRIPTOR: LazyLock<ToolDescriptor> = LazyLock::new(|| ToolDescriptor {
    id: ToolId::new("exposure").expect("constant id is valid"),
    version: 1,
    name: "Exposure",
    category: Category::Exposure,
    stage: Stage::Graph,
    params: vec![ParamDescriptor {
        id: ParamId::new(EXPOSURE).expect("constant id is valid"),
        label: "Exposure",
        kind: ParamKind::Float { min: -5.0, max: 5.0, default: 0.0, step: 0.01, unit: Unit::Stops },
    }],
});

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
        let gain = params.float(EXPOSURE).exp2();
        for p in image.pixels_mut() {
            p[0] *= gain;
            p[1] *= gain;
            p[2] *= gain;
        }
    }

    fn gpu_kernel(&self) -> GpuKernel {
        GpuKernel::PerPixel { wgsl: "return vec4<f32>(c.rgb * exp2(params.v[0].x), c.a);" }
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
        let mut p = Params::new();
        p.set(ParamId::new(EXPOSURE).expect("valid"), ParamValue::Float(stops));
        let r = ResolvedParams::resolve(Exposure.descriptor(), &p);
        let mut img = image(v);
        Exposure.apply_cpu(&mut img, &r);
        img.pixels()[0]
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
