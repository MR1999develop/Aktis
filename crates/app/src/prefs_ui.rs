//! Edit ▸ Preferences…: a window to set how the program looks and works.
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

            ui.add_space(SPACE[3]);
            choice = dialog_buttons(ui, &t, Some("OK"), "Cancel", |_| {});
        });
        match choice {
            Some(Choice::Action) => {
                if let Some(d) = self.draft.take() {
                    settings.theme = d.theme;
                    settings.viewer_background = d.viewer_background;
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
