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
