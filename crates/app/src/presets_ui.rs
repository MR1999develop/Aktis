//! Presets and style brushes in the window (Image ▸ Presets, Image ▸
//! Style brushes, the filmstrip's Apply Preset menu, the Masks tool's
//! Style brush menu), and the Save Preset window where the user chooses
//! what goes in. The files and the built-in brushes are in
//! [`editor::presets`].

use std::path::{Path, PathBuf};

use core_model::{Recipe, Style, StyleSelection};
use editor::presets::{BrushSettings, Entry, Library, NodeChoice, Preset};
use eframe::egui::{self, Button, RichText};
use tool_api::ToolRegistry;

use crate::dialog::{Choice, dialog, dialog_buttons};
use crate::theme::{FONT, SPACE, Tokens};

/// What the preset menus asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum PresetAction {
    /// Apply preset number `.0` (in [`PresetsUi::presets`] order) to the
    /// open photo or the selected photos.
    Apply(usize),
    /// Arm style brush number `.0` (in [`PresetsUi::brushes`] order).
    Arm(usize),
    /// Open the Save window for a preset (`false`) or a style brush (`true`).
    Save(bool),
    /// Show the presets folder in the file manager.
    ShowFolder,
    /// Read the presets folder again.
    Reload,
}

/// The Save Preset window while it is open.
#[derive(Debug, Clone)]
struct SaveDraft {
    name: String,
    brush: bool,
    recipe: Recipe,
    choices: Vec<NodeChoice>,
    selection: StyleSelection,
    settings: BrushSettings,
}

/// The presets of the window and the Save window.
#[derive(Debug, Default)]
pub struct PresetsUi {
    library: Library,
    save: Option<SaveDraft>,
}

/// "Group / Name", or the name for a preset at the top of the folder.
fn display_name(e: &Entry) -> String {
    if e.group.is_empty() { e.preset.name.clone() } else { format!("{} / {}", e.group, e.preset.name) }
}

impl PresetsUi {
    /// Reads the presets in `folder` again (with the built-in brushes);
    /// returns the files that could not be read.
    pub fn reload(&mut self, folder: Option<&Path>) -> Vec<String> {
        self.library = Library::scan(folder);
        self.library.problems.clone()
    }

    /// The presets, in menu order.
    #[must_use]
    pub fn presets(&self) -> Vec<&Entry> {
        self.library.presets().collect()
    }

    /// The style brushes, in menu order.
    #[must_use]
    pub fn brushes(&self) -> Vec<&Entry> {
        self.library.brushes().collect()
    }

    /// Names of the presets for the filmstrip's menu.
    #[must_use]
    pub fn preset_names(&self) -> Vec<String> {
        self.library.presets().map(display_name).collect()
    }

    /// Names of the style brushes for the Masks tool's menu.
    #[must_use]
    pub fn brush_names(&self) -> Vec<String> {
        self.library.brushes().map(display_name).collect()
    }

    /// The Image ▸ Presets menu. `apply_why` says why presets cannot be
    /// applied now (`None` when they can); `save_why` the same for saving.
    pub fn presets_menu(
        &self,
        ui: &mut egui::Ui,
        apply_why: Option<&str>,
        save_why: Option<&str>,
        actions: &mut Vec<PresetAction>,
    ) {
        let entries = self.presets();
        ui.add_enabled_ui(apply_why.is_none(), |ui| {
            ui.menu_button("Apply", |ui| {
                if entries.is_empty() {
                    ui.add_enabled(false, Button::new("No presets saved yet"));
                }
                grouped(ui, &entries, |i| actions.push(PresetAction::Apply(i)));
            });
        })
        .response
        .on_disabled_hover_text(apply_why.unwrap_or_default());
        if ui
            .add_enabled(save_why.is_none(), Button::new("Save preset\u{2026}"))
            .on_hover_text("Save chosen settings of this photo to apply them to others")
            .on_disabled_hover_text(save_why.unwrap_or_default())
            .clicked()
        {
            actions.push(PresetAction::Save(false));
            ui.close();
        }
        ui.separator();
        self.folder_items(ui, actions);
    }

    /// The Image ▸ Style brushes menu.
    pub fn brushes_menu(&self, ui: &mut egui::Ui, why: Option<&str>, actions: &mut Vec<PresetAction>) {
        let entries = self.brushes();
        ui.add_enabled_ui(why.is_none(), |ui| {
            grouped(ui, &entries, |i| actions.push(PresetAction::Arm(i)));
            ui.separator();
            if ui
                .button("Save style brush\u{2026}")
                .on_hover_text("Save the selected node's settings as a brush that paints them")
                .clicked()
            {
                actions.push(PresetAction::Save(true));
                ui.close();
            }
        })
        .response
        .on_disabled_hover_text(why.unwrap_or_default());
        ui.separator();
        self.folder_items(ui, actions);
    }

    fn folder_items(&self, ui: &mut egui::Ui, actions: &mut Vec<PresetAction>) {
        if ui.button("Show presets folder").clicked() {
            actions.push(PresetAction::ShowFolder);
            ui.close();
        }
        if ui.button("Reload presets").on_hover_text("Read the presets folder again").clicked() {
            actions.push(PresetAction::Reload);
            ui.close();
        }
        if !self.library.problems.is_empty() {
            ui.label(RichText::new(format!("{} files could not be read", self.library.problems.len())).weak())
                .on_hover_text(self.library.problems.join("\n"));
        }
    }

    /// Opens the Save window on the photo's `recipe`: every tool node
    /// without a mask chosen, nothing else. For a style brush, only
    /// `focus` (the selected node) is chosen when given, and `settings` is
    /// the brush it will paint with.
    pub fn open_save(
        &mut self,
        recipe: &Recipe,
        registry: &ToolRegistry,
        brush: bool,
        focus: Option<core_model::NodeId>,
        settings: BrushSettings,
    ) {
        let choices = editor::presets::node_choices(recipe, registry);
        let mut selection = editor::presets::default_selection(&choices);
        if brush && let Some(id) = focus.filter(|id| choices.iter().any(|c| c.node == *id)) {
            selection.nodes = vec![id];
        }
        let name = if brush { "My brush" } else { "My preset" }.to_owned();
        self.save = Some(SaveDraft { name, brush, recipe: recipe.clone(), choices, selection, settings });
    }

    /// Draws the Save window when open; returns the preset to write once
    /// the user confirms.
    pub fn show_save(&mut self, ctx: &egui::Context) -> Option<Preset> {
        let draft = self.save.as_mut()?;
        let t = Tokens::of(ctx.theme());
        let mut choice = None;
        dialog(ctx, "rne-save-preset", |ui| {
            let title = if draft.brush { "Save Style Brush" } else { "Save Preset" };
            ui.label(RichText::new(title).size(FONT[4]).strong());
            ui.add_space(SPACE[2]);
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(f32::INFINITY));
            });
            ui.horizontal(|ui| {
                ui.radio_value(&mut draft.brush, false, "Preset").on_hover_text("Applied to the whole photo");
                ui.radio_value(&mut draft.brush, true, "Style brush")
                    .on_hover_text("Painted with a brush on a new mask");
            });
            ui.add_space(SPACE[2]);
            ui.label(RichText::new("Include").strong().color(t.text_strong));
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                if !draft.brush {
                    let sel = &mut draft.selection;
                    ui.checkbox(&mut sel.white_balance, "White balance (development)");
                    ui.checkbox(&mut sel.profile, "Camera profile");
                    ui.checkbox(&mut sel.lens, "Lens corrections");
                    ui.checkbox(&mut sel.geometry, "Crop, straightening and perspective");
                    ui.separator();
                }
                if draft.choices.is_empty() {
                    ui.label(RichText::new("The photo has no adjustment nodes").color(t.text_weak));
                }
                for c in &draft.choices {
                    let mut on = draft.selection.nodes.contains(&c.node);
                    let label = if c.masked {
                        format!("{} \u{2014} values only, not its mask", c.name)
                    } else {
                        c.name.clone()
                    };
                    if ui.checkbox(&mut on, label).changed() {
                        if on {
                            draft.selection.nodes.push(c.node);
                        } else {
                            draft.selection.nodes.retain(|n| *n != c.node);
                        }
                    }
                }
            });
            if draft.brush {
                ui.add_space(SPACE[1]);
                let s = &draft.settings;
                ui.label(
                    RichText::new(format!(
                        "Brush: size {:.1} %, hardness {:.2}, flow {:.2} (the Masks tool's current brush)",
                        s.radius * 100.0,
                        s.hardness,
                        s.opacity
                    ))
                    .size(FONT[0])
                    .color(t.text_weak),
                );
            }
            ui.add_space(SPACE[3]);
            let style = draft_style(draft);
            let ready = !draft.name.trim().is_empty() && !style.is_empty();
            choice = dialog_buttons(ui, &t, ready.then_some("Save"), "Cancel", |ui| {
                if !ready {
                    ui.label(RichText::new("Give it a name and choose something to include").color(t.text_weak));
                }
            });
        });
        match choice {
            Some(Choice::Action) => {
                let draft = self.save.take()?;
                let style = draft_style(&draft);
                let brush = draft.brush.then_some(draft.settings);
                Some(Preset { name: draft.name.trim().to_owned(), style, brush })
            }
            Some(Choice::Cancel) => {
                self.save = None;
                None
            }
            None => None,
        }
    }
}

/// The style the draft would save (a brush keeps only tool nodes).
fn draft_style(draft: &SaveDraft) -> Style {
    let style = Style::from_recipe(&draft.recipe, &draft.selection);
    if draft.brush { style.local() } else { style }
}

/// Menu items for `entries`, in sub-menus by group; `chosen(i)` with the
/// index of the clicked one.
fn grouped(ui: &mut egui::Ui, entries: &[&Entry], mut chosen: impl FnMut(usize)) {
    let mut i = 0;
    while i < entries.len() {
        let group = entries[i].group.clone();
        let end = entries[i..].iter().position(|e| e.group != group).map_or(entries.len(), |n| i + n);
        if group.is_empty() {
            for (k, e) in entries[i..end].iter().enumerate() {
                if ui.button(e.preset.name.as_str()).clicked() {
                    chosen(i + k);
                    ui.close();
                }
            }
        } else {
            ui.menu_button(group.as_str(), |ui| {
                for (k, e) in entries[i..end].iter().enumerate() {
                    if ui.button(e.preset.name.as_str()).clicked() {
                        chosen(i + k);
                        ui.close();
                    }
                }
            });
        }
        i = end;
    }
}

/// The folder to show for "Show presets folder", made when missing.
///
/// # Errors
/// A message when there is no folder or it cannot be made.
pub fn ensure_folder(folder: Option<PathBuf>) -> Result<PathBuf, String> {
    let folder = folder.ok_or("this system names no data folder; choose one in Preferences")?;
    std::fs::create_dir_all(&folder).map_err(|e| format!("the folder {} cannot be made: {e}", folder.display()))?;
    Ok(folder)
}
