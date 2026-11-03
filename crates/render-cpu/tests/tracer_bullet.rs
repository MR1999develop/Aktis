//! End to end on the CPU: sensor data → development → layers edited
//! through commands → 8-bit sRGB. The GPU path and file I/O plug into the
//! same steps.

use commands::{Edit, EditSession};
use core_model::{
    BayerPattern, CfaPattern, LayerId, Matrix3, ParamId, ParamValue, Recipe, SensorImage, ToolId, ToolRef, WorkingSpace,
};
use tool_api::ToolRegistry;
use tool_exposure::Exposure;

fn grey_sensor(level: f32) -> SensorImage {
    let (w, h) = (8_u32, 6_u32);
    let data = vec![level; (w * h) as usize];
    SensorImage::new(w, h, data, CfaPattern::Bayer(BayerPattern::Rggb), [1.0; 3], Matrix3::IDENTITY)
        .expect("valid sensor")
}

fn exposure_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Exposure::shared()).expect("exposure registers");
    registry
}

fn exposure(layer: LayerId, stops: f32) -> Edit {
    Edit::SetParam {
        at: ToolRef::new(layer, ToolId::new("exposure").expect("id")),
        param: ParamId::new("exposure").expect("id"),
        value: ParamValue::Float(stops),
    }
}

#[test]
fn a_grey_card_one_stop_under_comes_out_as_middle_grey_after_plus_one_stop() {
    let registry = exposure_registry();

    // Edit the recipe the way the UI or MCP would.
    let mut session = EditSession::new(Recipe::default());
    session.apply(&registry, exposure(LayerId::BACKGROUND, 1.0)).expect("set +1 stop");

    // Develop and render.
    let recipe = session.recipe();
    let developed =
        develop::develop(&grey_sensor(0.09), &recipe.develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&recipe.layers, &registry, &developed);
    assert!(rendered.missing_tools.is_empty());

    let srgb = render_cpu::to_srgb8(&rendered.image);
    assert_eq!(srgb.len(), 8 * 6 * 3);
    // Middle grey shows at half through Film Standard: sRGB 128.
    assert!(srgb.iter().all(|v| (i32::from(*v) - 128).abs() <= 1), "first pixels: {:?}", &srgb[..6]);

    // Undo returns to the unexposed rendering: one stop under grey, in the
    // film curve's toe (sRGB 84).
    session.undo();
    let rendered = render_cpu::render(&session.recipe().layers, &registry, &developed);
    let srgb = render_cpu::to_srgb8(&rendered.image);
    assert!(srgb.iter().all(|v| (i32::from(*v) - 84).abs() <= 1), "first pixels: {:?}", &srgb[..6]);
}

#[test]
fn a_tool_missing_from_this_build_passes_the_image_through_and_is_reported() {
    let registry = ToolRegistry::new(); // no tools at all
    let mut recipe = Recipe::default();
    let curves = ToolId::new("curves").expect("id");
    recipe.layers.background.params_mut(&curves, 0).expect("added");

    let developed =
        develop::develop(&grey_sensor(0.18), &recipe.develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&recipe.layers, &registry, &developed);
    assert_eq!(rendered.missing_tools, vec![curves]);
    assert_eq!(rendered.image, developed);
}

#[test]
fn a_tool_switched_off_passes_the_image_through() {
    let registry = exposure_registry();
    let mut session = EditSession::new(Recipe::default());
    session.apply(&registry, exposure(LayerId::BACKGROUND, 2.0)).expect("set");
    let off =
        Edit::SetToolEnabled { layer: LayerId::BACKGROUND, tool: ToolId::new("exposure").expect("id"), enabled: false };
    session.apply(&registry, off).expect("off");

    let developed =
        develop::develop(&grey_sensor(0.18), &session.recipe().develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&session.recipe().layers, &registry, &developed);
    assert_eq!(rendered.image, developed);
}

#[test]
fn a_masked_layer_brightens_only_inside_its_mask_and_reports_missing_rasters() {
    use core_model::{AiTarget, MaskComponent, MaskDef, MaskImage, MaskOp, MaskSource, ModelId, RasterKey};

    let registry = exposure_registry();
    let mut session = EditSession::new(Recipe::default());

    // The model "selected" the left half of the frame.
    let key = RasterKey::from_bytes([7; 16]);
    let rasters = masks::MemoryRasters::new();
    rasters.insert(key, MaskImage::from_values(2, 1, vec![1.0, 0.0]).expect("2×1 raster"));
    let mut component =
        MaskComponent::ai("Left", MaskOp::Add, AiTarget::Subject, ModelId::new("test_model").expect("id"));
    if let MaskSource::Ai(ai) = &mut component.source {
        ai.raster = Some(key);
    }
    let mask = MaskDef { components: vec![component], refine_edges: false, ..MaskDef::default() };
    session.apply(&registry, Edit::AddLayer { name: "Left".into(), mask, tools: Vec::new() }).expect("add layer");
    let layer = session.recipe().layers.layers()[0].id;
    session.apply(&registry, exposure(layer, 1.0)).expect("+1 stop");

    let developed =
        develop::develop(&grey_sensor(0.09), &session.recipe().develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render_with_masks(&session.recipe().layers, &registry, &developed, &rasters);
    assert!(rendered.missing_masks.is_empty());
    let (w, h) = (developed.width(), developed.height());
    let left = rendered.image.get(0, h / 2).expect("left pixel");
    let right = rendered.image.get(w - 1, h / 2).expect("right pixel");
    let base = developed.get(w - 1, h / 2).expect("source pixel");
    assert!((left[1] - 2.0 * base[1]).abs() < 1e-5, "left doubled: {left:?} vs {base:?}");
    assert!((right[1] - base[1]).abs() < 1e-6, "right untouched: {right:?} vs {base:?}");

    // Without the raster the mask is empty: no effect anywhere, and it is reported.
    let rendered = render_cpu::render(&session.recipe().layers, &registry, &developed);
    assert_eq!(rendered.image, developed);
    assert_eq!(rendered.missing_masks, vec![(layer, 0)]);
}

#[test]
fn a_layer_mixes_in_the_result_of_all_its_tools_once() {
    use core_model::{BrushMask, MaskComponent, MaskDef, MaskOp};

    let registry = exposure_registry();
    let mut session = EditSession::new(Recipe::default());
    // A mask at half opacity everywhere: a brush layer filled, then faded.
    let mask = MaskDef {
        components: vec![MaskComponent::brush("Fill", MaskOp::Add, BrushMask::default())],
        invert: true,
        opacity: 0.5,
        ..MaskDef::default()
    };
    session.apply(&registry, Edit::AddLayer { name: "Half".into(), mask, tools: Vec::new() }).expect("layer");
    let layer = session.recipe().layers.layers()[0].id;
    session.apply(&registry, exposure(layer, 1.0)).expect("+1 stop on the layer");
    session.apply(&registry, exposure(LayerId::BACKGROUND, 1.0)).expect("+1 stop on the Background");

    let developed =
        develop::develop(&grey_sensor(0.09), &session.recipe().develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&session.recipe().layers, &registry, &developed);
    let base = developed.get(0, 0).expect("pixel")[1];
    let got = rendered.image.get(0, 0).expect("pixel")[1];
    // Background ×2, then half of the layer's ×2 mixed in: ×3.
    assert!((got - 3.0 * base).abs() < 1e-5, "{got} vs {base}");
}
