//! Applying styles (presets) and style brushes.

use commands::{Edit, EditError, EditKind, EditSession};
use core_model::{
    BrushMask, Geometry, MaskComponent, MaskDef, MaskOp, NodeId, NodeKind, ParamId, ParamValue, Params, PortKind,
    Recipe, Style, StyleNode, ToolId, WhiteBalance,
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

fn style(v: f32) -> Style {
    let mut params = Params::new();
    params.set(param(), ParamValue::Float(v));
    params.set(ParamId::new("no_such_param").expect("valid id"), ParamValue::Float(3.0));
    Style { nodes: vec![StyleNode { tool: exposure(), params }], ..Style::default() }
}

fn exposure_nodes(s: &EditSession) -> Vec<NodeId> {
    let g = &s.recipe().graph;
    core_model::tool_nodes(g)
        .into_iter()
        .filter(|id| g.node(*id).is_some_and(|n| n.kind == NodeKind::Tool(exposure())))
        .collect()
}

fn value(s: &EditSession, node: NodeId) -> Option<ParamValue> {
    s.recipe().graph.node(node).and_then(|n| n.params.get(&param()))
}

fn brush_mask() -> MaskDef {
    MaskDef { components: vec![MaskComponent::brush("Brush", MaskOp::Add, BrushMask::default())], ..MaskDef::default() }
}

#[test]
fn a_style_adds_a_missing_tool_then_sets_it_and_undoes_in_one_step() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let mut st = style(0.75);
    st.white_balance = Some(WhiteBalance::Multipliers([2.0, 1.0, 1.4]));
    st.geometry = Some(Geometry { angle: 1.5, ..Geometry::default() });
    s.apply(&reg, Edit::ApplyStyle { name: "Warm".into(), style: Box::new(st) }).expect("applies");
    let nodes = exposure_nodes(&s);
    assert_eq!(nodes.len(), 1);
    assert_eq!(value(&s, nodes[0]), Some(ParamValue::Float(0.75)));
    // Unknown parameters are dropped.
    let node = s.recipe().graph.node(nodes[0]).expect("node");
    assert_eq!(node.params.iter().count(), 1);
    assert_eq!(s.recipe().develop.white_balance, WhiteBalance::Multipliers([2.0, 1.0, 1.4]));
    assert!((s.recipe().geometry.angle - 1.5).abs() < 1e-6);
    let h = s.history();
    assert_eq!(
        h.last().map(|i| (i.label.as_str(), i.kind)),
        Some(("Apply preset \u{201c}Warm\u{201d}", EditKind::Develop))
    );

    // Applied again it sets the same node instead of adding one.
    s.apply(&reg, Edit::ApplyStyle { name: "Bright".into(), style: Box::new(style(1.25)) }).expect("applies");
    assert_eq!(exposure_nodes(&s), nodes);
    assert_eq!(value(&s, nodes[0]), Some(ParamValue::Float(1.25)));
    s.undo();
    assert_eq!(value(&s, nodes[0]), Some(ParamValue::Float(0.75)));
    s.undo();
    assert_eq!(s.recipe(), &Recipe::default());
}

#[test]
fn a_style_leaves_masked_nodes_alone() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::AddStyleBrush { name: "Dodge".into(), style: Box::new(style(0.5)), mask: brush_mask() })
        .expect("brush applies");
    let brushed = exposure_nodes(&s);
    s.apply(&reg, Edit::ApplyStyle { name: "Global".into(), style: Box::new(style(-1.0)) }).expect("applies");
    let all = exposure_nodes(&s);
    assert_eq!(all.len(), 2);
    assert_eq!(value(&s, brushed[0]), Some(ParamValue::Float(0.5)));
}

#[test]
fn a_style_brush_adds_named_nodes_behind_one_mask() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let mut st = style(0.5);
    st.nodes.push(st.nodes[0].clone());
    st.white_balance = Some(WhiteBalance::AsShot);
    s.apply(&reg, Edit::AddStyleBrush { name: " Dodge ".into(), style: Box::new(st), mask: brush_mask() })
        .expect("applies");
    let g = &s.recipe().graph;
    let nodes = exposure_nodes(&s);
    assert_eq!(nodes.len(), 2);
    let masks: Vec<NodeId> = nodes.iter().map(|n| g.input_link(*n, 1).expect("masked").from).collect();
    assert_eq!(masks[0], masks[1]);
    assert_eq!(g.node(masks[0]).map(|n| n.kind.clone()), Some(NodeKind::Mask));
    assert!(nodes.iter().all(|n| g.node(*n).is_some_and(|x| x.label == "Dodge")));
    assert_eq!(NodeKind::Tool(exposure()).inputs()[1], PortKind::Mask);
    // Only local adjustments are painted.
    assert_eq!(s.recipe().develop, Recipe::default().develop);
    assert_eq!(s.history().last().map(|i| i.kind), Some(EditKind::Mask));
    s.undo();
    assert_eq!(s.recipe(), &Recipe::default());
}

#[test]
fn styles_with_unknown_tools_or_nothing_to_paint_are_refused() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let unknown = Style {
        nodes: vec![StyleNode { tool: ToolId::new("teleport").expect("valid id"), params: Params::new() }],
        ..Style::default()
    };
    assert!(matches!(
        s.apply(&reg, Edit::ApplyStyle { name: "x".into(), style: Box::new(unknown) }),
        Err(EditError::UnknownTool(_))
    ));
    let empty = Style { white_balance: Some(WhiteBalance::AsShot), ..Style::default() };
    assert_eq!(
        s.apply(&reg, Edit::AddStyleBrush { name: "x".into(), style: Box::new(empty), mask: brush_mask() }),
        Err(EditError::EmptyStyle)
    );
    assert!(s.history().is_empty());
}

#[test]
fn style_steps_survive_a_history_rewrite() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::ApplyStyle { name: "A".into(), style: Box::new(style(0.5)) }).expect("applies");
    s.apply(&reg, Edit::AddStyleBrush { name: "B".into(), style: Box::new(style(0.25)), mask: brush_mask() })
        .expect("applies");
    // Delete the first step: the brush still applies.
    let report = s.rewrite_history(&reg, &[1]).expect("rewrites");
    assert!(report.skipped.is_empty());
    assert_eq!(exposure_nodes(&s).len(), 1);
}
