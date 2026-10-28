//! The bar under the menu: Undo and Redo on the left, the tools that act
//! on the photo in the middle, Export on the right.

use eframe::egui::{self, Align, Button, FontId, Key, Layout, RichText};

use crate::actions::Command;
use crate::theme::{SPACE, Tokens};

/// A tool of the bar: it takes over the mouse in the viewer while active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerTool {
    /// Crop rectangle, aspect ratio, straighten.
    Crop,
    /// Keystone correction.
    Perspective,
    /// Rotation by a line drawn along the horizon.
    Level,
    /// Brush and gradient masks on the selected node.
    Masks,
    /// AI selection (subject, objects, people) on the selected node.
    AiSelect,
    /// Heal: remove spots by copying texture from nearby.
    Heal,
    /// Clone: copy pixels from another place.
    Clone,
}

impl ViewerTool {
    /// Name on the bar.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Crop => "Crop",
            Self::Perspective => "Perspective",
            Self::Level => "Straighten",
            Self::Masks => "Masks",
            Self::AiSelect => "AI Select",
            Self::Heal => "Heal",
            Self::Clone => "Clone",
        }
    }

    /// The key that switches the tool on and off.
    #[must_use]
    pub fn key(self) -> Key {
        match self {
            Self::Crop => Key::C,
            Self::Perspective => Key::K,
            Self::Level => Key::R,
            Self::Masks => Key::M,
            Self::AiSelect => Key::A,
            Self::Heal => Key::H,
            Self::Clone => Key::S,
        }
    }

    /// The key as shown in tooltips.
    #[must_use]
    pub fn key_name(self) -> &'static str {
        match self {
            Self::Crop => "C",
            Self::Perspective => "K",
            Self::Level => "R",
            Self::Masks => "M",
            Self::AiSelect => "A",
            Self::Heal => "H",
            Self::Clone => "S",
        }
    }

    /// What the tool does, for its tooltip.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            Self::Crop => "Crop and straighten the photo",
            Self::Perspective => "Correct converging lines",
            Self::Level => "Draw a line along the horizon or a vertical to straighten the photo",
            Self::Masks => "Paint or draw a gradient mask on the selected node",
            Self::AiSelect => "Select the subject, objects or people for the selected node",
            Self::Heal => "Click a spot to remove it",
            Self::Clone => "Copy one place onto another",
        }
    }
}

/// The tools of the bar in groups.
pub const TOOL_GROUPS: &[&[ViewerTool]] = &[
    &[ViewerTool::Crop, ViewerTool::Perspective, ViewerTool::Level],
    &[ViewerTool::Masks, ViewerTool::AiSelect],
    &[ViewerTool::Heal, ViewerTool::Clone],
];

/// What the bar needs to know.
#[derive(Debug, Clone, Default)]
pub struct BarState {
    /// A photo is open.
    pub has_doc: bool,
    /// What Undo would undo.
    pub undo: Option<String>,
    /// What Redo would redo.
    pub redo: Option<String>,
    /// An export is running.
    pub exporting: bool,
    /// Photos Export would export (the selection, else the open photo).
    pub export_count: usize,
    /// The tool in use.
    pub active: Option<ViewerTool>,
    /// The exposure warning is on.
    pub exposure_warning: bool,
}

/// What the user did on the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAction {
    /// A menu command (Undo, Redo, Export).
    Command(Command),
    /// A tool was switched on (`Some`) or off (`None`).
    Tool(Option<ViewerTool>),
    /// The exposure warning was ticked (`true`) or unticked.
    ExposureWarning(bool),
}

/// Width a text button will take.
fn button_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = FontId::proportional(ui.style().text_styles.get(&egui::TextStyle::Button).map_or(14.0, |f| f.size));
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE);
    galley.size().x + 2.0 * ui.spacing().button_padding.x
}

/// Draws the bar; returns what was pressed.
pub fn toolbar(ui: &mut egui::Ui, state: &BarState) -> Vec<BarAction> {
    let t = Tokens::of(ui.ctx().theme());
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        let left = ui.cursor().left();
        let full = ui.available_width();
        // Left: Undo, Redo.
        for (command, label, what) in
            [(Command::Undo, "Undo", state.undo.as_deref()), (Command::Redo, "Redo", state.redo.as_deref())]
        {
            let shortcut = ui.ctx().format_shortcut(&command.shortcuts()[0]);
            let hover = what.map_or_else(
                || format!("Nothing to {}", label.to_lowercase()),
                |w| format!("{label} {w} ({shortcut})"),
            );
            if ui
                .add_enabled(what.is_some(), Button::new(label))
                .on_hover_text(hover.as_str())
                .on_disabled_hover_text(hover.as_str())
                .clicked()
            {
                actions.push(BarAction::Command(command));
            }
        }
        // Middle: the tools, centred on the bar when there is room.
        let spacing = ui.spacing().item_spacing.x;
        let separators = TOOL_GROUPS.len().saturating_sub(1);
        #[allow(clippy::cast_precision_loss)] // a handful of buttons
        let tools_width =
            TOOL_GROUPS.iter().flat_map(|g| g.iter()).map(|tool| button_width(ui, tool.name()) + spacing).sum::<f32>()
                + separators as f32 * (2.0 * spacing + 1.0);
        let used = ui.cursor().left() - left;
        let start = ((full - tools_width) / 2.0).max(used + SPACE[3]);
        ui.add_space(start - used);
        for (g, group) in TOOL_GROUPS.iter().enumerate() {
            if g > 0 {
                ui.separator();
            }
            for &tool in *group {
                let on = state.active == Some(tool);
                let button = Button::selectable(on, tool.name());
                let hover = format!("{} ({})", tool.hint(), tool.key_name());
                if ui
                    .add_enabled(state.has_doc, button)
                    .on_hover_text(hover)
                    .on_disabled_hover_text("Open a photo first")
                    .clicked()
                {
                    actions.push(BarAction::Tool(if on { None } else { Some(tool) }));
                }
            }
        }
        // Right: Export.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let export = Button::new(RichText::new("Export\u{2026}").color(t.on_accent).strong()).fill(t.accent);
            let why = if state.export_count == 0 {
                Some("Open or select a photo first")
            } else if state.exporting {
                Some("An export is running")
            } else {
                None
            };
            let shortcut = ui.ctx().format_shortcut(&Command::Export.shortcuts()[0]);
            let what = if state.export_count > 1 {
                format!("Export the {} selected photos", state.export_count)
            } else {
                "Export".to_owned()
            };
            if ui
                .add_enabled(why.is_none(), export)
                .on_hover_text(format!("{what} with format, size, name and metadata options ({shortcut})"))
                .on_disabled_hover_text(why.unwrap_or_default())
                .clicked()
            {
                actions.push(BarAction::Command(Command::Export));
            }
            ui.add_space(SPACE[3]);
            let mut warn = state.exposure_warning;
            if ui
                .checkbox(&mut warn, "Exposure warning")
                .on_hover_text(format!(
                    "Show burnt highlights in red over the photo; blocked shadows in blue: Preferences ({})",
                    ui.ctx().format_shortcut(&Command::ExposureWarning.shortcuts()[0])
                ))
                .changed()
            {
                actions.push(BarAction::ExposureWarning(warn));
            }
        });
    });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_its_own_key() {
        let keys: Vec<&str> = TOOL_GROUPS.iter().flat_map(|g| g.iter().map(|t| t.key_name())).collect();
        for (i, k) in keys.iter().enumerate() {
            assert!(!keys[i + 1..].contains(k), "{k} is used twice");
        }
        assert_eq!(keys.len(), 7);
    }
}
