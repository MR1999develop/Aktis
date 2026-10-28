//! Saving a history and rebuilding the session from it: the same recipe,
//! the same steps, Undo and Redo where they were.

use commands::{Edit, EditSession, SavedHistory};
use core_model::{NodeId, ParamId, ParamValue, Params, Recipe, ToolId};
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

fn set(node: NodeId, v: f32) -> Edit {
    Edit::SetParam { node, param: ParamId::new("exposure").expect("valid id"), value: ParamValue::Float(v) }
}

/// Source → Exposure with three changes, the last one undone.
fn edited(reg: &ToolRegistry) -> (EditSession, NodeId) {
    let mut s = EditSession::new(Recipe::default());
    let source = s.recipe().graph.source();
    s.apply(reg, Edit::InsertSerial { after: source, tool: exposure_tool(), params: Params::new() }).expect("insert");
    let node = s.recipe().graph.input_link(s.recipe().graph.output(), 0).expect("fed").from;
    for v in [0.5, 1.0, -0.25] {
        s.apply(reg, set(node, v)).expect("set");
    }
    s.undo();
    (s, node)
}

#[test]
fn a_restored_session_has_the_same_recipe_history_undo_and_redo() {
    let reg = registry();
    let (s, _) = edited(&reg);
    let saved = s.saved_history();
    assert_eq!(saved.steps.len(), 3);
    assert_eq!(saved.redo.len(), 1);
    let mut back = EditSession::restore(&reg, s.recipe(), saved.clone()).expect("restores");
    assert_eq!(back.recipe(), s.recipe());
    assert_eq!(back.history(), s.history());
    assert_eq!(back.undo_label(), s.undo_label());
    assert_eq!(back.redo_label(), s.redo_label());
    assert_eq!(back.saved_history(), saved, "saving again gives the same history");

    let mut original = s;
    assert_eq!(back.redo(), original.redo());
    assert_eq!(back.recipe(), original.recipe());
    for _ in 0..4 {
        assert_eq!(back.undo(), original.undo());
        assert_eq!(back.recipe(), original.recipe());
    }
    assert_eq!(back.recipe(), &Recipe::default());
}

#[test]
fn a_history_that_does_not_lead_to_the_recipe_is_not_restored() {
    let reg = registry();
    let (s, _) = edited(&reg);
    assert!(EditSession::restore(&reg, &Recipe::default(), s.saved_history()).is_none());
    // A build without the tool cannot replay the steps.
    assert!(EditSession::restore(&ToolRegistry::new(), s.recipe(), s.saved_history()).is_none());
}

#[test]
fn steps_that_no_longer_apply_stay_skipped_with_their_reason() {
    let reg = registry();
    let (mut s, _) = edited(&reg);
    s.redo();
    // Deleting the node's creation leaves the three changes without their node.
    s.rewrite_history(&reg, &[1, 2, 3]).expect("rewrite");
    let before = s.history();
    assert!(before.iter().all(|h| h.skipped.is_some()));
    let saved = s.saved_history();
    assert!(saved.redo.is_empty(), "a rewrite is not kept in the redo list");
    let back = EditSession::restore(&reg, s.recipe(), saved).expect("restores");
    assert_eq!(back.history(), before);
}

#[test]
fn redo_edits_that_no_longer_apply_are_dropped() {
    let reg = registry();
    let (s, node) = edited(&reg);
    let mut saved = s.saved_history();
    saved.redo.insert(0, set(NodeId::from_raw(99), 1.0));
    let back = EditSession::restore(&reg, s.recipe(), saved).expect("restores");
    assert_eq!(back.redo_label(), None);
    let saved = SavedHistory { redo: vec![set(node, 2.0), set(NodeId::from_raw(99), 1.0)], ..s.saved_history() };
    let mut back = EditSession::restore(&reg, s.recipe(), saved).expect("restores");
    assert!(back.redo().is_some());
    assert_eq!(back.redo(), None, "the edit on a missing node was dropped");
}
