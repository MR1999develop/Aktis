//! The catalog windows: New Catalog (a name and the folder it goes in) and
//! Import Images (a folder of RAW files, with or without its sub-folders),
//! and the catalog opened at start-up.
//!
//! Photos are imported into the open catalog by reference, as in Capture
//! One's "Add to catalog": the RAW files stay where they are.

use std::path::{Path, PathBuf};

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
}

impl NewCatalog {
    /// A form for a catalog in `folder`, with a name to replace.
    #[must_use]
    pub fn new(folder: PathBuf) -> Self {
        Self { name: "New Catalog".to_owned(), folder, error: None }
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

/// The Import Images window's fields and the RAW files they select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    folder: PathBuf,
    subfolders: bool,
    found: Vec<PathBuf>,
}

impl Import {
    /// An import from `folder`, sub-folders included as Capture One does.
    #[must_use]
    pub fn new(folder: PathBuf) -> Self {
        let mut import = Self { folder, subfolders: true, found: Vec::new() };
        import.scan();
        import
    }

    /// The folder the photos come from.
    #[must_use]
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Whether the sub-folders' photos are imported too.
    #[must_use]
    pub fn subfolders(&self) -> bool {
        self.subfolders
    }

    /// The RAW files the import adds.
    #[must_use]
    pub fn found(&self) -> &[PathBuf] {
        &self.found
    }

    /// Imports from `folder` instead.
    pub fn set_folder(&mut self, folder: PathBuf) {
        self.folder = folder;
        self.scan();
    }

    /// Includes the sub-folders or not.
    pub fn set_subfolders(&mut self, on: bool) {
        if on != self.subfolders {
            self.subfolders = on;
            self.scan();
        }
    }

    fn scan(&mut self) {
        self.found = catalogs::raw_files(&self.folder, self.subfolders);
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
    /// Import these photos.
    Import(Vec<PathBuf>),
}

/// The New Catalog window.
pub fn new_catalog_dialog(ctx: &egui::Context, form: &mut NewCatalog) -> Outcome {
    let t = Tokens::of(ctx.theme());
    let mut choice = None;
    let target = form.target();
    dialog(ctx, "rne-new-catalog", |ui| {
        ui.label(RichText::new("New Catalog").size(FONT[4]).strong());
        ui.add_space(SPACE[1]);
        ui.label(
            RichText::new("A catalog keeps the ratings, keywords and edits of the photos imported into it.")
                .color(t.text_weak),
        );
        ui.add_space(SPACE[2]);
        ui.label("Name");
        let field = ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(f32::INFINITY));
        if field.changed() {
            form.error = None;
        }
        if !field.has_focus() && form.name == "New Catalog" {
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
        choice = dialog_buttons(ui, &t, target.is_ok().then_some("Create"), "Cancel", |_| {});
    });
    match choice {
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

/// The Import Images window, importing into the catalog `catalog`.
pub fn import_dialog(ctx: &egui::Context, form: &mut Import, catalog: &str) -> Outcome {
    let t = Tokens::of(ctx.theme());
    let mut choice = None;
    dialog(ctx, "rne-import", |ui| {
        ui.label(RichText::new("Import Images").size(FONT[4]).strong());
        ui.add_space(SPACE[1]);
        ui.label(
            RichText::new(format!(
                "The photos are added to the catalog \u{201c}{catalog}\u{201d}; the files stay where they are."
            ))
            .color(t.text_weak),
        );
        ui.add_space(SPACE[2]);
        ui.label("Import from");
        ui.horizontal(|ui| {
            if ui.button("Choose\u{2026}").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(form.folder()).pick_folder()
            {
                form.set_folder(dir);
            }
            ui.add(egui::Label::new(RichText::new(form.folder().display().to_string()).color(t.text_weak)).truncate());
        });
        let mut subfolders = form.subfolders();
        if ui.checkbox(&mut subfolders, "Include sub-folders").changed() {
            form.set_subfolders(subfolders);
        }
        ui.add_space(SPACE[1]);
        let n = form.found().len();
        let count = match n {
            0 => "No RAW files here".to_owned(),
            1 => "1 RAW file".to_owned(),
            n => format!("{n} RAW files"),
        };
        ui.label(RichText::new(count).color(if n == 0 { t.warning } else { t.text_weak }));
        ui.add_space(SPACE[2]);
        choice = dialog_buttons(ui, &t, (n > 0).then_some("Import"), "Cancel", |_| {});
    });
    match choice {
        Some(Choice::Action) => Outcome::Import(form.found().to_vec()),
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
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_import_counts_follow_the_sub_folder_choice() {
        let dir = temp("import");
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("1.NEF"), b"").unwrap();
        std::fs::write(dir.join("b").join("2.NEF"), b"").unwrap();
        let mut form = Import::new(dir.clone());
        assert_eq!(form.found().len(), 2);
        form.set_subfolders(false);
        assert_eq!(form.found(), [dir.join("1.NEF")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_import_message_says_what_was_new() {
        assert_eq!(import_message(3, 3), "3 photos imported");
        assert_eq!(import_message(1, 1), "1 photo imported");
        assert_eq!(import_message(3, 1), "1 photo imported, 2 already in the catalog");
        assert_eq!(import_message(2, 0), "These photos are already in the catalog");
        assert_eq!(capitalized("a catalog"), "A catalog");
    }
}
