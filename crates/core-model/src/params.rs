//! Parameter values of a node, keyed by [`ParamId`].

use std::collections::BTreeMap;

use crate::ids::ParamId;

/// The value of one parameter. Which variant is valid for a parameter is
/// decided by the tool's descriptor (crate `tool-api`), not here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    /// A continuous value (exposure in stops, an amount, an angle).
    Float(f32),
    /// An on/off switch.
    Bool(bool),
    /// Index of the chosen option in a fixed list.
    Choice(u32),
}

/// The parameters of one node. Missing entries mean "use the default".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Params(BTreeMap<ParamId, ParamValue>);

impl Params {
    /// No parameters set: every value is its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored value, if any.
    #[must_use]
    pub fn get(&self, id: &ParamId) -> Option<ParamValue> {
        self.0.get(id).copied()
    }

    /// Stores `value` and returns the previous one.
    pub fn set(&mut self, id: ParamId, value: ParamValue) -> Option<ParamValue> {
        self.0.insert(id, value)
    }

    /// Removes the value, so the default applies again; returns the old value.
    pub fn reset(&mut self, id: &ParamId) -> Option<ParamValue> {
        self.0.remove(id)
    }

    /// Stored values in id order.
    pub fn iter(&self) -> impl Iterator<Item = (&ParamId, &ParamValue)> {
        self.0.iter()
    }
}
