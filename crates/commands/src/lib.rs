//! Edits to a [`Recipe`], with validation and undo/redo.
//!
//! The UI, the MCP server and scripts never touch a recipe directly: they
//! build an [`Edit`] and give it to an [`EditSession`]. The session checks
//! it against the layer rules and the tool registry, applies it, and records
//! how to undo it. So every change, from any source, is validated the same
//! way and can be undone with Ctrl+Z.

use std::collections::HashMap;
use std::fmt;

use core_model::{
    BaseCurve, Geometry, Layer, LayerError, LayerId, Layers, LensSettings, MaskDef, ParamId, ParamValue, Params,
    ProfileChoice, Recipe, Style, StyleTool, ToolId, ToolRef, ToolSettings, Tools, WhiteBalance,
};
use tool_api::ToolRegistry;

/// Undo steps kept per image before the oldest are dropped.
pub const MAX_UNDO_STEPS: usize = 500;

/// One change to a recipe.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Sets one parameter of a tool in a layer (the tool joins the layer if
    /// it was not there). Out-of-range values are clamped.
    SetParam {
        /// The tool.
        at: ToolRef,
        /// The parameter.
        param: ParamId,
        /// The new value.
        value: ParamValue,
    },
    /// Sets several parameters of one tool in one step (a curve point's
    /// input and output, a white-balance mode with its kelvin and tint).
    /// Out-of-range values are clamped.
    SetParams {
        /// The tool.
        at: ToolRef,
        /// The parameters and their new values.
        values: Vec<(ParamId, ParamValue)>,
    },
    /// Returns one parameter to its default.
    ResetParam {
        /// The tool.
        at: ToolRef,
        /// The parameter.
        param: ParamId,
    },
    /// Takes a tool out of a layer: every value back to its default.
    ResetTool {
        /// The layer.
        layer: LayerId,
        /// The tool.
        tool: ToolId,
    },
    /// Switches a tool of a layer on or off (off keeps its values).
    SetToolEnabled {
        /// The layer.
        layer: LayerId,
        /// The tool.
        tool: ToolId,
        /// `false` leaves the image as the tool found it.
        enabled: bool,
    },
    /// Adds a layer at the top: the tools it starts with (a style brush's;
    /// none for an empty layer) act inside `mask`.
    AddLayer {
        /// Its name in the Layers panel.
        name: String,
        /// Where it acts (usually the first stroke).
        mask: MaskDef,
        /// Its tools. Unknown parameters are dropped, values clamped.
        tools: Vec<StyleTool>,
    },
    /// Deletes a layer with its mask and tools.
    RemoveLayer {
        /// The layer.
        layer: LayerId,
    },
    /// Renames a layer.
    RenameLayer {
        /// The layer.
        layer: LayerId,
        /// The new name.
        name: String,
    },
    /// Moves a layer up or down the list (0 = just above the Background).
    MoveLayer {
        /// The layer.
        layer: LayerId,
        /// Where it goes.
        index: usize,
    },
    /// Switches a layer on or off.
    SetLayerEnabled {
        /// The layer.
        layer: LayerId,
        /// `false`: the layer does nothing.
        enabled: bool,
    },
    /// Replaces a layer's mask (a new stroke, another component, feather,
    /// opacity, invert …). Feather and opacity are clamped.
    SetLayerMask {
        /// The layer.
        layer: LayerId,
        /// The new mask.
        mask: MaskDef,
    },
    /// Changes the white balance of the development stage.
    SetWhiteBalance(WhiteBalance),
    /// Chooses the camera profile of the development stage.
    SetCameraProfile(ProfileChoice),
    /// Changes the lens corrections of the development stage.
    SetLensCorrection(LensSettings),
    /// Changes the perspective, straightening and crop of the finished image.
    SetGeometry(Geometry),
    /// Chooses the tone curve of the output render (Base Characteristics).
    SetBaseCurve(BaseCurve),
    /// Applies a style (a preset) to the whole photo, as Capture One does:
    /// the development settings and geometry it holds replace the photo's,
    /// and its tools' values are set on the Background.
    ApplyStyle {
        /// The style's name (for the history).
        name: String,
        /// The style.
        style: Box<Style>,
    },
}

/// "Change …" for several parameters at once: the parameters of one item
/// of a list (`spot3_x`, `spot3_kind`) are named by the item ("Change
/// spot 4"); otherwise the first names, then how many more.
fn params_label<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    /// `spot3_x` → ("spot", 3); `None` when the id is not of that shape.
    fn item(id: &str) -> Option<(&str, u32)> {
        let (head, _) = id.split_once('_')?;
        let digits = head.find(|c: char| c.is_ascii_digit())?;
        let (word, number) = head.split_at(digits);
        (!word.is_empty()).then_some(())?;
        Some((word, number.parse().ok()?))
    }
    let ids: Vec<&str> = ids.collect();
    let items: Vec<Option<(&str, u32)>> = ids.iter().map(|id| item(id)).collect();
    if let Some(Some(first)) = items.first()
        && items.iter().all(|i| i.as_ref() == Some(first))
    {
        return format!("Change {} {}", first.0, first.1.saturating_add(1));
    }
    let names: Vec<String> = ids.iter().map(|id| id.replace('_', " ")).collect();
    match names.len() {
        0 => "Change settings".to_owned(),
        1..=3 => format!("Change {}", names.join(", ")),
        n => format!("Change {} and {} more", names[..2].join(", "), n - 2),
    }
}

/// A tool id as words ("white balance").
fn words(tool: &ToolId) -> String {
    tool.as_str().replace('_', " ")
}

impl Edit {
    /// Text for "Undo …" / "Redo …", sentence case.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::SetParam { param, .. } => format!("Change {}", param.as_str().replace('_', " ")),
            Self::ResetParam { param, .. } => format!("Reset {}", param.as_str().replace('_', " ")),
            Self::SetParams { values, .. } => params_label(values.iter().map(|(p, _)| p.as_str())),
            Self::ResetTool { tool, .. } => format!("Reset {}", words(tool)),
            Self::SetToolEnabled { tool, enabled: true, .. } => format!("Turn on {}", words(tool)),
            Self::SetToolEnabled { tool, enabled: false, .. } => format!("Turn off {}", words(tool)),
            Self::AddLayer { name, .. } => format!("Add layer \u{201c}{name}\u{201d}"),
            Self::RemoveLayer { .. } => "Delete layer".to_owned(),
            Self::RenameLayer { .. } => "Rename layer".to_owned(),
            Self::MoveLayer { .. } => "Move layer".to_owned(),
            Self::SetLayerEnabled { enabled: true, .. } => "Turn on layer".to_owned(),
            Self::SetLayerEnabled { enabled: false, .. } => "Turn off layer".to_owned(),
            Self::SetLayerMask { .. } => "Change mask".to_owned(),
            Self::SetWhiteBalance(_) => "Change white balance".to_owned(),
            Self::SetCameraProfile(_) => "Change camera profile".to_owned(),
            Self::SetLensCorrection(_) => "Change lens corrections".to_owned(),
            Self::SetGeometry(_) => "Change crop and straightening".to_owned(),
            Self::SetBaseCurve(_) => "Change base curve".to_owned(),
            Self::ApplyStyle { name, .. } => format!("Apply preset \u{201c}{name}\u{201d}"),
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
            Self::ResetParam { .. } | Self::ResetTool { .. } => "default".to_owned(),
            Self::RenameLayer { name, .. } => name.clone(),
            Self::SetWhiteBalance(WhiteBalance::AsShot) => "as shot".to_owned(),
            Self::SetWhiteBalance(WhiteBalance::Multipliers(m)) => {
                format!("{:.2} \u{b7} {:.2} \u{b7} {:.2}", m[0], m[1], m[2])
            }
            Self::SetCameraProfile(ProfileChoice::Named(name)) => name.clone(),
            Self::SetLensCorrection(lens) => on_off(lens.enabled).to_owned(),
            Self::SetBaseCurve(c) => c.label().to_owned(),
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
            Self::SetParam { .. }
            | Self::SetParams { .. }
            | Self::ResetParam { .. }
            | Self::ResetTool { .. }
            | Self::SetToolEnabled { .. } => EditKind::Adjust,
            Self::SetWhiteBalance(_)
            | Self::SetCameraProfile(_)
            | Self::SetLensCorrection(_)
            | Self::SetGeometry(_)
            | Self::SetBaseCurve(_)
            | Self::ApplyStyle { .. } => EditKind::Develop,
            Self::SetLayerMask { .. } => EditKind::Mask,
            Self::AddLayer { .. }
            | Self::RemoveLayer { .. }
            | Self::RenameLayer { .. }
            | Self::MoveLayer { .. }
            | Self::SetLayerEnabled { .. } => EditKind::Layer,
        }
    }

    /// The same edit with every layer id passed through `map` (the
    /// Background stays); `None` when `map` drops a layer the edit needs.
    fn map_layers(&self, map: impl Fn(LayerId) -> Option<LayerId>) -> Option<Self> {
        let map = |id: LayerId| if id.is_background() { Some(id) } else { map(id) };
        let at = |at: &ToolRef| Some(ToolRef { layer: map(at.layer)?, ..at.clone() });
        Some(match self {
            Self::SetParam { at: a, param, value } => {
                Self::SetParam { at: at(a)?, param: param.clone(), value: *value }
            }
            Self::ResetParam { at: a, param } => Self::ResetParam { at: at(a)?, param: param.clone() },
            Self::SetParams { at: a, values } => Self::SetParams { at: at(a)?, values: values.clone() },
            Self::ResetTool { layer, tool } => Self::ResetTool { layer: map(*layer)?, tool: tool.clone() },
            Self::SetToolEnabled { layer, tool, enabled } => {
                Self::SetToolEnabled { layer: map(*layer)?, tool: tool.clone(), enabled: *enabled }
            }
            Self::RemoveLayer { layer } => Self::RemoveLayer { layer: map(*layer)? },
            Self::RenameLayer { layer, name } => Self::RenameLayer { layer: map(*layer)?, name: name.clone() },
            Self::MoveLayer { layer, index } => Self::MoveLayer { layer: map(*layer)?, index: *index },
            Self::SetLayerEnabled { layer, enabled } => {
                Self::SetLayerEnabled { layer: map(*layer)?, enabled: *enabled }
            }
            Self::SetLayerMask { layer, mask } => Self::SetLayerMask { layer: map(*layer)?, mask: mask.clone() },
            Self::AddLayer { .. }
            | Self::SetWhiteBalance(_)
            | Self::SetCameraProfile(_)
            | Self::SetLensCorrection(_)
            | Self::SetGeometry(_)
            | Self::SetBaseCurve(_)
            | Self::ApplyStyle { .. } => self.clone(),
        })
    }
}

/// Groups of edits, for colouring the History panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Development stage: white balance, camera profile, lens corrections.
    Develop,
    /// A tool: its values, on or off.
    Adjust,
    /// The layers: add, delete, rename, move, on or off.
    Layer,
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
    /// The layer rules refused the change.
    Layer(LayerError),
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
    /// A history rewrite named a step twice or a step that does not exist.
    BadHistoryOrder,
    /// The step acts on a layer made by a step that was deleted or no longer applies.
    LayerFromRemovedStep,
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
            Self::Layer(e) => e.fmt(f),
            Self::UnknownTool(id) => write!(f, "tool '{id}' is not available in this build"),
            Self::UnknownParam(tool, p) => write!(f, "tool '{tool}' has no parameter '{p}'"),
            Self::InvalidWhiteBalance => write!(f, "white balance multipliers must be positive numbers"),
            Self::InvalidProfileName => write!(f, "a camera profile needs a name"),
            Self::InvalidLensSettings => {
                write!(f, "lens zoom must be between 0.25 and 4, and a chosen lens needs a name")
            }
            Self::BadHistoryOrder => write!(f, "the new history order names a step twice or a missing step"),
            Self::LayerFromRemovedStep => write!(f, "it acts on a layer made by a removed step"),
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
            Self::Layer(e) => Some(e),
            _ => None,
        }
    }
}

impl From<LayerError> for EditError {
    fn from(e: LayerError) -> Self {
        Self::Layer(e)
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
    /// A tool of a layer as it was (`None`: not in the layer).
    Tool {
        layer: LayerId,
        tool: ToolId,
        previous: Option<ToolSettings>,
    },
    /// A layer above the Background as it was (name, on/off, mask, tools).
    Layer(Box<Layer>),
    WhiteBalance(WhiteBalance),
    Profile(ProfileChoice),
    Lens(LensSettings),
    Geometry(Geometry),
    Curve(BaseCurve),
    /// Adding, deleting or moving layers keeps all the layers as they were:
    /// this restores ids and order exactly.
    Layers(Box<Layers>),
    /// Edits that change several parts keep the whole previous recipe.
    Recipe(Box<Recipe>),
}

/// One entry of the history.
#[derive(Debug, Clone, PartialEq)]
struct Step {
    edit: Edit,
    /// How to undo it; `None` when the step does not apply (see `skipped`).
    undo: Option<Undo>,
    gesture: Option<GestureId>,
    /// Layers this step added, in id order.
    created: Vec<LayerId>,
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
    Step(Box<Step>),
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
    /// Layers the step added, in id order (needed to rewrite the history
    /// later: steps acting on them depend on this one).
    pub created: Vec<LayerId>,
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
/// the base, and a step that no longer makes sense (it changes a layer made
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
            undone.push(Undone::Step(Box::new(Step {
                edit,
                undo: None,
                gesture: None,
                created: Vec::new(),
                skipped: None,
            })));
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
                self.undone.push(Undone::Step(Box::new(step)));
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
                    self.steps.push(*step);
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

        // Layers made by deleted steps are gone for the steps that follow.
        let mut map: HashMap<LayerId, Option<LayerId>> = HashMap::new();
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
            let lookup = |id: LayerId| map.get(&id).copied().unwrap_or(Some(id));
            let applied = old
                .edit
                .map_layers(lookup)
                .ok_or(EditError::LayerFromRemovedStep)
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
    let layers = &recipe.layers;
    let known_layer = |id: LayerId| if layers.contains(id) { Ok(()) } else { Err(LayerError::UnknownLayer(id)) };
    let slot_ok = |at: &ToolRef| {
        let slots = layers.tools(at.layer).and_then(|t| t.get(&at.tool)).map_or(1, |s| s.slots().len() + 1);
        if usize::from(at.slot) < slots { Ok(()) } else { Err(LayerError::NoSuchSlot(at.clone())) }
    };
    match edit {
        Edit::SetParam { at, param, value } => {
            known_layer(at.layer)?;
            slot_ok(&at)?;
            let d = registry.get(&at.tool).ok_or_else(|| EditError::UnknownTool(at.tool.clone()))?.descriptor();
            let p = d.param(&param).ok_or_else(|| EditError::UnknownParam(at.tool.clone(), param.clone()))?;
            let value = p.sanitize(value);
            Ok(Edit::SetParam { at, param, value })
        }
        Edit::SetParams { at, values } => {
            known_layer(at.layer)?;
            slot_ok(&at)?;
            let d = registry.get(&at.tool).ok_or_else(|| EditError::UnknownTool(at.tool.clone()))?.descriptor();
            let mut clean = Vec::with_capacity(values.len());
            for (param, value) in values {
                let p = d.param(&param).ok_or_else(|| EditError::UnknownParam(at.tool.clone(), param.clone()))?;
                clean.push((param, p.sanitize(value)));
            }
            Ok(Edit::SetParams { at, values: clean })
        }
        Edit::ResetParam { at, param } => {
            known_layer(at.layer)?;
            let d = registry.get(&at.tool).ok_or_else(|| EditError::UnknownTool(at.tool.clone()))?.descriptor();
            if d.param(&param).is_none() {
                return Err(EditError::UnknownParam(at.tool, param));
            }
            Ok(Edit::ResetParam { at, param })
        }
        Edit::ResetTool { layer, tool } => {
            known_layer(layer)?;
            Ok(Edit::ResetTool { layer, tool })
        }
        Edit::SetToolEnabled { layer, tool, enabled } => {
            known_layer(layer)?;
            if registry.get(&tool).is_none() {
                return Err(EditError::UnknownTool(tool));
            }
            Ok(Edit::SetToolEnabled { layer, tool, enabled })
        }
        Edit::AddLayer { name, mask, tools } => {
            let style = validate_style(recipe, registry, Style { tools, ..Style::default() })?;
            Ok(Edit::AddLayer { name: name.trim().to_owned(), mask: mask.sanitized(), tools: style.tools })
        }
        Edit::RemoveLayer { layer }
        | Edit::RenameLayer { layer, .. }
        | Edit::MoveLayer { layer, .. }
        | Edit::SetLayerEnabled { layer, .. }
        | Edit::SetLayerMask { layer, .. }
            if layer.is_background() =>
        {
            Err(LayerError::Background.into())
        }
        Edit::RemoveLayer { layer } | Edit::MoveLayer { layer, .. } | Edit::SetLayerEnabled { layer, .. }
            if !layers.contains(layer) =>
        {
            Err(LayerError::UnknownLayer(layer).into())
        }
        Edit::RenameLayer { layer, name } => {
            known_layer(layer)?;
            Ok(Edit::RenameLayer { layer, name: name.trim().to_owned() })
        }
        Edit::SetLayerMask { layer, mask } => {
            known_layer(layer)?;
            Ok(Edit::SetLayerMask { layer, mask: mask.sanitized() })
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
        Edit::SetLensCorrection(lens) => {
            let mut lens = lens.sanitized();
            lens.custom = lens.custom.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty());
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
        Edit::ApplyStyle { name, style } => {
            let style = Box::new(validate_style(recipe, registry, *style)?);
            Ok(Edit::ApplyStyle { name, style })
        }
        other => Ok(other),
    }
}

/// `style` with its values checked like the single edits that set them:
/// tools must exist, unknown parameters are dropped, values clamped, and a
/// tool named twice keeps its last values.
fn validate_style(recipe: &Recipe, registry: &ToolRegistry, mut style: Style) -> Result<Style, EditError> {
    let mut tools: Vec<StyleTool> = Vec::with_capacity(style.tools.len());
    for n in style.tools {
        let tool = registry.get(&n.tool).ok_or_else(|| EditError::UnknownTool(n.tool.clone()))?;
        let mut clean = Params::new();
        for p in &tool.descriptor().params {
            if let Some(v) = n.params.get(&p.id) {
                clean.set(p.id.clone(), p.sanitize(v));
            }
        }
        tools.retain(|t| t.tool != n.tool);
        tools.push(StyleTool { tool: n.tool, params: clean });
    }
    style.tools = tools;
    if let Some(wb) = style.white_balance {
        validate(recipe, registry, Edit::SetWhiteBalance(wb))?;
    }
    if let Some(p) = style.profile.take() {
        let Edit::SetCameraProfile(p) = validate(recipe, registry, Edit::SetCameraProfile(p))? else {
            unreachable!("validation keeps the kind of edit")
        };
        style.profile = Some(p);
    }
    if let Some(l) = style.lens.take() {
        let Edit::SetLensCorrection(l) = validate(recipe, registry, Edit::SetLensCorrection(l))? else {
            unreachable!("validation keeps the kind of edit")
        };
        style.lens = Some(l);
    }
    style.geometry = style.geometry.map(|g| g.sanitized());
    Ok(style)
}

/// Sets the values of `tools` on `target`, each over the values already there.
fn set_tools(target: &mut Tools, tools: &[StyleTool]) {
    for n in tools {
        if let Some(params) = target.params_mut(&n.tool, 0) {
            for (param, value) in n.params.iter() {
                params.set(param.clone(), *value);
            }
        }
    }
}

/// Applies a validated style to `recipe` (see [`Edit::ApplyStyle`]).
fn apply_style(recipe: &mut Recipe, style: &Style) {
    if let Some(wb) = style.white_balance {
        recipe.develop.white_balance = wb;
    }
    if let Some(p) = &style.profile {
        recipe.develop.profile = p.clone();
    }
    if let Some(l) = &style.lens {
        recipe.develop.lens = l.clone();
    }
    if let Some(g) = style.geometry {
        recipe.geometry = g;
    }
    set_tools(&mut recipe.layers.background, &style.tools);
}

/// Applies an already validated edit to `recipe`; returns how to undo it
/// and the layers it added. Leaves the recipe unchanged on error.
fn perform(recipe: &mut Recipe, edit: &Edit) -> Result<(Undo, Vec<LayerId>), EditError> {
    let layers = &mut recipe.layers;
    let tool_before = |layers: &Layers, layer: LayerId, tool: &ToolId| Undo::Tool {
        layer,
        tool: tool.clone(),
        previous: layers.tools(layer).and_then(|t| t.get(tool)).cloned(),
    };
    let layer_before = |layers: &Layers, id: LayerId| {
        layers.layer(id).map(|l| Undo::Layer(Box::new(l.clone()))).ok_or(LayerError::UnknownLayer(id))
    };
    let undo = match edit {
        Edit::SetParam { at, param, value } => {
            let undo = tool_before(layers, at.layer, &at.tool);
            layers.params_mut(at)?.set(param.clone(), *value);
            undo
        }
        Edit::SetParams { at, values } => {
            let undo = tool_before(layers, at.layer, &at.tool);
            let params = layers.params_mut(at)?;
            for (p, v) in values {
                params.set(p.clone(), *v);
            }
            undo
        }
        Edit::ResetParam { at, param } => {
            let undo = tool_before(layers, at.layer, &at.tool);
            if layers.params(at).is_some() {
                layers.params_mut(at)?.reset(param);
            }
            undo
        }
        Edit::ResetTool { layer, tool } => {
            let undo = tool_before(layers, *layer, tool);
            layers.tools_mut(*layer).ok_or(LayerError::UnknownLayer(*layer))?.remove(tool);
            undo
        }
        Edit::SetToolEnabled { layer, tool, enabled } => {
            let undo = tool_before(layers, *layer, tool);
            layers.tools_mut(*layer).ok_or(LayerError::UnknownLayer(*layer))?.set_enabled(tool, *enabled);
            undo
        }
        Edit::AddLayer { name, mask, tools } => {
            let before = layers.clone();
            let mut layer_tools = Tools::new();
            set_tools(&mut layer_tools, tools);
            let id = layers.add(name.clone(), mask.clone(), layer_tools);
            return Ok((Undo::Layers(Box::new(before)), vec![id]));
        }
        Edit::RemoveLayer { layer } => {
            let before = layers.clone();
            layers.remove(*layer)?;
            Undo::Layers(Box::new(before))
        }
        Edit::MoveLayer { layer, index } => {
            let before = layers.clone();
            layers.move_to(*layer, *index)?;
            Undo::Layers(Box::new(before))
        }
        Edit::RenameLayer { layer, name } => {
            let undo = layer_before(layers, *layer)?;
            if let Some(l) = layers.layer_mut(*layer) {
                l.name.clone_from(name);
            }
            undo
        }
        Edit::SetLayerEnabled { layer, enabled } => {
            let undo = layer_before(layers, *layer)?;
            if let Some(l) = layers.layer_mut(*layer) {
                l.enabled = *enabled;
            }
            undo
        }
        Edit::SetLayerMask { layer, mask } => {
            let undo = layer_before(layers, *layer)?;
            if let Some(l) = layers.layer_mut(*layer) {
                l.mask = mask.clone();
            }
            undo
        }
        Edit::SetWhiteBalance(wb) => Undo::WhiteBalance(std::mem::replace(&mut recipe.develop.white_balance, *wb)),
        Edit::SetCameraProfile(p) => Undo::Profile(std::mem::replace(&mut recipe.develop.profile, p.clone())),
        Edit::SetLensCorrection(l) => Undo::Lens(std::mem::replace(&mut recipe.develop.lens, l.clone())),
        Edit::SetGeometry(g) => Undo::Geometry(std::mem::replace(&mut recipe.geometry, *g)),
        Edit::SetBaseCurve(c) => Undo::Curve(std::mem::replace(&mut recipe.curve, *c)),
        Edit::ApplyStyle { style, .. } => {
            let before = recipe.clone();
            apply_style(recipe, style);
            Undo::Recipe(Box::new(before))
        }
    };
    Ok((undo, Vec::new()))
}

/// Restores what an applied edit changed.
fn revert(recipe: &mut Recipe, undo: &Undo) {
    match undo {
        Undo::Tool { layer, tool, previous } => {
            if let Some(tools) = recipe.layers.tools_mut(*layer) {
                match previous {
                    Some(settings) => tools.insert(tool.clone(), settings.clone()),
                    None => tools.remove(tool),
                };
            }
        }
        Undo::Layer(previous) => {
            if let Some(l) = recipe.layers.layer_mut(previous.id) {
                *l = (**previous).clone();
            }
        }
        Undo::WhiteBalance(wb) => recipe.develop.white_balance = *wb,
        Undo::Profile(p) => recipe.develop.profile = p.clone(),
        Undo::Lens(l) => recipe.develop.lens = l.clone(),
        Undo::Geometry(g) => recipe.geometry = *g,
        Undo::Curve(c) => recipe.curve = *c,
        Undo::Layers(l) => recipe.layers = (**l).clone(),
        Undo::Recipe(r) => *recipe = (**r).clone(),
    }
}

fn same_param(a: &Edit, b: &Edit) -> bool {
    matches!(
        (a, b),
        (Edit::SetParam { at: n1, param: p1, .. }, Edit::SetParam { at: n2, param: p2, .. })
            if n1 == n2 && p1 == p2
    ) || matches!((a, b), (Edit::SetLayerMask { layer: l1, .. }, Edit::SetLayerMask { layer: l2, .. }) if l1 == l2)
        || matches!((a, b), (Edit::SetGeometry(_), Edit::SetGeometry(_)))
        || matches!(
            (a, b),
            (Edit::SetParams { at: n1, values: v1 }, Edit::SetParams { at: n2, values: v2 })
                if n1 == n2 && v1.len() == v2.len() && v1.iter().zip(v2).all(|(x, y)| x.0 == y.0)
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_values_are_named_by_their_item_or_counted() {
        let ids = |v: &[&'static str]| params_label(v.iter().copied());
        assert_eq!(ids(&["spot3_x", "spot3_y", "spot3_kind"]), "Change spot 4");
        assert_eq!(ids(&["kelvin", "tint"]), "Change kelvin, tint");
        assert_eq!(ids(&["spot0_x", "spot1_x"]), "Change spot0 x, spot1 x");
        assert_eq!(ids(&["a", "b", "c", "d", "e"]), "Change a, b and 3 more");
        assert_eq!(ids(&[]), "Change settings");
    }

    #[test]
    fn numbers_in_the_history_have_a_sign_and_no_minus_zero() {
        assert_eq!(format_number(0.5), "+0.50");
        assert_eq!(format_number(-1.25), "-1.25");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(12.0), "+12");
        assert_eq!(format_number(-0.001), "0.00");
    }
}
