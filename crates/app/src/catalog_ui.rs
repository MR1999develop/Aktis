//! The New Catalog and Save Catalog As windows (a name and the folder it
//! goes in), the catalog
//! opened at start-up, and the messages of an import (the Import Images
//! window is `import_window`).

use std::path::PathBuf;

use editor::catalogs;
use eframe::egui::{self, RichText};

use crate::dialog::{Choice, dialog, dialog_buttons};
use crate::settings::Settings;
use crate::theme::{FONT, SPACE, Tokens};

/// The catalog to open at start-up: the one open when the editor was
/// closed, else `Default` in the catalogs folder (made the first time from
/// the single catalog of earlier versions). Returns it and, when there is
/// one, a message for the user.
#[must_use]
pub fn startup_catalog(settings: &Settings) -> (PathBuf, Option<String>) {
    if let Some(last) = settings.catalog.as_ref().filter(|c| catalogs::is_catalog(c)) {
        return (last.clone(), None);
    }
    let folder = settings.catalogs_folder().unwrap_or_else(|| ai_models::data_dir().join("Catalogs"));
    let legacy = ai_models::data_dir().join(catalog::FILE_NAME);
    match catalogs::default_catalog(&folder, Some(&legacy)) {
        Ok((catalog, true)) => (
            catalog,
            Some("Your ratings, keywords and edits are now in the catalog \u{201c}Default\u{201d}".to_owned()),
        ),
        Ok((catalog, false)) => (catalog, None),
        // The library reports that the catalog cannot be opened.
        Err(e) => (
            folder.join(format!("{}.{}", catalogs::DEFAULT_NAME, catalog::CATALOG_EXTENSION)),
            Some(format!("The catalogs folder {} cannot be used: {e}", folder.display())),
        ),
    }
}

/// The New Catalog window's fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCatalog {
    /// The name typed.
    pub name: String,
    /// The folder the catalog goes in.
    pub folder: PathBuf,
    /// Why the last Create failed, shown until the fields change.
    pub error: Option<String>,
    /// Save Catalog As: a copy of the open catalog, not an empty one.
    pub save_as: bool,
    /// The name has not been touched yet (it is selected for typing over).
    fresh: bool,
}

impl NewCatalog {
    /// A form for a catalog in `folder`, with a name to replace.
    #[must_use]
    pub fn new(folder: PathBuf) -> Self {
        Self { name: "New Catalog".to_owned(), folder, error: None, save_as: false, fresh: true }
    }

    /// A form for Save Catalog As, in `folder`.
    #[must_use]
    pub fn save_as(folder: PathBuf) -> Self {
        Self { name: "My Catalog".to_owned(), save_as: true, ..Self::new(folder) }
    }

    /// The catalog folder that would be made, or why it cannot be.
    ///
    /// # Errors
    /// The name is empty or already used in the folder.
    pub fn target(&self) -> Result<PathBuf, String> {
        catalogs::new_catalog_path(&self.folder, &self.name)
    }

    /// Makes the catalog's folder; returns it.
    ///
    /// # Errors
    /// As [`Self::target`], or the folder cannot be made.
    pub fn create(&self) -> Result<PathBuf, String> {
        let path = self.target()?;
        std::fs::create_dir_all(&path).map_err(|e| format!("{} cannot be made: {e}", path.display()))?;
        Ok(path)
    }
}

/// What a catalog window asks of the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still open.
    Open,
    /// Closed without doing anything.
    Cancelled,
    /// Open this catalog (just made).
    Catalog(PathBuf),
    /// Save a copy of the open catalog as this new catalog folder, then open it.
    SaveAs(PathBuf),
    /// Import these photos.
    Import(Vec<PathBuf>),
}

/// The New Catalog window.
pub fn new_catalog_dialog(ctx: &egui::Context, form: &mut NewCatalog) -> Outcome {
    let t = Tokens::of(ctx.theme());
    let mut choice = None;
    let target = form.target();
    let (title, intro, verb) = if form.save_as {
        (
            "Save Catalog As",
            "A copy of the open catalog, with all its photos, edits, ratings, keywords and albums, under a new \
             name; the copy is then the open catalog. The photo files stay where they are.",
            "Save",
        )
    } else {
        ("New Catalog", "A catalog keeps the ratings, keywords and edits of the photos imported into it.", "Create")
    };
    dialog(ctx, "rne-new-catalog", |ui| {
        ui.label(RichText::new(title).size(FONT[4]).strong());
        ui.add_space(SPACE[1]);
        ui.label(RichText::new(intro).color(t.text_weak));
        ui.add_space(SPACE[2]);
        ui.label("Name");
        let field = ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(f32::INFINITY));
        if field.changed() {
            form.error = None;
            form.fresh = false;
        }
        if !field.has_focus() && form.fresh {
            field.request_focus();
        }
        ui.add_space(SPACE[1]);
        ui.label("Location");
        ui.horizontal(|ui| {
            if ui.button("Choose\u{2026}").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(&form.folder).pick_folder()
            {
                form.folder = dir;
                form.error = None;
            }
            ui.add(egui::Label::new(RichText::new(form.folder.display().to_string()).color(t.text_weak)).truncate());
        });
        if let Some(why) = target.as_ref().err().or(form.error.as_ref()) {
            ui.add_space(SPACE[1]);
            ui.label(RichText::new(capitalized(why)).color(t.warning));
        }
        ui.add_space(SPACE[2]);
        choice = dialog_buttons(ui, &t, target.is_ok().then_some(verb), "Cancel", |_| {});
    });
    match choice {
        // The app makes the copy (it saves the open photo first).
        Some(Choice::Action) if form.save_as => match form.target() {
            Ok(path) => Outcome::SaveAs(path),
            Err(e) => {
                form.error = Some(e);
                Outcome::Open
            }
        },
        Some(Choice::Action) => match form.create() {
            Ok(path) => Outcome::Catalog(path),
            // Shown under the fields at the next frame (the folder exists now, say).
            Err(e) => {
                form.error = Some(e);
                Outcome::Open
            }
        },
        Some(Choice::Cancel) => Outcome::Cancelled,
        None => Outcome::Open,
    }
}

/// The toast after an import of `wanted` photos, `added` of them new.
#[must_use]
pub fn import_message(wanted: usize, added: usize) -> String {
    let photos = |n: usize| if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
    match (wanted, added) {
        (_, 0) => "These photos are already in the catalog".to_owned(),
        (w, a) if w == a => format!("{} imported", photos(a)),
        (w, a) => format!("{} imported, {} already in the catalog", photos(a), w - a),
    }
}

/// The toast at the end of an import job: `added` photos imported,
/// `duplicates` left out, `failed` that could not be copied, whether it was
/// `stopped`.
#[must_use]
pub fn import_summary(added: usize, duplicates: usize, failed: usize, stopped: bool) -> String {
    let photos = |n: usize| if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
    let mut text = match (added, stopped) {
        (0, true) => "Import stopped".to_owned(),
        (0, false) => "No photo imported".to_owned(),
        (n, true) => format!("Import stopped: {} imported", photos(n)),
        (n, false) => format!("{} imported", photos(n)),
    };
    if duplicates > 0 {
        text.push_str(&format!(", {duplicates} already in the catalog"));
    }
    if failed > 0 {
        text.push_str(&format!(", {failed} failed"));
    }
    text
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rne-catalog-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_catalog_is_made_once_under_its_name() {
        let dir = temp("new");
        let mut form = NewCatalog::new(dir.clone());
        form.name = "Travel".to_owned();
        let made = form.create().unwrap();
        assert!(catalogs::is_catalog(&made));
        assert!(form.target().is_err(), "the name is taken now");
        let copy = NewCatalog::save_as(dir.clone());
        assert!(copy.save_as && copy.target().is_ok_and(|p| p.ends_with("My Catalog.rnecatalog")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_import_message_says_what_was_new() {
        assert_eq!(import_message(3, 3), "3 photos imported");
        assert_eq!(import_message(1, 1), "1 photo imported");
        assert_eq!(import_message(3, 1), "1 photo imported, 2 already in the catalog");
        assert_eq!(import_message(2, 0), "These photos are already in the catalog");
        assert_eq!(capitalized("a catalog"), "A catalog");
        assert_eq!(import_summary(3, 0, 0, false), "3 photos imported");
        assert_eq!(import_summary(1, 2, 1, false), "1 photo imported, 2 already in the catalog, 1 failed");
        assert_eq!(import_summary(0, 4, 0, false), "No photo imported, 4 already in the catalog");
        assert_eq!(import_summary(5, 0, 0, true), "Import stopped: 5 photos imported");
    }
}
