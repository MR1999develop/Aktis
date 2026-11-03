//! The edits after the fixed development stage, in layers as Capture One's.
//!
//! The Background layer holds the tools that act on the whole photo. Each
//! further layer has a mask and tools of its own: its tools act on the
//! image the layers below it made, and the result is mixed in through the
//! mask. Layers act from the bottom of the list (the first) to the top.
//!
//! In a layer every tool appears at most once and the tools always act in
//! the same order, which the tools themselves declare (the pipeline order
//! of the tool registry); a tool that is not in a layer does nothing there.
//! A tool may hold several slots of parameters that act one after another
//! (the retouch tool, whose spots fill one slot after the other).

use std::collections::BTreeMap;
use std::fmt;

use crate::ids::ToolId;
use crate::mask::MaskDef;
use crate::params::Params;

/// Identifier of a layer inside one photo's edits. Never reused there;
/// [`LayerId::BACKGROUND`] is the Background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LayerId(u32);

impl LayerId {
    /// The Background layer.
    pub const BACKGROUND: Self = Self(0);

    /// The raw number, for display and serialisation.
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }

    /// Rebuilds an id read back from storage. Only the layers decide whether
    /// it exists.
    #[must_use]
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// `true` for the Background.
    #[must_use]
    pub fn is_background(self) -> bool {
        self == Self::BACKGROUND
    }
}

impl fmt::Display for LayerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One tool in a layer, addressed by the editing commands.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolRef {
    /// The layer.
    pub layer: LayerId,
    /// The tool.
    pub tool: ToolId,
    /// The slot of parameters (0 for every tool but retouch).
    pub slot: u16,
}

impl ToolRef {
    /// Slot 0 of `tool` in `layer`.
    #[must_use]
    pub fn new(layer: LayerId, tool: ToolId) -> Self {
        Self { layer, tool, slot: 0 }
    }

    /// Slot 0 of `tool` on the Background.
    #[must_use]
    pub fn background(tool: ToolId) -> Self {
        Self::new(LayerId::BACKGROUND, tool)
    }

    /// The same tool and layer, another slot.
    #[must_use]
    pub fn with_slot(mut self, slot: u16) -> Self {
        self.slot = slot;
        self
    }
}

impl fmt::Display for ToolRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.layer.is_background() {
            write!(f, "{}", self.tool)?;
        } else {
            write!(f, "{} (layer {})", self.tool, self.layer)?;
        }
        if self.slot > 0 {
            write!(f, " #{}", self.slot + 1)?;
        }
        Ok(())
    }
}

/// Why a layer edit was refused. Nothing changed when it is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerError {
    /// No layer with this id.
    UnknownLayer(LayerId),
    /// The Background cannot be removed, moved, masked or switched off.
    Background,
    /// The slot is neither an existing one nor the next.
    NoSuchSlot(ToolRef),
    /// Edits read back from storage break a rule (the text says which).
    Malformed(&'static str),
}

impl fmt::Display for LayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownLayer(id) => write!(f, "layer {id} does not exist"),
            Self::Background => write!(f, "the Background layer cannot be removed, moved, masked or switched off"),
            Self::NoSuchSlot(at) => write!(f, "{at} does not exist"),
            Self::Malformed(why) => write!(f, "the stored layers are not valid: {why}"),
        }
    }
}

impl std::error::Error for LayerError {}

/// One tool of a layer: switched on or off, and its parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSettings {
    /// A tool switched off does nothing, but keeps its values.
    pub enabled: bool,
    /// The slots of parameters, acting in order; never empty.
    slots: Vec<Params>,
}

impl ToolSettings {
    /// Switched on, with one slot.
    #[must_use]
    pub fn new(params: Params) -> Self {
        Self { enabled: true, slots: vec![params] }
    }

    /// Rebuilds settings read back from storage.
    ///
    /// # Errors
    /// [`LayerError::Malformed`] when `slots` is empty.
    pub fn from_parts(enabled: bool, slots: Vec<Params>) -> Result<Self, LayerError> {
        if slots.is_empty() {
            return Err(LayerError::Malformed("a tool has no parameters"));
        }
        Ok(Self { enabled, slots })
    }

    /// The parameters of slot 0.
    #[must_use]
    pub fn params(&self) -> &Params {
        &self.slots[0]
    }

    /// All slots in order.
    #[must_use]
    pub fn slots(&self) -> &[Params] {
        &self.slots
    }
}

/// The tools of one layer, by tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tools(BTreeMap<ToolId, ToolSettings>);

impl Tools {
    /// No tool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The settings of `tool`, if it is in the layer.
    #[must_use]
    pub fn get(&self, tool: &ToolId) -> Option<&ToolSettings> {
        self.0.get(tool)
    }

    /// The parameters of a slot, if the tool and the slot exist.
    #[must_use]
    pub fn params(&self, tool: &ToolId, slot: u16) -> Option<&Params> {
        self.0.get(tool).and_then(|t| t.slots.get(usize::from(slot)))
    }

    /// The parameters of a slot to change. A missing tool is added for slot 0
    /// (switched on); the slot after the last is added empty; `None` when the
    /// slot is past that.
    pub fn params_mut(&mut self, tool: &ToolId, slot: u16) -> Option<&mut Params> {
        let slot = usize::from(slot);
        if slot > 0 && !self.0.contains_key(tool) {
            return None;
        }
        let settings = self.0.entry(tool.clone()).or_insert_with(|| ToolSettings::new(Params::new()));
        if slot == settings.slots.len() {
            settings.slots.push(Params::new());
        }
        settings.slots.get_mut(slot)
    }

    /// Switches `tool` on or off (adding it when missing); returns the state before.
    pub fn set_enabled(&mut self, tool: &ToolId, enabled: bool) -> bool {
        let settings = self.0.entry(tool.clone()).or_insert_with(|| ToolSettings::new(Params::new()));
        std::mem::replace(&mut settings.enabled, enabled)
    }

    /// Puts `settings` in place of the tool's; returns the earlier ones.
    pub fn insert(&mut self, tool: ToolId, settings: ToolSettings) -> Option<ToolSettings> {
        self.0.insert(tool, settings)
    }

    /// Takes `tool` out of the layer.
    pub fn remove(&mut self, tool: &ToolId) -> Option<ToolSettings> {
        self.0.remove(tool)
    }

    /// Removes the last slot of `tool` when there is more than one.
    pub fn pop_slot(&mut self, tool: &ToolId) -> Option<Params> {
        let settings = self.0.get_mut(tool)?;
        (settings.slots.len() > 1).then(|| settings.slots.pop()).flatten()
    }

    /// The tools in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&ToolId, &ToolSettings)> {
        self.0.iter()
    }

    /// No tool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What acts, in order: the tools switched on, the tools of `order`
    /// first in that order, then the others (unknown to the registry) in
    /// id order; each slot of a tool in turn.
    #[must_use]
    pub fn steps<'a>(&'a self, order: &[ToolId]) -> Vec<(&'a ToolId, u16, &'a Params)> {
        let known = order.iter().filter_map(|t| self.0.get_key_value(t));
        let unknown = self.0.iter().filter(|(t, _)| !order.contains(t));
        known
            .chain(unknown)
            .filter(|(_, s)| s.enabled)
            .flat_map(|(t, s)| {
                s.slots.iter().enumerate().map(move |(i, p)| (t, u16::try_from(i).unwrap_or(u16::MAX), p))
            })
            .collect()
    }
}

/// A layer above the Background: a mask, and the tools it mixes in.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Identifier, unique among the photo's layers.
    pub id: LayerId,
    /// Name in the Layers panel.
    pub name: String,
    /// A layer switched off does nothing.
    pub enabled: bool,
    /// Where the layer acts; its opacity is the layer's.
    pub mask: MaskDef,
    /// What it does there.
    pub tools: Tools,
}

/// The Background and the layers above it.
#[derive(Debug, Clone, PartialEq)]
pub struct Layers {
    /// The tools of the Background.
    pub background: Tools,
    layers: Vec<Layer>,
    next_id: u32,
}

impl Default for Layers {
    fn default() -> Self {
        Self::new()
    }
}

impl Layers {
    /// The largest next free id stored layers may have: far below
    /// `u32::MAX`, so adding layers one by one can never wrap round to the
    /// Background's id.
    pub const MAX_NEXT_ID: u32 = 1 << 30;

    /// Only an empty Background.
    #[must_use]
    pub fn new() -> Self {
        Self { background: Tools::new(), layers: Vec::new(), next_id: 1 }
    }

    /// Rebuilds layers read back from storage, after checking the rules the
    /// edits keep: ids unique, above the Background's and below `next_id`,
    /// which leaves room for new ids (at most [`Self::MAX_NEXT_ID`]).
    ///
    /// # Errors
    /// [`LayerError::Malformed`] naming the first rule broken.
    pub fn from_parts(background: Tools, layers: Vec<Layer>, next_id: u32) -> Result<Self, LayerError> {
        if next_id == 0 || next_id > Self::MAX_NEXT_ID {
            return Err(LayerError::Malformed("the next free layer id is out of range"));
        }
        for (i, layer) in layers.iter().enumerate() {
            if layer.id.is_background() || layer.id.0 >= next_id {
                return Err(LayerError::Malformed("a layer id is not between the Background's and the next free id"));
            }
            if layers[..i].iter().any(|l| l.id == layer.id) {
                return Err(LayerError::Malformed("two layers share an id"));
            }
        }
        Ok(Self { background, layers, next_id })
    }

    /// The id the next added layer will get (stored, so ids are never reused).
    #[must_use]
    pub fn next_id(&self) -> u32 {
        self.next_id
    }

    /// The layers above the Background, from the bottom.
    #[must_use]
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// A layer above the Background.
    #[must_use]
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// A layer above the Background, to change.
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Where a layer is in the list (0 = just above the Background).
    #[must_use]
    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// `true` for the Background and the layers that exist.
    #[must_use]
    pub fn contains(&self, id: LayerId) -> bool {
        id.is_background() || self.index_of(id).is_some()
    }

    /// The tools of a layer (the Background's for [`LayerId::BACKGROUND`]).
    #[must_use]
    pub fn tools(&self, id: LayerId) -> Option<&Tools> {
        if id.is_background() { Some(&self.background) } else { self.layer(id).map(|l| &l.tools) }
    }

    /// The tools of a layer, to change.
    pub fn tools_mut(&mut self, id: LayerId) -> Option<&mut Tools> {
        if id.is_background() { Some(&mut self.background) } else { self.layer_mut(id).map(|l| &mut l.tools) }
    }

    /// The parameters at `at`, if they exist.
    #[must_use]
    pub fn params(&self, at: &ToolRef) -> Option<&Params> {
        self.tools(at.layer)?.params(&at.tool, at.slot)
    }

    /// The parameters at `at`, to change; the tool is added when missing.
    ///
    /// # Errors
    /// [`LayerError::UnknownLayer`], or [`LayerError::NoSuchSlot`] when the
    /// slot is past the next free one.
    pub fn params_mut(&mut self, at: &ToolRef) -> Result<&mut Params, LayerError> {
        let tools = self.tools_mut(at.layer).ok_or(LayerError::UnknownLayer(at.layer))?;
        tools.params_mut(&at.tool, at.slot).ok_or_else(|| LayerError::NoSuchSlot(at.clone()))
    }

    /// Adds a layer at the top and returns its id.
    pub fn add(&mut self, name: String, mask: MaskDef, tools: Tools) -> LayerId {
        let id = LayerId(self.next_id);
        // Stored layers start at most at MAX_NEXT_ID and each add is one user action.
        self.next_id = self.next_id.checked_add(1).expect("fewer than 3 billion layers are ever added to a photo");
        self.layers.push(Layer { id, name, enabled: true, mask, tools });
        id
    }

    /// Puts back a layer taken out with [`Self::remove`] at `index` (clamped).
    ///
    /// # Errors
    /// [`LayerError::Malformed`] when its id is the Background's, in use or
    /// was never given out.
    pub fn restore(&mut self, index: usize, layer: Layer) -> Result<(), LayerError> {
        if layer.id.is_background() || layer.id.0 >= self.next_id || self.index_of(layer.id).is_some() {
            return Err(LayerError::Malformed("the layer's id cannot come back"));
        }
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
        Ok(())
    }

    /// Takes a layer out; returns where it was and the layer.
    ///
    /// # Errors
    /// [`LayerError::Background`] or [`LayerError::UnknownLayer`].
    pub fn remove(&mut self, id: LayerId) -> Result<(usize, Layer), LayerError> {
        if id.is_background() {
            return Err(LayerError::Background);
        }
        let index = self.index_of(id).ok_or(LayerError::UnknownLayer(id))?;
        Ok((index, self.layers.remove(index)))
    }

    /// Moves a layer to `index` (clamped); returns where it was.
    ///
    /// # Errors
    /// [`LayerError::Background`] or [`LayerError::UnknownLayer`].
    pub fn move_to(&mut self, id: LayerId, index: usize) -> Result<usize, LayerError> {
        let (from, layer) = self.remove(id)?;
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
        Ok(from)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{ParamId, ParamValue};

    fn tool(name: &str) -> ToolId {
        ToolId::new(name).unwrap()
    }

    fn exposure(v: f32) -> Params {
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(v));
        p
    }

    #[test]
    fn tools_act_in_the_given_order_then_the_unknown_ones_and_off_tools_do_nothing() {
        let mut t = Tools::new();
        for name in ["zeta", "curve", "exposure", "clarity"] {
            t.params_mut(&tool(name), 0).unwrap();
        }
        t.set_enabled(&tool("clarity"), false);
        let order = [tool("exposure"), tool("clarity"), tool("curve")];
        let steps: Vec<&str> = t.steps(&order).iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(steps, ["exposure", "curve", "zeta"]);
    }

    #[test]
    fn slots_fill_one_after_another_and_act_in_turn() {
        let mut t = Tools::new();
        let r = tool("retouch");
        assert!(t.params_mut(&r, 1).is_none(), "slot 1 before slot 0");
        *t.params_mut(&r, 0).unwrap() = exposure(1.0);
        *t.params_mut(&r, 1).unwrap() = exposure(2.0);
        assert!(t.params_mut(&r, 3).is_none());
        let slots: Vec<u16> = t.steps(std::slice::from_ref(&r)).iter().map(|s| s.1).collect();
        assert_eq!(slots, [0, 1]);
        assert_eq!(t.pop_slot(&r), Some(exposure(2.0)));
        assert_eq!(t.pop_slot(&r), None, "the first slot stays");
    }

    #[test]
    fn layers_keep_their_ids_through_remove_restore_and_move() {
        let mut l = Layers::new();
        let a = l.add("A".into(), MaskDef::default(), Tools::new());
        let b = l.add("B".into(), MaskDef::default(), Tools::new());
        assert_eq!(l.move_to(b, 0), Ok(1));
        assert_eq!(l.layers().iter().map(|x| x.id).collect::<Vec<_>>(), [b, a]);
        let (at, layer) = l.remove(b).unwrap();
        assert_eq!(l.add("C".into(), MaskDef::default(), Tools::new()).get(), 3, "ids are never reused");
        l.restore(at, layer.clone()).unwrap();
        assert_eq!(l.index_of(b), Some(0));
        assert!(l.restore(0, layer).is_err(), "twice");
        assert_eq!(l.remove(LayerId::BACKGROUND), Err(LayerError::Background));
        assert!(l.contains(LayerId::BACKGROUND) && l.tools(a).is_some());
        let at = ToolRef::new(a, tool("exposure"));
        *l.params_mut(&at).unwrap() = exposure(0.5);
        assert_eq!(l.params(&at), Some(&exposure(0.5)));
    }

    #[test]
    fn stored_layers_with_bad_ids_are_refused() {
        let layer = |id| Layer {
            id: LayerId(id),
            name: String::new(),
            enabled: true,
            mask: MaskDef::default(),
            tools: Tools::new(),
        };
        assert!(Layers::from_parts(Tools::new(), vec![layer(1), layer(2)], 3).is_ok());
        assert!(Layers::from_parts(Tools::new(), vec![layer(1), layer(1)], 3).is_err());
        assert!(Layers::from_parts(Tools::new(), vec![layer(0)], 3).is_err());
        assert!(Layers::from_parts(Tools::new(), vec![layer(3)], 3).is_err());
        assert!(Layers::from_parts(Tools::new(), Vec::new(), 0).is_err(), "0 is the Background's");
        assert!(Layers::from_parts(Tools::new(), Vec::new(), Layers::MAX_NEXT_ID).is_ok());
        assert!(Layers::from_parts(Tools::new(), Vec::new(), u32::MAX).is_err(), "would wrap round to 0");
    }
}
