//! Behaviour of an edit session: validation, undo and redo.

use commands::{Edit, EditError, EditSession};
use core_model::{
    LayerError, LayerId, LensSettings, ParamId, ParamValue, ProfileChoice, Recipe, ToolId, ToolRef, WhiteBalance,
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

/// Exposure on the Background.
fn background() -> ToolRef {
    ToolRef::background(exposure_tool())
}

fn set(at: &ToolRef, v: f32) -> Edit {
    Edit::SetParam { at: at.clone(), param: exposure_param(), value: ParamValue::Float(v) }
}

fn exposure_of(s: &EditSession, at: &ToolRef) -> Option<ParamValue> {
    s.recipe().layers.params(at).and_then(|p| p.get(&exposure_param()))
}

#[test]
fn undo_after_set_param_restores_the_previous_value_and_redo_reapplies_it() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let at = background();
    s.apply(&reg, set(&at, 1.5)).expect("set");
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(1.5)));

    assert_eq!(s.undo(), Some("Change exposure".to_owned()));
    assert_eq!(exposure_of(&s, &at), None);
    assert_eq!(s.recipe(), &Recipe::default(), "the tool leaves the layer again");

    assert_eq!(s.redo(), Some("Change exposure".to_owned()));
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(1.5)));
}

#[test]
fn values_out_of_range_are_clamped_before_they_are_stored() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, set(&background(), 40.0)).expect("set");
    assert_eq!(exposure_of(&s, &background()), Some(ParamValue::Float(4.0)));
}

#[test]
fn a_slider_drag_is_one_undo_step_that_returns_to_the_value_before_the_drag() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let at = background();
    s.apply(&reg, set(&at, 0.5)).expect("set");

    let drag = s.begin_gesture();
    for v in [0.6, 0.8, 1.0, 1.2] {
        s.apply_in_gesture(&reg, set(&at, v), Some(drag)).expect("drag step");
    }
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(1.2)));

    s.undo();
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(0.5)));
}

#[test]
fn separate_gestures_are_separate_undo_steps() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let at = background();
    for v in [0.5, 1.0] {
        let g = s.begin_gesture();
        s.apply_in_gesture(&reg, set(&at, v), Some(g)).expect("set");
    }
    s.undo();
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(0.5)));
}

#[test]
fn a_new_edit_clears_redo() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, set(&background(), 1.0)).expect("set");
    s.undo();
    let off = Edit::SetToolEnabled { layer: LayerId::BACKGROUND, tool: exposure_tool(), enabled: false };
    s.apply(&reg, off).expect("off");
    assert_eq!(s.redo_label(), None);
    assert_eq!(s.undo_label(), Some("Turn off exposure".to_owned()));
}

#[test]
fn a_tool_switched_off_keeps_its_values_and_reset_takes_it_out() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let at = background();
    s.apply(&reg, set(&at, 1.0)).expect("set");
    let off = Edit::SetToolEnabled { layer: LayerId::BACKGROUND, tool: exposure_tool(), enabled: false };
    s.apply(&reg, off).expect("off");
    let settings = s.recipe().layers.background.get(&exposure_tool()).expect("still there").clone();
    assert!(!settings.enabled);
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(1.0)));
    s.apply(&reg, Edit::ResetTool { layer: LayerId::BACKGROUND, tool: exposure_tool() }).expect("reset");
    assert!(s.recipe().layers.background.is_empty());
    s.undo();
    assert_eq!(s.recipe().layers.background.get(&exposure_tool()), Some(&settings));
}

#[test]
fn refused_edits_change_neither_recipe_nor_history() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, set(&background(), 0.5)).expect("set");
    let before = s.recipe().clone();
    let undo_before = s.undo_label();

    let unknown_param = ParamId::new("warmth").expect("valid id");
    assert_eq!(
        s.apply(&reg, Edit::SetParam { at: background(), param: unknown_param.clone(), value: ParamValue::Float(1.0) }),
        Err(EditError::UnknownParam(exposure_tool(), unknown_param))
    );
    let nowhere = ToolRef::new(LayerId::from_raw(7), exposure_tool());
    assert_eq!(s.apply(&reg, set(&nowhere, 1.0)), Err(EditError::Layer(LayerError::UnknownLayer(nowhere.layer))));
    let far = background().with_slot(5);
    assert_eq!(s.apply(&reg, set(&far, 1.0)), Err(EditError::Layer(LayerError::NoSuchSlot(far))));
    let curves = ToolId::new("curves").expect("valid id");
    assert_eq!(
        s.apply(
            &reg,
            Edit::SetParam {
                at: ToolRef::background(curves.clone()),
                param: exposure_param(),
                value: ParamValue::Float(1.0)
            }
        ),
        Err(EditError::UnknownTool(curves))
    );
    assert_eq!(s.apply(&reg, Edit::RemoveLayer { layer: LayerId::BACKGROUND }), Err(LayerError::Background.into()));
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

/// A session with one layer masked to the subject; returns the layer.
fn session_with_layer(reg: &ToolRegistry) -> (EditSession, LayerId) {
    let mut s = EditSession::new(Recipe::default());
    s.apply(reg, Edit::AddLayer { name: " Subject ".to_owned(), mask: subject_mask(), tools: Vec::new() })
        .expect("add layer");
    let layer = s.recipe().layers.layers()[0].id;
    (s, layer)
}

#[test]
fn a_new_layer_has_its_mask_and_own_tools_and_undo_removes_it() {
    let reg = registry();
    let (mut s, layer) = session_with_layer(&reg);
    let l = s.recipe().layers.layer(layer).expect("layer");
    assert_eq!((l.name.as_str(), &l.mask, l.enabled), ("Subject", &subject_mask(), true));
    assert_eq!(s.undo_label().as_deref(), Some("Add layer \u{201c}Subject\u{201d}"));
    let at = ToolRef::new(layer, exposure_tool());
    s.apply(&reg, set(&at, 1.0)).expect("set on the layer");
    assert_eq!(exposure_of(&s, &at), Some(ParamValue::Float(1.0)));
    assert_eq!(exposure_of(&s, &background()), None, "the Background is untouched");
    s.undo();
    s.undo();
    assert!(s.recipe().layers.layers().is_empty());
    s.redo();
    assert_eq!(s.recipe().layers.layers()[0].id, layer, "redo restores the same layer id");
}

#[test]
fn layers_are_renamed_moved_switched_off_and_deleted_with_undo() {
    let reg = registry();
    let (mut s, a) = session_with_layer(&reg);
    s.apply(&reg, Edit::AddLayer { name: "B".to_owned(), mask: subject_mask(), tools: Vec::new() }).expect("B");
    let b = s.recipe().layers.layers()[1].id;
    s.apply(&reg, Edit::MoveLayer { layer: b, index: 0 }).expect("move");
    assert_eq!(s.recipe().layers.index_of(b), Some(0));
    s.apply(&reg, Edit::RenameLayer { layer: a, name: "Sky".to_owned() }).expect("rename");
    s.apply(&reg, Edit::SetLayerEnabled { layer: a, enabled: false }).expect("off");
    let l = s.recipe().layers.layer(a).expect("a");
    assert_eq!((l.name.as_str(), l.enabled), ("Sky", false));
    s.apply(&reg, Edit::RemoveLayer { layer: a }).expect("delete");
    assert!(s.recipe().layers.layer(a).is_none());
    s.undo();
    assert_eq!(s.recipe().layers.index_of(a), Some(1), "back in its place");
    s.undo();
    s.undo();
    assert_eq!(s.recipe().layers.layer(a).map(|l| l.name.as_str()), Some("Subject"));
}

#[test]
fn changing_a_mask_is_undoable_clamped_and_merged_within_a_gesture() {
    let reg = registry();
    let (mut s, layer) = session_with_layer(&reg);
    let mask_of = |s: &EditSession| s.recipe().layers.layer(layer).expect("layer").mask.clone();

    let gesture = s.begin_gesture();
    for feather in [0.01, 0.02, 0.5] {
        let mask = core_model::MaskDef { feather, ..subject_mask() };
        s.apply_in_gesture(&reg, Edit::SetLayerMask { layer, mask }, Some(gesture)).expect("set");
    }
    assert!((mask_of(&s).feather - core_model::MAX_FEATHER).abs() < 1e-6, "clamped");
    assert_eq!(s.undo().as_deref(), Some("Change mask"));
    assert_eq!(mask_of(&s), subject_mask(), "one undo step for the whole drag");

    assert_eq!(
        s.apply(&reg, Edit::SetLayerMask { layer: LayerId::BACKGROUND, mask: subject_mask() }),
        Err(LayerError::Background.into()),
        "the Background has no mask"
    );
}

#[test]
fn several_parameters_change_in_one_step_and_merge_within_a_gesture() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let at = background();
    let contrast = ParamId::new("contrast").expect("valid id");
    let two = |a: f32, b: f32| Edit::SetParams {
        at: at.clone(),
        values: vec![(exposure_param(), ParamValue::Float(a)), (contrast.clone(), ParamValue::Float(b))],
    };
    let g = s.begin_gesture();
    s.apply_in_gesture(&reg, two(0.5, 10.0), Some(g)).expect("first");
    s.apply_in_gesture(&reg, two(1.0, 99.0), Some(g)).expect("second");
    s.apply_in_gesture(&reg, two(1.5, 500.0), Some(g)).expect("clamped");
    assert_eq!(s.history().len(), 1, "one undo step");
    assert_eq!(s.undo_label(), Some("Change exposure, contrast".to_owned()));
    let params = s.recipe().layers.params(&at).expect("tool");
    assert_eq!(params.get(&contrast), Some(ParamValue::Float(100.0)));
    s.undo();
    assert_eq!(s.recipe().layers.params(&at), None);
    let unknown = ParamId::new("warmth").expect("valid id");
    assert!(s.apply(&reg, Edit::SetParams { at, values: vec![(unknown, ParamValue::Float(1.0))] }).is_err());
}

#[test]
fn slots_are_added_one_after_the_other() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, set(&background(), 0.5)).expect("slot 0");
    s.apply(&reg, set(&background().with_slot(1), 1.0)).expect("slot 1");
    let slots = s.recipe().layers.background.get(&exposure_tool()).expect("tool").slots().len();
    assert_eq!(slots, 2);
    s.undo();
    assert_eq!(s.recipe().layers.background.get(&exposure_tool()).expect("tool").slots().len(), 1);
}

#[test]
fn geometry_is_one_undo_step_and_is_kept_in_range() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let g = core_model::Geometry { angle: 80.0, ..core_model::Geometry::default() };
    s.apply(&reg, Edit::SetGeometry(g)).expect("geometry");
    assert!((s.recipe().geometry.angle - core_model::MAX_ANGLE).abs() < f32::EPSILON);
    assert_eq!(s.undo_label(), Some("Change crop and straightening".to_owned()));
    s.undo();
    assert!(s.recipe().geometry.is_identity());
}
