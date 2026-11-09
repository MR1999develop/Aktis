//! The GPU engine must give the CPU reference's pixels.
//!
//! Without a GPU the test reports it and passes, unless `AKTIS_REQUIRE_GPU=1`
//! is set (as on machines that must test the GPU path).

use std::sync::Arc;

use commands::{Edit, EditSession};
use core_model::{
    BrushMask, ImageF32, LayerId, MaskComponent, MaskDef, MaskOp, ParamId, ParamValue, Recipe, Rgba, ToolId, ToolRef,
    WorkingSpace,
};
use gpu::{GpuContext, GpuRenderer};
use tool_api::ToolRegistry;
use tool_exposure::Exposure;

fn renderer() -> Option<GpuRenderer> {
    match GpuContext::new() {
        Ok(ctx) => {
            println!("GPU: {}", ctx.adapter_name());
            Some(GpuRenderer::new(Arc::new(ctx)))
        }
        Err(e) if std::env::var_os("AKTIS_REQUIRE_GPU").is_some_and(|v| !v.is_empty()) => {
            panic!("GPU required but unavailable: {e}")
        }
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// Deterministic pseudo-random image, including negatives and values above 1.
fn test_image(width: u32, height: u32) -> ImageF32 {
    let mut state: u32 = 0x1234_5678;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        #[allow(clippy::cast_precision_loss)] // only used to make test values
        let v = (state % 10_000) as f32 / 4_000.0 - 0.25;
        v
    };
    let pixels: Vec<Rgba> = (0..width * height).map(|_| [next(), next(), next(), 1.0]).collect();
    ImageF32::from_pixels(width, height, WorkingSpace::LinearRec2020, pixels).expect("valid image")
}

fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Exposure::shared()).expect("exposure registers");
    r
}

fn set_exposure(s: &mut EditSession, reg: &ToolRegistry, at: ToolRef, v: f32) {
    let param = ParamId::new("exposure").expect("id");
    s.apply(reg, Edit::SetParam { at, param, value: ParamValue::Float(v) }).expect("set");
}

fn exposure_at(layer: LayerId) -> ToolRef {
    ToolRef::new(layer, ToolId::new("exposure").expect("id"))
}

/// Everywhere at `opacity`: an empty brush, inverted.
fn everywhere(opacity: f32) -> MaskDef {
    MaskDef {
        components: vec![MaskComponent::brush("Fill", MaskOp::Add, BrushMask::default())],
        invert: true,
        opacity,
        ..MaskDef::default()
    }
}

/// Exposure +a then b (two slots) on the Background, the tool on or off.
fn recipe(reg: &ToolRegistry, a: f32, b: f32, enabled: bool) -> Recipe {
    let mut s = EditSession::new(Recipe::default());
    set_exposure(&mut s, reg, exposure_at(LayerId::BACKGROUND), a);
    set_exposure(&mut s, reg, exposure_at(LayerId::BACKGROUND).with_slot(1), b);
    let tool = ToolId::new("exposure").expect("id");
    s.apply(reg, Edit::SetToolEnabled { layer: LayerId::BACKGROUND, tool, enabled }).expect("enable");
    s.recipe().clone()
}

fn assert_close(gpu: &ImageF32, cpu: &ImageF32) {
    assert_eq!((gpu.width(), gpu.height()), (cpu.width(), cpu.height()));
    for (i, (g, c)) in gpu.pixels().iter().zip(cpu.pixels()).enumerate() {
        for k in 0..4 {
            // exp2 on the GPU may differ in the last bits: relative tolerance.
            let tol = 1e-5 * c[k].abs().max(1.0);
            assert!((g[k] - c[k]).abs() <= tol, "pixel {i} channel {k}: gpu {} cpu {}", g[k], c[k]);
        }
    }
}

#[test]
fn a_chain_of_exposures_matches_the_cpu_reference() {
    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(97, 61); // not a multiple of the workgroup size
    for (a, b, enabled) in [(1.0, -0.5, true), (0.0, 0.0, true), (2.5, 3.0, false), (-5.0, 5.0, true)] {
        let r = recipe(&reg, a, b, enabled);
        let on_gpu = gpu.render(&r.layers, &reg, &src).expect("gpu render");
        let on_cpu = render_cpu::render(&r.layers, &reg, &src);
        assert_close(&on_gpu.image, &on_cpu.image);
    }
}

#[test]
fn layers_mix_in_on_the_gpu_as_on_the_cpu() {
    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(97, 61);
    let mut s = EditSession::new(recipe(&reg, 0.5, 0.0, true));
    for (opacity, v) in [(0.5, 1.0), (0.25, -2.0)] {
        s.apply(&reg, Edit::AddLayer { name: "L".into(), mask: everywhere(opacity), tools: Vec::new() })
            .expect("layer");
        let layer = s.recipe().layers.layers().last().expect("layer").id;
        set_exposure(&mut s, &reg, exposure_at(layer), v);
    }
    let r = s.recipe();
    let on_gpu = gpu.render(&r.layers, &reg, &src).expect("gpu render");
    let on_cpu = render_cpu::render(&r.layers, &reg, &src);
    assert_close(&on_gpu.image, &on_cpu.image);
    assert_ne!(on_cpu.image, src);
}

#[test]
fn a_one_pixel_image_and_no_edits_work() {
    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(1, 1);
    let r = gpu.render(&Recipe::default().layers, &reg, &src).expect("render");
    assert_eq!(r.image, src);
}

#[test]
fn missing_tools_pass_through_on_the_gpu_too() {
    let Some(gpu) = renderer() else { return };
    let mut recipe = Recipe::default();
    let curves = ToolId::new("curves").expect("id");
    recipe.layers.background.params_mut(&curves, 0).expect("added");
    let src = test_image(8, 8);
    let r = gpu.render(&recipe.layers, &ToolRegistry::new(), &src).expect("render");
    assert_eq!(r.missing_tools, vec![curves]);
    assert_eq!(r.image, src);
}

/// A host tool that needs the whole image: mirrors it left to right.
#[derive(Debug)]
struct Mirror(tool_api::ToolDescriptor);

/// A host tool whose engine is missing.
#[derive(Debug)]
struct Broken(tool_api::ToolDescriptor);

fn host_descriptor(id: &str, order: u16) -> tool_api::ToolDescriptor {
    tool_api::ToolDescriptor {
        id: ToolId::new(id).expect("id"),
        version: 1,
        name: "test host tool",
        category: tool_api::Category::Details,
        order,
        starts_on: true,
        params: vec![],
    }
}

impl tool_api::Tool for Mirror {
    fn descriptor(&self) -> &tool_api::ToolDescriptor {
        &self.0
    }
    fn apply_cpu(&self, image: &mut ImageF32, _params: &tool_api::ResolvedParams) {
        let w = image.width() as usize;
        for row in image.pixels_mut().chunks_exact_mut(w) {
            row.reverse();
        }
    }
    fn gpu_kernel(&self) -> tool_api::GpuKernel {
        tool_api::GpuKernel::Host
    }
}

impl tool_api::Tool for Broken {
    fn descriptor(&self) -> &tool_api::ToolDescriptor {
        &self.0
    }
    fn apply_cpu(&self, _image: &mut ImageF32, _params: &tool_api::ResolvedParams) {}
    fn apply(&self, _image: &mut ImageF32, _params: &tool_api::ResolvedParams) -> Result<(), tool_api::ToolError> {
        Err(tool_api::ToolError::Unavailable("model missing".to_owned()))
    }
    fn gpu_kernel(&self) -> tool_api::GpuKernel {
        tool_api::GpuKernel::Host
    }
}

fn host_registry() -> ToolRegistry {
    let mut r = registry();
    r.register(Arc::new(Mirror(host_descriptor("mirror", 60)))).expect("mirror registers");
    r.register(Arc::new(Broken(host_descriptor("broken", 61)))).expect("broken registers");
    r
}

/// Background: exposure(+1) then `host`; above it a layer everywhere with
/// exposure(−0.5), and `host` too when `in_layer`.
fn host_recipe(reg: &ToolRegistry, host: &str, in_layer: bool) -> Recipe {
    let mut s = EditSession::new(Recipe::default());
    set_exposure(&mut s, reg, exposure_at(LayerId::BACKGROUND), 1.0);
    let host = ToolId::new(host).expect("id");
    s.apply(reg, Edit::SetToolEnabled { layer: LayerId::BACKGROUND, tool: host.clone(), enabled: true }).expect("host");
    s.apply(reg, Edit::AddLayer { name: "L".into(), mask: everywhere(0.75), tools: Vec::new() }).expect("layer");
    let layer = s.recipe().layers.layers()[0].id;
    set_exposure(&mut s, reg, exposure_at(layer), -0.5);
    if in_layer {
        s.apply(reg, Edit::SetToolEnabled { layer, tool: host, enabled: true }).expect("host in layer");
    }
    s.recipe().clone()
}

#[test]
fn a_host_tool_between_kernels_matches_the_cpu_reference() {
    let Some(gpu) = renderer() else { return };
    let reg = host_registry();
    let src = test_image(97, 61);
    for in_layer in [false, true] {
        let r = host_recipe(&reg, "mirror", in_layer);
        let on_gpu = gpu.render(&r.layers, &reg, &src).expect("gpu render");
        let on_cpu = render_cpu::render(&r.layers, &reg, &src);
        assert_close(&on_gpu.image, &on_cpu.image);
        assert!(on_gpu.tool_errors.is_empty() && on_cpu.tool_errors.is_empty());
    }
    // the mirror really ran: the first pixel comes from the end of the row,
    // ×2, then a quarter of it kept and three quarters at −0.5 stop.
    let r = host_recipe(&reg, "mirror", false);
    let factor = 2.0 * (0.25 + 0.75 / 2.0_f32.sqrt());
    let on_cpu = render_cpu::render(&r.layers, &reg, &src);
    assert!((on_cpu.image.pixels()[0][0] - src.pixels()[96][0] * factor).abs() < 1e-5);
}

#[test]
fn a_host_tool_that_cannot_run_passes_the_image_through_and_is_reported() {
    let reg = host_registry();
    let src = test_image(9, 7);
    let r = host_recipe(&reg, "broken", true);
    let on_cpu = render_cpu::render(&r.layers, &reg, &src);
    assert_eq!(on_cpu.tool_errors.len(), 2, "on the Background and on the layer");
    assert_eq!(on_cpu.tool_errors[0].1, tool_api::ToolError::Unavailable("model missing".to_owned()));
    let Some(gpu) = renderer() else { return };
    let on_gpu = gpu.render(&r.layers, &reg, &src).expect("gpu render");
    assert_eq!(on_gpu.tool_errors, on_cpu.tool_errors);
    assert_close(&on_gpu.image, &on_cpu.image);
}

#[test]
fn a_masked_layer_matches_the_cpu_reference_and_reports_the_same_missing_rasters() {
    use core_model::{AiTarget, MaskComponent, MaskDef, MaskImage, MaskOp, MaskSource, ModelId, RasterKey};

    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(97, 61);

    // Exposure +2 through a mask made of a soft raster (refined on the image)
    // and a component that has not been computed yet.
    let rasters = masks::MemoryRasters::new();
    let key = RasterKey::from_bytes([3; 16]);
    let values: Vec<f32> = (0..12).map(|i| (i % 4) as f32 / 3.0).collect();
    rasters.insert(key, MaskImage::from_values(4, 3, values).expect("4×3 raster"));
    let model = ModelId::new("test_model").expect("id");
    let mut computed = MaskComponent::ai("Computed", MaskOp::Add, AiTarget::Subject, model.clone());
    if let MaskSource::Ai(ai) = &mut computed.source {
        ai.raster = Some(key);
    }
    let pending = MaskComponent::ai("Pending", MaskOp::Add, AiTarget::Subject, model);
    let mask = MaskDef { components: vec![computed, pending], feather: 0.02, ..MaskDef::default() };
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::AddLayer { name: "Subject".into(), mask, tools: Vec::new() }).expect("add layer");
    let layer = s.recipe().layers.layers()[0].id;
    set_exposure(&mut s, &reg, exposure_at(layer), 2.0);
    let recipe = s.recipe().clone();

    let on_gpu = gpu.render_with_masks(&recipe.layers, &reg, &src, &rasters).expect("gpu render");
    let on_cpu = render_cpu::render_with_masks(&recipe.layers, &reg, &src, &rasters);
    assert_close(&on_gpu.image, &on_cpu.image);
    assert_eq!(on_gpu.missing_masks, on_cpu.missing_masks);
    assert_eq!(on_gpu.missing_masks.len(), 1);
    assert_ne!(on_gpu.image, src, "the mask let some of the exposure through");
}
