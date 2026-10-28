//! Edits to a [`Recipe`], with validation and undo/redo.
//!
//! The UI, the MCP server and scripts never touch a recipe directly: they
//! build an [`Edit`] and give it to an [`EditSession`]. The session checks
//! it against the graph rules and the tool registry, applies it, and records
//! how to undo it. So every change, from any source, is validated the same
//! way and can be undone with Ctrl+Z.

use std::collections::HashMap;
use std::fmt;

use core_model::{
    Geometry, GraphError, LensSettings, Link, MaskDef, NodeGraph, NodeId, NodeKind, ParamId, ParamValue, Params,
    PortKind, ProfileChoice, Recipe, ToolId, WhiteBalance,
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
    /// Sets several parameters of one tool node in one step (a curve point's
    /// input and output, a white-balance mode with its kelvin and tint).
    /// Out-of-range values are clamped.
    SetParams {
        /// The tool node.
        node: NodeId,
        /// The parameters and their new values.
        values: Vec<(ParamId, ParamValue)>,
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
        /// Values the new node starts with (usually none: every parameter
        /// at its default). Unknown keys are dropped, values sanitised.
        params: Params,
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
    /// Changes the perspective, straightening and crop of the finished image.
    SetGeometry(Geometry),
    /// Adds a mask node and connects it to the key input of a tool node, so
    /// the tool acts only inside the mask. A mask already on that input is
    /// disconnected (its node stays in the graph).
    AddMask {
        /// The tool node to restrict.
        to: NodeId,
        /// What the new mask selects.
        def: MaskDef,
    },
    /// Replaces the definition of a mask node (a new click, another
    /// component, feather, invert …). Feather and opacity are clamped.
    SetMask {
        /// The mask node.
        node: NodeId,
        /// The new definition.
        def: MaskDef,
    },
}

impl Edit {
    /// Text for "Undo …" / "Redo …", sentence case.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::SetParam { param, .. } => format!("Change {}", param.as_str().replace('_', " ")),
            Self::ResetParam { param, .. } => format!("Reset {}", param.as_str().replace('_', " ")),
            Self::SetParams { values, .. } => {
                let names: Vec<String> = values.iter().map(|(p, _)| p.as_str().replace('_', " ")).collect();
                format!("Change {}", names.join(", "))
            }
            Self::InsertSerial { .. } => "Add serial node".to_owned(),
            Self::RemoveNode { .. } => "Remove node".to_owned(),
            Self::Connect(_) => "Connect nodes".to_owned(),
            Self::Disconnect { .. } => "Disconnect nodes".to_owned(),
            Self::SetEnabled { enabled: true, .. } => "Enable node".to_owned(),
            Self::SetEnabled { enabled: false, .. } => "Bypass node".to_owned(),
            Self::SetWhiteBalance(_) => "Change white balance".to_owned(),
            Self::SetCameraProfile(_) => "Change camera profile".to_owned(),
            Self::SetLensCorrection(_) => "Change lens corrections".to_owned(),
            Self::SetGeometry(_) => "Change crop and straightening".to_owned(),
            Self::AddMask { .. } => "Add mask".to_owned(),
            Self::SetMask { .. } => "Change mask".to_owned(),
        }
    }

    /// The new value in a few characters, for the History panel ("+0.50",
    /// "off", "as shot"); empty when the label says it all.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::SetParam { value, .. } => match value {
                ParamValue::Float(v) => format_number(*v),
                ParamValue::Bool(b) => on_off(*b).to_owned(),
                ParamValue::Choice(c) => format!("#{}", c + 1),
            },
            Self::ResetParam { .. } => "default".to_owned(),
            Self::InsertSerial { tool, .. } => tool.as_str().replace('_', " "),
            Self::SetEnabled { enabled, .. } => on_off(*enabled).to_owned(),
            Self::SetWhiteBalance(WhiteBalance::AsShot) => "as shot".to_owned(),
            Self::SetWhiteBalance(WhiteBalance::Multipliers(m)) => {
                format!("{:.2} \u{b7} {:.2} \u{b7} {:.2}", m[0], m[1], m[2])
            }
            Self::SetCameraProfile(ProfileChoice::Named(name)) => name.clone(),
            Self::SetLensCorrection(lens) => on_off(lens.enabled).to_owned(),
            Self::SetGeometry(g) => {
                let mut parts = Vec::new();
                if g.angle != 0.0 {
                    parts.push(format!("{:+.1}\u{b0}", g.angle));
                }
                if g.vertical != 0.0 || g.horizontal != 0.0 {
                    parts.push("perspective".to_owned());
                }
                if g.crop.is_some() {
                    parts.push("cropped".to_owned());
                }
                if parts.is_empty() { "none".to_owned() } else { parts.join(", ") }
            }
            _ => String::new(),
        }
    }

    /// What kind of change this is (the History panel colours it).
    #[must_use]
    pub fn kind(&self) -> EditKind {
        match self {
            Self::SetParam { .. } | Self::SetParams { .. } | Self::ResetParam { .. } => EditKind::Adjust,
            Self::SetWhiteBalance(_)
            | Self::SetCameraProfile(_)
            | Self::SetLensCorrection(_)
            | Self::SetGeometry(_) => EditKind::Develop,
            Self::AddMask { .. } | Self::SetMask { .. } => EditKind::Mask,
            Self::InsertSerial { .. }
            | Self::RemoveNode { .. }
            | Self::Connect(_)
            | Self::Disconnect { .. }
            | Self::SetEnabled { .. } => EditKind::Node,
        }
    }

    /// The same edit with every node id passed through `map`; `None` when
    /// `map` drops a node the edit needs.
    fn map_nodes(&self, map: impl Fn(NodeId) -> Option<NodeId>) -> Option<Self> {
        Some(match self {
            Self::SetParam { node, param, value } => {
                Self::SetParam { node: map(*node)?, param: param.clone(), value: *value }
            }
            Self::ResetParam { node, param } => Self::ResetParam { node: map(*node)?, param: param.clone() },
            Self::SetParams { node, values } => Self::SetParams { node: map(*node)?, values: values.clone() },
            Self::InsertSerial { after, tool, params } => {
                Self::InsertSerial { after: map(*after)?, tool: tool.clone(), params: params.clone() }
            }
            Self::RemoveNode { node } => Self::RemoveNode { node: map(*node)? },
            Self::Connect(link) => Self::Connect(Link { from: map(link.from)?, to: map(link.to)?, ..*link }),
            Self::Disconnect { to, port } => Self::Disconnect { to: map(*to)?, port: *port },
            Self::SetEnabled { node, enabled } => Self::SetEnabled { node: map(*node)?, enabled: *enabled },
            Self::AddMask { to, def } => Self::AddMask { to: map(*to)?, def: def.clone() },
            Self::SetMask { node, def } => Self::SetMask { node: map(*node)?, def: def.clone() },
            Self::SetWhiteBalance(_)
            | Self::SetCameraProfile(_)
            | Self::SetLensCorrection(_)
            | Self::SetGeometry(_) => self.clone(),
        })
    }
}

/// Groups of edits, for colouring the History panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Development stage: white balance, camera profile, lens corrections.
    Develop,
    /// A tool parameter.
    Adjust,
    /// The node graph: add, remove, connect, bypass.
    Node,
    /// A mask.
    Mask,
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// "+0.50", "-1.20", "12"; never "-0.00".
fn format_number(v: f32) -> String {
    let places = if v.fract().abs() < 1e-6 { 0 } else { 2 };
    let text = format!("{v:.places$}");
    if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') {
        return text.trim_start_matches('-').to_owned();
    }
    if v > 0.0 { format!("+{text}") } else { text }
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
    /// The node exists but is not a mask node.
    NotAMaskNode(NodeId),
    /// The node has no key (mask) input.
    NoMaskInput(NodeId),
    /// A history rewrite named a step twice or a step that does not exist.
    BadHistoryOrder,
    /// The step acts on a node made by a step that was deleted or no longer applies.
    NodeFromRemovedStep,
    /// A step that no longer applied when the history was saved; the text
    /// says why (read back from the catalog).
    Saved(String),
    /// The edit was saved by a newer version of the editor and is shown
    /// read-only: nothing can change.
    ReadOnly,
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
            Self::NotAMaskNode(id) => write!(f, "node {id} is not a mask node"),
            Self::NoMaskInput(id) => write!(f, "node {id} cannot take a mask"),
            Self::BadHistoryOrder => write!(f, "the new history order names a step twice or a missing step"),
            Self::NodeFromRemovedStep => write!(f, "it acts on a node made by a removed step"),
            Self::Saved(why) => f.write_str(why),
            Self::ReadOnly => {
                write!(f, "this photo is read-only: its edits were saved by a newer version of the editor")
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
/// All `SetParam` edits of the same parameter (or `SetParams` edits of the
/// same parameters) within one gesture become a single undo step.
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
    Params {
        node: NodeId,
        previous: Vec<(ParamId, Option<ParamValue>)>,
    },
    Enabled {
        node: NodeId,
        previous: bool,
    },
    WhiteBalance(WhiteBalance),
    Profile(ProfileChoice),
    Lens(LensSettings),
    Geometry(Geometry),
    Mask {
        node: NodeId,
        previous: MaskDef,
    },
    /// Structural edits keep the whole previous graph: graphs are small and
    /// this restores ids and links exactly.
    Graph(Box<NodeGraph>),
}

/// One entry of the history.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    edit: Edit,
    /// How to undo it; `None` when the step does not apply (see `skipped`).
    undo: Option<Undo>,
    gesture: Option<GestureId>,
    /// Nodes this step added to the graph, in id order.
    created: Vec<NodeId>,
    /// Why the step no longer applies after the history was rewritten.
    skipped: Option<EditError>,
}

/// The whole history at one moment, to undo a rewrite.
#[derive(Debug, Clone, PartialEq)]
struct Snapshot {
    base: Recipe,
    recipe: Recipe,
    steps: Vec<Step>,
}

/// A reorder or deletion of history steps.
#[derive(Debug, Clone, PartialEq)]
struct Rewrite {
    label: String,
    before: Snapshot,
    after: Snapshot,
}

/// What Ctrl+Z undoes, newest last.
#[derive(Debug, Clone, PartialEq)]
enum Done {
    /// The last step of the history.
    Step,
    /// A rewrite of the history.
    Rewrite(Box<Rewrite>),
}

/// What Ctrl+Y redoes, newest last.
#[derive(Debug, Clone, PartialEq)]
enum Undone {
    Step(Step),
    Rewrite(Box<Rewrite>),
}

/// One row of the History panel.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    /// "Change exposure".
    pub label: String,
    /// The new value in a few characters ("+0.50"), may be empty.
    pub detail: String,
    /// For the colour of the row.
    pub kind: EditKind,
    /// Why the step no longer applies (shown, never dropped silently).
    pub skipped: Option<String>,
}

/// What a history rewrite did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RewriteReport {
    /// Steps that no longer apply, by their new position, with the reason.
    pub skipped: Vec<(usize, String)>,
    /// Steps deleted.
    pub removed: usize,
}

/// One step of a [`SavedHistory`].
#[derive(Debug, Clone, PartialEq)]
pub struct SavedStep {
    /// The edit, as it was applied.
    pub edit: Edit,
    /// Nodes the step added, in id order (needed to rewrite the history
    /// later: steps acting on them depend on this one).
    pub created: Vec<NodeId>,
    /// Why the step no longer applied when it was saved; `None` when it applied.
    pub skipped: Option<String>,
}

/// A history in a form that can be stored and replayed: the recipe it
/// starts from, the steps, and what Redo would redo. Undoing a history
/// rewrite is not kept: the rewritten steps are the history.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedHistory {
    /// The recipe the history starts from.
    pub base: Recipe,
    /// The steps, oldest first.
    pub steps: Vec<SavedStep>,
    /// The edits Redo would redo, the next one first.
    pub redo: Vec<Edit>,
}

/// A recipe being edited, with its history and undo/redo.
///
/// The history is the list of steps from the opened recipe (the *base*) to
/// the current one. Steps can be reordered or deleted
/// ([`Self::rewrite_history`]): the remaining steps are applied again from
/// the base, and a step that no longer makes sense (it changes a node made
/// by a deleted step, for example) stays in the list, marked as skipped,
/// instead of disappearing. Rewrites are undone with Ctrl+Z like any edit.
#[derive(Debug, Clone)]
pub struct EditSession {
    base: Recipe,
    recipe: Recipe,
    steps: Vec<Step>,
    done: Vec<Done>,
    undone: Vec<Undone>,
    next_gesture: u64,
}

impl EditSession {
    /// Starts editing `recipe` with an empty history.
    #[must_use]
    pub fn new(recipe: Recipe) -> Self {
        Self { base: recipe.clone(), recipe, steps: Vec::new(), done: Vec::new(), undone: Vec::new(), next_gesture: 0 }
    }

    /// Rebuilds a session from a saved history by applying its steps again
    /// on its base, with the same validation as new edits. The redo list
    /// keeps the edits that still apply one after the other.
    ///
    /// Returns `None` when a step that applied no longer does in this build
    /// (a tool is missing), or when the steps do not lead to `recipe`: the
    /// caller then starts from `recipe` with an empty history, so the edit
    /// itself is never lost.
    #[must_use]
    pub fn restore(registry: &ToolRegistry, recipe: &Recipe, saved: SavedHistory) -> Option<Self> {
        let mut current = saved.base.clone();
        let mut steps = Vec::with_capacity(saved.steps.len());
        for s in saved.steps {
            match s.skipped {
                Some(why) => steps.push(Step {
                    edit: s.edit,
                    undo: None,
                    gesture: None,
                    created: s.created,
                    skipped: Some(EditError::Saved(why)),
                }),
                None => {
                    let edit = validate(&current, registry, s.edit).ok()?;
                    let (undo, created) = perform(&mut current, &edit).ok()?;
                    steps.push(Step { edit, undo: Some(undo), gesture: None, created, skipped: None });
                }
            }
        }
        if current != *recipe {
            return None;
        }
        let mut probe = current.clone();
        let mut undone = Vec::new();
        for edit in saved.redo {
            let Ok(edit) = validate(&probe, registry, edit) else { break };
            if perform(&mut probe, &edit).is_err() {
                break;
            }
            undone.push(Undone::Step(Step { edit, undo: None, gesture: None, created: Vec::new(), skipped: None }));
        }
        // The next redo is the last entry.
        undone.reverse();
        let done = vec![Done::Step; steps.len()];
        let mut session = Self { base: saved.base, recipe: current, steps, done, undone, next_gesture: 0 };
        session.trim();
        Some(session)
    }

    /// The history in a form that can be stored and given back to
    /// [`Self::restore`].
    #[must_use]
    pub fn saved_history(&self) -> SavedHistory {
        let steps = self
            .steps
            .iter()
            .map(|s| SavedStep {
                edit: s.edit.clone(),
                created: s.created.clone(),
                skipped: s.skipped.as_ref().map(ToString::to_string),
            })
            .collect();
        // From the next redo down, until a history rewrite (not kept).
        let redo = self
            .undone
            .iter()
            .rev()
            .map_while(|u| match u {
                Undone::Step(s) => Some(s.edit.clone()),
                Undone::Rewrite(_) => None,
            })
            .collect();
        SavedHistory { base: self.base.clone(), steps, redo }
    }

    /// The current recipe.
    #[must_use]
    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    /// The recipe the history starts from.
    #[must_use]
    pub fn base(&self) -> &Recipe {
        &self.base
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
        let edit = validate(&self.recipe, registry, edit)?;
        let (undo, created) = perform(&mut self.recipe, &edit)?;
        self.undone.clear();
        if matches!(self.done.last(), Some(Done::Step))
            && let Some(top) = self.steps.last_mut()
            && gesture.is_some()
            && top.gesture == gesture
            && same_param(&top.edit, &edit)
        {
            // Keep the value from before the gesture began.
            top.edit = edit;
            return Ok(());
        }
        self.steps.push(Step { edit, undo: Some(undo), gesture, created, skipped: None });
        self.done.push(Done::Step);
        self.trim();
        Ok(())
    }

    /// Undoes the last step or history rewrite; returns its label, or `None`
    /// when there is nothing to undo.
    pub fn undo(&mut self) -> Option<String> {
        match self.done.pop()? {
            Done::Step => {
                let step = self.steps.pop()?;
                if let Some(undo) = &step.undo {
                    revert(&mut self.recipe, undo);
                }
                let label = step.edit.label();
                self.undone.push(Undone::Step(step));
                Some(label)
            }
            Done::Rewrite(r) => {
                self.restore_snapshot(r.before.clone());
                let label = r.label.clone();
                self.undone.push(Undone::Rewrite(r));
                Some(label)
            }
        }
    }

    /// Redoes the last undone step or rewrite; returns its label, or `None`
    /// when there is nothing to redo.
    pub fn redo(&mut self) -> Option<String> {
        match self.undone.pop()? {
            Undone::Step(mut step) => {
                // Undo restored the exact state the edit was validated against, so it cannot fail now.
                if let Ok((undo, created)) = perform(&mut self.recipe, &step.edit) {
                    let label = step.edit.label();
                    step.undo = Some(undo);
                    step.created = created;
                    self.steps.push(step);
                    self.done.push(Done::Step);
                    Some(label)
                } else {
                    self.undone.clear();
                    None
                }
            }
            Undone::Rewrite(r) => {
                self.restore_snapshot(r.after.clone());
                let label = r.label.clone();
                self.done.push(Done::Rewrite(r));
                Some(label)
            }
        }
    }

    /// Label of what [`Self::undo`] would undo.
    #[must_use]
    pub fn undo_label(&self) -> Option<String> {
        match self.done.last()? {
            Done::Step => self.steps.last().map(|s| s.edit.label()),
            Done::Rewrite(r) => Some(r.label.clone()),
        }
    }

    /// Label of what [`Self::redo`] would redo.
    #[must_use]
    pub fn redo_label(&self) -> Option<String> {
        match self.undone.last()? {
            Undone::Step(s) => Some(s.edit.label()),
            Undone::Rewrite(r) => Some(r.label.clone()),
        }
    }

    /// The history, oldest step first.
    #[must_use]
    pub fn history(&self) -> Vec<HistoryItem> {
        self.steps
            .iter()
            .map(|s| HistoryItem {
                label: s.edit.label(),
                detail: s.edit.detail(),
                kind: s.edit.kind(),
                skipped: s.skipped.as_ref().map(ToString::to_string),
            })
            .collect()
    }

    /// Rebuilds the history from the steps at positions `order` (current
    /// positions, in the new order); steps not named are deleted. The steps
    /// are applied again from the base; one that no longer applies stays in
    /// the history, marked as skipped. One undo step.
    ///
    /// # Errors
    /// [`EditError::BadHistoryOrder`] when `order` names a position twice or
    /// one past the end; nothing changes.
    pub fn rewrite_history(&mut self, registry: &ToolRegistry, order: &[usize]) -> Result<RewriteReport, EditError> {
        let mut seen = vec![false; self.steps.len()];
        for &i in order {
            match seen.get_mut(i) {
                Some(s) if !*s => *s = true,
                _ => return Err(EditError::BadHistoryOrder),
            }
        }
        let identity = order.len() == self.steps.len() && order.iter().enumerate().all(|(n, i)| n == *i);
        if identity {
            return Ok(RewriteReport::default());
        }
        let removed = self.steps.len() - order.len();

        // Nodes made by deleted steps are gone for the steps that follow.
        let mut map: HashMap<NodeId, Option<NodeId>> = HashMap::new();
        for (i, step) in self.steps.iter().enumerate() {
            if !seen[i] {
                for id in &step.created {
                    map.insert(*id, None);
                }
            }
        }

        let mut recipe = self.base.clone();
        let mut steps = Vec::with_capacity(order.len());
        let mut report = RewriteReport { skipped: Vec::new(), removed };
        for (position, &i) in order.iter().enumerate() {
            let old = &self.steps[i];
            let lookup = |id: NodeId| map.get(&id).copied().unwrap_or(Some(id));
            let applied = old
                .edit
                .map_nodes(lookup)
                .ok_or(EditError::NodeFromRemovedStep)
                .and_then(|edit| validate(&recipe, registry, edit))
                .and_then(|edit| perform(&mut recipe, &edit).map(|done| (edit, done)));
            match applied {
                Ok((edit, (undo, created))) => {
                    for (from, to) in old.created.iter().zip(&created) {
                        map.insert(*from, Some(*to));
                    }
                    steps.push(Step { edit, undo: Some(undo), gesture: None, created, skipped: None });
                }
                Err(why) => {
                    for id in &old.created {
                        map.insert(*id, None);
                    }
                    report.skipped.push((position, why.to_string()));
                    steps.push(Step {
                        edit: old.edit.clone(),
                        undo: None,
                        gesture: None,
                        created: old.created.clone(),
                        skipped: Some(why),
                    });
                }
            }
        }

        let label = if removed == 0 {
            "Reorder history".to_owned()
        } else if removed == 1 {
            "Delete a history step".to_owned()
        } else {
            format!("Delete {removed} history steps")
        };
        let before = Snapshot { base: self.base.clone(), recipe: self.recipe.clone(), steps: self.steps.clone() };
        let after = Snapshot { base: self.base.clone(), recipe, steps };
        self.restore_snapshot(after.clone());
        self.undone.clear();
        self.done.push(Done::Rewrite(Box::new(Rewrite { label, before, after })));
        self.trim();
        Ok(report)
    }

    fn restore_snapshot(&mut self, snapshot: Snapshot) {
        self.base = snapshot.base;
        self.recipe = snapshot.recipe;
        self.steps = snapshot.steps;
    }

    /// Keeps at most [`MAX_UNDO_STEPS`] steps and undo entries: the oldest
    /// step becomes part of the base.
    fn trim(&mut self) {
        while self.steps.len() > MAX_UNDO_STEPS {
            let oldest = self.steps.remove(0);
            if oldest.skipped.is_none() {
                // It applied on the base before, so it applies again.
                let _ = perform(&mut self.base, &oldest.edit);
            }
        }
        if self.done.len() > MAX_UNDO_STEPS {
            let excess = self.done.len() - MAX_UNDO_STEPS;
            self.done.drain(..excess);
        }
    }
}

/// Checks `edit` against `recipe` and the registry and normalises it (clamped values).
fn validate(recipe: &Recipe, registry: &ToolRegistry, edit: Edit) -> Result<Edit, EditError> {
    match edit {
        Edit::SetParam { node, param, value } => {
            let tool = tool_of(recipe, node)?;
            let d = registry.get(&tool).ok_or_else(|| EditError::UnknownTool(tool.clone()))?.descriptor();
            let p = d.param(&param).ok_or_else(|| EditError::UnknownParam(tool.clone(), param.clone()))?;
            let value = p.sanitize(value);
            Ok(Edit::SetParam { node, param, value })
        }
        Edit::SetParams { node, values } => {
            let tool = tool_of(recipe, node)?;
            let d = registry.get(&tool).ok_or_else(|| EditError::UnknownTool(tool.clone()))?.descriptor();
            let mut clean = Vec::with_capacity(values.len());
            for (param, value) in values {
                let p = d.param(&param).ok_or_else(|| EditError::UnknownParam(tool.clone(), param.clone()))?;
                clean.push((param, p.sanitize(value)));
            }
            Ok(Edit::SetParams { node, values: clean })
        }
        Edit::ResetParam { node, param } => {
            let tool = tool_of(recipe, node)?;
            let d = registry.get(&tool).ok_or_else(|| EditError::UnknownTool(tool.clone()))?.descriptor();
            if d.param(&param).is_none() {
                return Err(EditError::UnknownParam(tool, param));
            }
            Ok(Edit::ResetParam { node, param })
        }
        Edit::InsertSerial { after, tool, params } => {
            let Some(t) = registry.get(&tool) else { return Err(EditError::UnknownTool(tool)) };
            let d = t.descriptor();
            let mut clean = Params::new();
            for p in &d.params {
                if let Some(v) = params.get(&p.id) {
                    clean.set(p.id.clone(), p.sanitize(v));
                }
            }
            Ok(Edit::InsertSerial { after, tool, params: clean })
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
        Edit::SetGeometry(g) => Ok(Edit::SetGeometry(g.sanitized())),
        Edit::AddMask { to, def } => {
            let node = recipe.graph.node(to).ok_or(GraphError::UnknownNode(to))?;
            if !node.kind.inputs().contains(&PortKind::Mask) {
                return Err(EditError::NoMaskInput(to));
            }
            Ok(Edit::AddMask { to, def: def.sanitized() })
        }
        Edit::SetMask { node, def } => {
            let n = recipe.graph.node(node).ok_or(GraphError::UnknownNode(node))?;
            if n.kind != NodeKind::Mask {
                return Err(EditError::NotAMaskNode(node));
            }
            Ok(Edit::SetMask { node, def: def.sanitized() })
        }
        other => Ok(other),
    }
}

fn tool_of(recipe: &Recipe, node: NodeId) -> Result<ToolId, EditError> {
    match &recipe.graph.node(node).ok_or(GraphError::UnknownNode(node))?.kind {
        NodeKind::Tool(t) => Ok(t.clone()),
        _ => Err(EditError::NotAToolNode(node)),
    }
}

/// Applies an already validated edit to `recipe`; returns how to undo it
/// and the nodes it added. Leaves the recipe unchanged on error.
fn perform(recipe: &mut Recipe, edit: &Edit) -> Result<(Undo, Vec<NodeId>), EditError> {
    let graph = &mut recipe.graph;
    let undo = match edit {
        Edit::SetParam { node, param, value } => {
            let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
            let previous = n.params().set(param.clone(), *value);
            Undo::Param { node: *node, param: param.clone(), previous }
        }
        Edit::ResetParam { node, param } => {
            let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
            let previous = n.params().reset(param);
            Undo::Param { node: *node, param: param.clone(), previous }
        }
        Edit::SetParams { node, values } => {
            let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
            // Undone newest first, so a parameter named twice gets its first previous value back.
            let previous = values.iter().map(|(p, v)| (p.clone(), n.params().set(p.clone(), *v))).collect();
            Undo::Params { node: *node, previous }
        }
        Edit::SetEnabled { node, enabled } => {
            let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
            let previous = n.set_enabled(*enabled);
            Undo::Enabled { node: *node, previous }
        }
        Edit::SetWhiteBalance(wb) => Undo::WhiteBalance(std::mem::replace(&mut recipe.develop.white_balance, *wb)),
        Edit::SetCameraProfile(p) => Undo::Profile(std::mem::replace(&mut recipe.develop.profile, p.clone())),
        Edit::SetLensCorrection(l) => Undo::Lens(std::mem::replace(&mut recipe.develop.lens, l.clone())),
        Edit::SetGeometry(g) => Undo::Geometry(std::mem::replace(&mut recipe.geometry, *g)),
        Edit::SetMask { node, def } => {
            let mut n = graph.node_mut(*node).ok_or(GraphError::UnknownNode(*node))?;
            let slot = n.mask().ok_or(EditError::NotAMaskNode(*node))?;
            let previous = std::mem::replace(slot, def.clone());
            Undo::Mask { node: *node, previous }
        }
        Edit::AddMask { to, def } => {
            let before = graph.clone();
            let mut next = before.clone();
            let mask = next.add_mask_node(def.clone());
            let port = next
                .node(*to)
                .and_then(|n| n.kind.inputs().iter().position(|k| *k == PortKind::Mask))
                .and_then(|p| u8::try_from(p).ok())
                .ok_or(EditError::NoMaskInput(*to))?;
            next.connect(Link { from: mask, from_port: 0, to: *to, to_port: port })?;
            *graph = next;
            return Ok((Undo::Graph(Box::new(before.clone())), new_nodes(&before, graph)));
        }
        Edit::InsertSerial { .. } | Edit::RemoveNode { .. } | Edit::Connect(_) | Edit::Disconnect { .. } => {
            // Work on a copy and publish it only on success: the strong guarantee.
            let before = graph.clone();
            let mut next = before.clone();
            match edit {
                Edit::InsertSerial { after, tool, params } => {
                    next.insert_serial(*after, tool.clone(), params.clone())?;
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
            return Ok((Undo::Graph(Box::new(before.clone())), new_nodes(&before, graph)));
        }
    };
    Ok((undo, Vec::new()))
}

/// Nodes of `after` that `before` did not have, in id order.
fn new_nodes(before: &NodeGraph, after: &NodeGraph) -> Vec<NodeId> {
    let mut ids: Vec<NodeId> = after.nodes().map(|n| n.id).filter(|id| before.node(*id).is_none()).collect();
    ids.sort();
    ids
}

/// Restores what an applied edit changed.
fn revert(recipe: &mut Recipe, undo: &Undo) {
    match undo {
        Undo::Param { node, param, previous } => {
            if let Some(mut n) = recipe.graph.node_mut(*node) {
                match previous {
                    Some(v) => n.params().set(param.clone(), *v),
                    None => n.params().reset(param),
                };
            }
        }
        Undo::Params { node, previous } => {
            if let Some(mut n) = recipe.graph.node_mut(*node) {
                for (param, value) in previous.iter().rev() {
                    match value {
                        Some(v) => n.params().set(param.clone(), *v),
                        None => n.params().reset(param),
                    };
                }
            }
        }
        Undo::Enabled { node, previous } => {
            if let Some(mut n) = recipe.graph.node_mut(*node) {
                n.set_enabled(*previous);
            }
        }
        Undo::WhiteBalance(wb) => recipe.develop.white_balance = *wb,
        Undo::Profile(p) => recipe.develop.profile = p.clone(),
        Undo::Lens(l) => recipe.develop.lens = l.clone(),
        Undo::Geometry(g) => recipe.geometry = *g,
        Undo::Mask { node, previous } => {
            if let Some(mut n) = recipe.graph.node_mut(*node)
                && let Some(def) = n.mask()
            {
                *def = previous.clone();
            }
        }
        Undo::Graph(g) => recipe.graph = (**g).clone(),
    }
}

fn same_param(a: &Edit, b: &Edit) -> bool {
    matches!(
        (a, b),
        (Edit::SetParam { node: n1, param: p1, .. }, Edit::SetParam { node: n2, param: p2, .. })
            if n1 == n2 && p1 == p2
    ) || matches!((a, b), (Edit::SetMask { node: n1, .. }, Edit::SetMask { node: n2, .. }) if n1 == n2)
        || matches!((a, b), (Edit::SetGeometry(_), Edit::SetGeometry(_)))
        || matches!(
            (a, b),
            (Edit::SetParams { node: n1, values: v1 }, Edit::SetParams { node: n2, values: v2 })
                if n1 == n2 && v1.len() == v2.len() && v1.iter().zip(v2).all(|(x, y)| x.0 == y.0)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_in_the_history_have_a_sign_and_no_minus_zero() {
        assert_eq!(format_number(0.5), "+0.50");
        assert_eq!(format_number(-1.25), "-1.25");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(12.0), "+12");
        assert_eq!(format_number(-0.001), "0.00");
    }
}
