//! The Library's catalog side, as Capture One's Library tool: Catalog
//! Collections (All Images, Recent Imports, Recent Edits, Trash), the
//! albums and the catalog's folder tree; moving photos to the Trash and
//! back; albums made, renamed and deleted, and photos put in them by
//! dragging them onto an album or from the card menu.

use super::CardAction;
use super::{
    Album, Button, Choice, Collection, FONT, FolderNode, Library, LibraryAction, Path, PathBuf, Rect, RichText, SPACE,
    Sense, Stroke, StrokeKind, TREE_INDENT, TextEdit, Tokens, choice, dialog, dialog_buttons, egui, icons, vec2,
};

/// What a filmstrip card carries while it is dragged: the photo (the
/// selection it belongs to goes with it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DraggedPhoto(pub(super) PathBuf);

/// The New Album / Rename Album window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AlbumDialog {
    /// The album renamed; `None` for a new one.
    id: Option<i64>,
    name: String,
    /// Photos the new album starts with.
    photos: Vec<PathBuf>,
    focused: bool,
}

impl Library {
    /// The catalog's collections, albums and folders.
    pub(super) fn collections(&mut self, ui: &mut egui::Ui, t: &Tokens, actions: &mut Vec<LibraryAction>) {
        let mut chosen = None;
        let shown = self.collection.clone();
        let on = |c: &Collection| shown.as_ref() == Some(c);
        let heading = |ui: &mut egui::Ui, text: &str| {
            ui.add_space(SPACE[0]);
            ui.label(RichText::new(text).size(FONT[0]).color(t.text_weak));
        };

        heading(ui, "Catalog Collections");
        if choice(ui, on(&Collection::All), format!("All Images  {}", self.counts.all)).clicked() {
            chosen = Some(Collection::All);
        }
        egui::CollapsingHeader::new(format!("Recent Imports  {}", self.imports.len()))
            .id_salt("rne-recent-imports")
            .default_open(true)
            .show(ui, |ui| {
                if self.imports.is_empty() {
                    ui.label(RichText::new("No imports yet").size(FONT[0]).color(t.text_weak));
                }
                for batch in &self.imports {
                    let c = Collection::Import(batch.number);
                    let r = choice(ui, on(&c), format!("{}  {}", batch.day_text(), batch.count))
                        .on_hover_text(format!("Import {}", batch.number));
                    if r.clicked() {
                        chosen = Some(c);
                    }
                }
            });
        let edits = format!("Recent Edits  {}", self.counts.edits);
        let r = choice(ui, on(&Collection::RecentEdits), edits)
            .on_hover_text(format!("Photos edited in the last {} days", catalog::RECENT_EDIT_DAYS));
        if r.clicked() {
            chosen = Some(Collection::RecentEdits);
        }
        let trash = choice(ui, on(&Collection::Trash), format!("Trash  {}", self.counts.trash))
            .on_hover_text("Photos moved to the Trash stay in the catalog until it is emptied");
        if trash.clicked() {
            chosen = Some(Collection::Trash);
        }
        let can_empty = self.counts.trash > 0;
        trash.context_menu(|ui| {
            if ui.add_enabled(can_empty, Button::new("Empty Trash\u{2026}")).clicked() {
                self.confirm_empty_trash = true;
                ui.close();
            }
        });

        ui.add_space(SPACE[1]);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Albums").size(FONT[0]).color(t.text_weak));
            let new = ui.small_button("+").on_hover_text("New album (or drop photos here)");
            if let Some(dragged) = new.dnd_release_payload::<DraggedPhoto>() {
                let photos = self.strip.targets(&dragged.0);
                self.begin_new_album(photos);
            } else if new.clicked() {
                self.begin_new_album(Vec::new());
            }
            drop_highlight(ui, &new, t);
        });
        if self.albums.is_empty() {
            ui.label(RichText::new("No albums yet").size(FONT[0]).color(t.text_weak));
        }
        let mut drop = None;
        let mut rename = None;
        let mut delete = None;
        for album in &self.albums {
            let c = Collection::Album(album.id);
            let r = choice(ui, on(&c), format!("{}  {}", album.name, album.count))
                .on_hover_text("Drop photos here to add them");
            if r.clicked() {
                chosen = Some(c);
            }
            if let Some(dragged) = r.dnd_release_payload::<DraggedPhoto>() {
                drop = Some((album.id, dragged.0.clone()));
            }
            drop_highlight(ui, &r, t);
            r.context_menu(|ui| {
                if ui.button("Rename Album\u{2026}").clicked() {
                    rename = Some(album.clone());
                    ui.close();
                }
                if ui.button("Delete Album\u{2026}").clicked() {
                    delete = Some(album.clone());
                    ui.close();
                }
            });
        }
        if let Some((id, photo)) = drop {
            let photos = self.strip.targets(&photo);
            self.add_to_album(id, &photos, actions);
        }
        if let Some(album) = rename {
            self.album_dialog =
                Some(AlbumDialog { id: Some(album.id), name: album.name, photos: Vec::new(), focused: false });
        }
        if delete.is_some() {
            self.delete_album = delete;
        }

        ui.add_space(SPACE[1]);
        heading(ui, "Folders");
        if self.folder_nodes.is_empty() {
            ui.label(
                RichText::new("No photos yet: File \u{25b8} Import Images\u{2026}").size(FONT[0]).color(t.text_weak),
            );
        }
        let mut toggle = None;
        let nodes = std::mem::take(&mut self.folder_nodes);
        for node in &nodes {
            self.folder_row(ui, t, node, 0, &shown, &mut chosen, &mut toggle);
        }
        self.folder_nodes = nodes;
        if let Some(path) = toggle
            && !self.folders_closed.remove(&path)
        {
            self.folders_closed.insert(path);
        }
        if let Some(c) = chosen {
            self.show_collection(c);
        }
    }

    /// One folder of the catalog's tree and, when open, its sub-folders.
    #[allow(clippy::too_many_arguments)] // one row of a recursive tree
    fn folder_row(
        &self,
        ui: &mut egui::Ui,
        t: &Tokens,
        node: &FolderNode,
        depth: usize,
        shown: &Option<Collection>,
        chosen: &mut Option<Collection>,
        toggle: &mut Option<PathBuf>,
    ) {
        let open = !self.folders_closed.contains(&node.path);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.add_space(depth as f32 * TREE_INDENT);
            let (rect, arrow) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::click());
            if !node.children.is_empty() {
                let color = if arrow.hovered() { t.text_strong } else { t.text_weak };
                icons::disclosure(ui.painter(), rect, open, color);
                if arrow.clicked() {
                    *toggle = Some(node.path.clone());
                }
            }
            let c = Collection::Folder(node.path.clone());
            let r = choice(ui, shown.as_ref() == Some(&c), format!("{}  {}", node.name, node.total)).on_hover_text(
                format!("{}\n{} photos here, {} with the sub-folders", node.path.display(), node.own, node.total),
            );
            if r.clicked() {
                *chosen = Some(c);
            }
            if r.double_clicked() && !node.children.is_empty() {
                *toggle = Some(node.path.clone());
            }
        });
        if open {
            for child in &node.children {
                self.folder_row(ui, t, child, depth + 1, shown, chosen, toggle);
            }
        }
    }

    /// While photos are dragged from the filmstrip, how many go along, by the pointer.
    pub(super) fn drag_hint(&self, ctx: &egui::Context, t: &Tokens) {
        let Some(dragged) = egui::DragAndDrop::payload::<DraggedPhoto>(ctx) else { return };
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else { return };
        let n = self.strip.targets(&dragged.0).len();
        let text = if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("rne-drag-photos")));
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(FONT[1]), t.text_strong);
        let rect = Rect::from_min_size(pos + vec2(14.0, 10.0), galley.size() + vec2(12.0, 6.0));
        painter.rect_filled(rect, egui::CornerRadius::same(4), t.bg_overlay);
        painter.galley(rect.min + vec2(6.0, 3.0), galley, t.text_strong);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    }

    /// Shows the collection on screen again after its photos changed (and
    /// All Images when it is gone).
    fn reshow(&mut self) {
        self.refresh_collections();
        let gone = matches!(&self.collection, Some(Collection::Album(id)) if !self.albums.iter().any(|a| a.id == *id));
        match self.collection.clone() {
            Some(_) if gone => self.show_collection(Collection::All),
            Some(c) => self.show_collection(c),
            None => {}
        }
    }

    /// Moves `paths` to the Trash.
    pub(super) fn trash(&mut self, paths: &[PathBuf], actions: &mut Vec<LibraryAction>) {
        let Some(c) = self.catalog.as_mut() else { return };
        if paths.is_empty() {
            return;
        }
        match c.trash(paths) {
            Ok(()) => {
                self.reshow();
                actions.push(LibraryAction::Trashed { photos: paths.to_vec(), emptied: false });
            }
            Err(e) => actions.push(LibraryAction::Error(format!("The photos could not be moved to the Trash: {e}"))),
        }
    }

    /// Takes `paths` out of the Trash.
    pub(super) fn restore(&mut self, paths: &[PathBuf], actions: &mut Vec<LibraryAction>) {
        let Some(c) = self.catalog.as_mut() else { return };
        match c.restore(paths) {
            Ok(()) => self.reshow(),
            Err(e) => actions.push(LibraryAction::Error(format!("The photos could not be restored: {e}"))),
        }
    }

    /// Puts `paths` in album `id`.
    pub(super) fn add_to_album(&mut self, id: i64, paths: &[PathBuf], actions: &mut Vec<LibraryAction>) {
        let Some(c) = self.catalog.as_mut() else { return };
        match c.add_to_album(id, paths) {
            Ok(_) => self.reshow(),
            Err(e) => actions.push(LibraryAction::Error(format!("The photos could not be added to the album: {e}"))),
        }
    }

    /// Takes `paths` out of album `id`.
    pub(super) fn remove_from_album(&mut self, id: i64, paths: &[PathBuf], actions: &mut Vec<LibraryAction>) {
        let Some(c) = self.catalog.as_mut() else { return };
        match c.remove_from_album(id, paths) {
            Ok(()) => self.reshow(),
            Err(e) => actions.push(LibraryAction::Error(format!("The photos could not be taken out: {e}"))),
        }
    }

    /// Opens the New Album window; the album starts with `photos`.
    pub(super) fn begin_new_album(&mut self, photos: Vec<PathBuf>) {
        let name = format!("Album {}", self.albums.len() + 1);
        self.album_dialog = Some(AlbumDialog { id: None, name, photos, focused: false });
    }

    /// The New/Rename Album window and the confirmations of deleting an
    /// album and of emptying the Trash.
    pub(super) fn album_dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<LibraryAction>) {
        let t = Tokens::of(ctx.theme());
        if let Some(d) = &mut self.album_dialog {
            let mut choice = None;
            let title = if d.id.is_some() { "Rename Album" } else { "New Album" };
            dialog(ctx, "rne-album", |ui| {
                ui.label(RichText::new(title).size(FONT[4]).strong());
                ui.add_space(SPACE[1]);
                if !d.photos.is_empty() {
                    let n = d.photos.len();
                    let text = if n == 1 { "With 1 photo".to_owned() } else { format!("With {n} photos") };
                    ui.label(RichText::new(text).color(t.text_weak));
                    ui.add_space(SPACE[1]);
                }
                let field = ui.add(TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
                if !d.focused {
                    field.request_focus();
                    d.focused = true;
                }
                ui.add_space(SPACE[2]);
                let ready = !d.name.trim().is_empty();
                let verb = if d.id.is_some() { "Rename" } else { "Create" };
                choice = dialog_buttons(ui, &t, ready.then_some(verb), "Cancel", |_| {});
            });
            match choice {
                Some(Choice::Action) => {
                    if let Some(d) = self.album_dialog.take()
                        && let Some(c) = self.catalog.as_mut()
                    {
                        let done = match d.id {
                            Some(id) => c.rename_album(id, &d.name),
                            None => c.create_album(&d.name).and_then(|id| c.add_to_album(id, &d.photos).map(|_| ())),
                        };
                        match done {
                            Ok(()) => self.reshow(),
                            Err(e) => actions.push(LibraryAction::Error(format!("The album could not be saved: {e}"))),
                        }
                    }
                }
                Some(Choice::Cancel) => self.album_dialog = None,
                None => {}
            }
        }
        if let Some(album) = self.delete_album.clone() {
            let mut choice = None;
            dialog(ctx, "rne-delete-album", |ui| {
                ui.label(RichText::new(format!("Delete \u{201c}{}\u{201d}?", album.name)).size(FONT[4]).strong());
                ui.add_space(SPACE[1]);
                ui.label(RichText::new("Its photos stay in the catalog.").color(t.text_weak));
                ui.add_space(SPACE[2]);
                choice = dialog_buttons(ui, &t, Some("Delete Album"), "Cancel", |_| {});
            });
            match choice {
                Some(Choice::Action) => {
                    self.delete_album = None;
                    if let Some(c) = self.catalog.as_mut() {
                        match c.delete_album(album.id) {
                            Ok(()) => self.reshow(),
                            Err(e) => {
                                actions.push(LibraryAction::Error(format!("The album could not be deleted: {e}")))
                            }
                        }
                    }
                }
                Some(Choice::Cancel) => self.delete_album = None,
                None => {}
            }
        }
        if self.confirm_empty_trash {
            let n = self.counts.trash;
            let mut choice = None;
            dialog(ctx, "rne-empty-trash", |ui| {
                let photos = if n == 1 { "1 photo".to_owned() } else { format!("{n} photos") };
                ui.label(RichText::new(format!("Remove {photos} from the catalog?")).size(FONT[4]).strong());
                ui.add_space(SPACE[1]);
                ui.label(
                    RichText::new(
                        "Their ratings, keywords and edits are removed from the catalog. \
                         The files stay where they are on the disk.",
                    )
                    .color(t.text_weak),
                );
                ui.add_space(SPACE[2]);
                choice = dialog_buttons(ui, &t, Some("Remove from Catalog"), "Cancel", |_| {});
            });
            match choice {
                Some(Choice::Action) => {
                    self.confirm_empty_trash = false;
                    if let Some(c) = self.catalog.as_mut() {
                        match c.empty_trash() {
                            Ok(gone) => {
                                for p in &gone {
                                    self.forget_thumbnail(p);
                                }
                                self.reshow();
                                actions.push(LibraryAction::Trashed { photos: gone, emptied: true });
                            }
                            Err(e) => {
                                actions.push(LibraryAction::Error(format!("The Trash could not be emptied: {e}")))
                            }
                        }
                    }
                }
                Some(Choice::Cancel) => self.confirm_empty_trash = false,
                None => {}
            }
        }
    }
}

/// An accent outline on `target` while photos dragged from the filmstrip
/// are over it.
fn drop_highlight(ui: &egui::Ui, target: &egui::Response, t: &Tokens) {
    if target.dnd_hover_payload::<DraggedPhoto>().is_some() {
        ui.painter().rect_stroke(
            target.rect.expand(1.0),
            egui::CornerRadius::same(4),
            Stroke::new(1.5, t.accent),
            StrokeKind::Outside,
        );
    }
}

/// The catalog items of a card menu: Add to Album, Remove from Album and
/// Move to Trash, or Restore in the Trash. `group` says when they act on
/// the selection; a photo of a disk folder (not in a collection) has none.
pub(super) fn card_menu_items(
    ui: &mut egui::Ui,
    path: &Path,
    group: &str,
    albums: &[Album],
    collection: Option<&Collection>,
    actions: &mut Vec<CardAction>,
) {
    let Some(collection) = collection else { return };
    if collection.is_trash() {
        if ui.button(format!("Restore{group}")).on_hover_text("Back where it was in the catalog").clicked() {
            actions.push(CardAction::Restore(path.to_owned()));
            ui.close();
        }
        return;
    }
    ui.menu_button(format!("Add to Album{group}"), |ui| {
        for album in albums {
            if ui.button(album.name.as_str()).clicked() {
                actions.push(CardAction::AddToAlbum(path.to_owned(), album.id));
                ui.close();
            }
        }
        if !albums.is_empty() {
            ui.separator();
        }
        if ui.button("New Album\u{2026}").clicked() {
            actions.push(CardAction::NewAlbum(path.to_owned()));
            ui.close();
        }
    });
    if let Collection::Album(id) = collection
        && ui.button(format!("Remove from Album{group}")).clicked()
    {
        actions.push(CardAction::RemoveFromAlbum(path.to_owned(), *id));
        ui.close();
    }
    if ui.add(Button::new(format!("Move to Trash{group}")).shortcut_text("Delete")).clicked() {
        actions.push(CardAction::Trash(path.to_owned()));
        ui.close();
    }
}
