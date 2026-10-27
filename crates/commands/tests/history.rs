//! Editing the history: reorder and delete steps, undo the rewrite, and
//! see the steps that no longer apply.

use commands::{Edit, EditError, EditKind, EditSession};
use core_model::{NodeId, ParamId, ParamValue, Recipe, ToolId};
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

fn set(node: NodeId, v: f32) -> Edit {
    Edit::SetParam { node, param: exposure_param(), value: ParamValue::Float(v) }
}

fn exposure_of(s: &EditSession, node: NodeId) -> Option<ParamValue> {
    s.recipe().graph.node(node).and_then(|n| n.params.get(&exposure_param()))
}

/// The node feeding the output (the last of the chain).
fn last_node(s: &EditSession) -> NodeId {
    s.recipe().graph.input_link(s.recipe().graph.output(), 0).expect("output fed").from
}

/// Source → Exposure (node) with three exposure changes: 0.5, 1.0, -0.25.
fn three_changes(reg: &ToolRegistry) -> (EditSession, NodeId) {
    let mut s = EditSession::new(Recipe::default());
    let source = s.recipe().graph.source();
    s.apply(reg, Edit::InsertSerial { after: source, tool: exposure_tool() }).expect("insert");
    let node = last_node(&s);
    for v in [0.5, 1.0, -0.25] {
        s.apply(reg, set(node, v)).expect("set");
    }
    (s, node)
}

#[test]
fn the_history_lists_every_step_oldest_first_with_its_value() {
    let reg = registry();
    let (s, _) = three_changes(&reg);
    let h = s.history();
    let labels: Vec<&str> = h.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["Add serial node", "Change exposure", "Change exposure", "Change exposure"]);
    let details: Vec<&str> = h.iter().map(|i| i.detail.as_str()).collect();
    assert_eq!(details, ["", "+0.50", "+1", "-0.25"]);
    assert_eq!(h[0].kind, EditKind::Node);
    assert_eq!(h[1].kind, EditKind::Adjust);
    assert!(h.iter().all(|i| i.skipped.is_none()));
}

#[test]
fn deleting_the_last_change_brings_back_the_previous_value_and_undo_restores_it() {
    let reg = registry();
    let (mut s, node) = three_changes(&reg);
    let report = s.rewrite_history(&reg, &[0, 1, 2]).expect("rewrite");
    assert_eq!(report.removed, 1);
    assert!(report.skipped.is_empty());
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(1.0)));
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.undo_label().as_deref(), Some("Delete a history step"));

    assert_eq!(s.undo().as_deref(), Some("Delete a history step"));
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(-0.25)));
    assert_eq!(s.history().len(), 4);

    assert_eq!(s.redo().as_deref(), Some("Delete a history step"));
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(1.0)));
}

#[test]
fn reordering_changes_which_value_wins() {
    let reg = registry();
    let (mut s, node) = three_changes(&reg);
    // Move "-0.25" before "1.0": the last applied is now 1.0.
    s.rewrite_history(&reg, &[0, 1, 3, 2]).expect("rewrite");
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(1.0)));
    assert_eq!(s.undo_label().as_deref(), Some("Reorder history"));
    let details: Vec<String> = s.history().into_iter().map(|i| i.detail).collect();
    assert_eq!(details, ["", "+0.50", "-0.25", "+1"]);
}

#[test]
fn a_change_to_a_node_whose_creation_was_deleted_stays_visible_as_skipped() {
    let reg = registry();
    let (mut s, node) = three_changes(&reg);
    let report = s.rewrite_history(&reg, &[1, 2, 3]).expect("rewrite");
    assert_eq!(report.skipped.len(), 3, "{report:?}");
    let h = s.history();
    assert_eq!(h.len(), 3);
    assert!(h.iter().all(|i| i.skipped.is_some()));
    assert!(s.recipe().graph.node(node).is_none(), "the exposure node is gone");
    assert_eq!(s.recipe(), s.base());

    // Undo brings everything back, and a later rewrite can make them apply again.
    s.undo();
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(-0.25)));
    assert!(s.history().iter().all(|i| i.skipped.is_none()));
}

#[test]
fn later_steps_follow_their_node_when_an_earlier_node_is_deleted() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let source = s.recipe().graph.source();
    s.apply(&reg, Edit::InsertSerial { after: source, tool: exposure_tool() }).expect("first");
    let first = last_node(&s);
    s.apply(&reg, Edit::InsertSerial { after: first, tool: exposure_tool() }).expect("second");
    let second = last_node(&s);
    s.apply(&reg, set(second, 2.0)).expect("set");

    // Delete the first insert: the second node is created again (with a new
    // id) and the change still reaches it.
    let report = s.rewrite_history(&reg, &[1, 2]).expect("rewrite");
    assert_eq!(report.removed, 1);
    let skipped: Vec<_> = s.history().into_iter().filter(|i| i.skipped.is_some()).collect();
    // The second insert was placed after the first node, which no longer exists.
    assert_eq!(skipped.len(), 2, "{report:?}");
}

#[test]
fn a_replayed_insert_keeps_later_edits_on_the_new_node() {
    let reg = registry();
    let mut s = EditSession::new(Recipe::default());
    let source = s.recipe().graph.source();
    s.apply(&reg, Edit::InsertSerial { after: source, tool: exposure_tool() }).expect("insert");
    let node = last_node(&s);
    s.apply(&reg, set(node, 0.75)).expect("set 1");
    s.apply(&reg, set(node, 1.25)).expect("set 2");
    // Delete the middle change only: the insert is replayed and may get a new id.
    s.rewrite_history(&reg, &[0, 2]).expect("rewrite");
    let now = last_node(&s);
    assert_eq!(exposure_of(&s, now), Some(ParamValue::Float(1.25)));
    assert!(s.history().iter().all(|i| i.skipped.is_none()));
}

#[test]
fn a_bad_order_is_refused_and_changes_nothing() {
    let reg = registry();
    let (mut s, node) = three_changes(&reg);
    assert_eq!(s.rewrite_history(&reg, &[0, 0]), Err(EditError::BadHistoryOrder));
    assert_eq!(s.rewrite_history(&reg, &[0, 9]), Err(EditError::BadHistoryOrder));
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(-0.25)));
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
    let (mut s, node) = three_changes(&reg);
    s.rewrite_history(&reg, &[0, 1]).expect("rewrite");
    s.apply(&reg, set(node, 3.0)).expect("new edit");
    assert_eq!(s.history().len(), 3);
    assert_eq!(s.undo().as_deref(), Some("Change exposure"));
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(0.5)));
    assert_eq!(s.undo().as_deref(), Some("Delete 2 history steps"));
    assert_eq!(exposure_of(&s, node), Some(ParamValue::Float(-0.25)));
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
