//! Edits to a [`Recipe`], with validation and undo/redo.
//!
//! The UI, the MCP server and scripts never touch a recipe directly: they
//! build an [`Edit`] and give it to an [`EditSession`]. The session checks
//! it against the graph rules and the tool registry, applies it, and records
//! how to undo it. So every change, from any source, is validated the same
//! way and can be undone with Ctrl+Z.

use std::fmt;

use core_model::{
    GraphError, LensSettings, Link, NodeGraph, NodeId, NodeKind, ParamId, ParamValue, Params, ProfileChoice, Recipe,
    ToolId, WhiteBalance,
};
use tool_api::ToolRegistry;

/// Undo steps kept per image before the oldest are dropped.
pub const MAX_UNDO_STEPS: usize = 500;

/// One change to a recipe.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Sets one parameter of a tool node. Out-of-range values are clamped.
    SetParam {
        /// The tool node.
        node: NodeId,
        /// The parameter.
        param: ParamId,
        /// The new value.
        value: ParamValue,
    },
    /// Returns one parameter to its default.
    ResetParam {
        /// The tool node.
        node: NodeId,
        /// The parameter.
        param: ParamId,
    },
    /// Adds a tool node after `after` ("Add serial node").
    InsertSerial {
        /// Node whose output the new node takes.
        after: NodeId,
        /// Tool of the new node.
        tool: ToolId,
    },
    /// Removes a node, reconnecting a serial chain around it.
    RemoveNode {
        /// The node to remove.
        node: NodeId,
    },
    /// Connects two ports; a link already feeding the input is replaced.
    Connect(Link),
    /// Removes the link feeding an input.
    Disconnect {
        /// Node whose input is freed.
        to: NodeId,
        /// Input port index.
        port: u8,
    },
    /// Enables or bypasses a node.
    SetEnabled {
        /// The node.
        node: NodeId,
        /// `false` passes the image through unchanged.
        enabled: bool,
    },
    /// Changes the white balance of the development stage.
    SetWhiteBalance(WhiteBalance),
    /// Chooses the camera profile of the development stage.
    SetCameraProfile(ProfileChoice),
    /// Changes the lens corrections of the development stage.
    SetLensCorrection(LensSettings),
}

impl Edit {
    /// Text for "Undo …" / "Redo …", sentence case.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::SetParam { param, .. } => format!("Change {}", param.as_str().replace('_', " ")),
            Self::ResetParam { param, .. } => format!("Reset {}", param.as_str().replace('_', " ")),
            Self::InsertSerial { .. } => "Add serial node".to_owned(),
            Self::RemoveNode { .. } => "Remove node".to_owned(),
            Self::Connect(_) => "Connect nodes".to_owned(),
            Self::Disconnect { .. } => "Disconnect nodes".to_owned(),
            Self::SetEnabled { enabled: true, .. } => "Enable node".to_owned(),
            Self::SetEnabled { enabled: false, .. } => "Bypass node".to_owned(),
            Self::SetWhiteBalance(_) => "Change white balance".to_owned(),
            Self::SetCameraProfile(_) => "Change camera profile".to_owned(),
            Self::SetLensCorrection(_) => "Change lens corrections".to_owned(),
        }
    }
}

/// Why an edit was refused. The recipe is unchanged when an error is returned.
#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    /// The graph rules refused the change.
    Graph(GraphError),
    /// The node exists but is not a tool node.
    NotAToolNode(NodeId),
    /// This build has no tool with that id.
    UnknownTool(ToolId),
    /// The tool has no parameter with that id.
    UnknownParam(ToolId, ParamId),
    /// A white-balance multiplier is not finite and positive.
    InvalidWhiteBalance,
    /// A named camera profile has an empty name.
    InvalidProfileName,
    /// The lens correction zoom is outside 0.25–4, or a chosen lens name is empty.
    InvalidLensSettings,
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(e) => e.fmt(f),
            Self::NotAToolNode(id) => write!(f, "node {id} has no parameters"),
            Self::UnknownTool(id) => write!(f, "tool '{id}' is not available in this build"),
            Self::UnknownParam(tool, p) => write!(f, "tool '{tool}' has no parameter '{p}'"),
            Self::InvalidWhiteBalance => write!(f, "white balance multipliers must be positive numbers"),
            Self::InvalidProfileName => write!(f, "a camera profile needs a name"),
            Self::InvalidLensSettings => {
                write!(f, "lens zoom must be between 0.25 and 4, and a chosen lens needs a name")
            }
        }
    }
}

impl std::error::Error for EditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Graph(e) => Some(e),
            _ => None,
        }
    }
}

impl From<GraphError> for EditError {
    fn from(e: GraphError) -> Self {
        Self::Graph(e)
    }
}

/// Identifies one continuous gesture (a slider drag, typing in a field).
/// All `SetParam` edits of the same parameter within one gesture become a
/// single undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GestureId(u64);

/// What is needed to undo one applied edit.
#[derive(Debug, Clone, PartialEq)]
enum Undo {
    Param {
        node: NodeId,
        param: ParamId,
        previous: Option<ParamValue>,
    },
    Enabled {
        node: NodeId,
        previous: bool,
    },
    WhiteBalance(WhiteBalance),
    Profile(ProfileChoice),
    Lens(LensSettings),
    /// Structural edits keep the whole previous graph: graphs are small and
    /// this restores ids and links exactly.
    Graph(Box<NodeGraph>),
}

#[derive(Debug, Clone, PartialEq)]
struct Step {
    edit: Edit,
    undo: Undo,
    gesture: Option<GestureId>,
}

/// A recipe being edited, with its undo and redo history.
#[derive(Debug, Clone)]
pub struct EditSession {
    recipe: Recipe,
    done: Vec<Step>,
    undone: Vec<Step>,
    next_gesture: u64,
}

impl EditSession {
    /// Starts editing `recipe` with an empty history.
    #[must_use]
    pub fn new(recipe: Recipe) -> Self {
        Self { recipe, done: Vec::new(), undone: Vec::new(), next_gesture: 0 }
    }

    /// The current recipe.
    #[must_use]
    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    /// Starts a continuous gesture; pass the id to every edit it produces.
    pub fn begin_gesture(&mut self) -> GestureId {
        self.next_gesture += 1;
        GestureId(self.next_gesture)
    }

    /// Validates and applies `edit` as a new undo step.
    ///
    /// # Errors
    /// [`EditError`]; the recipe and the history are unchanged.
    pub fn apply(&mut self, registry: &ToolRegistry, edit: Edit) -> Result<(), EditError> {
        self.apply_in_gesture(registry, edit, None)
    }

    /// Like [`Self::apply`], but consecutive `SetParam` edits of the same
    /// parameter in the same gesture merge into one undo step.
    ///
    /// # Errors
    /// [`EditError`]; the recipe and the history are unchanged.
    pub fn apply_in_gesture(
        &mut self,
        registry: &ToolRegistry,
        edit: Edit,
        gesture: Option<GestureId>,
    ) -> Result<(), EditError> {
        let edit = self.validate(registry, edit)?;
        let undo = self.perform(&edit)?;
        self.undone.clear();
        if let Some(top) = self.done.last_mut()
            && gesture.is_some()
            && top.gesture == gesture
            && same_param(&top.edit, &edit)
        {
            // Keep the value from before the gesture began.
            top.edit = edit;
            return Ok(());
        }
        self.done.push(Step { edit, undo, gesture });
        if self.done.len() > MAX_UNDO_STEPS {
            self.done.remove(0);
        }
        Ok(())
    }

    /// Undoes the last step; returns its label, or `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Option<String> {
        let step = self.done.pop()?;
        let label = step.edit.label();
        match &step.undo {
            Undo::Param { node, param, previous } => {
                if let Some(mut n) = self.recipe.graph.node_mut(*node) {
                    match previous {
                        Some(v) => n.params().set(param.clone(), *v),
                        None => n.params().reset(param),
                    };
                }
            }
            Undo::Enabled { node, previous } => {
                if let Some(mut n) = self.recipe.graph.node_mut(*node) {
                    n.set_enabled(*previous);
                }
            }
            Undo::WhiteBalance(wb) => self.recipe.develop.white_balance = *wb,
            Undo::Profile(p) => self.recipe.develop.profile = p.clone(),
            Undo::Lens(l) => self.recipe.develop.lens = l.clone(),
            Undo::Graph(g) => self.recipe.graph = (**g).clone(),
        }
        self.undone.push(step);
        Some(label)
    }

    /// Redoes the last undone step; returns its label, or `None` when there is nothing to redo.
    pub fn redo(&mut self) -> Option<String> {
        let mut step = self.undone.pop()?;
        // Undo restored the exact state the edit was validated against, so it cannot fail now.
        match self.perform(&step.edit) {
            Ok(undo) => {
                let label = step.edit.label();
                step.undo = undo;
                self.done.push(step);
                Some(label)
            }
            Err(_) => {
                self.undone.clear();
                None
            }
        }
    }

    /// Label of the step [`Self::undo`] would undo.
    #[must_use]
    pub fn undo_label(&self) -> Option<String> {
        self.done.last().map(|s| s.edit.label())
    }

    /// Label of the step [`Self::redo`] would redo.
    #[must_use]
    pub fn redo_label(&self) -> Option<String> {
        self.undone.last().map(|s| s.edit.label())
    }

    /// Checks `edit` against the registry and normalises it (clamped values).
    fn validate(&self, registry: &ToolRegistry, edit: Edit) -> Result<Edit, EditError> {
        match edit {
            Edit::SetParam { node, param, value } => {
                let tool = self.tool_of(node)?;
                let d = registry.get(&tool).ok_or_else(|| EditError::UnknownTool(tool.clone()))?.descriptor();
                let p = d.param(&param).ok_or_else(|| EditError::UnknownParam(tool.clone(), param.clone()))?;
                let value = p.sanitize(value);
                Ok(Edit::SetParam { node, param, value })
            }
            Edit::ResetParam { node, param } => {
                let tool = self.tool_of(node)?;
                let d = registry.get(&tool).ok_or_else(|| EditError::UnknownTool(tool.clone()))?.descriptor();
                if d.param(&param).is_none() {
                    return Err(EditError::UnknownParam(tool, param));
                }
                Ok(Edit::ResetParam { node, param })
            }
            Edit::InsertSerial { after, tool } => {
                if registry.get(&tool).is_none() {
                    return Err(EditError::UnknownTool(tool));
                }
                Ok(Edit::InsertSerial { after, tool })
            }
            Edit::SetWhiteBalance(WhiteBalance::Multipliers(m)) if !m.iter().all(|v| v.is_finite() && *v > 0.0) => {
                Err(EditError::InvalidWhiteBalance)
            }
            Edit::SetCameraProfile(ProfileChoice::Named(name)) => {
                let name = name.trim();
                if name.is_empty() {
                    return Err(EditError::InvalidProfileName);
                }
                Ok(Edit::SetCameraProfile(ProfileChoice::Named(name.to_owned())))
            }
            Edit::SetLensCorrection(mut lens) => {
                if lens.zoom.is_some_and(|z| !(z.is_finite() && (0.25..=4.0).contains(&z))) {
                    return Err(EditError::InvalidLensSettings);
                }
                if let Some(name) = &lens.lens {
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(EditError::InvalidLensSettings);
                    }
                    lens.lens = Some(name.to_owned());
                }
                Ok(Edit::SetLensCorrection(lens))
            }
            other => Ok(other),
        }
    }

    fn tool_of(&self, node: NodeId) -> Result<ToolId, EditError> {
        match &self.recipe.graph.node(node).ok_or(GraphError::UnknownNode(node))?.kind {
            NodeKind::Tool(t) => Ok(t.clone()),
            _ => Err(EditError::NotAToolNode(node)),
        }
    }

    /// Applies an already validated edit and returns how to undo it.
    /// Leaves the recipe unchanged on error.
    fn perform(&mut self, edit: &Edit) -> Result<Undo, EditError> {
        let graph = &mut self.recipe.graph;
        match edit {
            Edit::SetParam { node, param, value } => {
                let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
                let previous = n.params().set(param.clone(), *value);
                Ok(Undo::Param { node: *node, param: param.clone(), previous })
            }
            Edit::ResetParam { node, param } => {
                let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
                let previous = n.params().reset(param);
                Ok(Undo::Param { node: *node, param: param.clone(), previous })
            }
            Edit::SetEnabled { node, enabled } => {
                let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
                let previous = n.set_enabled(*enabled);
                Ok(Undo::Enabled { node: *node, previous })
            }
            Edit::SetWhiteBalance(wb) => {
                let previous = std::mem::replace(&mut self.recipe.develop.white_balance, *wb);
                Ok(Undo::WhiteBalance(previous))
            }
            Edit::SetCameraProfile(p) => {
                let previous = std::mem::replace(&mut self.recipe.develop.profile, p.clone());
                Ok(Undo::Profile(previous))
            }
            Edit::SetLensCorrection(l) => {
                let previous = std::mem::replace(&mut self.recipe.develop.lens, l.clone());
                Ok(Undo::Lens(previous))
            }
            Edit::InsertSerial { .. } | Edit::RemoveNode { .. } | Edit::Connect(_) | Edit::Disconnect { .. } => {
                // Work on a copy and publish it only on success: the strong guarantee.
                let before = graph.clone();
                let mut next = before.clone();
                match edit {
                    Edit::InsertSerial { after, tool } => {
                        next.insert_serial(*after, tool.clone(), Params::new())?;
                    }
                    Edit::RemoveNode { node } => {
                        next.remove_node(*node)?;
                    }
                    Edit::Connect(link) => {
                        next.connect(*link)?;
                    }
                    Edit::Disconnect { to, port } => {
                        next.disconnect(*to, *port);
                    }
                    _ => unreachable!("outer match restricts to structural edits"),
                }
                *graph = next;
                Ok(Undo::Graph(Box::new(before)))
            }
        }
    }
}

fn same_param(a: &Edit, b: &Edit) -> bool {
    matches!(
        (a, b),
        (Edit::SetParam { node: n1, param: p1, .. }, Edit::SetParam { node: n2, param: p2, .. })
            if n1 == n2 && p1 == p2
    )
}
