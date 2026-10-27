//! Tool and parameter descriptors, and parameters resolved against them.

use std::fmt;

use core_model::{ParamId, ParamValue, Params, ToolId};

/// Maximum parameters per tool: they travel to the GPU as `array<vec4<f32>, 4>`.
pub const MAX_PARAMS: usize = 16;

/// Where a tool lives in the recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// In the fixed development stage, at this position (lower runs first).
    Development(u16),
    /// As a node in the free graph.
    Graph,
}

/// Group shown in the UI's tool tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Exposure and tone.
    Exposure,
    /// Colour.
    Color,
    /// Sharpness, noise, clarity.
    Details,
    /// Lens corrections.
    Lens,
    /// Crop, rotation, perspective.
    Geometry,
    /// Masks and selections.
    Selection,
}

/// Unit shown next to a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// No unit.
    None,
    /// Photographic stops (EV).
    Stops,
    /// Percent.
    Percent,
    /// Degrees of angle.
    Degrees,
    /// Kelvin.
    Kelvin,
}

/// The type, range and default of one parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    /// A continuous value, edited with a slider and a number field.
    Float {
        /// Lowest accepted value.
        min: f32,
        /// Highest accepted value.
        max: f32,
        /// Value of a fresh node and after "Reset".
        default: f32,
        /// Step for arrow keys (Shift = 10 steps).
        step: f32,
        /// Unit shown next to the value.
        unit: Unit,
    },
    /// An on/off switch.
    Bool {
        /// Value of a fresh node.
        default: bool,
    },
    /// One option from a fixed list.
    Choice {
        /// Option labels, in order.
        options: Vec<&'static str>,
        /// Index of the default option.
        default: u32,
    },
}

/// One parameter of a tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDescriptor {
    /// Key in the recipe. Never changes once shipped.
    pub id: ParamId,
    /// Label shown in the UI, sentence case.
    pub label: &'static str,
    /// Type, range and default.
    pub kind: ParamKind,
}

/// Everything the editor knows about a tool without running it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDescriptor {
    /// Stable identifier; the key in saved recipes.
    pub id: ToolId,
    /// Version of the parameter layout, for [`crate::Tool::migrate`].
    pub version: u32,
    /// Name shown in the UI.
    pub name: &'static str,
    /// UI group.
    pub category: Category,
    /// Development stage or graph node.
    pub stage: Stage,
    /// Parameters, in UI order (also the GPU uniform order).
    pub params: Vec<ParamDescriptor>,
}

/// A descriptor that breaks the rules. Found at registration, so a broken
/// tool never reaches the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum DescriptorError {
    /// More than [`MAX_PARAMS`] parameters.
    TooManyParams(usize),
    /// Two parameters with the same id.
    DuplicateParam(ParamId),
    /// A float range that is empty, not finite, or does not contain its default.
    BadRange(ParamId),
    /// A choice with no options or a default outside them.
    BadChoice(ParamId),
}

impl fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyParams(n) => write!(f, "{n} parameters, the limit is {MAX_PARAMS}"),
            Self::DuplicateParam(id) => write!(f, "parameter '{id}' is declared twice"),
            Self::BadRange(id) => write!(f, "parameter '{id}' has an invalid range or default"),
            Self::BadChoice(id) => write!(f, "parameter '{id}' has no options or an invalid default"),
        }
    }
}

impl std::error::Error for DescriptorError {}

impl ToolDescriptor {
    /// Checks the descriptor's own consistency.
    ///
    /// # Errors
    /// The first [`DescriptorError`] found.
    pub fn validate(&self) -> Result<(), DescriptorError> {
        if self.params.len() > MAX_PARAMS {
            return Err(DescriptorError::TooManyParams(self.params.len()));
        }
        for (i, p) in self.params.iter().enumerate() {
            if self.params[..i].iter().any(|q| q.id == p.id) {
                return Err(DescriptorError::DuplicateParam(p.id.clone()));
            }
            match &p.kind {
                ParamKind::Float { min, max, default, step, .. } => {
                    let finite = [min, max, default, step].iter().all(|v| v.is_finite());
                    if !finite || min >= max || default < min || default > max || *step <= 0.0 {
                        return Err(DescriptorError::BadRange(p.id.clone()));
                    }
                }
                ParamKind::Choice { options, default } => {
                    if options.is_empty() || *default as usize >= options.len() {
                        return Err(DescriptorError::BadChoice(p.id.clone()));
                    }
                }
                ParamKind::Bool { .. } => {}
            }
        }
        Ok(())
    }

    /// The descriptor of parameter `id`.
    #[must_use]
    pub fn param(&self, id: &ParamId) -> Option<&ParamDescriptor> {
        self.params.iter().find(|p| &p.id == id)
    }
}

impl ParamDescriptor {
    /// The default value.
    #[must_use]
    pub fn default_value(&self) -> ParamValue {
        match &self.kind {
            ParamKind::Float { default, .. } => ParamValue::Float(*default),
            ParamKind::Bool { default } => ParamValue::Bool(*default),
            ParamKind::Choice { default, .. } => ParamValue::Choice(*default),
        }
    }

    /// Brings `value` into this parameter's domain: out-of-range floats are
    /// clamped to the nearest limit; values of the wrong type, non-finite
    /// floats and unknown choices become the default.
    #[must_use]
    pub fn sanitize(&self, value: ParamValue) -> ParamValue {
        match (&self.kind, value) {
            (ParamKind::Float { min, max, .. }, ParamValue::Float(v)) if v.is_finite() => {
                ParamValue::Float(v.clamp(*min, *max))
            }
            (ParamKind::Bool { .. }, ParamValue::Bool(b)) => ParamValue::Bool(b),
            (ParamKind::Choice { options, .. }, ParamValue::Choice(c)) if (c as usize) < options.len() => {
                ParamValue::Choice(c)
            }
            _ => self.default_value(),
        }
    }
}

/// A node's parameters checked against its tool: every declared parameter
/// has a valid value, in descriptor order. Tools read only this type.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedParams {
    values: Vec<(ParamId, ParamValue)>,
}

impl ResolvedParams {
    /// Resolves `params` against `descriptor`. Missing values take the default,
    /// invalid ones are sanitised, unknown keys are ignored.
    #[must_use]
    pub fn resolve(descriptor: &ToolDescriptor, params: &Params) -> Self {
        let values = descriptor
            .params
            .iter()
            .map(|p| {
                let v = params.get(&p.id).map_or_else(|| p.default_value(), |v| p.sanitize(v));
                (p.id.clone(), v)
            })
            .collect();
        Self { values }
    }

    fn value(&self, id: &str) -> ParamValue {
        // Asking for a parameter the tool did not declare is a bug in the tool,
        // caught by its unit tests.
        self.values
            .iter()
            .find(|(pid, _)| pid.as_str() == id)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| panic!("tool asked for undeclared parameter '{id}'"))
    }

    /// The value of float parameter `id`.
    ///
    /// # Panics
    /// When the tool did not declare `id` as a float (a bug in the tool).
    #[must_use]
    pub fn float(&self, id: &str) -> f32 {
        match self.value(id) {
            ParamValue::Float(v) => v,
            other => panic!("parameter '{id}' is {other:?}, not a float"),
        }
    }

    /// The value of boolean parameter `id`.
    ///
    /// # Panics
    /// When the tool did not declare `id` as a boolean (a bug in the tool).
    #[must_use]
    pub fn bool(&self, id: &str) -> bool {
        match self.value(id) {
            ParamValue::Bool(v) => v,
            other => panic!("parameter '{id}' is {other:?}, not a boolean"),
        }
    }

    /// The index of choice parameter `id`.
    ///
    /// # Panics
    /// When the tool did not declare `id` as a choice (a bug in the tool).
    #[must_use]
    pub fn choice(&self, id: &str) -> u32 {
        match self.value(id) {
            ParamValue::Choice(v) => v,
            other => panic!("parameter '{id}' is {other:?}, not a choice"),
        }
    }

    /// The values packed for the GPU, in descriptor order: floats as is,
    /// booleans as 0/1, choices as their index; unused slots are 0.
    #[must_use]
    pub fn gpu_uniforms(&self) -> [f32; MAX_PARAMS] {
        let mut out = [0.0; MAX_PARAMS];
        for (slot, (_, v)) in out.iter_mut().zip(&self.values) {
            *slot = match *v {
                ParamValue::Float(f) => f,
                ParamValue::Bool(b) => f32::from(u8::from(b)),
                #[allow(clippy::cast_precision_loss)] // choice indices are small
                ParamValue::Choice(c) => c as f32,
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> ParamId {
        ParamId::new(s).expect("valid id")
    }

    fn descriptor() -> ToolDescriptor {
        ToolDescriptor {
            id: ToolId::new("demo").expect("valid id"),
            version: 1,
            name: "Demo",
            category: Category::Exposure,
            stage: Stage::Graph,
            params: vec![
                ParamDescriptor {
                    id: pid("amount"),
                    label: "Amount",
                    kind: ParamKind::Float { min: -1.0, max: 1.0, default: 0.25, step: 0.01, unit: Unit::None },
                },
                ParamDescriptor { id: pid("invert"), label: "Invert", kind: ParamKind::Bool { default: false } },
                ParamDescriptor {
                    id: pid("method"),
                    label: "Method",
                    kind: ParamKind::Choice { options: vec!["Natural", "Punch"], default: 0 },
                },
            ],
        }
    }

    #[test]
    fn missing_values_resolve_to_defaults() {
        let r = ResolvedParams::resolve(&descriptor(), &Params::new());
        assert!((r.float("amount") - 0.25).abs() < f32::EPSILON);
        assert!(!r.bool("invert"));
        assert_eq!(r.choice("method"), 0);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_wrong_types_fall_back_to_default() {
        let mut p = Params::new();
        p.set(pid("amount"), ParamValue::Float(7.0));
        p.set(pid("invert"), ParamValue::Float(1.0));
        p.set(pid("method"), ParamValue::Choice(9));
        let r = ResolvedParams::resolve(&descriptor(), &p);
        assert!((r.float("amount") - 1.0).abs() < f32::EPSILON);
        assert!(!r.bool("invert"));
        assert_eq!(r.choice("method"), 0);
    }

    #[test]
    fn non_finite_floats_become_the_default() {
        let mut p = Params::new();
        p.set(pid("amount"), ParamValue::Float(f32::NAN));
        let r = ResolvedParams::resolve(&descriptor(), &p);
        assert!((r.float("amount") - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn gpu_uniforms_follow_descriptor_order() {
        let mut p = Params::new();
        p.set(pid("invert"), ParamValue::Bool(true));
        p.set(pid("method"), ParamValue::Choice(1));
        let u = ResolvedParams::resolve(&descriptor(), &p).gpu_uniforms();
        assert_eq!(&u[..4], &[0.25, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn invalid_descriptors_are_reported() {
        let mut d = descriptor();
        d.params.push(d.params[0].clone());
        assert_eq!(d.validate(), Err(DescriptorError::DuplicateParam(pid("amount"))));

        let mut d = descriptor();
        d.params[0].kind = ParamKind::Float { min: 0.0, max: 1.0, default: 2.0, step: 0.1, unit: Unit::None };
        assert_eq!(d.validate(), Err(DescriptorError::BadRange(pid("amount"))));

        let mut d = descriptor();
        d.params[2].kind = ParamKind::Choice { options: vec![], default: 0 };
        assert_eq!(d.validate(), Err(DescriptorError::BadChoice(pid("method"))));

        assert_eq!(descriptor().validate(), Ok(()));
    }

    #[test]
    #[should_panic(expected = "undeclared parameter")]
    fn asking_for_an_undeclared_parameter_is_a_bug() {
        let r = ResolvedParams::resolve(&descriptor(), &Params::new());
        let _ = r.float("nonexistent");
    }
}
