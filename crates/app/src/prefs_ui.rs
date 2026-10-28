//! Edit ▸ Preferences…: a window to set how the program looks and works
//! and where the presets are kept.
//! The changes are made on a copy and kept with OK (Cancel or Esc drops them).

use std::path::PathBuf;

use eframe::egui::{self, RichText};

use crate::dialog::{Choice, dialog, dialog_buttons};
use crate::settings::{Settings, ThemeChoice, ViewerBackground};
use crate::theme::{FONT, SPACE, Tokens, viewer_background};

/// The Preferences window while it is open: the settings being changed.
#[derive(Debug, Default)]
pub struct Preferences {
    draft: Option<Settings>,
}

impl Preferences {
    /// Opens the window on a copy of `settings`.
    pub fn open(&mut self, settings: &Settings) {
        self.draft = Some(settings.clone());
    }

    /// Draws the window when open; with OK the changes go into `settings`
    /// and `true` is returned.
    pub fn show(&mut self, ctx: &egui::Context, settings: &mut Settings) -> bool {
        let Some(draft) = &mut self.draft else { return false };
        let t = Tokens::of(ctx.theme());
        let mut choice = None;
        dialog(ctx, "rne-preferences", |ui| {
            ui.label(RichText::new("Preferences").size(FONT[4]).strong());
            ui.add_space(SPACE[2]);

            section(ui, &t, "Appearance");
            ui.horizontal(|ui| {
                ui.label("Theme");
                for (c, label) in
                    [(ThemeChoice::System, "System"), (ThemeChoice::Light, "Light"), (ThemeChoice::Dark, "Dark")]
                {
                    ui.radio_value(&mut draft.theme, c, label);
                }
            });
            ui.add_space(SPACE[2]);

            section(ui, &t, "Viewer");
            ui.label("Background around the photo");
            ui.horizontal_wrapped(|ui| {
                for b in ViewerBackground::ALL {
                    let swatch = viewer_background(b, t.canvas);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, egui::CornerRadius::same(3), swatch);
                    ui.painter().rect_stroke(
                        rect,
                        egui::CornerRadius::same(3),
                        egui::Stroke::new(1.0, t.stroke_strong),
                        egui::StrokeKind::Inside,
                    );
                    ui.radio_value(&mut draft.viewer_background, b, b.label());
                    ui.add_space(SPACE[1]);
                }
            });
            ui.label(
                RichText::new("White, as in Capture One, gives more contrast around the photo.")
                    .size(FONT[0])
                    .color(t.text_weak),
            );
            ui.add_space(SPACE[2]);

            section(ui, &t, "Presets");
            ui.horizontal(|ui| {
                ui.label("Folder");
                let shown = draft.presets_folder().map_or_else(|| "None".to_owned(), |d| d.display().to_string());
                ui.label(RichText::new(shown).color(t.text_weak));
            });
            ui.horizontal(|ui| {
                if ui.button("Choose\u{2026}").clicked() {
                    let mut pick = rfd::FileDialog::new().set_title("Presets folder");
                    if let Some(dir) = draft.presets_folder().filter(|d| d.is_dir()) {
                        pick = pick.set_directory(dir);
                    }
                    if let Some(dir) = pick.pick_folder() {
                        draft.presets_dir = Some(PathBuf::from(dir));
                    }
                }
                if ui.add_enabled(draft.presets_dir.is_some(), egui::Button::new("Default")).clicked() {
                    draft.presets_dir = None;
                }
            });
            ui.label(
                RichText::new(
                    "Presets (.rnepreset) and style brushes (.rnebrush) are files in this folder; sub-folders are groups.",
                )
                .size(FONT[0])
                .color(t.text_weak),
            );

            ui.add_space(SPACE[3]);
            choice = dialog_buttons(ui, &t, Some("OK"), "Cancel", |_| {});
        });
        match choice {
            Some(Choice::Action) => {
                if let Some(d) = self.draft.take() {
                    settings.theme = d.theme;
                    settings.viewer_background = d.viewer_background;
                    settings.presets_dir = d.presets_dir;
                }
                true
            }
            Some(Choice::Cancel) => {
                self.draft = None;
                false
            }
            None => false,
        }
    }
}

fn section(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.label(RichText::new(title).strong().color(t.text_strong));
    ui.add_space(SPACE[0]);
}
