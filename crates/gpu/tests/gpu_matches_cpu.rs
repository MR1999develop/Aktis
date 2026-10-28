//! The GPU engine must give the CPU reference's pixels.
//!
//! Without a GPU the test reports it and passes, unless `RNE_REQUIRE_GPU=1`
//! is set (as on machines that must test the GPU path).

use std::sync::Arc;

use commands::{Edit, EditSession};
use core_model::{ImageF32, ParamId, ParamValue, Recipe, Rgba, ToolId, WorkingSpace};
use gpu::{GpuContext, GpuRenderer};
use tool_api::ToolRegistry;
use tool_exposure::Exposure;

fn renderer() -> Option<GpuRenderer> {
    match GpuContext::new() {
        Ok(ctx) => {
            println!("GPU: {}", ctx.adapter_name());
            Some(GpuRenderer::new(Arc::new(ctx)))
        }
        Err(e) if std::env::var_os("RNE_REQUIRE_GPU").is_some() => panic!("GPU required but unavailable: {e}"),
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

/// Source → exposure(+a) → exposure(b) → Output, the second one optionally bypassed.
fn recipe(reg: &ToolRegistry, a: f32, b: f32, second_enabled: bool) -> Recipe {
    let mut s = EditSession::new(Recipe::default());
    let tool = ToolId::new("exposure").expect("id");
    let param = ParamId::new("exposure").expect("id");
    let source = s.recipe().graph.source();
    s.apply(reg, Edit::InsertSerial { after: source, tool: tool.clone(), params: core_model::Params::new() })
        .expect("first");
    let first = s.recipe().graph.input_link(s.recipe().graph.output(), 0).expect("fed").from;
    s.apply(reg, Edit::InsertSerial { after: first, tool, params: core_model::Params::new() }).expect("second");
    let second = s.recipe().graph.input_link(s.recipe().graph.output(), 0).expect("fed").from;
    s.apply(reg, Edit::SetParam { node: first, param: param.clone(), value: ParamValue::Float(a) }).expect("a");
    s.apply(reg, Edit::SetParam { node: second, param, value: ParamValue::Float(b) }).expect("b");
    s.apply(reg, Edit::SetEnabled { node: second, enabled: second_enabled }).expect("enable");
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
fn a_chain_of_exposure_nodes_matches_the_cpu_reference() {
    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(97, 61); // not a multiple of the workgroup size
    for (a, b, enabled) in [(1.0, -0.5, true), (0.0, 0.0, true), (2.5, 3.0, false), (-5.0, 5.0, true)] {
        let r = recipe(&reg, a, b, enabled);
        let on_gpu = gpu.render(&r.graph, &reg, &src).expect("gpu render");
        let on_cpu = render_cpu::render(&r.graph, &reg, &src).expect("cpu render");
        assert_close(&on_gpu.image, &on_cpu.image);
    }
}

#[test]
fn a_one_pixel_image_and_an_empty_graph_work() {
    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(1, 1);
    let r = gpu.render(&Recipe::default().graph, &reg, &src).expect("render");
    assert_eq!(r.image, src);
}

#[test]
fn missing_tools_pass_through_on_the_gpu_too() {
    let Some(gpu) = renderer() else { return };
    let mut recipe = Recipe::default();
    let source = recipe.graph.source();
    let curves = ToolId::new("curves").expect("id");
    recipe.graph.insert_serial(source, curves.clone(), core_model::Params::new()).expect("insert");
    let src = test_image(8, 8);
    let r = gpu.render(&recipe.graph, &ToolRegistry::new(), &src).expect("render");
    assert_eq!(r.missing_tools, vec![curves]);
    assert_eq!(r.image, src);
}

/// A host tool that needs the whole image: mirrors it left to right.
#[derive(Debug)]
struct Mirror(tool_api::ToolDescriptor);

/// A host tool whose engine is missing.
#[derive(Debug)]
struct Broken(tool_api::ToolDescriptor);

fn host_descriptor(id: &str) -> tool_api::ToolDescriptor {
    tool_api::ToolDescriptor {
        id: ToolId::new(id).expect("id"),
        version: 1,
        name: "test host tool",
        category: tool_api::Category::Details,
        stage: tool_api::Stage::Graph,
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
    r.register(Arc::new(Mirror(host_descriptor("mirror")))).expect("mirror registers");
    r.register(Arc::new(Broken(host_descriptor("broken")))).expect("broken registers");
    r
}

/// Source → exposure(+1) → `host` → exposure(−0.5) → Output.
fn host_recipe(reg: &ToolRegistry, host: &str) -> Recipe {
    let mut s = EditSession::new(Recipe::default());
    let exposure = ToolId::new("exposure").expect("id");
    let param = ParamId::new("exposure").expect("id");
    let last = |s: &EditSession| s.recipe().graph.input_link(s.recipe().graph.output(), 0).expect("fed").from;
    let source = s.recipe().graph.source();
    s.apply(reg, Edit::InsertSerial { after: source, tool: exposure.clone(), params: core_model::Params::new() })
        .expect("first");
    let first = last(&s);
    s.apply(reg, Edit::SetParam { node: first, param: param.clone(), value: ParamValue::Float(1.0) }).expect("+1");
    s.apply(
        reg,
        Edit::InsertSerial { after: first, tool: ToolId::new(host).expect("id"), params: core_model::Params::new() },
    )
    .expect("host");
    let host_node = last(&s);
    s.apply(reg, Edit::InsertSerial { after: host_node, tool: exposure, params: core_model::Params::new() })
        .expect("second");
    let second = last(&s);
    s.apply(reg, Edit::SetParam { node: second, param, value: ParamValue::Float(-0.5) }).expect("-0.5");
    s.recipe().clone()
}

#[test]
fn a_host_tool_between_kernels_matches_the_cpu_reference() {
    let Some(gpu) = renderer() else { return };
    let reg = host_registry();
    let src = test_image(97, 61);
    let r = host_recipe(&reg, "mirror");
    let on_gpu = gpu.render(&r.graph, &reg, &src).expect("gpu render");
    let on_cpu = render_cpu::render(&r.graph, &reg, &src).expect("cpu render");
    assert_close(&on_gpu.image, &on_cpu.image);
    assert!(on_gpu.tool_errors.is_empty() && on_cpu.tool_errors.is_empty());
    // the mirror really ran: the first pixel comes from the end of the row
    assert!((on_cpu.image.pixels()[0][0] - src.pixels()[96][0] * 2.0_f32.sqrt()).abs() < 1e-5);
}

#[test]
fn a_host_tool_that_cannot_run_passes_the_image_through_and_is_reported() {
    let reg = host_registry();
    let src = test_image(9, 7);
    let r = host_recipe(&reg, "broken");
    let on_cpu = render_cpu::render(&r.graph, &reg, &src).expect("cpu render");
    assert_eq!(on_cpu.tool_errors.len(), 1);
    assert_eq!(on_cpu.tool_errors[0].1, tool_api::ToolError::Unavailable("model missing".to_owned()));
    let Some(gpu) = renderer() else { return };
    let on_gpu = gpu.render(&r.graph, &reg, &src).expect("gpu render");
    assert_eq!(on_gpu.tool_errors, on_cpu.tool_errors);
    assert_close(&on_gpu.image, &on_cpu.image);
}

#[test]
fn a_masked_node_matches_the_cpu_reference_and_reports_the_same_missing_rasters() {
    use core_model::{AiTarget, MaskComponent, MaskDef, MaskImage, MaskOp, MaskSource, ModelId, RasterKey};

    let Some(gpu) = renderer() else { return };
    let reg = registry();
    let src = test_image(97, 61);

    // Exposure +2 through a mask made of a soft raster (refined on the image)
    // and a component that has not been computed yet.
    let mut recipe = recipe(&reg, 2.0, 0.0, false);
    let first = recipe.graph.links().find(|l| l.from == recipe.graph.source()).expect("source feeds a node").to;
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
    let def = MaskDef { components: vec![computed, pending], feather: 0.02, ..MaskDef::default() };
    let mut s = EditSession::new(recipe);
    s.apply(&reg, Edit::AddMask { to: first, def }).expect("add mask");
    recipe = s.recipe().clone();

    let on_gpu = gpu.render_with_masks(&recipe.graph, &reg, &src, &rasters).expect("gpu render");
    let on_cpu = render_cpu::render_with_masks(&recipe.graph, &reg, &src, &rasters).expect("cpu render");
    assert_close(&on_gpu.image, &on_cpu.image);
    assert_eq!(on_gpu.missing_masks, on_cpu.missing_masks);
    assert_eq!(on_gpu.missing_masks.len(), 1);
    assert_ne!(on_gpu.image, src, "the mask let some of the exposure through");
}
