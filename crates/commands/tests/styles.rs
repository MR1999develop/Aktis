//! Applying styles (presets) and style brushes.

use commands::{Edit, EditError, EditKind, EditSession};
use core_model::{
    BrushMask, Geometry, LayerId, MaskComponent, MaskDef, MaskOp, ParamId, ParamValue, Params, Recipe, Style,
    StyleTool, ToolId, ToolRef, WhiteBalance,
};
use tool_api::ToolRegistry;
use tool_exposure::Exposure;

fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Exposure::shared()).expect("exposure registers");
    r
}

fn exposure() -> ToolId {
    ToolId::new("exposure").expect("valid id")
}

fn param() -> ParamId {
    ParamId::new("exposure").expect("valid id")
}

fn tool(v: f32) -> StyleTool {
    let mut params = Params::new();
    params.set(param(), ParamValue::Float(v));
    params.set(ParamId::new("no_such_param").expect("valid id"), ParamValue::Float(3.0));
    StyleTool { tool: exposure(), params }
}

fn style(v: f32) -> Style {
    Style { tools: vec![tool(v)], ..Style::default() }
}

fn value(s: &EditSession, layer: LayerId) -> Option<ParamValue> {
    s.recipe().layers.params(&ToolRef::new(layer, exposure())).and_then(|p| p.get(&param()))
}

fn brush_mask() -> MaskDef {
    MaskDef { components: vec![MaskComponent::brush("Brush", MaskOp::Add, BrushMask::default())], ..MaskDef::default() }
}

#[test]
fn a_style_sets_the_background_and_undoes_in_one_step() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let mut st = style(0.75);
    st.white_balance = Some(WhiteBalance::Multipliers([2.0, 1.0, 1.4]));
    st.geometry = Some(Geometry { angle: 1.5, ..Geometry::default() });
    s.apply(&reg, Edit::ApplyStyle { name: "Warm".into(), style: Box::new(st) }).expect("applies");
    assert_eq!(value(&s, LayerId::BACKGROUND), Some(ParamValue::Float(0.75)));
    // Unknown parameters are dropped.
    let params = s.recipe().layers.params(&ToolRef::background(exposure())).expect("tool");
    assert_eq!(params.iter().count(), 1);
    assert_eq!(s.recipe().develop.white_balance, WhiteBalance::Multipliers([2.0, 1.0, 1.4]));
    assert!((s.recipe().geometry.angle - 1.5).abs() < 1e-6);
    let h = s.history();
    assert_eq!(
        h.last().map(|i| (i.label.as_str(), i.kind)),
        Some(("Apply preset \u{201c}Warm\u{201d}", EditKind::Develop))
    );

    s.apply(&reg, Edit::ApplyStyle { name: "Bright".into(), style: Box::new(style(1.25)) }).expect("applies");
    assert_eq!(value(&s, LayerId::BACKGROUND), Some(ParamValue::Float(1.25)));
    s.undo();
    assert_eq!(value(&s, LayerId::BACKGROUND), Some(ParamValue::Float(0.75)));
    s.undo();
    assert_eq!(s.recipe(), &Recipe::default());
}

#[test]
fn a_style_leaves_the_layers_alone() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::AddLayer { name: "Dodge".into(), mask: brush_mask(), tools: vec![tool(0.5)] })
        .expect("brush applies");
    let layer = s.recipe().layers.layers()[0].id;
    s.apply(&reg, Edit::ApplyStyle { name: "Global".into(), style: Box::new(style(-1.0)) }).expect("applies");
    assert_eq!(value(&s, layer), Some(ParamValue::Float(0.5)));
    assert_eq!(value(&s, LayerId::BACKGROUND), Some(ParamValue::Float(-1.0)));
}

#[test]
fn a_style_brush_is_a_named_layer_with_its_tools_behind_its_mask() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::AddLayer { name: " Dodge ".into(), mask: brush_mask(), tools: vec![tool(0.25), tool(0.5)] })
        .expect("applies");
    let layer = &s.recipe().layers.layers()[0];
    assert_eq!((layer.name.as_str(), &layer.mask), ("Dodge", &brush_mask()));
    assert_eq!(value(&s, layer.id), Some(ParamValue::Float(0.5)), "a tool named twice keeps its last values");
    assert_eq!(s.recipe().develop, Recipe::default().develop);
    assert_eq!(s.history().last().map(|i| i.kind), Some(EditKind::Layer));
    s.undo();
    assert_eq!(s.recipe(), &Recipe::default());
}

#[test]
fn styles_with_unknown_tools_are_refused() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let unknown = Style {
        tools: vec![StyleTool { tool: ToolId::new("teleport").expect("valid id"), params: Params::new() }],
        ..Style::default()
    };
    assert!(matches!(
        s.apply(&reg, Edit::ApplyStyle { name: "x".into(), style: Box::new(unknown.clone()) }),
        Err(EditError::UnknownTool(_))
    ));
    assert!(matches!(
        s.apply(&reg, Edit::AddLayer { name: "x".into(), mask: brush_mask(), tools: unknown.tools }),
        Err(EditError::UnknownTool(_))
    ));
    assert!(s.history().is_empty());
}

#[test]
fn style_steps_survive_a_history_rewrite() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::ApplyStyle { name: "A".into(), style: Box::new(style(0.5)) }).expect("applies");
    s.apply(&reg, Edit::AddLayer { name: "B".into(), mask: brush_mask(), tools: vec![tool(0.25)] }).expect("applies");
    // Delete the first step: the brush still applies.
    let report = s.rewrite_history(&reg, &[1]).expect("rewrites");
    assert!(report.skipped.is_empty());
    assert_eq!(s.recipe().layers.layers().len(), 1);
    assert_eq!(value(&s, LayerId::BACKGROUND), None);
}
