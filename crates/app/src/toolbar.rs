//! The bar under the menu: the tools that act on the photo (most still to
//! come, shown disabled with their key), Undo, Redo and Export.

use eframe::egui::{self, Align, Button, Layout, RichText};

use crate::actions::Command;
use crate::theme::{SPACE, Tokens};

/// The tools of the bar in groups: name and key.
pub const TOOL_GROUPS: &[&[(&str, &str)]] = &[
    &[("Crop", "C"), ("Perspective", "K"), ("Level", "L")],
    &[("Masks", "M"), ("AI Select", "A")],
    &[("Heal", "H"), ("Clone", "S")],
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
}

/// Draws the bar; the buttons pressed are added to `commands`.
pub fn toolbar(ui: &mut egui::Ui, state: &BarState, commands: &mut Vec<Command>) {
    let t = Tokens::of(ui.ctx().theme());
    ui.horizontal(|ui| {
        for (g, group) in TOOL_GROUPS.iter().enumerate() {
            if g > 0 {
                ui.separator();
            }
            for (name, key) in *group {
                ui.add_enabled(false, Button::new(*name))
                    .on_disabled_hover_text(format!("{name} ({key}): coming soon"));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let export = Button::new(RichText::new("Export").color(t.on_accent).strong()).fill(t.accent);
            let why = if !state.has_doc {
                Some("Open a photo first")
            } else if state.exporting {
                Some("An export is running")
            } else {
                None
            };
            let shortcut = ui.ctx().format_shortcut(&Command::Export.shortcuts()[0]);
            if ui
                .add_enabled(why.is_none(), export)
                .on_hover_text(format!("Export a JPEG ({shortcut})"))
                .on_disabled_hover_text(why.unwrap_or_default())
                .clicked()
            {
                commands.push(Command::Export);
            }
            ui.add_space(SPACE[2]);
            for (command, label, what) in
                [(Command::Redo, "Redo", state.redo.as_deref()), (Command::Undo, "Undo", state.undo.as_deref())]
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
                    commands.push(command);
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_its_own_key() {
        let keys: Vec<&str> = TOOL_GROUPS.iter().flat_map(|g| g.iter().map(|(_, k)| *k)).collect();
        for (i, k) in keys.iter().enumerate() {
            assert!(!keys[i + 1..].contains(k), "{k} is used twice");
        }
        assert_eq!(keys.len(), 7);
    }
}
