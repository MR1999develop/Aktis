//! The Disk part of the Library: what a right-click on a folder of the
//! computer offers — Import…, Cut and Paste (moving the folder), Rename…,
//! Delete… (to the system's wastebasket), Show in Explorer — with a
//! warning before each change. The catalog follows a folder moved or
//! renamed, so its photos keep their edits.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use super::{
    Button, Choice, FONT, Library, LibraryAction, RichText, SPACE, TextEdit, Tokens, dialog, dialog_buttons, egui,
    launch,
};

/// A change to a folder waiting for the user's yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DiskDialog {
    /// A new name for `path`.
    Rename { path: PathBuf, name: String, error: Option<String>, focused: bool },
    /// Move `path` to the wastebasket; `photos` of the catalog are inside;
    /// `remove` takes them out of the catalog too.
    Delete { path: PathBuf, photos: usize, remove: bool },
    /// Move `from` into `into`; `photos` of the catalog follow.
    Paste { from: PathBuf, into: PathBuf, photos: usize },
}

/// A folder being moved to the wastebasket, on a thread.
#[derive(Debug)]
pub(super) struct Deleting {
    path: PathBuf,
    remove: bool,
    done: Receiver<Result<(), String>>,
}

/// What the right-click menu of a disk folder asked for.
pub(super) enum FolderMenu {
    Import(PathBuf),
    Cut(PathBuf),
    Paste(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
}

/// The right-click menu of disk folder `path`; `cut` is the folder cut
/// earlier, if any.
pub(super) fn folder_menu(ui: &mut egui::Ui, path: &Path, cut: Option<&Path>) -> Option<FolderMenu> {
    let mut out = None;
    let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, why: &str, menu: FolderMenu| {
        if ui.add_enabled(enabled, Button::new(label)).on_disabled_hover_text(why).clicked() {
            out = Some(menu);
            ui.close();
        }
    };
    item(ui, true, "Import\u{2026}", "", FolderMenu::Import(path.to_owned()));
    ui.separator();
    item(ui, true, "Cut", "", FolderMenu::Cut(path.to_owned()));
    let paste_why = match cut {
        None => Some("Cut a folder first"),
        Some(c) if path.starts_with(c) => Some("A folder cannot go inside itself"),
        Some(c) if c.parent() == Some(path) => Some("The folder is already here"),
        Some(_) => None,
    };
    let label = cut.map_or_else(
        || "Paste".to_owned(),
        |c| format!("Paste \u{201c}{}\u{201d} here", editor::library::folder_name(c)),
    );
    item(ui, paste_why.is_none(), &label, paste_why.unwrap_or(""), FolderMenu::Paste(path.to_owned()));
    item(
        ui,
        path.parent().is_some(),
        "Rename\u{2026}",
        "A drive cannot be renamed",
        FolderMenu::Rename(path.to_owned()),
    );
    ui.separator();
    item(
        ui,
        path.parent().is_some(),
        "Delete\u{2026}",
        "A drive cannot be deleted",
        FolderMenu::Delete(path.to_owned()),
    );
    item(ui, true, "Show in Explorer", "", FolderMenu::Reveal(path.to_owned()));
    out
}

/// Why `name` cannot be the new name of a folder in `parent`, if it cannot.
pub(super) fn rename_problem(parent: &Path, old: &Path, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("The folder needs a name".to_owned());
    }
    if name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) || name == "." || name == ".." {
        return Some("A folder name cannot hold / \\ : * ? \" < > |".to_owned());
    }
    let new = parent.join(name);
    // Only the case changing is a rename too (Windows folders ignore case).
    if new.exists() && !old.to_string_lossy().eq_ignore_ascii_case(&new.to_string_lossy()) {
        return Some(format!("There is already a folder \u{201c}{name}\u{201d} here"));
    }
    None
}

impl Library {
    /// Does what the menu of a disk folder asked for.
    pub(super) fn folder_action(&mut self, menu: FolderMenu, actions: &mut Vec<LibraryAction>) {
        match menu {
            FolderMenu::Import(path) => actions.push(LibraryAction::ImportFolder(path)),
            FolderMenu::Cut(path) => self.disk_cut = Some(path),
            FolderMenu::Paste(into) => {
                if let Some(from) = self.disk_cut.clone() {
                    let photos = self.photos_below(&from);
                    self.disk_dialog = Some(DiskDialog::Paste { from, into, photos });
                }
            }
            FolderMenu::Rename(path) => {
                let name = editor::library::folder_name(&path);
                self.disk_dialog = Some(DiskDialog::Rename { path, name, error: None, focused: false });
            }
            FolderMenu::Delete(path) => {
                let photos = self.photos_below(&path);
                self.disk_dialog = Some(DiskDialog::Delete { path, photos, remove: true });
            }
            FolderMenu::Reveal(path) => {
                if let Err(e) = launch(&platform::reveal_in_file_manager(platform::Os::current(), &path)) {
                    actions.push(LibraryAction::Error(format!("Cannot open the folder: {e}")));
                }
            }
        }
    }

    fn photos_below(&self, dir: &Path) -> usize {
        self.catalog.as_ref().and_then(|c| c.count_below(dir).ok()).unwrap_or(0)
    }

    /// Folder `from` became `to` on the disk: the catalog, the tree and the
    /// filmstrip follow.
    fn folder_moved(&mut self, from: &Path, to: &Path, actions: &mut Vec<LibraryAction>) {
        if let Some(c) = self.catalog.as_mut()
            && let Err(e) = c.move_folder(from, to)
        {
            actions.push(LibraryAction::Error(format!("The catalog could not follow the folder: {e}")));
        }
        for dir in [from.parent(), to.parent()].into_iter().flatten() {
            self.relist.push(dir.to_owned());
        }
        if let Some(shown) = self.strip.folder.clone()
            && self.collection.is_none()
            && let Ok(rest) = shown.strip_prefix(from)
        {
            self.loading = Some(to.join(rest));
            self.relist.push(to.join(rest));
        }
        self.reshow_catalog();
        actions.push(LibraryAction::FolderMoved { from: from.to_owned(), to: to.to_owned() });
    }

    /// The catalog's collections read again, the one shown too.
    fn reshow_catalog(&mut self) {
        self.refresh_collections();
        if let Some(c) = self.collection.clone() {
            self.show_collection(c);
        }
    }

    /// The warnings before a folder changes, and the end of a deletion.
    pub(super) fn disk_dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<LibraryAction>) {
        self.poll_deleting(actions);
        let Some(d) = &mut self.disk_dialog else { return };
        let t = Tokens::of(ctx.theme());
        let mut choice = None;
        match d {
            DiskDialog::Rename { path, name, error, focused } => {
                let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let problem = rename_problem(&parent, path, name);
                dialog(ctx, "aktis-rename-folder", |ui| {
                    ui.label(RichText::new("Rename Folder").size(FONT[4]).strong());
                    ui.add_space(SPACE[1]);
                    ui.label(RichText::new(path.display().to_string()).color(t.text_weak));
                    ui.add_space(SPACE[1]);
                    let field = ui.add(TextEdit::singleline(name).desired_width(f32::INFINITY));
                    if !*focused {
                        field.request_focus();
                        *focused = true;
                    }
                    if field.changed() {
                        *error = None;
                    }
                    if let Some(e) = error.as_ref().or(problem.as_ref()) {
                        ui.label(RichText::new(e).color(t.warning));
                    }
                    ui.label(
                        RichText::new("Photos of the catalog inside keep their edits.")
                            .size(FONT[0])
                            .color(t.text_weak),
                    );
                    ui.add_space(SPACE[2]);
                    choice = dialog_buttons(ui, &t, problem.is_none().then_some("Rename"), "Cancel", |_| {});
                });
            }
            DiskDialog::Delete { path, photos, remove } => {
                dialog(ctx, "aktis-delete-folder", |ui| {
                    let name = editor::library::folder_name(path);
                    ui.label(RichText::new(format!("Delete \u{201c}{name}\u{201d}?")).size(FONT[4]).strong());
                    ui.add_space(SPACE[1]);
                    ui.label(
                        RichText::new(format!(
                            "The folder and everything in it go to the Recycle Bin, from where they can be restored.\n{}",
                            path.display()
                        ))
                        .color(t.text_weak),
                    );
                    if *photos > 0 {
                        ui.add_space(SPACE[1]);
                        let n = if *photos == 1 { "1 photo".to_owned() } else { format!("{photos} photos") };
                        ui.checkbox(remove, format!("Also remove its {n} from the catalog"))
                            .on_hover_text("Otherwise they stay in the catalog without their files");
                    }
                    ui.add_space(SPACE[2]);
                    choice = dialog_buttons(ui, &t, Some("Delete"), "Cancel", |_| {});
                });
            }
            DiskDialog::Paste { from, into, photos } => {
                dialog(ctx, "aktis-move-folder", |ui| {
                    let (name, target) = (editor::library::folder_name(from), editor::library::folder_name(into));
                    ui.label(
                        RichText::new(format!("Move \u{201c}{name}\u{201d} into \u{201c}{target}\u{201d}?"))
                            .size(FONT[4])
                            .strong(),
                    );
                    ui.add_space(SPACE[1]);
                    ui.label(
                        RichText::new(format!("{}\n\u{2192} {}", from.display(), into.display())).color(t.text_weak),
                    );
                    if *photos > 0 {
                        let n = if *photos == 1 { "1 photo".to_owned() } else { format!("{photos} photos") };
                        ui.label(
                            RichText::new(format!("{n} of the catalog move with it and keep their edits."))
                                .size(FONT[0])
                                .color(t.text_weak),
                        );
                    }
                    ui.add_space(SPACE[2]);
                    choice = dialog_buttons(ui, &t, Some("Move"), "Cancel", |_| {});
                });
            }
        }
        match choice {
            Some(Choice::Cancel) => self.disk_dialog = None,
            Some(Choice::Action) => {
                if let Some(d) = self.disk_dialog.take() {
                    self.confirmed(d, actions);
                }
            }
            None => {}
        }
    }

    /// Carries out a change the user said yes to.
    fn confirmed(&mut self, d: DiskDialog, actions: &mut Vec<LibraryAction>) {
        match d {
            DiskDialog::Rename { path, name, .. } => {
                let Some(parent) = path.parent() else { return };
                let to = parent.join(name.trim());
                match std::fs::rename(&path, &to) {
                    Ok(()) => self.folder_moved(&path, &to, actions),
                    Err(e) => {
                        let error = Some(format!("The folder could not be renamed: {e}"));
                        self.disk_dialog = Some(DiskDialog::Rename { path, name, error, focused: true });
                    }
                }
            }
            DiskDialog::Paste { from, into, .. } => {
                let to = into.join(from.file_name().unwrap_or_default());
                if to.exists() {
                    actions.push(LibraryAction::Error(format!("There is already a folder {} there", to.display())));
                    return;
                }
                match std::fs::rename(&from, &to) {
                    Ok(()) => {
                        self.disk_cut = None;
                        self.folder_moved(&from, &to, actions);
                    }
                    // Moving to another drive needs a copy, not done yet.
                    Err(e) => actions.push(LibraryAction::Error(format!(
                        "The folder could not be moved (another drive is not supported yet): {e}"
                    ))),
                }
            }
            DiskDialog::Delete { path, remove, .. } => {
                let (tx, rx) = channel();
                let launch = platform::move_to_trash(platform::Os::current(), &path, true);
                let spawned = std::thread::Builder::new().name("aktis-trash-folder".into()).spawn(move || {
                    let result = std::process::Command::new(launch.program)
                        .args(&launch.args)
                        .output()
                        .map_err(|e| e.to_string())
                        .and_then(|o| {
                            if o.status.success() {
                                Ok(())
                            } else {
                                Err(String::from_utf8_lossy(&o.stderr).trim().to_owned())
                            }
                        });
                    let _ = tx.send(result);
                });
                match spawned {
                    Ok(_) => self.deleting = Some(Deleting { path, remove, done: rx }),
                    Err(e) => actions.push(LibraryAction::Error(format!("The folder could not be deleted: {e}"))),
                }
            }
        }
    }

    /// The end of a folder's move to the wastebasket.
    fn poll_deleting(&mut self, actions: &mut Vec<LibraryAction>) {
        let Some(d) = &self.deleting else { return };
        let Ok(result) = d.done.try_recv() else { return };
        let Some(d) = self.deleting.take() else { return };
        match result {
            Ok(()) if !d.path.exists() => {
                if let Some(parent) = d.path.parent() {
                    self.relist.push(parent.to_owned());
                }
                if self.collection.is_none() && self.strip.folder.as_ref().is_some_and(|f| f.starts_with(&d.path)) {
                    self.strip = editor::Filmstrip::default();
                }
                let gone = if d.remove {
                    self.catalog.as_mut().and_then(|c| c.remove_below(&d.path).ok()).unwrap_or_default()
                } else {
                    Vec::new()
                };
                self.reshow_catalog();
                if !gone.is_empty() {
                    actions.push(LibraryAction::Trashed { photos: gone, emptied: true, next: None });
                }
            }
            Ok(()) => actions.push(LibraryAction::Error("The folder was not deleted".to_owned())),
            Err(why) => actions.push(LibraryAction::Error(format!("The folder could not be deleted: {why}"))),
        }
    }

    /// Whether a folder is being deleted (shown under the Disk tree).
    #[must_use]
    pub(super) fn deleting(&self) -> Option<&Path> {
        self.deleting.as_ref().map(|d| d.path.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_folder_name_is_checked_before_renaming() {
        let dir = std::env::temp_dir().join(format!("aktis-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Trip")).ok();
        std::fs::create_dir_all(dir.join("Other")).ok();
        let old = dir.join("Trip");
        assert!(rename_problem(&dir, &old, "  ").is_some());
        assert!(rename_problem(&dir, &old, "a/b").is_some());
        assert!(rename_problem(&dir, &old, "Other").is_some(), "taken");
        assert!(rename_problem(&dir, &old, "Trip 2026").is_none());
        assert!(rename_problem(&dir, &old, "TRIP").is_none(), "only the case changes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
