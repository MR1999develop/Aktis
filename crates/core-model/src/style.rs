//! Styles: a chosen part of a photo's edits, kept to be applied to other
//! photos (Capture One's styles and Lightroom's presets), or painted on
//! with a brush (Capture One's style brushes).
//!
//! A style holds values, never node ids or masks: the development settings
//! it was saved with (each one optional), the geometry (optional) and the
//! parameters of tool nodes, by tool. Masks belong to one photo and are
//! never part of a style.

use crate::{Geometry, LensSettings, NodeGraph, NodeId, NodeKind, Params, ProfileChoice, Recipe, ToolId, WhiteBalance};

/// The values of one tool node of a style.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleNode {
    /// The tool.
    pub tool: ToolId,
    /// Its parameters (those at their default may be missing).
    pub params: Params,
}

/// A chosen part of a photo's edits.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Style {
    /// White balance, when included.
    pub white_balance: Option<WhiteBalance>,
    /// Camera profile, when included.
    pub profile: Option<ProfileChoice>,
    /// Lens corrections, when included.
    pub lens: Option<LensSettings>,
    /// Straightening, perspective and crop, when included.
    pub geometry: Option<Geometry>,
    /// Tool nodes, in the order they act.
    pub nodes: Vec<StyleNode>,
}

/// What of a recipe goes into a style (the choices of the Save Style window).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleSelection {
    /// Include the white balance.
    pub white_balance: bool,
    /// Include the camera profile.
    pub profile: bool,
    /// Include the lens corrections.
    pub lens: bool,
    /// Include straightening, perspective and crop.
    pub geometry: bool,
    /// The tool nodes to include.
    pub nodes: Vec<NodeId>,
}

impl Style {
    /// The part of `recipe` that `selection` names. Nodes that are not tool
    /// nodes are left out; the nodes keep the order they act in.
    #[must_use]
    pub fn from_recipe(recipe: &Recipe, selection: &StyleSelection) -> Self {
        let develop = &recipe.develop;
        let nodes = tool_nodes(&recipe.graph)
            .into_iter()
            .filter(|id| selection.nodes.contains(id))
            .filter_map(|id| {
                let node = recipe.graph.node(id)?;
                match &node.kind {
                    NodeKind::Tool(tool) => Some(StyleNode { tool: tool.clone(), params: node.params.clone() }),
                    _ => None,
                }
            })
            .collect();
        Self {
            white_balance: selection.white_balance.then_some(develop.white_balance),
            profile: selection.profile.then(|| develop.profile.clone()),
            lens: selection.lens.then(|| develop.lens.clone()),
            geometry: selection.geometry.then_some(recipe.geometry),
            nodes,
        }
    }

    /// Nothing would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.white_balance.is_none()
            && self.profile.is_none()
            && self.lens.is_none()
            && self.geometry.is_none()
            && self.nodes.is_empty()
    }

    /// Only tool nodes: what a style brush can paint (development settings
    /// and geometry act on the whole photo).
    #[must_use]
    pub fn local(&self) -> Self {
        Self { nodes: self.nodes.clone(), ..Self::default() }
    }
}

/// Values saved as the defaults of some tools for one camera model, as
/// Capture One's Save as Defaults: a photo of that camera never edited
/// starts with these nodes, a node of one of these tools added to such a
/// photo starts with these values, and resetting goes back to them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraDefaults {
    /// The camera ("Sony ILCE-7M3"), as the editor names the photo's camera.
    pub camera: String,
    /// One node per tool, in the order they were first saved.
    pub nodes: Vec<StyleNode>,
}

impl CameraDefaults {
    /// No defaults yet for `camera`.
    #[must_use]
    pub fn new(camera: impl Into<String>) -> Self {
        Self { camera: camera.into(), nodes: Vec::new() }
    }

    /// The saved values of `tool`, if any.
    #[must_use]
    pub fn params(&self, tool: &ToolId) -> Option<&Params> {
        self.nodes.iter().find(|n| n.tool == *tool).map(|n| &n.params)
    }

    /// Saves `params` as the defaults of `tool`, replacing earlier ones in place.
    pub fn set(&mut self, tool: ToolId, params: Params) {
        match self.nodes.iter_mut().find(|n| n.tool == tool) {
            Some(n) => n.params = params,
            None => self.nodes.push(StyleNode { tool, params }),
        }
    }

    /// Forgets the defaults of `tool`; `true` when there were some.
    pub fn remove(&mut self, tool: &ToolId) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|n| n.tool != *tool);
        self.nodes.len() != before
    }

    /// `true` when nothing is saved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// The tool nodes of `graph` in the order they act (evaluation order), or in
/// id order when the graph cannot be ordered.
#[must_use]
pub fn tool_nodes(graph: &NodeGraph) -> Vec<NodeId> {
    let order = graph.evaluation_order().unwrap_or_else(|_| {
        let mut ids: Vec<NodeId> = graph.nodes().map(|n| n.id).collect();
        ids.sort();
        ids
    });
    order.into_iter().filter(|id| matches!(graph.node(*id).map(|n| &n.kind), Some(NodeKind::Tool(_)))).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{ParamId, ParamValue};

    fn tool(name: &str) -> ToolId {
        ToolId::new(name).unwrap()
    }

    #[test]
    fn camera_defaults_keep_one_node_per_tool() {
        let mut d = CameraDefaults::new("Sony ILCE-7M3");
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.3));
        d.set(tool("exposure"), p.clone());
        d.set(tool("hdr"), Params::new());
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.7));
        d.set(tool("exposure"), p.clone());
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.nodes[0].tool, tool("exposure"), "replaced in place");
        assert_eq!(d.params(&tool("exposure")), Some(&p));
        assert!(d.remove(&tool("hdr")) && !d.remove(&tool("hdr")));
        assert!(!d.is_empty());
    }

    #[test]
    fn only_the_chosen_parts_are_kept_in_acting_order() {
        let mut r = Recipe::default();
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.5));
        let a = r.graph.insert_serial(r.graph.source(), tool("exposure"), p.clone()).unwrap();
        let b = r.graph.insert_serial(a, tool("curve"), Params::new()).unwrap();
        let c = r.graph.insert_serial(b, tool("clarity"), Params::new()).unwrap();
        r.develop.white_balance = WhiteBalance::Multipliers([2.0, 1.0, 1.5]);
        let sel = StyleSelection { white_balance: true, nodes: vec![c, a], ..StyleSelection::default() };
        let s = Style::from_recipe(&r, &sel);
        assert_eq!(s.white_balance, Some(WhiteBalance::Multipliers([2.0, 1.0, 1.5])));
        assert_eq!((s.profile.as_ref(), s.lens.as_ref(), s.geometry), (None, None, None));
        let tools: Vec<&str> = s.nodes.iter().map(|n| n.tool.as_str()).collect();
        assert_eq!(tools, ["exposure", "clarity"]);
        assert_eq!(s.nodes[0].params, p);
        assert!(!s.is_empty() && s.local().white_balance.is_none());
        assert!(Style::default().is_empty());
        assert_eq!(tool_nodes(&r.graph), vec![a, b, c]);
    }
}
