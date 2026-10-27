//! Behaviour of an edit session: validation, undo and redo.

use commands::{Edit, EditError, EditSession};
use core_model::{
    GraphError, LensSettings, Link, NodeId, ParamId, ParamValue, ProfileChoice, Recipe, ToolId, WhiteBalance,
};
use tool_api::ToolRegistry;
use tool_exposure::Exposure;

fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Exposure::shared()).expect("exposure registers");
    r
}

fn exposure_tool() -> ToolId {
    ToolId::new("exposure").expect("valid id")
}

fn exposure_param() -> ParamId {
    ParamId::new("exposure").expect("valid id")
}

/// A session with Source → Exposure → Output; returns the exposure node.
fn session_with_exposure(reg: &ToolRegistry) -> (EditSession, NodeId) {
    let mut s = EditSession::new(Recipe::default());
    let source = s.recipe().graph.source();
    s.apply(reg, Edit::InsertSerial { after: source, tool: exposure_tool() }).expect("insert");
    let node = s.recipe().graph.input_link(s.recipe().graph.output(), 0).expect("output fed").from;
    (s, node)
}

fn exposure_of(s: &EditSession, node: NodeId) -> Option<ParamValue> {
    s.recipe().graph.node(node).and_then(|n| n.params.get(&exposure_param()))
}

#[test]
fn undo_after_set_param_restores_the_previous_value_and_redo_reapplies_it() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    let set = Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(1.5) };
    s.apply(&reg, set).expect("set");
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(1.5)));

    assert_eq!(s.undo(), Some("Change exposure".to_owned()));
    assert_eq!(exposure_of(&s, node), None);

    assert_eq!(s.redo(), Some("Change exposure".to_owned()));
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(1.5)));
}

#[test]
fn undo_and_redo_return_exactly_to_the_same_recipe() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let source = s.recipe().graph.source();
    let start = s.recipe().clone();
    s.apply(&reg, Edit::InsertSerial { after: source, tool: exposure_tool() }).expect("insert");
    let after_insert = s.recipe().clone();
    s.undo();
    assert_eq!(s.recipe(), &start);
    s.redo();
    assert_eq!(s.recipe(), &after_insert);
}

#[test]
fn values_out_of_range_are_clamped_before_they_are_stored() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    s.apply(&reg, Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(40.0) }).expect("set");
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(5.0)));
}

#[test]
fn a_slider_drag_is_one_undo_step_that_returns_to_the_value_before_the_drag() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    s.apply(&reg, Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(0.5) }).expect("set");

    let drag = s.begin_gesture();
    for v in [0.6, 0.8, 1.0, 1.2] {
        let edit = Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(v) };
        s.apply_in_gesture(&reg, edit, Some(drag)).expect("drag step");
    }
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(1.2)));

    s.undo();
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(0.5)));
}

#[test]
fn separate_gestures_are_separate_undo_steps() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    for v in [0.5, 1.0] {
        let g = s.begin_gesture();
        let edit = Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(v) };
        s.apply_in_gesture(&reg, edit, Some(g)).expect("set");
    }
    s.undo();
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(0.5)));
}

#[test]
fn a_new_edit_clears_redo() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    s.apply(&reg, Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(1.0) }).expect("set");
    s.undo();
    s.apply(&reg, Edit::SetEnabled { node, enabled: false }).expect("bypass");
    assert_eq!(s.redo_label(), None);
    assert_eq!(s.undo_label(), Some("Bypass node".to_owned()));
}

#[test]
fn refused_edits_change_neither_recipe_nor_history() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    let before = s.recipe().clone();
    let undo_before = s.undo_label();

    let unknown_param = ParamId::new("brightness").expect("valid id");
    assert_eq!(
        s.apply(&reg, Edit::SetParam { node, param: unknown_param.clone(), value: ParamValue::Float(1.0) }),
        Err(EditError::UnknownParam(exposure_tool(), unknown_param))
    );
    let source = s.recipe().graph.source();
    assert_eq!(
        s.apply(&reg, Edit::SetParam { node: source, param: exposure_param(), value: ParamValue::Float(1.0) }),
        Err(EditError::NotAToolNode(source))
    );
    let loop_link = Link { from: node, from_port: 0, to: node, to_port: 0 };
    assert_eq!(s.apply(&reg, Edit::Connect(loop_link)), Err(EditError::Graph(GraphError::Cycle)));
    let curves = ToolId::new("curves").expect("valid id");
    assert_eq!(
        s.apply(&reg, Edit::InsertSerial { after: source, tool: curves.clone() }),
        Err(EditError::UnknownTool(curves))
    );
    assert_eq!(
        s.apply(&reg, Edit::SetWhiteBalance(WhiteBalance::Multipliers([1.0, f32::NAN, 1.0]))),
        Err(EditError::InvalidWhiteBalance)
    );

    assert_eq!(s.recipe(), &before);
    assert_eq!(s.undo_label(), undo_before);
}

#[test]
fn camera_profile_and_lens_corrections_are_undoable_edits() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, Edit::SetCameraProfile(ProfileChoice::Named("  Adobe Standard ".to_owned()))).expect("profile");
    assert_eq!(s.recipe().develop.profile, ProfileChoice::Named("Adobe Standard".to_owned()));
    s.apply(
        &reg,
        Edit::SetLensCorrection(LensSettings { vignetting: false, zoom: Some(1.1), ..LensSettings::default() }),
    )
    .expect("lens");
    assert!(!s.recipe().develop.lens.vignetting);
    assert_eq!(s.undo().as_deref(), Some("Change lens corrections"));
    assert_eq!(s.recipe().develop.lens, LensSettings::default());
    assert_eq!(s.undo().as_deref(), Some("Change camera profile"));
    assert_eq!(s.recipe().develop.profile, ProfileChoice::Builtin);
    assert_eq!(s.redo().as_deref(), Some("Change camera profile"));

    let before = s.recipe().clone();
    assert_eq!(
        s.apply(&reg, Edit::SetCameraProfile(ProfileChoice::Named(" ".to_owned()))),
        Err(EditError::InvalidProfileName)
    );
    let bad_zoom = LensSettings { zoom: Some(f32::NAN), ..LensSettings::default() };
    assert_eq!(s.apply(&reg, Edit::SetLensCorrection(bad_zoom)), Err(EditError::InvalidLensSettings));
    let empty_lens = LensSettings { lens: Some(String::new()), ..LensSettings::default() };
    assert_eq!(s.apply(&reg, Edit::SetLensCorrection(empty_lens)), Err(EditError::InvalidLensSettings));
    assert_eq!(s.recipe(), &before);
}

#[test]
fn removing_a_node_can_be_undone_with_its_parameters() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    s.apply(&reg, Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(-1.0) }).expect("set");
    s.apply(&reg, Edit::RemoveNode { node }).expect("remove");
    assert!(s.recipe().graph.node(node).is_none());
    s.undo();
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(-1.0)));
    let output = s.recipe().graph.output();
    assert_eq!(s.recipe().graph.input_link(output, 0).map(|l| l.from), Some(node));
}

#[test]
fn nothing_to_undo_or_redo_returns_none() {
    let mut s = EditSession::new(Recipe::default());
    assert_eq!(s.undo(), None);
    assert_eq!(s.redo(), None);
}

fn subject_mask() -> core_model::MaskDef {
    let model = core_model::ModelId::new("birefnet_general").expect("valid id");
    let component =
        core_model::MaskComponent::ai("Subject", core_model::MaskOp::Add, core_model::AiTarget::Subject, model);
    core_model::MaskDef { components: vec![component], ..core_model::MaskDef::default() }
}

/// The mask node feeding `node`'s key input.
fn mask_of(s: &EditSession, node: NodeId) -> Option<NodeId> {
    s.recipe().graph.input_link(node, 1).map(|l| l.from)
}

#[test]
fn adding_a_mask_connects_a_new_mask_node_to_the_key_input_and_undo_removes_it() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    let before = s.recipe().graph.clone();
    s.apply(&reg, Edit::AddMask { to: node, def: subject_mask() }).expect("add mask");
    let mask = mask_of(&s, node).expect("key input fed");
    assert_eq!(s.recipe().graph.node(mask).and_then(|n| n.mask.clone()), Some(subject_mask()));
    assert_eq!(s.undo_label().as_deref(), Some("Add mask"));
    s.undo();
    assert_eq!(s.recipe().graph, before);
    s.redo();
    assert_eq!(mask_of(&s, node), Some(mask), "redo restores the same node id");
}

#[test]
fn a_mask_can_only_go_on_a_node_with_a_key_input() {
    let reg = registry();
    let (mut s, _) = session_with_exposure(&reg);
    let output = s.recipe().graph.output();
    let before = s.recipe().graph.clone();
    assert_eq!(s.apply(&reg, Edit::AddMask { to: output, def: subject_mask() }), Err(EditError::NoMaskInput(output)));
    assert_eq!(s.recipe().graph, before);
    assert_eq!(s.undo_label().as_deref(), Some("Add serial node"), "no step recorded");
}

#[test]
fn changing_a_mask_is_undoable_clamped_and_merged_within_a_gesture() {
    let reg = registry();
    let (mut s, node) = session_with_exposure(&reg);
    s.apply(&reg, Edit::AddMask { to: node, def: subject_mask() }).expect("add mask");
    let mask = mask_of(&s, node).expect("fed");
    let def_of = |s: &EditSession| s.recipe().graph.node(mask).and_then(|n| n.mask.clone()).expect("mask node");

    let gesture = s.begin_gesture();
    for feather in [0.01, 0.02, 0.5] {
        let def = core_model::MaskDef { feather, ..subject_mask() };
        s.apply_in_gesture(&reg, Edit::SetMask { node: mask, def }, Some(gesture)).expect("set");
    }
    assert!((def_of(&s).feather - core_model::MAX_FEATHER).abs() < 1e-6, "clamped");
    assert_eq!(s.undo().as_deref(), Some("Change mask"));
    assert_eq!(def_of(&s), subject_mask(), "one undo step for the whole drag");

    assert_eq!(
        s.apply(&reg, Edit::SetMask { node, def: subject_mask() }),
        Err(EditError::NotAMaskNode(node)),
        "a tool node has no mask definition"
    );
}
