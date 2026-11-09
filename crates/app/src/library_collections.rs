//! The Library's catalog side, as Capture One's Library tool: Catalog
//! Collections (All Images, Recent Imports, Recent Edits, Trash), the
//! albums and the catalog's folder tree; moving photos to the Trash and
//! back; albums made, renamed and deleted, and photos put in them by
//! dragging them onto an album or from the card menu.

use super::{
    Album, Button, Choice, Collection, FONT, FolderNode, Library, LibraryAction, Path, PathBuf, Rect, RichText, SPACE,
    Sense, Stroke, StrokeKind, TREE_INDENT, TextEdit, Tokens, dialog, dialog_buttons, egui, icons, vec2,
};
use super::{CardAction, CardState};

/// What a filmstrip card carries while it is dragged: the photo (the
/// selection it belongs to goes with it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DraggedPhoto(pub(super) PathBuf);

/// A change of the Library that can be undone (the last one only), with
/// Ctrl+Z over the Library, Edit ▸ Undo … in the Library or the message's
/// Undo button.
#[derive(Debug, Clone)]
pub enum LibraryUndo {
    /// Photos moved to the Trash: they come back.
    Trash(Vec<PathBuf>),
    /// Two albums merged: they come apart again.
    Merge(Box<catalog::MergeUndo>),
}

impl LibraryUndo {
    /// What it undoes, for menus ("move to Trash").
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Trash(_) => "move to Trash",
            Self::Merge(_) => "album merge",
        }
    }
}

/// The New Album / Rename Album window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AlbumDialog {
    /// The album renamed; `None` for a new one.
    id: Option<i64>,
    /// The album a new one goes in (a sub-album).
    parent: Option<i64>,
    name: String,
    /// Photos the new album starts with.
    photos: Vec<PathBuf>,
    focused: bool,
}

/// Height of a row of the Library.
const ROW_HEIGHT: f32 = 20.0;

/// A Library row: `name` on the left, `count` on the right in a weaker
/// colour, highlighted when `on`; the name is cut short rather than
/// pushing the count away.
fn count_row(ui: &mut egui::Ui, t: &Tokens, on: bool, name: &str, count: usize) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    if on {
        painter.rect_filled(rect, egui::CornerRadius::same(4), ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        painter.rect_filled(rect, egui::CornerRadius::same(4), ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let font = egui::FontId::proportional(FONT[1]);
    let number = painter.layout_no_wrap(count.to_string(), font.clone(), t.text_weak);
    let right = rect.right() - 6.0 - number.size().x;
    painter.galley(egui::pos2(right, rect.center().y - number.size().y / 2.0), number, t.text_weak);
    let room = (right - rect.left() - 12.0).max(10.0);
    let text_color = if on { t.text_strong } else { ui.visuals().text_color() };
    let mut job = egui::text::LayoutJob::single_section(name.to_owned(), egui::TextFormat::simple(font, text_color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(room);
    let galley = painter.layout_job(job);
    painter.galley(egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0), galley, text_color);
    response
}

impl Library {
    /// The catalog's search field, collections, albums and folders.
    pub(super) fn collections(&mut self, ui: &mut egui::Ui, t: &Tokens, actions: &mut Vec<LibraryAction>) {
        let mut chosen = None;
        // Search by name in the whole catalog, as typed.
        let field = ui.add(
            TextEdit::singleline(&mut self.search).hint_text("Search the catalog by name").desired_width(f32::INFINITY),
        );
        let mut searched = false;
        if field.changed() {
            searched = true;
            chosen = Some(if self.search.trim().is_empty() {
                Collection::All
            } else {
                Collection::Search(self.search.trim().to_owned())
            });
        }
        if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.clear();
            searched = true;
            chosen = Some(Collection::All);
        }
        let shown = self.collection.clone();
        let on = |c: &Collection| shown.as_ref() == Some(c);
        let heading = |ui: &mut egui::Ui, text: &str| {
            ui.add_space(SPACE[0]);
            ui.label(RichText::new(text).size(FONT[0]).color(t.text_weak));
        };

        heading(ui, "Catalog Collections");
        if count_row(ui, t, on(&Collection::All), "All Images", self.counts.all).clicked() {
            chosen = Some(Collection::All);
        }
        egui::CollapsingHeader::new("Recent Imports").id_salt("aktis-recent-imports").default_open(true).show(
            ui,
            |ui| {
                if self.imports.is_empty() {
                    ui.label(RichText::new("No imports yet").size(FONT[0]).color(t.text_weak));
                }
                for batch in &self.imports {
                    let c = Collection::Import(batch.number);
                    let photos =
                        if batch.count == 1 { "1 photo".to_owned() } else { format!("{} photos", batch.count) };
                    let r = count_row(ui, t, on(&c), &batch.when_text(), batch.count)
                        .on_hover_text(format!("Import {}: {photos}", batch.number));
                    if r.clicked() {
                        chosen = Some(c);
                    }
                }
            },
        );
        let r = count_row(ui, t, on(&Collection::RecentEdits), "Recent Edits", self.counts.edits)
            .on_hover_text(format!("Photos edited in the last {} days", catalog::RECENT_EDIT_DAYS));
        if r.clicked() {
            chosen = Some(Collection::RecentEdits);
        }
        let trash = count_row(ui, t, on(&Collection::Trash), "Trash", self.counts.trash)
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
                self.begin_new_album(photos, None);
            } else if new.clicked() {
                self.begin_new_album(Vec::new(), None);
            }
            drop_highlight(ui, &new, t);
        });
        if self.albums.is_empty() {
            ui.label(RichText::new("No albums yet").size(FONT[0]).color(t.text_weak));
        }
        let mut drop = None;
        let mut menu = None;
        let mut toggle = None;
        let tree = catalog::album_tree(&self.albums);
        // Rows of closed albums' sub-albums are skipped.
        let mut hidden_below: Option<usize> = None;
        for (i, (depth, album)) in tree.iter().enumerate() {
            if hidden_below.is_some_and(|d| *depth > d) {
                continue;
            }
            hidden_below = None;
            let has_children = tree.get(i + 1).is_some_and(|(d, _)| d > depth);
            let open = !self.albums_closed.contains(&album.id);
            if has_children && !open {
                hidden_below = Some(*depth);
            }
            let c = Collection::Album(album.id);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(*depth as f32 * TREE_INDENT);
                let (rect, arrow) = ui.allocate_exact_size(vec2(16.0, ROW_HEIGHT), Sense::click());
                if has_children {
                    let color = if arrow.hovered() { t.text_strong } else { t.text_weak };
                    icons::disclosure(ui.painter(), rect, open, color);
                    if arrow.clicked() {
                        toggle = Some(album.id);
                    }
                }
                let r =
                    count_row(ui, t, on(&c), &album.name, album.count).on_hover_text("Drop photos here to add them");
                if r.clicked() {
                    chosen = Some(c.clone());
                }
                if let Some(dragged) = r.dnd_release_payload::<DraggedPhoto>() {
                    drop = Some((album.id, dragged.0.clone()));
                }
                drop_highlight(ui, &r, t);
                r.context_menu(|ui| {
                    if let Some(m) = album_menu(ui, album, &self.albums) {
                        menu = Some(m);
                    }
                });
            });
        }
        if let Some(id) = toggle
            && !self.albums_closed.remove(&id)
        {
            self.albums_closed.insert(id);
        }
        if let Some((id, photo)) = drop {
            let photos = self.strip.targets(&photo);
            self.add_to_album(id, &photos, actions);
        }
        match menu {
            Some(AlbumMenu::SubAlbum(parent)) => {
                self.albums_closed.remove(&parent);
                self.begin_new_album(Vec::new(), Some(parent));
            }
            Some(AlbumMenu::Rename(album)) => {
                self.album_dialog = Some(AlbumDialog {
                    id: Some(album.id),
                    parent: None,
                    name: album.name,
                    photos: Vec::new(),
                    focused: false,
                });
            }
            Some(AlbumMenu::Merge { from, into }) => self.merge_albums(from, into, actions),
            Some(AlbumMenu::Delete(album)) => self.delete_album = Some(album),
            None => {}
        }

        ui.add_space(SPACE[1]);
        heading(ui, "Folders");
        if self.folder_nodes.is_empty() {
            ui.label(
                RichText::new("No photos yet: File \u{203a} Import Images\u{2026}").size(FONT[0]).color(t.text_weak),
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
            // Another collection chosen: the search is over.
            if !searched {
                self.search.clear();
            }
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
            let (rect, arrow) = ui.allocate_exact_size(vec2(16.0, ROW_HEIGHT), Sense::click());
            if !node.children.is_empty() {
                let color = if arrow.hovered() { t.text_strong } else { t.text_weak };
                icons::disclosure(ui.painter(), rect, open, color);
                if arrow.clicked() {
                    *toggle = Some(node.path.clone());
                }
            }
            let c = Collection::Folder(node.path.clone());
            let r = count_row(ui, t, shown.as_ref() == Some(&c), &node.name, node.total).on_hover_text(format!(
                "{}\n{} photos here, {} with the sub-folders",
                node.path.display(),
                node.own,
                node.total
            ));
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
        let painter =
            ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("aktis-drag-photos")));
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

    /// Moves `paths` to the Trash; with `current` (the open photo) among
    /// them, the window is told which photo to show instead.
    pub(super) fn trash(&mut self, paths: &[PathBuf], current: Option<&Path>, actions: &mut Vec<LibraryAction>) {
        let Some(c) = self.catalog.as_mut() else { return };
        if paths.is_empty() {
            return;
        }
        let next = current.and_then(|cur| self.strip.after_removing(paths, cur));
        match c.trash(paths) {
            Ok(()) => {
                self.last_undo = Some(LibraryUndo::Trash(paths.to_vec()));
                self.reshow();
                if let Some(n) = &next {
                    self.strip.select_only(n);
                    self.scroll_to = Some(n.clone());
                }
                actions.push(LibraryAction::Trashed { photos: paths.to_vec(), emptied: false, next });
            }
            Err(e) => actions.push(LibraryAction::Error(format!("The photos could not be moved to the Trash: {e}"))),
        }
    }

    /// Merges album `from` into album `into`; the window offers to undo it.
    fn merge_albums(&mut self, from: i64, into: i64, actions: &mut Vec<LibraryAction>) {
        let Some(c) = self.catalog.as_mut() else { return };
        let names = |id: i64| self.albums.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_default();
        let (from_name, into_name) = (names(from), names(into));
        match c.merge_albums(from, into) {
            Ok(undo) => {
                if self.collection == Some(Collection::Album(from)) {
                    self.collection = Some(Collection::Album(into));
                }
                self.reshow();
                let text = format!("\u{201c}{from_name}\u{201d} merged into \u{201c}{into_name}\u{201d}");
                self.last_undo = Some(LibraryUndo::Merge(Box::new(undo)));
                actions.push(LibraryAction::Merged { text });
            }
            Err(e) => actions.push(LibraryAction::Error(format!("The albums could not be merged: {e}"))),
        }
    }

    /// What Undo would take back in the Library ("move to Trash"), if anything.
    #[must_use]
    pub fn undo_label(&self) -> Option<&'static str> {
        self.last_undo.as_ref().map(LibraryUndo::label)
    }

    /// The pointer is over the Library (Explorer or filmstrip): Ctrl+Z is
    /// the Library's there.
    #[must_use]
    pub fn hovered(&self) -> bool {
        self.explorer_hovered || self.strip_hovered
    }

    /// Takes back the Library's last change; returns what was undone, for
    /// the message (`None` when there was nothing).
    ///
    /// # Errors
    /// A message when the catalog refuses; the change stays undoable.
    pub fn undo(&mut self) -> Result<Option<&'static str>, String> {
        let Some(undo) = self.last_undo.take() else { return Ok(None) };
        let c = self.catalog.as_mut().ok_or("there is no catalog")?;
        let done = match &undo {
            LibraryUndo::Trash(paths) => c.restore(paths),
            LibraryUndo::Merge(m) => c.undo_merge(m),
        };
        if let Err(e) = done {
            let text = e.to_string();
            self.last_undo = Some(undo);
            return Err(text);
        }
        self.reshow();
        Ok(Some(undo.label()))
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

    /// Opens the New Album window; the album starts with `photos`, inside
    /// album `parent` when given.
    pub(super) fn begin_new_album(&mut self, photos: Vec<PathBuf>, parent: Option<i64>) {
        let name = format!("Album {}", self.albums.len() + 1);
        self.album_dialog = Some(AlbumDialog { id: None, parent, name, photos, focused: false });
    }

    /// The New/Rename Album window and the confirmations of deleting an
    /// album and of emptying the Trash.
    pub(super) fn album_dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<LibraryAction>) {
        let t = Tokens::of(ctx.theme());
        if let Some(d) = &mut self.album_dialog {
            let mut choice = None;
            let title = match (d.id, d.parent) {
                (Some(_), _) => "Rename Album",
                (None, Some(_)) => "New Sub-album",
                (None, None) => "New Album",
            };
            dialog(ctx, "aktis-album", |ui| {
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
                            None => c
                                .create_album(&d.name, d.parent)
                                .and_then(|id| c.add_to_album(id, &d.photos).map(|_| ())),
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
            dialog(ctx, "aktis-delete-album", |ui| {
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
            dialog(ctx, "aktis-empty-trash", |ui| {
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
                                // What was in the Trash cannot come back now.
                                if matches!(self.last_undo, Some(LibraryUndo::Trash(_))) {
                                    self.last_undo = None;
                                }
                                for p in &gone {
                                    self.forget_thumbnail(p);
                                }
                                self.reshow();
                                actions.push(LibraryAction::Trashed { photos: gone, emptied: true, next: None });
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

/// What the right-click menu of an album asked for.
enum AlbumMenu {
    SubAlbum(i64),
    Rename(Album),
    Merge { from: i64, into: i64 },
    Delete(Album),
}

/// The right-click menu of `album`: New Sub-album, Rename, Merge Into
/// (any album but itself and its sub-albums), Delete.
fn album_menu(ui: &mut egui::Ui, album: &Album, albums: &[Album]) -> Option<AlbumMenu> {
    let mut out = None;
    if ui.button("New Sub-album\u{2026}").clicked() {
        out = Some(AlbumMenu::SubAlbum(album.id));
        ui.close();
    }
    if ui.button("Rename Album\u{2026}").clicked() {
        out = Some(AlbumMenu::Rename(album.clone()));
        ui.close();
    }
    let inside = catalog::album_descendants(albums, album.id);
    let targets: Vec<&Album> = albums.iter().filter(|a| a.id != album.id && !inside.contains(&a.id)).collect();
    ui.add_enabled_ui(!targets.is_empty(), |ui| {
        ui.menu_button("Merge Into", |ui| {
            for target in targets {
                if ui
                    .button(target.name.as_str())
                    .on_hover_text("Its photos and sub-albums go there; it can be undone")
                    .clicked()
                {
                    out = Some(AlbumMenu::Merge { from: album.id, into: target.id });
                    ui.close();
                }
            }
        });
    });
    ui.separator();
    if ui.button("Delete Album\u{2026}").clicked() {
        out = Some(AlbumMenu::Delete(album.clone()));
        ui.close();
    }
    out
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
    state: CardState<'_>,
    actions: &mut Vec<CardAction>,
) {
    let (albums, collection) = (state.albums, state.collection);
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
    // A variant's card deletes that variant; the photo and the others stay.
    if let Some(variant) = state.variant {
        let delete = Button::new(format!("Delete {}\u{2026}", variant.name));
        let delete = if state.open { delete.shortcut_text("Delete") } else { delete };
        if ui.add(delete).on_hover_text("Only this variant: the photo and its other variants stay").clicked() {
            actions.push(CardAction::DeleteVariant(path.to_owned(), variant.clone()));
            ui.close();
        }
        if ui.button(format!("Move Photo to Trash{group}")).on_hover_text("The photo with all its variants").clicked() {
            actions.push(CardAction::Trash(path.to_owned()));
            ui.close();
        }
        return;
    }
    if ui.add(Button::new(format!("Move to Trash{group}")).shortcut_text("Delete")).clicked() {
        actions.push(CardAction::Trash(path.to_owned()));
        ui.close();
    }
}
