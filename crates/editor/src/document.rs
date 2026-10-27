//! An open image: its developed pixels, its recipe and the edit history.
//!
//! Every change goes through [`commands::EditSession`], so the window gets
//! the same validation and undo as the command line and, later, MCP. Each
//! change bumps [`Document::generation`]; renders carry the generation they
//! were made for, so late results are recognised and dropped.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use commands::{Edit, EditError, EditSession, GestureId};
use core_model::{GraphError, ImageF32, NodeId, NodeKind, Orientation, ParamId, ParamValue, Params, Recipe, ToolId};
use tool_api::{ParamDescriptor, ToolRegistry};

/// Tools placed in the graph of a newly opened image, in order, when this build has them.
pub const STARTING_TOOLS: &[&str] = &["exposure"];

/// A decoded and developed image, as the open job delivers it.
#[derive(Debug, Clone)]
pub struct Opened {
    /// The RAW file.
    pub path: PathBuf,
    /// "Maker Model".
    pub camera: String,
    /// How to turn the image upright.
    pub orientation: Orientation,
    /// Full-size developed image (linear, working space).
    pub developed: Arc<ImageF32>,
    /// Smaller copy for fast previews (may be the same image when small).
    pub preview: Arc<ImageF32>,
    /// `false` when the file had no white balance.
    pub has_camera_white_balance: bool,
}

/// One parameter of a node, ready to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamRow {
    /// Type, range, label.
    pub descriptor: ParamDescriptor,
    /// Current value (the default when unset).
    pub value: ParamValue,
}

/// One tool node, ready to draw in the inspector.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    /// The node.
    pub node: NodeId,
    /// Tool name ("Exposure").
    pub name: &'static str,
    /// `false` when bypassed.
    pub enabled: bool,
    /// Parameters in UI order.
    pub params: Vec<ParamRow>,
}

/// An open image with its recipe and history.
#[derive(Debug)]
pub struct Document {
    opened: Opened,
    session: EditSession,
    generation: u64,
}

impl Document {
    /// A document for a freshly opened image, with the [`STARTING_TOOLS`]
    /// this build has, at their defaults. The history starts empty.
    /// `first_generation` should be above every generation of earlier
    /// documents, so their late renders are recognised.
    ///
    /// # Errors
    /// [`GraphError`] only if the starting graph cannot be built (a bug).
    pub fn new(opened: Opened, registry: &ToolRegistry, first_generation: u64) -> Result<Self, GraphError> {
        let mut recipe = Recipe::default();
        let mut last = recipe.graph.source();
        for name in STARTING_TOOLS {
            let Ok(id) = ToolId::new(name) else { continue };
            if registry.get(&id).is_some() {
                last = recipe.graph.insert_serial(last, id, Params::new())?;
            }
        }
        Ok(Self { opened, session: EditSession::new(recipe), generation: first_generation })
    }

    /// The RAW file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.opened.path
    }

    /// File name for titles ("DSC_0001.NEF").
    #[must_use]
    pub fn file_name(&self) -> String {
        self.opened.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    /// "Maker Model".
    #[must_use]
    pub fn camera(&self) -> &str {
        &self.opened.camera
    }

    /// How to turn the image upright.
    #[must_use]
    pub fn orientation(&self) -> Orientation {
        self.opened.orientation
    }

    /// Full-size developed image.
    #[must_use]
    pub fn developed(&self) -> &Arc<ImageF32> {
        &self.opened.developed
    }

    /// Smaller copy for previews.
    #[must_use]
    pub fn preview(&self) -> &Arc<ImageF32> {
        &self.opened.preview
    }

    /// `true` when the preview is smaller than the full image, so a full render adds detail.
    #[must_use]
    pub fn preview_is_reduced(&self) -> bool {
        self.opened.preview.width() < self.opened.developed.width()
    }

    /// Width and height of the upright full-size image.
    #[must_use]
    pub fn upright_size(&self) -> (u32, u32) {
        let (w, h) = (self.opened.developed.width(), self.opened.developed.height());
        if self.opened.orientation.swaps_axes() { (h, w) } else { (w, h) }
    }

    /// `false` when the file had no white balance.
    #[must_use]
    pub fn has_camera_white_balance(&self) -> bool {
        self.opened.has_camera_white_balance
    }

    /// The current recipe.
    #[must_use]
    pub fn recipe(&self) -> &Recipe {
        self.session.recipe()
    }

    /// Increases with every change to the recipe.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Tool nodes in processing order with their parameters, for the inspector.
    /// Nodes of tools missing from this build are listed without parameters.
    #[must_use]
    pub fn node_rows(&self, registry: &ToolRegistry) -> Vec<NodeRow> {
        let graph = &self.recipe().graph;
        let order = graph.evaluation_order().unwrap_or_default();
        order
            .into_iter()
            .filter_map(|id| graph.node(id))
            .filter_map(|node| {
                let NodeKind::Tool(tool_id) = &node.kind else { return None };
                let Some(tool) = registry.get(tool_id) else {
                    return Some(NodeRow {
                        node: node.id,
                        name: "Missing tool",
                        enabled: node.enabled,
                        params: vec![],
                    });
                };
                let d = tool.descriptor();
                let params = d
                    .params
                    .iter()
                    .map(|p| {
                        let value = node.params.get(&p.id).map_or_else(|| p.default_value(), |v| p.sanitize(v));
                        ParamRow { descriptor: p.clone(), value }
                    })
                    .collect();
                Some(NodeRow { node: node.id, name: d.name, enabled: node.enabled, params })
            })
            .collect()
    }

    /// Starts a continuous gesture (a slider drag): its changes become one undo step.
    pub fn begin_gesture(&mut self) -> GestureId {
        self.session.begin_gesture()
    }

    /// Sets a parameter. A value equal to the current one changes nothing
    /// (no undo step, no new render).
    ///
    /// # Errors
    /// [`EditError`] when the node or parameter does not exist; nothing changes.
    pub fn set_param(
        &mut self,
        registry: &ToolRegistry,
        node: NodeId,
        param: ParamId,
        value: ParamValue,
        gesture: Option<GestureId>,
    ) -> Result<(), EditError> {
        if self.current_value(node, &param) == Some(value) {
            return Ok(());
        }
        self.session.apply_in_gesture(registry, Edit::SetParam { node, param, value }, gesture)?;
        self.generation += 1;
        Ok(())
    }

    /// Returns a parameter to its default (one undo step).
    ///
    /// # Errors
    /// [`EditError`] when the node or parameter does not exist; nothing changes.
    pub fn reset_param(&mut self, registry: &ToolRegistry, node: NodeId, param: ParamId) -> Result<(), EditError> {
        if self.recipe().graph.node(node).is_some_and(|n| n.params.get(&param).is_none()) {
            return Ok(());
        }
        self.session.apply(registry, Edit::ResetParam { node, param })?;
        self.generation += 1;
        Ok(())
    }

    /// Enables or bypasses a node (one undo step).
    ///
    /// # Errors
    /// [`EditError`] when the node does not exist; nothing changes.
    pub fn set_enabled(&mut self, registry: &ToolRegistry, node: NodeId, enabled: bool) -> Result<(), EditError> {
        if self.recipe().graph.node(node).is_some_and(|n| n.enabled == enabled) {
            return Ok(());
        }
        self.session.apply(registry, Edit::SetEnabled { node, enabled })?;
        self.generation += 1;
        Ok(())
    }

    /// Undoes the last change; returns its label.
    pub fn undo(&mut self) -> Option<String> {
        let label = self.session.undo()?;
        self.generation += 1;
        Some(label)
    }

    /// Redoes the last undone change; returns its label.
    pub fn redo(&mut self) -> Option<String> {
        let label = self.session.redo()?;
        self.generation += 1;
        Some(label)
    }

    /// "Change exposure" when there is something to undo.
    #[must_use]
    pub fn undo_label(&self) -> Option<String> {
        self.session.undo_label()
    }

    /// Label of the change [`Self::redo`] would redo.
    #[must_use]
    pub fn redo_label(&self) -> Option<String> {
        self.session.redo_label()
    }

    /// Where an export goes by default: `<folder>/<file stem>.jpg`.
    #[must_use]
    pub fn export_path(&self, folder: &Path) -> PathBuf {
        let stem = self.opened.path.file_stem().map_or_else(|| "image".into(), |s| s.to_string_lossy().into_owned());
        folder.join(format!("{stem}.jpg"))
    }

    fn current_value(&self, node: NodeId, param: &ParamId) -> Option<ParamValue> {
        self.recipe().graph.node(node)?.params.get(param)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use core_model::WorkingSpace;

    fn registry() -> ToolRegistry {
        engine::builtin_tools().unwrap()
    }

    fn opened(path: &str, orientation: Orientation) -> Opened {
        let img = Arc::new(ImageF32::new(6, 4, WorkingSpace::LinearRec2020).unwrap());
        Opened {
            path: PathBuf::from(path),
            camera: "Nikon D3S".into(),
            orientation,
            developed: Arc::clone(&img),
            preview: img,
            has_camera_white_balance: true,
        }
    }

    fn exposure_param() -> ParamId {
        ParamId::new(tool_exposure_id()).unwrap()
    }

    fn tool_exposure_id() -> &'static str {
        "exposure"
    }

    #[test]
    fn a_new_document_has_the_starting_tools_and_an_empty_history() {
        let reg = registry();
        let doc = Document::new(opened("a/DSC_1.NEF", Orientation::Normal), &reg, 1).unwrap();
        let rows = doc.node_rows(&reg);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Exposure");
        assert_eq!(rows[0].params[0].value, ParamValue::Float(0.0));
        assert_eq!(doc.undo_label(), None);
        assert_eq!(doc.file_name(), "DSC_1.NEF");
    }

    #[test]
    fn a_slider_drag_is_one_undo_step_and_each_change_is_a_new_generation() {
        let reg = registry();
        let mut doc = Document::new(opened("x.NEF", Orientation::Normal), &reg, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        let g0 = doc.generation();
        let drag = doc.begin_gesture();
        for v in [0.1, 0.5, 1.2] {
            doc.set_param(&reg, node, exposure_param(), ParamValue::Float(v), Some(drag)).unwrap();
        }
        assert_eq!(doc.generation(), g0 + 3);
        assert_eq!(doc.undo_label().as_deref(), Some("Change exposure"));
        assert_eq!(doc.undo().as_deref(), Some("Change exposure"));
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(0.0));
        assert_eq!(doc.undo(), None);
        assert!(doc.redo().is_some());
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(1.2));
    }

    #[test]
    fn setting_the_same_value_changes_nothing() {
        let reg = registry();
        let mut doc = Document::new(opened("x.NEF", Orientation::Normal), &reg, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        let g = doc.generation();
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(1.0), None).unwrap();
        assert_eq!(doc.generation(), g);
        doc.undo();
        assert_eq!(doc.undo_label(), None);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_reset_returns_to_default() {
        let reg = registry();
        let mut doc = Document::new(opened("x.NEF", Orientation::Normal), &reg, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_param(&reg, node, exposure_param(), ParamValue::Float(99.0), None).unwrap();
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(5.0));
        doc.reset_param(&reg, node, exposure_param()).unwrap();
        assert_eq!(doc.node_rows(&reg)[0].params[0].value, ParamValue::Float(0.0));
        assert_eq!(doc.undo_label().as_deref(), Some("Reset exposure"));
        let g = doc.generation();
        doc.reset_param(&reg, node, exposure_param()).unwrap();
        assert_eq!(doc.generation(), g, "resetting a default value is not a change");
    }

    #[test]
    fn bypass_is_undoable() {
        let reg = registry();
        let mut doc = Document::new(opened("x.NEF", Orientation::Normal), &reg, 1).unwrap();
        let node = doc.node_rows(&reg)[0].node;
        doc.set_enabled(&reg, node, false).unwrap();
        assert!(!doc.node_rows(&reg)[0].enabled);
        doc.undo();
        assert!(doc.node_rows(&reg)[0].enabled);
    }

    #[test]
    fn unknown_nodes_are_refused_without_changes() {
        let reg = registry();
        let mut doc = Document::new(opened("x.NEF", Orientation::Normal), &reg, 1).unwrap();
        let g = doc.generation();
        let source = doc.recipe().graph.source();
        assert!(doc.set_param(&reg, source, exposure_param(), ParamValue::Float(1.0), None).is_err());
        assert_eq!(doc.generation(), g);
    }

    #[test]
    fn upright_size_and_export_name_follow_the_file() {
        let reg = registry();
        let doc = Document::new(opened("dir/IMG 7.CR3", Orientation::Rotate90), &reg, 1).unwrap();
        assert_eq!(doc.upright_size(), (4, 6));
        assert_eq!(doc.export_path(Path::new("out")), Path::new("out").join("IMG 7.jpg"));
        assert!(!doc.preview_is_reduced());
    }

    #[test]
    fn missing_tools_are_listed_without_parameters() {
        let reg = registry();
        let doc = Document::new(opened("x.NEF", Orientation::Normal), &reg, 1).unwrap();
        let rows = doc.node_rows(&ToolRegistry::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Missing tool");
        assert!(rows[0].params.is_empty());
    }
}
