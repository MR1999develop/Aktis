//! The catalog windows: New Catalog (a name and the folder it goes in) and
//! Import Images (a folder of RAW files, with or without its sub-folders),
//! and the catalog opened at start-up.
//!
//! Photos are imported into the open catalog by reference, as in Capture
//! One's "Add to catalog": the RAW files stay where they are.

use std::path::{Path, PathBuf};

use catalog::{Day, FolderPattern};
use editor::catalogs;
use eframe::egui::{self, RichText};

use crate::dialog::{Choice, dialog, dialog_buttons, dialog_with_width};
use crate::settings::{ImportSettings, Settings};
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

/// The Import Images window's fields and the photos they select.
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

    /// The photos (RAW, JPEG, TIFF) in the source.
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

/// Width of the Import Images window.
const IMPORT_WIDTH: f32 = 560.0;

/// The Import Images window, importing into the catalog `catalog` with
/// `options` (kept for the next import); `presets` are the names offered.
pub fn import_dialog(
    ctx: &egui::Context,
    form: &mut Import,
    options: &mut ImportSettings,
    presets: &[String],
    catalog: &str,
) -> Outcome {
    let t = Tokens::of(ctx.theme());
    let mut choice = None;
    let mut files = options.files(form.found());
    dialog_with_width(ctx, "rne-import", IMPORT_WIDTH, |ui| {
        ui.label(RichText::new("Import Images").size(FONT[4]).strong());
        ui.add_space(SPACE[0]);
        ui.label(RichText::new(format!("Into the catalog \u{201c}{catalog}\u{201d}")).color(t.text_weak));
        ui.add_space(SPACE[2]);
        let max = (ui.ctx().content_rect().height() * 0.7).max(240.0);
        egui::ScrollArea::vertical().max_height(max).auto_shrink([false, true]).show(ui, |ui| {
            source_section(ui, &t, form);
            // The RAW+JPEG choice, when the source has pairs.
            let raw_only =
                catalog::without_jpeg_twins(form.found(), raw_io::has_raw_extension, raw_io::has_jpeg_extension);
            let pairs = form.found().len() - raw_only.len();
            if pairs > 0 {
                ui.horizontal(|ui| {
                    ui.label(format!("RAW + JPEG pairs ({pairs})"));
                    ui.radio_value(&mut options.jpeg_twins, true, "Import both");
                    ui.radio_value(&mut options.jpeg_twins, false, "Only the RAW file");
                });
            }
            files = options.files(form.found());
            destination_section(ui, &t, options);
            options_section(ui, &t, options, presets);
        });
        ui.add_space(SPACE[2]);
        let n = files.len();
        let problem = if n == 0 { Some("No photos to import".to_owned()) } else { options.problem() };
        if let Some(why) = &problem {
            ui.label(RichText::new(why).color(t.warning));
            ui.add_space(SPACE[1]);
        }
        let action = match n {
            1 => "Import 1 photo".to_owned(),
            n => format!("Import {n} photos"),
        };
        choice = dialog_buttons(ui, &t, problem.is_none().then_some(action.as_str()), "Cancel", |_| {});
    });
    match choice {
        Some(Choice::Action) => Outcome::Import(files),
        Some(Choice::Cancel) => Outcome::Cancelled,
        None => Outcome::Open,
    }
}

fn heading(ui: &mut egui::Ui, t: &Tokens, title: &str) {
    ui.add_space(SPACE[1]);
    ui.label(RichText::new(title).strong().color(t.text_strong));
}

/// A folder and its Choose… button; returns the folder chosen.
fn folder_row(ui: &mut egui::Ui, t: &Tokens, folder: Option<&Path>, title: &str) -> Option<PathBuf> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        if ui.button("Choose\u{2026}").clicked() {
            let mut pick = rfd::FileDialog::new().set_title(title);
            if let Some(dir) = folder.filter(|d| d.is_dir()) {
                pick = pick.set_directory(dir);
            }
            chosen = pick.pick_folder();
        }
        let shown = folder.map_or_else(|| "No folder chosen".to_owned(), |d| d.display().to_string());
        ui.add(egui::Label::new(RichText::new(shown).color(t.text_weak)).truncate());
    });
    chosen
}

fn source_section(ui: &mut egui::Ui, t: &Tokens, form: &mut Import) {
    heading(ui, t, "Import From");
    if let Some(dir) = folder_row(ui, t, Some(form.folder()), "Import images from a folder") {
        form.set_folder(dir);
    }
    let mut subfolders = form.subfolders();
    if ui.checkbox(&mut subfolders, "Include sub-folders").changed() {
        form.set_subfolders(subfolders);
    }
    let count = match form.found().len() {
        0 => "No photos here (RAW, DNG, JPEG, TIFF)".to_owned(),
        1 => "1 photo".to_owned(),
        n => format!("{n} photos"),
    };
    ui.label(RichText::new(count).size(FONT[0]).color(t.text_weak));
}

fn destination_section(ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings) {
    heading(ui, t, "Import To");
    ui.radio_value(&mut o.copy, false, "Add to the catalog: the files stay where they are");
    ui.radio_value(&mut o.copy, true, "Copy to a folder");
    if o.copy {
        ui.indent("rne-import-copy", |ui| {
            if let Some(dir) = folder_row(ui, t, o.copy_dir.as_deref(), "Folder for the copies") {
                o.copy_dir = Some(dir);
            }
            ui.horizontal(|ui| {
                ui.label("Sub-folders");
                ui.add(egui::TextEdit::singleline(&mut o.pattern).desired_width(200.0))
                    .on_hover_text("{yyyy} year, {yy} short year, {mm} month, {dd} day; / starts a sub-folder");
                if ui.small_button("Default").clicked() {
                    o.pattern = catalog::import::DEFAULT_PATTERN.to_owned();
                }
            });
            let example = FolderPattern::new(&o.pattern).folder_for(Day::from_exif("2026:10:29"));
            let example = if example.as_os_str().is_empty() {
                "Every copy straight in the folder".to_owned()
            } else {
                format!("A photo of 29 October 2026 goes in {}", example.display())
            };
            ui.label(RichText::new(example).size(FONT[0]).color(t.text_weak));
        });
    }
    ui.checkbox(&mut o.backup, "Back up the originals to a second folder");
    if o.backup {
        ui.indent("rne-import-backup", |ui| {
            if let Some(dir) = folder_row(ui, t, o.backup_dir.as_deref(), "Backup folder") {
                o.backup_dir = Some(dir);
            }
        });
    }
}

fn options_section(ui: &mut egui::Ui, t: &Tokens, o: &mut ImportSettings, presets: &[String]) {
    heading(ui, t, "Options");
    ui.checkbox(&mut o.skip_duplicates, "Leave out photos already in the catalog")
        .on_hover_text("The same file name and capture time, wherever the file is");
    ui.horizontal(|ui| {
        ui.label("Preset");
        let shown = o.preset.clone().unwrap_or_else(|| "None".to_owned());
        egui::ComboBox::from_id_salt("rne-import-preset").selected_text(shown).width(260.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut o.preset, None, "None");
            for name in presets {
                ui.selectable_value(&mut o.preset, Some(name.clone()), name.as_str());
            }
        });
    });
    heading(ui, t, "Metadata");
    egui::Grid::new("rne-import-metadata").num_columns(2).spacing([SPACE[2], SPACE[1]]).show(ui, |ui| {
        ui.label("Photographer");
        ui.add(egui::TextEdit::singleline(&mut o.artist).desired_width(300.0));
        ui.end_row();
        ui.label("Copyright");
        ui.add(egui::TextEdit::singleline(&mut o.copyright).desired_width(300.0).hint_text("\u{a9} 2026 Your Name"));
        ui.end_row();
        ui.label("Keywords");
        ui.add(egui::TextEdit::singleline(&mut o.keywords).desired_width(300.0).hint_text("sea, holiday"))
            .on_hover_text("Separated by commas");
        ui.end_row();
        ui.label("Rating");
        ui.horizontal(|ui| {
            for r in 0..=catalog::MAX_RATING {
                let label = if r == 0 { "None".to_owned() } else { "\u{2605}".repeat(usize::from(r)) };
                ui.selectable_value(&mut o.rating, r, label);
            }
        });
        ui.end_row();
    });
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
        assert_eq!(import_summary(3, 0, 0, false), "3 photos imported");
        assert_eq!(import_summary(1, 2, 1, false), "1 photo imported, 2 already in the catalog, 1 failed");
        assert_eq!(import_summary(0, 4, 0, false), "No photo imported, 4 already in the catalog");
        assert_eq!(import_summary(5, 0, 0, true), "Import stopped: 5 photos imported");
    }
}
