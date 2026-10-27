//! End to end on the CPU: sensor data → development → node graph edited
//! through commands → 8-bit sRGB. The GPU path and file I/O plug into the
//! same steps.

use commands::{Edit, EditSession};
use core_model::{BayerPattern, CfaPattern, Matrix3, ParamId, ParamValue, Recipe, SensorImage, ToolId, WorkingSpace};
use tool_api::ToolRegistry;
use tool_exposure::Exposure;

fn grey_sensor(level: f32) -> SensorImage {
    let (w, h) = (8_u32, 6_u32);
    let data = vec![level; (w * h) as usize];
    SensorImage::new(w, h, data, CfaPattern::Bayer(BayerPattern::Rggb), [1.0; 3], Matrix3::IDENTITY)
        .expect("valid sensor")
}

#[test]
fn a_grey_card_one_stop_under_comes_out_as_middle_grey_after_plus_one_stop() {
    let mut registry = ToolRegistry::new();
    registry.register(Exposure::shared()).expect("exposure registers");

    // Edit the recipe the way the UI or MCP would.
    let mut session = EditSession::new(Recipe::default());
    let source = session.recipe().graph.source();
    session
        .apply(&registry, Edit::InsertSerial { after: source, tool: ToolId::new("exposure").expect("id") })
        .expect("insert exposure");
    let node = session.recipe().graph.input_link(session.recipe().graph.output(), 0).expect("fed").from;
    let param = ParamId::new("exposure").expect("id");
    session.apply(&registry, Edit::SetParam { node, param, value: ParamValue::Float(1.0) }).expect("set +1 stop");

    // Develop and render.
    let recipe = session.recipe();
    let developed =
        develop::develop(&grey_sensor(0.09), &recipe.develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&recipe.graph, &registry, &developed).expect("renders");
    assert!(rendered.missing_tools.is_empty());

    let srgb = render_cpu::to_srgb8(&rendered.image);
    assert_eq!(srgb.len(), 8 * 6 * 3);
    // Linear 0.18 encodes to sRGB 118.
    assert!(srgb.iter().all(|v| (i32::from(*v) - 118).abs() <= 1), "first pixels: {:?}", &srgb[..6]);

    // Undo returns to the unexposed rendering: linear 0.09 → sRGB 84.
    session.undo();
    let rendered = render_cpu::render(&session.recipe().graph, &registry, &developed).expect("renders");
    let srgb = render_cpu::to_srgb8(&rendered.image);
    assert!(srgb.iter().all(|v| (i32::from(*v) - 84).abs() <= 1), "first pixels: {:?}", &srgb[..6]);
}

#[test]
fn a_tool_missing_from_this_build_passes_the_image_through_and_is_reported() {
    let registry = ToolRegistry::new(); // no tools at all
    let mut recipe = Recipe::default();
    let source = recipe.graph.source();
    let curves = ToolId::new("curves").expect("id");
    recipe.graph.insert_serial(source, curves.clone(), core_model::Params::new()).expect("insert");

    let developed =
        develop::develop(&grey_sensor(0.18), &recipe.develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&recipe.graph, &registry, &developed).expect("renders");
    assert_eq!(rendered.missing_tools, vec![curves]);
    assert_eq!(rendered.image, developed);
}

#[test]
fn a_bypassed_node_passes_the_image_through() {
    let mut registry = ToolRegistry::new();
    registry.register(Exposure::shared()).expect("exposure registers");
    let mut session = EditSession::new(Recipe::default());
    let source = session.recipe().graph.source();
    session
        .apply(&registry, Edit::InsertSerial { after: source, tool: ToolId::new("exposure").expect("id") })
        .expect("insert");
    let node = session.recipe().graph.input_link(session.recipe().graph.output(), 0).expect("fed").from;
    let param = ParamId::new("exposure").expect("id");
    session.apply(&registry, Edit::SetParam { node, param, value: ParamValue::Float(2.0) }).expect("set");
    session.apply(&registry, Edit::SetEnabled { node, enabled: false }).expect("bypass");

    let developed =
        develop::develop(&grey_sensor(0.18), &session.recipe().develop, WorkingSpace::LinearRec2020).expect("develops");
    let rendered = render_cpu::render(&session.recipe().graph, &registry, &developed).expect("renders");
    assert_eq!(rendered.image, developed);
}
