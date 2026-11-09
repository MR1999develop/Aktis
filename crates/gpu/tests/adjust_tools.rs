//! Every per-pixel adjustment tool (`tools/adjust`, and Exposure) must give
//! on the GPU the pixels of its CPU reference, at its defaults and at
//! pseudo-random settings.
//!
//! Without a GPU the test reports it and passes, unless `AKTIS_REQUIRE_GPU=1`.

use std::sync::Arc;

use commands::{Edit, EditSession};
use core_model::{ImageF32, ParamValue, Params, Recipe, Rgba, ToolRef, WorkingSpace};
use gpu::{GpuContext, GpuRenderer};
use tool_api::{GpuKernel, ParamKind, ToolRegistry};

fn renderer() -> Option<GpuRenderer> {
    match GpuContext::new() {
        Ok(ctx) => Some(GpuRenderer::new(Arc::new(ctx))),
        Err(e) if std::env::var_os("AKTIS_REQUIRE_GPU").is_some_and(|v| !v.is_empty()) => {
            panic!("GPU required but unavailable: {e}")
        }
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        #[allow(clippy::cast_precision_loss)] // test values
        let v = (self.0 % 10_000) as f32 / 10_000.0;
        v
    }
}

fn image(rng: &mut Rng) -> ImageF32 {
    let pixels: Vec<Rgba> =
        (0..48 * 32).map(|_| [rng.next() * 3.0 - 0.05, rng.next() * 1.5, rng.next() * 0.8, 1.0]).collect();
    ImageF32::from_pixels(48, 32, WorkingSpace::LinearRec2020, pixels).expect("valid image")
}

#[test]
fn every_adjustment_kernel_matches_its_cpu_reference() {
    let Some(gpu) = renderer() else { return };
    let mut reg = ToolRegistry::new();
    reg.register(tool_exposure::Exposure::shared()).expect("exposure");
    for t in tool_adjust::tools() {
        reg.register(t).expect("tool");
    }
    let mut rng = Rng(0x9e37_79b9);
    let src = image(&mut rng);
    for tool in reg.iter() {
        if tool.gpu_kernel() == GpuKernel::Host {
            continue;
        }
        let d = tool.descriptor();
        for case in 0..4 {
            let mut params = Params::new();
            if case > 0 {
                for p in &d.params {
                    if let ParamKind::Float { min, max, .. } = p.kind {
                        params.set(p.id.clone(), ParamValue::Float(min + (max - min) * rng.next()));
                    }
                }
            }
            let mut s = EditSession::new(Recipe::default());
            let values = params.iter().map(|(p, v)| (p.clone(), *v)).collect();
            s.apply(&reg, Edit::SetParams { at: ToolRef::background(d.id.clone()), values }).expect("set");
            let layers = s.recipe().layers.clone();
            let cpu = render_cpu::render(&layers, &reg, &src).image;
            let gpu_img = gpu.render(&layers, &reg, &src).expect("gpu").image;
            for (i, (g, c)) in gpu_img.pixels().iter().zip(cpu.pixels()).enumerate() {
                let scale = c[0].abs().max(c[1].abs()).max(c[2].abs()).max(1.0);
                for k in 0..4 {
                    // Transcendentals differ in the last bits; the colour editor's
                    // rotation cancels large values, hence the scale of the pixel.
                    assert!(
                        (g[k] - c[k]).abs() <= 2e-3 * scale,
                        "{} case {case} pixel {i} channel {k}: gpu {} cpu {}",
                        d.id,
                        g[k],
                        c[k]
                    );
                }
            }
        }
    }
}
