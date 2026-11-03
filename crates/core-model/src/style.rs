//! Styles: a chosen part of a photo's edits, kept to be applied to other
//! photos (Capture One's styles and Lightroom's presets), or painted on
//! with a brush (Capture One's style brushes).
//!
//! A style holds values, never layers or masks: the development settings
//! it was saved with (each one optional), the geometry (optional) and the
//! parameters of tools, by tool. Masks belong to one photo and are never
//! part of a style.

use crate::{Geometry, LensSettings, Params, ProfileChoice, Recipe, ToolId, WhiteBalance};

/// The values of one tool of a style.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleTool {
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
    /// Tools, each at most once (they act in the fixed order of the tools).
    pub tools: Vec<StyleTool>,
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
    /// The tools of the Background to include.
    pub tools: Vec<ToolId>,
}

impl Style {
    /// The part of `recipe` that `selection` names. The tools come from
    /// the Background (its first slot), in id order; tools it does not hold
    /// are left out.
    #[must_use]
    pub fn from_recipe(recipe: &Recipe, selection: &StyleSelection) -> Self {
        let develop = &recipe.develop;
        let tools = recipe
            .layers
            .background
            .iter()
            .filter(|(tool, _)| selection.tools.contains(tool))
            .map(|(tool, s)| StyleTool { tool: tool.clone(), params: s.params().clone() })
            .collect();
        Self {
            white_balance: selection.white_balance.then_some(develop.white_balance),
            profile: selection.profile.then(|| develop.profile.clone()),
            lens: selection.lens.then(|| develop.lens.clone()),
            geometry: selection.geometry.then_some(recipe.geometry),
            tools,
        }
    }

    /// Nothing would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.white_balance.is_none()
            && self.profile.is_none()
            && self.lens.is_none()
            && self.geometry.is_none()
            && self.tools.is_empty()
    }

    /// Only the tools: what a style brush can paint (development settings
    /// and geometry act on the whole photo).
    #[must_use]
    pub fn local(&self) -> Self {
        Self { tools: self.tools.clone(), ..Self::default() }
    }
}

/// Values saved as the defaults of some tools for one camera model, as
/// Capture One's Save as Defaults: a photo of that camera never edited
/// starts with these tools, one of these tools first used on such a
/// photo starts with these values, and resetting goes back to them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraDefaults {
    /// The camera ("Sony ILCE-7M3"), as the editor names the photo's camera.
    pub camera: String,
    /// One entry per tool, in the order they were first saved.
    pub tools: Vec<StyleTool>,
}

impl CameraDefaults {
    /// No defaults yet for `camera`.
    #[must_use]
    pub fn new(camera: impl Into<String>) -> Self {
        Self { camera: camera.into(), tools: Vec::new() }
    }

    /// The saved values of `tool`, if any.
    #[must_use]
    pub fn params(&self, tool: &ToolId) -> Option<&Params> {
        self.tools.iter().find(|n| n.tool == *tool).map(|n| &n.params)
    }

    /// Saves `params` as the defaults of `tool`, replacing earlier ones in place.
    pub fn set(&mut self, tool: ToolId, params: Params) {
        match self.tools.iter_mut().find(|n| n.tool == tool) {
            Some(n) => n.params = params,
            None => self.tools.push(StyleTool { tool, params }),
        }
    }

    /// Forgets the defaults of `tool`; `true` when there were some.
    pub fn remove(&mut self, tool: &ToolId) -> bool {
        let before = self.tools.len();
        self.tools.retain(|n| n.tool != *tool);
        self.tools.len() != before
    }

    /// `true` when nothing is saved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
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

    #[test]
    fn camera_defaults_keep_one_entry_per_tool() {
        let mut d = CameraDefaults::new("Sony ILCE-7M3");
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.3));
        d.set(tool("exposure"), p.clone());
        d.set(tool("hdr"), Params::new());
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.7));
        d.set(tool("exposure"), p.clone());
        assert_eq!(d.tools.len(), 2);
        assert_eq!(d.tools[0].tool, tool("exposure"), "replaced in place");
        assert_eq!(d.params(&tool("exposure")), Some(&p));
        assert!(d.remove(&tool("hdr")) && !d.remove(&tool("hdr")));
        assert!(!d.is_empty());
    }

    #[test]
    fn only_the_chosen_parts_are_kept() {
        let mut r = Recipe::default();
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.5));
        for (name, params) in [("exposure", p.clone()), ("curve", Params::new()), ("clarity", Params::new())] {
            *r.layers.background.params_mut(&tool(name), 0).unwrap() = params;
        }
        r.develop.white_balance = WhiteBalance::Multipliers([2.0, 1.0, 1.5]);
        let sel = StyleSelection {
            white_balance: true,
            tools: vec![tool("exposure"), tool("clarity")],
            ..StyleSelection::default()
        };
        let s = Style::from_recipe(&r, &sel);
        assert_eq!(s.white_balance, Some(WhiteBalance::Multipliers([2.0, 1.0, 1.5])));
        assert_eq!((s.profile.as_ref(), s.lens.as_ref(), s.geometry), (None, None, None));
        let tools: Vec<&str> = s.tools.iter().map(|n| n.tool.as_str()).collect();
        assert_eq!(tools, ["clarity", "exposure"]);
        assert_eq!(s.tools[1].params, p);
        assert!(!s.is_empty() && s.local().white_balance.is_none());
        assert!(Style::default().is_empty());
    }
}
