//! Editing the history: reorder and delete steps, undo the rewrite, and
//! see the steps that no longer apply.

use commands::{Edit, EditError, EditKind, EditSession};
use core_model::{LayerId, MaskDef, ParamId, ParamValue, Recipe, ToolId, ToolRef};
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

fn on(layer: LayerId) -> ToolRef {
    ToolRef::new(layer, exposure_tool())
}

fn set(layer: LayerId, v: f32) -> Edit {
    Edit::SetParam { at: on(layer), param: exposure_param(), value: ParamValue::Float(v) }
}

fn add_layer(name: &str) -> Edit {
    Edit::AddLayer { name: name.to_owned(), mask: MaskDef::default(), tools: Vec::new() }
}

fn exposure_of(s: &EditSession, layer: LayerId) -> Option<ParamValue> {
    s.recipe().layers.params(&on(layer)).and_then(|p| p.get(&exposure_param()))
}

/// The top layer.
fn last_layer(s: &EditSession) -> LayerId {
    s.recipe().layers.layers().last().expect("a layer").id
}

/// A layer with three exposure changes: 0.5, 1.0, -0.25.
fn three_changes(reg: &ToolRegistry) -> (EditSession, LayerId) {
    let mut s = EditSession::new(Recipe::default());
    s.apply(reg, add_layer("Sky")).expect("add layer");
    let layer = last_layer(&s);
    for v in [0.5, 1.0, -0.25] {
        s.apply(reg, set(layer, v)).expect("set");
    }
    (s, layer)
}

#[test]
fn the_history_lists_every_step_oldest_first_with_its_value() {
    let reg = registry();
    let (s, _) = three_changes(&reg);
    let h = s.history();
    let labels: Vec<&str> = h.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["Add layer \u{201c}Sky\u{201d}", "Change exposure", "Change exposure", "Change exposure"]);
    let details: Vec<&str> = h.iter().map(|i| i.detail.as_str()).collect();
    assert_eq!(details, ["", "+0.50", "+1", "-0.25"]);
    assert_eq!(h[0].kind, EditKind::Layer);
    assert_eq!(h[1].kind, EditKind::Adjust);
    assert!(h.iter().all(|i| i.skipped.is_none()));
}

#[test]
fn deleting_the_last_change_brings_back_the_previous_value_and_undo_restores_it() {
    let reg = registry();
    let (mut s, layer) = three_changes(&reg);
    let report = s.rewrite_history(&reg, &[0, 1, 2]).expect("rewrite");
    assert_eq!(report.removed, 1);
    assert!(report.skipped.is_empty());
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(1.0)));
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.undo_label().as_deref(), Some("Delete a history step"));

    assert_eq!(s.undo().as_deref(), Some("Delete a history step"));
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(-0.25)));
    assert_eq!(s.history().len(), 4);

    assert_eq!(s.redo().as_deref(), Some("Delete a history step"));
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(1.0)));
}

#[test]
fn reordering_changes_which_value_wins() {
    let reg = registry();
    let (mut s, layer) = three_changes(&reg);
    // Move "-0.25" before "1.0": the last applied is now 1.0.
    s.rewrite_history(&reg, &[0, 1, 3, 2]).expect("rewrite");
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(1.0)));
    assert_eq!(s.undo_label().as_deref(), Some("Reorder history"));
    let details: Vec<String> = s.history().into_iter().map(|i| i.detail).collect();
    assert_eq!(details, ["", "+0.50", "-0.25", "+1"]);
}

#[test]
fn a_change_to_a_layer_whose_creation_was_deleted_stays_visible_as_skipped() {
    let reg = registry();
    let (mut s, layer) = three_changes(&reg);
    let report = s.rewrite_history(&reg, &[1, 2, 3]).expect("rewrite");
    assert_eq!(report.skipped.len(), 3, "{report:?}");
    let h = s.history();
    assert_eq!(h.len(), 3);
    assert!(h.iter().all(|i| i.skipped.is_some()));
    assert!(s.recipe().layers.layer(layer).is_none(), "the layer is gone");
    assert_eq!(s.recipe(), s.base());

    // Undo brings everything back.
    s.undo();
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(-0.25)));
    assert!(s.history().iter().all(|i| i.skipped.is_none()));
}

#[test]
fn later_steps_follow_their_layer_when_an_earlier_layer_is_deleted() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, add_layer("A")).expect("first");
    s.apply(&reg, add_layer("B")).expect("second");
    let second = last_layer(&s);
    s.apply(&reg, set(second, 2.0)).expect("set");

    // Delete the first layer's creation: the second is made again (with a
    // new id) and the change still reaches it.
    let report = s.rewrite_history(&reg, &[1, 2]).expect("rewrite");
    assert_eq!(report.removed, 1);
    assert!(report.skipped.is_empty(), "{report:?}");
    let now = last_layer(&s);
    assert_ne!(now, second);
    assert_eq!(exposure_of(&s, now), Some(ParamValue::Float(2.0)));
}

#[test]
fn background_changes_never_depend_on_layers() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    s.apply(&reg, add_layer("A")).expect("layer");
    s.apply(&reg, set(LayerId::BACKGROUND, 0.75)).expect("set");
    s.rewrite_history(&reg, &[1]).expect("rewrite");
    assert_eq!(exposure_of(&s, LayerId::BACKGROUND), Some(ParamValue::Float(0.75)));
    assert!(s.history().iter().all(|i| i.skipped.is_none()));
}

#[test]
fn a_bad_order_is_refused_and_changes_nothing() {
    let reg = registry();
    let (mut s, layer) = three_changes(&reg);
    assert_eq!(s.rewrite_history(&reg, &[0, 0]), Err(EditError::BadHistoryOrder));
    assert_eq!(s.rewrite_history(&reg, &[0, 9]), Err(EditError::BadHistoryOrder));
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(-0.25)));
    assert_eq!(s.undo_label().as_deref(), Some("Change exposure"));
}

#[test]
fn the_same_order_is_not_a_change() {
    let reg = registry();
    let (mut s, _) = three_changes(&reg);
    let report = s.rewrite_history(&reg, &[0, 1, 2, 3]).expect("rewrite");
    assert_eq!(report.removed, 0);
    assert_eq!(s.undo_label().as_deref(), Some("Change exposure"));
}

#[test]
fn edits_after_a_rewrite_undo_back_through_it() {
    let reg = registry();
    let (mut s, layer) = three_changes(&reg);
    s.rewrite_history(&reg, &[0, 1]).expect("rewrite");
    s.apply(&reg, set(layer, 3.0)).expect("new edit");
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.undo().as_deref(), Some("Change exposure"));
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(0.5)));
    assert_eq!(s.undo().as_deref(), Some("Delete 2 history steps"));
    assert_eq!(exposure_of(&s, layer), Some(ParamValue::Float(-0.25)));
    assert_eq!(s.history().len(), 4);
}

#[test]
fn deleting_everything_returns_to_the_opened_recipe() {
    let reg = registry();
    let (mut s, _) = three_changes(&reg);
    s.rewrite_history(&reg, &[]).expect("rewrite");
    assert!(s.history().is_empty());
    assert_eq!(s.recipe(), s.base());
    assert_eq!(s.undo_label().as_deref(), Some("Delete 4 history steps"));
}
