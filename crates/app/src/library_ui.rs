//! The Explorer (folder tree, filters, AI keywords) and the filmstrip, with
//! the catalog behind them.
//!
//! Folder listings and thumbnails come from the worker; ratings, labels and
//! keywords live in the catalog (SQLite in the editor's data folder). What
//! the window does (opening, exporting, a rename of the open file) is
//! handed to it as a [`LibraryAction`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use catalog::{
    Album, Catalog, Collection, ColorLabel, FIRST_VARIANT, FolderNode, ImportBatch, ImportMetadata, ImportedPhoto,
    MAX_RATING, PhotoMeta, ShotKey, StoredVariant, VariantInfo,
};
use editor::variants::Save;
use editor::{Filmstrip, FolderListing, FolderTree, StripPhoto, ThumbCache, ThumbJob, ThumbState, Worker};
use eframe::egui::{
    self, Align, Button, Color32, CornerRadius, FontId, Key, Label, Layout, Modifiers, Rect, RichText, Sense, Stroke,
    StrokeKind, TextEdit, TextureHandle, pos2, vec2,
};

use crate::actions::Command;
use crate::dialog::{Choice, dialog, dialog_buttons};
use crate::icons;
use crate::theme::{BADGE_STAR, FONT, SPACE, Tokens};

/// Thumbnails kept on the GPU (about 270 KB each).
const THUMB_CACHE: usize = 300;
/// Cards before and after the visible ones whose thumbnails are asked for.
const PREFETCH_CARDS: usize = 12;
/// Inner margin of a filmstrip card.
const CARD_PAD: f32 = 6.0;
/// Thumbnail frame width ÷ height.
const THUMB_ASPECT: f32 = 1.5;
/// Height of the name line and of the stars line under a thumbnail.
const CARD_LINE: f32 = 16.0;
/// Width of one star of a card.
const STAR_CELL: f32 = 14.0;
/// Keyword chips shown at most.
const MAX_TAG_CHIPS: usize = 30;
/// Indent per level of the folder tree.
const TREE_INDENT: f32 = 12.0;
/// Thumbnails asked for the overview of a multiple selection, at most.
const MAX_OVERVIEW_THUMBS: usize = 60;
/// Height of the line above the overview tiles.
const OVERVIEW_HEADER: f32 = 30.0;

#[path = "library_collections.rs"]
mod collections;
#[path = "library_disk.rs"]
mod disk;

/// How many photos the Library's catalog collections hold, as last read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    all: usize,
    edits: usize,
    trash: usize,
}

/// What the library asks of the window.
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryAction {
    /// Open this photo in the editor.
    Open(PathBuf),
    /// Open variant `.1` of this photo (its card in the filmstrip was clicked).
    OpenVariant(PathBuf, u32),
    /// Delete this variant of this photo (asks first); the photo and its
    /// other variants stay.
    DeleteVariant(PathBuf, VariantInfo),
    /// Open the export window for these photos.
    Export(Vec<PathBuf>),
    /// Export these photos with the previous settings, without the window.
    ExportAgain(Vec<PathBuf>),
    /// A file was renamed on disk.
    Renamed {
        /// Old path.
        from: PathBuf,
        /// New path.
        to: PathBuf,
    },
    /// A command of the open photo's card menu (its variants).
    Command(Command),
    /// Apply preset number `.1` (in the order of [`Library::set_presets`]) to these photos.
    ApplyPreset(Vec<PathBuf>, usize),
    /// Save a preset from this photo's edit.
    SavePreset(PathBuf),
    /// Start "AI-tag this folder".
    TagFolder,
    /// Stop it.
    StopTagging,
    /// Tell the user something went wrong.
    Error(String),
    /// These photos left the catalog's collections, moved to the Trash or
    /// (`emptied`) removed from the catalog with the Trash: the window
    /// closes the open photo if it is one of them.
    Trashed {
        /// The photos.
        photos: Vec<PathBuf>,
        /// The Trash was emptied.
        emptied: bool,
        /// The photo to open in place of the open one when it went: the
        /// next one in the filmstrip, else the one before.
        next: Option<PathBuf>,
    },
    /// Open the Import Images window on this folder.
    ImportFolder(PathBuf),
    /// A folder of the disk was moved or renamed (the catalog followed).
    FolderMoved {
        /// Where it was.
        from: PathBuf,
        /// Where it is.
        to: PathBuf,
    },
    /// Two albums were merged (`text` says which); the Library can undo it.
    Merged {
        /// What happened, for the message.
        text: String,
    },
}

/// Progress of a running "AI-tag this folder", for the Explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaggingProgress {
    /// Photos done.
    pub done: usize,
    /// Photos to do.
    pub total: usize,
}

/// Why each variant command of the open photo's card menu is unavailable
/// (`None` when it is available).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VariantBlocks {
    /// New variant.
    pub new: Option<&'static str>,
    /// Clone variant.
    pub clone: Option<&'static str>,
    /// Delete variant.
    pub delete: Option<&'static str>,
}

/// What a click in a card or its menu asked for.
#[derive(Debug, Clone, PartialEq)]
enum CardAction {
    Click { path: PathBuf, ctrl: bool, shift: bool, variant: Option<u32> },
    Rate(PathBuf, u8),
    Label(PathBuf, Option<ColorLabel>),
    Rename(PathBuf),
    OpenWith(PathBuf),
    Reveal(PathBuf),
    Export(PathBuf),
    ExportAgain(PathBuf),
    Command(Command),
    ApplyPreset(PathBuf, usize),
    SavePreset(PathBuf),
    Trash(PathBuf),
    DeleteVariant(PathBuf, VariantInfo),
    Restore(PathBuf),
    AddToAlbum(PathBuf, i64),
    NewAlbum(PathBuf),
    RemoveFromAlbum(PathBuf, i64),
}

/// The Rename dialog.
#[derive(Debug, Clone)]
struct RenameDialog {
    path: PathBuf,
    stem: String,
    error: Option<String>,
    focused: bool,
}

/// Folders, filmstrip, catalog and thumbnails.
pub struct Library {
    tree: FolderTree,
    /// The open catalog's folder.
    catalog_dir: PathBuf,
    /// The catalog collection in the filmstrip; `None` for a folder of the disk.
    collection: Option<Collection>,
    /// The catalog's folders, imports, albums and counts, as last read.
    folder_nodes: Vec<FolderNode>,
    imports: Vec<ImportBatch>,
    albums: Vec<Album>,
    counts: Counts,
    /// Catalog folders shown closed in the Library.
    folders_closed: HashSet<PathBuf>,
    /// Albums whose sub-albums are hidden.
    albums_closed: HashSet<i64>,
    /// The text of the catalog's search field.
    search: String,
    /// The New Album / Rename Album window, while open.
    album_dialog: Option<collections::AlbumDialog>,
    /// The album whose deletion waits for confirmation.
    delete_album: Option<Album>,
    /// Emptying the Trash waits for confirmation.
    confirm_empty_trash: bool,
    /// The disk folder cut (Cut / Paste), if any.
    disk_cut: Option<PathBuf>,
    /// The warning before a disk folder changes, while open.
    disk_dialog: Option<disk::DiskDialog>,
    /// A disk folder being moved to the wastebasket.
    deleting: Option<disk::Deleting>,
    /// Disk folders to list again (they changed).
    relist: Vec<PathBuf>,
    /// The photos of the current folder and the filters.
    pub strip: Filmstrip,
    catalog: Option<Catalog>,
    /// Why the catalog file could not be used (told once by the window).
    pub catalog_problem: Option<String>,
    thumbs: ThumbCache<TextureHandle>,
    /// Large previews of the photos filling the viewer (several selected).
    previews: ThumbCache<TextureHandle>,
    /// The photos filling the viewer, for large previews.
    wanted_previews: Vec<PathBuf>,
    /// The folder whose listing will fill the filmstrip.
    loading: Option<PathBuf>,
    /// Photos around the visible part of the filmstrip, for thumbnails.
    wanted: Vec<PathBuf>,
    rename: Option<RenameDialog>,
    /// Bring this photo into view in the filmstrip.
    scroll_to: Option<PathBuf>,
    /// Filmstrip cards drawn at the last frame.
    shown_cards: std::ops::Range<usize>,
    /// The pointer was over the filmstrip at the last frame (Delete trashes then).
    strip_hovered: bool,
    /// The pointer was over the Explorer at the last frame (Ctrl+Z undoes
    /// the Library's last change then).
    explorer_hovered: bool,
    /// The Library's last change that can be undone.
    last_undo: Option<collections::LibraryUndo>,
    /// Names of the presets offered in the card menu ("Group / Name").
    presets: Vec<String>,
    /// The variants of the photos that have several (a card each), as last read.
    variant_lists: HashMap<PathBuf, Vec<VariantInfo>>,
    /// The lists must be read again (a variant stored or deleted, another strip).
    variant_lists_stale: bool,
    /// Thumbnails of single variants, from their stored previews (`None`:
    /// none stored, the photo's thumbnail stands in).
    variant_thumbs: HashMap<(PathBuf, u32), Option<TextureHandle>>,
}

/// The variant open in the editor and the open photo's variants (an
/// unsaved new one included), for the filmstrip's cards.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenVariants<'a> {
    /// Number of the open variant.
    pub number: Option<u32>,
    /// The open photo's variants.
    pub list: &'a [VariantInfo],
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("folder", &self.strip.folder)
            .field("photos", &self.strip.photos.len())
            .field("thumbnails", &self.thumbs.len())
            .finish_non_exhaustive()
    }
}

impl Library {
    /// The Explorer's roots and the catalog in folder `catalog_dir`; nothing
    /// is listed yet.
    #[must_use]
    pub fn new(catalog_dir: &Path) -> Self {
        let roots = platform::library_roots(platform::Os::current(), |k| std::env::var_os(k), Path::is_dir);
        let file = editor::catalogs::database(catalog_dir);
        let (catalog, catalog_problem) = match Catalog::open(&file) {
            Ok(c) => (Some(c), None),
            Err(e) => (
                // Ratings still work for this session.
                Catalog::open_in_memory().ok(),
                Some(format!("Ratings, keywords and edits will not be kept: {e} ({})", file.display())),
            ),
        };
        let mut library = Self {
            tree: FolderTree::new(roots),
            catalog_dir: catalog_dir.to_owned(),
            collection: None,
            folder_nodes: Vec::new(),
            imports: Vec::new(),
            albums: Vec::new(),
            counts: Counts::default(),
            folders_closed: HashSet::new(),
            albums_closed: HashSet::new(),
            search: String::new(),
            album_dialog: None,
            delete_album: None,
            confirm_empty_trash: false,
            disk_cut: None,
            disk_dialog: None,
            deleting: None,
            relist: Vec::new(),
            strip: Filmstrip::default(),
            catalog,
            catalog_problem,
            thumbs: ThumbCache::new(THUMB_CACHE),
            previews: ThumbCache::new(editor::overview::MAX_TILES * 2),
            wanted_previews: Vec::new(),
            loading: None,
            wanted: Vec::new(),
            rename: None,
            scroll_to: None,
            shown_cards: 0..0,
            strip_hovered: false,
            explorer_hovered: false,
            last_undo: None,
            presets: Vec::new(),
            variant_lists: HashMap::new(),
            variant_lists_stale: true,
            variant_thumbs: HashMap::new(),
        };
        library.refresh_collections();
        library
    }

    /// The open catalog's folder.
    #[must_use]
    pub fn catalog_dir(&self) -> &Path {
        &self.catalog_dir
    }

    /// Saves a copy of the open catalog as the new catalog folder `dir`
    /// (everything in it: photos, edits, ratings, keywords, albums, the
    /// Trash). The open catalog does not change.
    ///
    /// # Errors
    /// A message when there is no catalog open or the copy cannot be
    /// written; nothing is left behind then.
    pub fn save_catalog_copy(&self, dir: &Path) -> Result<(), String> {
        let catalog = self.catalog.as_ref().ok_or("no catalog is open")?;
        if dir.exists() {
            return Err(format!("{} exists already", dir.display()));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{} cannot be made: {e}", dir.display()))?;
        catalog.save_copy(&editor::catalogs::database(dir)).map_err(|e| {
            let _ = std::fs::remove_dir_all(dir);
            e.to_string()
        })
    }

    /// Opens the catalog in folder `dir` (made when it is new) in place of
    /// the open one; the filmstrip shows its photos.
    ///
    /// # Errors
    /// A message when the catalog cannot be opened; the open one stays.
    pub fn open_catalog(&mut self, dir: &Path) -> Result<(), String> {
        let catalog = Catalog::open(&editor::catalogs::database(dir)).map_err(|e| e.to_string())?;
        self.catalog = Some(catalog);
        self.catalog_dir = dir.to_owned();
        self.thumbs.clear();
        self.previews.clear();
        self.variant_thumbs.clear();
        self.variant_lists_stale = true;
        self.strip = Filmstrip::default();
        self.loading = None;
        self.refresh_collections();
        self.show_collection(Collection::All);
        Ok(())
    }

    /// Adds `paths` to the catalog as one import and shows it; returns how
    /// many were new.
    ///
    /// # Errors
    /// A message when the catalog refuses them.
    pub fn import(&mut self, paths: &[PathBuf]) -> Result<usize, String> {
        let catalog = self.catalog.as_mut().ok_or("there is no catalog")?;
        let batch = catalog.next_import().map_err(|e| e.to_string())?;
        let added = catalog.import(paths).map_err(|e| e.to_string())?;
        self.refresh_collections();
        if added > 0 {
            self.show_collection(Collection::Import(batch));
        }
        Ok(added)
    }

    /// The number of the next import and the shots already in the catalog,
    /// to start an import job.
    ///
    /// # Errors
    /// A message when the catalog cannot be read.
    pub fn begin_import(&self) -> Result<(i64, HashSet<ShotKey>), String> {
        let catalog = self.catalog.as_ref().ok_or("there is no catalog")?;
        let batch = catalog.next_import().map_err(|e| e.to_string())?;
        let known = catalog.known_shots().map_err(|e| e.to_string())?;
        Ok((batch, known))
    }

    /// Adds photos an import job reported to import `batch` and gives them
    /// `metadata`. The filmstrip shows the import from its `first` photos,
    /// and follows it while it shows it.
    ///
    /// # Errors
    /// A message when the catalog refuses them.
    pub fn add_imported(
        &mut self,
        batch: i64,
        photos: &[ImportedPhoto],
        metadata: &ImportMetadata,
        first: bool,
    ) -> Result<usize, String> {
        let catalog = self.catalog.as_mut().ok_or("there is no catalog")?;
        let added = catalog.add_imported(batch, photos).map_err(|e| e.to_string())?;
        if !metadata.is_empty() {
            let paths: Vec<PathBuf> = photos.iter().map(|p| p.path.clone()).collect();
            catalog.apply_metadata(&paths, metadata).map_err(|e| e.to_string())?;
        }
        self.refresh_collections();
        // As Capture One: the photos show up in the import as they come in
        // (the selection stays, the collection being the same).
        if first || self.collection == Some(Collection::Import(batch)) {
            self.show_collection(Collection::Import(batch));
        }
        Ok(added)
    }

    /// What the Import Images window needs to tell duplicates: the shots
    /// and the files in the catalog.
    #[must_use]
    pub fn import_context(&self) -> (HashSet<ShotKey>, HashSet<PathBuf>) {
        let Some(c) = self.catalog.as_ref() else { return (HashSet::new(), HashSet::new()) };
        (c.known_shots().unwrap_or_default(), c.paths().unwrap_or_default().into_iter().collect())
    }

    /// `true` when `path` was imported into the catalog.
    #[must_use]
    pub fn is_imported(&self, path: &Path) -> bool {
        self.catalog.as_ref().is_some_and(|c| c.is_imported(path).unwrap_or(false))
    }

    /// How many photos the catalog holds.
    #[must_use]
    pub fn photo_count(&self) -> usize {
        self.counts.all
    }

    /// The catalog collection in the filmstrip, if it shows one.
    #[must_use]
    pub fn collection(&self) -> Option<&Collection> {
        self.collection.as_ref()
    }

    /// Shows the photos of `collection` in the filmstrip.
    pub fn show_collection(&mut self, collection: Collection) {
        let paths = self.catalog.as_ref().and_then(|c| c.collection(&collection).ok()).unwrap_or_default();
        let metas = self.metas(&paths);
        if self.collection.as_ref() != Some(&collection) {
            self.thumbs.clear();
            self.previews.clear();
            self.strip.filter.tags.clear();
        }
        self.loading = None;
        // The filmstrip names what it shows after its "folder": the collection's name.
        self.strip.set_folder(self.catalog_dir.join(collection.label()), paths.into_iter().zip(metas).collect());
        self.variant_lists_stale = true;
        self.collection = Some(collection);
    }

    /// Reads again the catalog's folders, imports, albums and counts for
    /// the Library. Also which photos of a disk folder shown are not
    /// imported yet.
    fn refresh_collections(&mut self) {
        let Some(c) = self.catalog.as_ref() else { return };
        self.folder_nodes = catalog::folder_tree(&c.imported_folders().unwrap_or_default());
        self.imports = c.recent_imports(catalog::RECENT_IMPORTS).unwrap_or_default();
        self.albums = c.albums().unwrap_or_default();
        let count = |k: &Collection| c.count(k).unwrap_or(0);
        self.counts = Counts {
            all: count(&Collection::All),
            edits: count(&Collection::RecentEdits),
            trash: count(&Collection::Trash),
        };
    }

    /// Name of what the filmstrip shows: the collection (an import by its
    /// day, an album by its name) or the disk folder.
    #[must_use]
    pub fn shown_name(&self) -> Option<String> {
        match &self.collection {
            Some(Collection::Import(n)) => Some(
                self.imports
                    .iter()
                    .find(|i| i.number == *n)
                    .map_or_else(|| Collection::Import(*n).label(), |i| format!("Import of {}", i.when_text())),
            ),
            Some(Collection::Album(id)) => Some(
                self.albums
                    .iter()
                    .find(|a| a.id == *id)
                    .map_or_else(|| Collection::Album(*id).label(), |a| a.name.clone()),
            ),
            Some(c) => Some(c.label()),
            None => self.strip.folder.as_deref().map(editor::library::folder_name),
        }
    }

    /// The folder in the filmstrip.
    #[must_use]
    pub fn folder(&self) -> Option<&Path> {
        self.strip.folder.as_deref()
    }

    /// Shows `dir` in the filmstrip and opens the tree down to it.
    pub fn show_folder(&mut self, dir: PathBuf, worker: Option<&Worker>) {
        let Some(worker) = worker else { return };
        self.collection = None;
        let mut to_list = self.tree.reveal(&dir);
        if !to_list.contains(&dir) {
            to_list.push(dir.clone());
        }
        for d in to_list {
            worker.list_folder(d);
        }
        self.loading = Some(dir);
    }

    /// Brings `path` into view in the filmstrip (it was opened from elsewhere).
    pub fn reveal_photo(&mut self, path: &Path) {
        self.scroll_to = Some(path.to_owned());
        // Opening a photo outside the selection selects it alone.
        if self.strip.index_of(path).is_some() && !self.strip.is_selected(path) {
            self.strip.select_only(path);
        }
    }

    /// `true` when the Explorer's filters let no photo of the filmstrip
    /// through: the viewer shows none.
    #[must_use]
    pub fn nothing_matches(&self) -> bool {
        self.strip.filtered_to_nothing()
    }

    /// Turns every filter off.
    pub fn clear_filters(&mut self) {
        self.strip.filter = catalog::Filter::default();
    }

    /// Photos selected in the filmstrip (visible ones, in order).
    #[must_use]
    pub fn selection(&self) -> Vec<PathBuf> {
        self.strip.selection()
    }

    /// A folder listing from the worker: sub-folders for the tree, photos
    /// for the filmstrip when it is the folder being shown. Returns a
    /// message when the shown folder cannot be read.
    pub fn on_listed(&mut self, dir: PathBuf, result: Result<FolderListing, String>) -> Option<String> {
        let for_strip = self.loading.as_deref() == Some(dir.as_path());
        match result {
            Ok(listing) => {
                self.tree.set_children(&dir, listing.subfolders);
                if for_strip {
                    self.loading = None;
                    let metas = self.metas(&listing.photos);
                    if self.strip.folder.as_deref() != Some(dir.as_path()) {
                        self.thumbs.clear();
                        self.previews.clear();
                        // Keywords belong to a folder's photos; the other filters carry over.
                        self.strip.filter.tags.clear();
                    }
                    self.strip.set_folder(dir, listing.photos.into_iter().zip(metas).collect());
                    self.variant_lists_stale = true;
                    self.refresh_collections();
                }
                None
            }
            Err(message) => {
                // Not readable (permissions, a drive without a disc): shown without sub-folders.
                self.tree.set_children(&dir, Vec::new());
                if for_strip {
                    self.loading = None;
                    let text = format!("Cannot read {}: {message}", dir.display());
                    self.strip.set_folder(dir, Vec::new());
                    return Some(text);
                }
                None
            }
        }
    }

    /// A thumbnail from the worker.
    pub fn on_thumbnail(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        result: Result<std::sync::Arc<raw_io::Thumbnail>, String>,
    ) {
        if self.strip.index_of(&path).is_none() {
            return; // another folder is shown now
        }
        let state = match result {
            Ok(t) => {
                let image = egui::ColorImage::from_rgb([t.width as usize, t.height as usize], &t.rgb);
                ThumbState::Ready(ctx.load_texture(
                    format!("thumb:{}", path.display()),
                    image,
                    crate::photo_texture::smooth(),
                ))
            }
            Err(why) => ThumbState::Failed(why),
        };
        self.thumbs.insert(path, state);
    }

    /// A large preview arrived (several photos fill the viewer).
    pub fn on_preview(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        result: Result<std::sync::Arc<raw_io::Thumbnail>, String>,
    ) {
        if self.strip.index_of(&path).is_none() {
            return; // another folder is shown now
        }
        let state = match result {
            Ok(t) => {
                let image = egui::ColorImage::from_rgb([t.width as usize, t.height as usize], &t.rgb);
                ThumbState::Ready(ctx.load_texture(
                    format!("preview:{}", path.display()),
                    image,
                    crate::photo_texture::smooth(),
                ))
            }
            Err(why) => ThumbState::Failed(why),
        };
        self.previews.insert(path, state);
    }

    /// Asks the worker for the thumbnails of the photos in view and the
    /// large previews of those filling the viewer, when those lists changed.
    pub fn request_thumbnails(&mut self, worker: Option<&Worker>) {
        let Some(worker) = worker else { return };
        // Folders changed on the disk are listed again.
        for dir in self.relist.drain(..) {
            worker.list_folder(dir);
        }
        if let Some(list) = self.previews.to_request(&self.wanted_previews) {
            worker.request_previews(self.thumb_jobs(list));
        }
        if let Some(list) = self.thumbs.to_request(&self.wanted) {
            worker.request_thumbnails(self.thumb_jobs(list));
        }
    }

    /// Thumbnail jobs for `paths`, each with the preview of its edit when
    /// the catalog keeps one (shown instead of the camera's, as in Capture One).
    fn thumb_jobs(&self, paths: Vec<PathBuf>) -> Vec<ThumbJob> {
        let edited = self
            .catalog
            .as_ref()
            .and_then(|c| c.previews(&paths).ok())
            .filter(|e| e.len() == paths.len())
            .unwrap_or_else(|| vec![None; paths.len()]);
        paths.into_iter().zip(edited).map(|(path, jpeg)| ThumbJob { path, edited: jpeg.map(Arc::from) }).collect()
    }

    /// The presets the card menu offers, in order ("Group / Name").
    pub fn set_presets(&mut self, names: Vec<String>) {
        self.presets = names;
    }

    /// Forgets the thumbnail and large preview of `path`, so they are made
    /// again (its edited preview changed).
    pub fn forget_thumbnail(&mut self, path: &Path) {
        self.thumbs.forget(path);
        self.previews.forget(path);
        self.variant_thumbs.retain(|(p, _), _| p != path);
    }

    /// Shows `image`, the photo `path` as its variant `number` is now
    /// edited, as its filmstrip thumbnail at once (the catalog's preview is
    /// written later, with the edit).
    pub fn show_live_thumbnail(&mut self, ctx: &egui::Context, path: &Path, number: u32, image: &engine::Rgb8Image) {
        if self.strip.index_of(path).is_none() {
            return;
        }
        let small = shrink_rgb(image, editor::worker::THUMBNAIL_SIDE);
        let texture =
            ctx.load_texture(format!("live:{}:{number}", path.display()), small, crate::photo_texture::smooth());
        self.variant_thumbs.insert((path.to_owned(), number), Some(texture.clone()));
        self.thumbs.insert(path.to_owned(), ThumbState::Ready(texture));
    }

    /// Reads again which photos have several variants, when that may have changed.
    fn reload_variant_lists(&mut self) {
        if !self.variant_lists_stale {
            return;
        }
        self.variant_lists_stale = false;
        self.variant_lists = self
            .catalog
            .as_ref()
            .and_then(|c| c.multi_variants().ok())
            .map(|list| list.into_iter().collect())
            .unwrap_or_default();
    }

    // ----- edits -----

    /// The variants stored for `path` (empty for a photo never edited, or
    /// when the catalog cannot be read).
    #[must_use]
    pub fn variants(&self, path: &Path) -> Vec<VariantInfo> {
        self.catalog.as_ref().and_then(|c| c.variants(path).ok()).unwrap_or_default()
    }

    /// The variants stored for `path`, or why the catalog cannot be read.
    ///
    /// # Errors
    /// A message when the catalog cannot be read.
    pub fn try_variants(&self, path: &Path) -> Result<Vec<VariantInfo>, String> {
        match &self.catalog {
            Some(c) => c.variants(path).map_err(|e| e.to_string()),
            None => Ok(Vec::new()),
        }
    }

    /// The variant shown for `path` and its stored edit (variant 1 and
    /// nothing for a photo never edited).
    ///
    /// # Errors
    /// A message when the catalog cannot be read.
    pub fn shown_variant(&self, path: &Path) -> Result<(u32, Option<StoredVariant>), String> {
        let Some(c) = &self.catalog else { return Ok((FIRST_VARIANT, None)) };
        let number = c.current_variant(path).map_err(|e| e.to_string())?.unwrap_or(FIRST_VARIANT);
        let stored = c.load_variant(path, number).map_err(|e| e.to_string())?;
        Ok((number, stored))
    }

    /// The stored edit of variant `number` of `path`.
    ///
    /// # Errors
    /// A message when the catalog cannot be read.
    pub fn load_variant(&self, path: &Path, number: u32) -> Result<Option<StoredVariant>, String> {
        let Some(c) = &self.catalog else { return Ok(None) };
        c.load_variant(path, number).map_err(|e| e.to_string())
    }

    /// Writes what `save` holds for `path`; a new preview replaces the
    /// filmstrip's thumbnail.
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn save_variant(&mut self, path: &Path, save: &Save) -> Result<(), String> {
        let Some(c) = &mut self.catalog else { return Ok(()) };
        match (&save.data, &save.preview) {
            (Some(data), preview) => c.save_variant(path, &save.info, data, preview.as_deref()),
            (None, Some(jpeg)) => c.set_variant_preview(path, save.info.number, jpeg),
            (None, None) => return Ok(()),
        }
        .map_err(|e| e.to_string())?;
        // A new variant may now be stored: the filmstrip shows it as a card.
        self.variant_lists_stale |= save.data.is_some();
        if save.preview.is_some() {
            self.forget_thumbnail(path);
        }
        Ok(())
    }

    /// `true` when the catalog keeps a preview of the edit shown for `path`.
    #[must_use]
    pub fn has_edited_preview(&self, path: &Path) -> bool {
        self.catalog
            .as_ref()
            .and_then(|c| c.previews(&[path.to_owned()]).ok())
            .is_some_and(|p| p.first().is_some_and(Option::is_some))
    }

    /// Writes an edit changed without opening the photo (a preset applied
    /// from the filmstrip). Its stored preview no longer matches, so it is
    /// dropped: the filmstrip shows the camera's until the photo is opened.
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn save_restyled(&mut self, path: &Path, save: &Save) -> Result<(), String> {
        let Some(c) = &mut self.catalog else { return Err("the catalog is not available".to_owned()) };
        let Some(data) = &save.data else { return Ok(()) };
        c.save_variant(path, &save.info, data, None).map_err(|e| e.to_string())?;
        c.clear_variant_preview(path, save.info.number).map_err(|e| e.to_string())?;
        self.forget_thumbnail(path);
        Ok(())
    }

    /// Remembers which variant of `path` is shown (in the filmstrip too).
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn set_current_variant(&mut self, path: &Path, number: u32) -> Result<(), String> {
        let Some(c) = &mut self.catalog else { return Ok(()) };
        c.set_current_variant(path, number).map_err(|e| e.to_string())?;
        self.forget_thumbnail(path);
        Ok(())
    }

    /// Deletes variant `number` of `path` from the catalog.
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn delete_variant(&mut self, path: &Path, number: u32) -> Result<(), String> {
        let Some(c) = &mut self.catalog else { return Ok(()) };
        c.delete_variant(path, number).map_err(|e| e.to_string())?;
        self.variant_lists_stale = true;
        self.variant_thumbs.remove(&(path.to_owned(), number));
        Ok(())
    }

    fn metas(&self, photos: &[PathBuf]) -> Vec<PhotoMeta> {
        self.catalog
            .as_ref()
            .and_then(|c| c.metas(photos).ok())
            .filter(|m| m.len() == photos.len())
            .unwrap_or_else(|| vec![PhotoMeta::default(); photos.len()])
    }

    /// Re-reads the catalog data of `paths` into the filmstrip.
    fn refresh(&mut self, paths: &[PathBuf]) {
        let metas = self.metas(paths);
        for (p, m) in paths.iter().zip(metas) {
            self.strip.set_meta(p, m);
        }
    }

    /// The catalog data of `path` when it is in the filmstrip.
    #[must_use]
    pub fn meta_of(&self, path: &Path) -> Option<&PhotoMeta> {
        self.strip.index_of(path).map(|i| &self.strip.photos[i].meta)
    }

    /// The catalog data of `path` even when it is not in the filmstrip.
    #[must_use]
    pub fn catalog_meta(&self, path: &Path) -> PhotoMeta {
        self.meta_of(path).cloned().unwrap_or_else(|| self.metas(&[path.to_owned()]).pop().unwrap_or_default())
    }

    /// Sets the rating of `paths`.
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn set_rating(&mut self, paths: &[PathBuf], rating: u8) -> Result<(), String> {
        if let Some(c) = &mut self.catalog {
            c.set_rating(paths, rating.min(MAX_RATING)).map_err(|e| e.to_string())?;
        }
        self.refresh(paths);
        Ok(())
    }

    /// Sets the colour label of `paths`.
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn set_label(&mut self, paths: &[PathBuf], label: Option<ColorLabel>) -> Result<(), String> {
        if let Some(c) = &mut self.catalog {
            c.set_label(paths, label).map_err(|e| e.to_string())?;
        }
        self.refresh(paths);
        Ok(())
    }

    /// Stores the AI keywords found for `path` (the user's own are kept).
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn set_ai_tags(&mut self, path: &Path, tags: &[(String, f32)]) -> Result<(), String> {
        if let Some(c) = &mut self.catalog {
            c.set_ai_tags(path, tags).map_err(|e| e.to_string())?;
        }
        self.refresh(&[path.to_owned()]);
        Ok(())
    }

    /// Adds a keyword typed by the user.
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn add_user_tag(&mut self, path: &Path, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        if let Some(c) = &mut self.catalog {
            c.add_user_tag(path, name).map_err(|e| e.to_string())?;
        }
        self.refresh(&[path.to_owned()]);
        Ok(())
    }

    /// Removes a keyword (AI or the user's).
    ///
    /// # Errors
    /// A message when the catalog refused.
    pub fn remove_tag(&mut self, path: &Path, name: &str) -> Result<(), String> {
        if let Some(c) = &mut self.catalog {
            c.remove_tag(path, name).map_err(|e| e.to_string())?;
        }
        self.refresh(&[path.to_owned()]);
        Ok(())
    }

    /// Opens the Rename dialog for `path`.
    pub fn begin_rename(&mut self, path: PathBuf) {
        let stem = editor::file_stem(&path);
        self.rename = Some(RenameDialog { path, stem, error: None, focused: false });
    }

    /// Renames the file on disk and everywhere the library knows it.
    fn rename_file(&mut self, from: &Path, stem: &str) -> Result<Option<PathBuf>, String> {
        let to = editor::rename_target(from, stem, Path::exists).map_err(|e| e.to_string())?;
        if to == from {
            return Ok(None);
        }
        std::fs::rename(from, &to).map_err(|e| format!("The file could not be renamed: {e}"))?;
        if let Some(c) = &mut self.catalog {
            // The file is renamed; losing its rating is the lesser evil, so go on.
            let _ = c.rename(from, &to);
        }
        self.strip.rename(from, to.clone());
        self.thumbs.rename(from, to.clone());
        self.previews.rename(from, to.clone());
        Ok(Some(to))
    }

    // ----- keyboard -----

    /// Filmstrip keys when no text field has the keyboard: arrows go to
    /// the next or previous photo, 0–5 rate, 6–9 set a colour label, F2
    /// renames. They act on the open photo (`current`).
    pub fn keyboard(&mut self, ctx: &egui::Context, current: Option<&Path>, actions: &mut Vec<LibraryAction>) {
        if ctx.egui_wants_keyboard_input() || self.rename.is_some() || ctx.any_popup_open() {
            return;
        }
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        // Previous / next: the selection becomes that photo alone; not while
        // the pointer is over a colour wheel, whose arrows change it.
        let wheel_keys = crate::color_ui::arrows_taken(ctx);
        for (key, step) in [(Key::ArrowDown, 1), (Key::ArrowRight, 1), (Key::ArrowUp, -1), (Key::ArrowLeft, -1)] {
            if !wheel_keys
                && pressed(key)
                && let Some(next) = self.strip.neighbour(current, step).map(Path::to_owned)
            {
                self.strip.select_only(&next);
                self.scroll_to = Some(next.clone());
                actions.push(LibraryAction::Open(next));
            }
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.strip.select_all();
        }
        // Escape leaves a multiple selection (and only then: menus use it too).
        if self.strip.selection().len() > 1
            && pressed(Key::Escape)
            && let Some(path) = current.filter(|p| self.strip.index_of(p).is_some())
        {
            self.strip.select_only(path);
        }
        // Ratings and labels act on the selection, else on the open photo.
        let targets: Vec<PathBuf> =
            self.strip.selected_or(current).into_iter().filter(|p| self.strip.index_of(p).is_some()).collect();
        if targets.is_empty() {
            return;
        }
        let ratings = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5];
        for (rating, key) in (0_u8..).zip(ratings) {
            if pressed(key)
                && let Err(e) = self.set_rating(&targets, rating)
            {
                actions.push(LibraryAction::Error(e));
            }
        }
        let labels = [
            (Key::Num6, ColorLabel::Red),
            (Key::Num7, ColorLabel::Yellow),
            (Key::Num8, ColorLabel::Green),
            (Key::Num9, ColorLabel::Blue),
        ];
        for (key, label) in labels {
            if pressed(key) {
                // The same key again takes the label off (when every target has it).
                let all_have = targets.iter().all(|p| self.meta_of(p).and_then(|m| m.label) == Some(label));
                let next = if all_have { None } else { Some(label) };
                if let Err(e) = self.set_label(&targets, next) {
                    actions.push(LibraryAction::Error(e));
                }
            }
        }
        if pressed(Key::F2)
            && let Some(path) = current.filter(|p| self.strip.index_of(p).is_some())
        {
            self.begin_rename(path.to_owned());
        }
        // Delete moves to the Trash, as in Capture One (not from a disk folder),
        // with the pointer over the filmstrip: elsewhere it deletes a layer or a spot.
        if self.strip_hovered && self.collection.as_ref().is_some_and(|c| !c.is_trash()) && pressed(Key::Delete) {
            // The open photo alone, with several variants: only its open
            // variant goes (asked first), as in Capture One.
            let several = current.and_then(|p| self.variant_lists.get(p)).is_some_and(|v| v.len() > 1);
            if several && targets.len() == 1 && current == Some(targets[0].as_path()) {
                actions.push(LibraryAction::Command(Command::DeleteVariant));
            } else {
                self.trash(&targets, current, actions);
            }
        }
    }

    // ----- Explorer -----

    /// The Explorer: folders, filters and AI keywords.
    pub fn explorer(
        &mut self,
        ui: &mut egui::Ui,
        worker: Option<&Worker>,
        tagging: Option<TaggingProgress>,
        actions: &mut Vec<LibraryAction>,
    ) {
        let t = Tokens::of(ui.ctx().theme());
        self.explorer_hovered = ui.rect_contains_pointer(ui.max_rect());
        ui.add_space(SPACE[1]);
        ui.label(
            RichText::new(format!("Catalog \u{b7} {}", editor::catalogs::name(&self.catalog_dir)))
                .size(FONT[3])
                .strong(),
        )
        .on_hover_text(self.catalog_dir.display().to_string());
        ui.add_space(SPACE[0]);
        egui::ScrollArea::vertical()
            .id_salt("explorer-catalog")
            .max_height((ui.available_height() * 0.45).max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| self.collections(ui, &t, actions));
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Disk").strong().color(t.text_weak))
                .on_hover_text("Browse the folders of this computer to import their photos");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.collection.is_none()
                    && let Some(dir) = self.strip.folder.clone()
                    && ui.small_button("Refresh").on_hover_text("Read the folder again").clicked()
                {
                    self.show_folder(dir, worker);
                }
            });
        });
        let tree_height = (ui.available_height() * 0.3).max(90.0);
        egui::ScrollArea::vertical()
            .id_salt("explorer-folders")
            .max_height(tree_height)
            .auto_shrink([false, true])
            .show(ui, |ui| self.folder_tree(ui, &t, worker, actions));
        if let Some(path) = self.deleting() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new(format!("Deleting {}\u{2026}", editor::library::folder_name(path)))
                        .size(FONT[0])
                        .color(t.text_weak),
                );
            });
        }
        ui.separator();
        egui::ScrollArea::vertical().id_salt("explorer-filters").auto_shrink([false, false]).show(ui, |ui| {
            self.filters(ui, &t);
            ui.add_space(SPACE[2]);
            self.ai_tags(ui, &t, tagging, actions);
        });
    }

    fn folder_tree(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        worker: Option<&Worker>,
        actions: &mut Vec<LibraryAction>,
    ) {
        let mut toggle = None;
        let mut select = None;
        let mut menu = None;
        let cut = self.disk_cut.clone();
        let current = if self.collection.is_none() { self.strip.folder.clone() } else { None };
        if self.tree.roots.is_empty() {
            ui.weak("No folders found");
        }
        for (depth, node) in self.tree.visible_rows() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.add_space(depth as f32 * TREE_INDENT);
                let leaf = node.children.as_ref().is_some_and(Vec::is_empty);
                let (rect, arrow) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::click());
                if !leaf {
                    let color = if arrow.hovered() { t.text_strong } else { t.text_weak };
                    icons::disclosure(ui.painter(), rect, node.expanded, color);
                    if arrow.clicked() {
                        toggle = Some(node.path.clone());
                    }
                }
                let selected = current.as_deref() == Some(node.path.as_path());
                let name = ui
                    .add(Button::selectable(selected, node.name.as_str()).truncate())
                    .on_hover_text(node.path.display().to_string());
                if name.clicked() {
                    select = Some(node.path.clone());
                }
                if name.double_clicked() {
                    toggle = Some(node.path.clone());
                }
                name.context_menu(|ui| {
                    if let Some(m) = disk::folder_menu(ui, &node.path, cut.as_deref()) {
                        menu = Some(m);
                    }
                });
            });
        }
        if let Some(path) = toggle
            && self.tree.toggle(&path)
            && let Some(w) = worker
        {
            w.list_folder(path);
        }
        if let Some(path) = select {
            self.show_folder(path, worker);
        }
        if let Some(m) = menu {
            self.folder_action(m, actions);
        }
    }

    fn filters(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let filter = &mut self.strip.filter;
        ui.label(RichText::new("Rating").color(t.text_weak));
        ui.horizontal_wrapped(|ui| {
            if chip(ui, t, filter.ratings.is_all(), "All", false).clicked() {
                filter.ratings.clear();
            }
            for r in 0..=MAX_RATING {
                if chip(ui, t, filter.ratings.contains(r), &r.to_string(), true)
                    .on_hover_text(if r == 0 { "No rating".to_owned() } else { format!("Exactly {r} stars") })
                    .clicked()
                {
                    filter.ratings.toggle(r);
                }
            }
        });
        ui.add_space(SPACE[1]);
        ui.label(RichText::new("Colour label").color(t.text_weak));
        ui.horizontal_wrapped(|ui| {
            if chip(ui, t, filter.labels.is_all(), "All", false).clicked() {
                filter.labels.clear();
            }
            for label in ColorLabel::ALL {
                if swatch(ui, t, filter.labels.contains(Some(label)), label_color(label))
                    .on_hover_text(label.name())
                    .clicked()
                {
                    filter.labels.toggle(Some(label));
                }
            }
            if chip(ui, t, filter.labels.contains(None), "None", false).on_hover_text("No colour label").clicked() {
                filter.labels.toggle(None);
            }
        });
        ui.add_space(SPACE[1]);
        ui.add(TextEdit::singleline(&mut filter.text).hint_text("Filter by name").desired_width(f32::INFINITY));
        let shown = self.strip.visible().len();
        let total = self.strip.photos.len();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Showing {shown} of {total}")).size(FONT[0]).color(t.text_weak));
            if !self.strip.filter.is_empty() && ui.small_button("Clear filters").clicked() {
                self.clear_filters();
            }
        });
    }

    fn ai_tags(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        tagging: Option<TaggingProgress>,
        actions: &mut Vec<LibraryAction>,
    ) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("AI tags").color(t.text_weak));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new("ON-DEVICE").size(FONT[0]).color(t.success))
                    .on_hover_text("Keywords are found on this computer; no photo leaves it");
                let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                icons::dot(ui.painter(), rect.center(), 3.5, t.success);
            });
        });
        let counts = self.strip.tag_counts();
        if counts.is_empty() {
            ui.label(RichText::new("No keywords in this folder yet").size(FONT[0]).color(t.text_weak));
        }
        ui.horizontal_wrapped(|ui| {
            for (name, count) in counts.iter().take(MAX_TAG_CHIPS) {
                let on = self.strip.filter.tags.iter().any(|s| s.eq_ignore_ascii_case(name));
                if chip(ui, t, on, &format!("{name} {count}"), false).clicked() {
                    self.strip.filter.toggle_tag(name);
                }
            }
        });
        ui.add_space(SPACE[1]);
        match tagging {
            Some(p) => {
                ui.horizontal(|ui| {
                    #[allow(clippy::cast_precision_loss)] // photo counts
                    let fraction = if p.total == 0 { 1.0 } else { p.done as f32 / p.total as f32 };
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width((ui.available_width() - 60.0).max(60.0))
                            .text(format!("Tagging {} / {}", p.done, p.total)),
                    );
                    if ui.button("Stop").clicked() {
                        actions.push(LibraryAction::StopTagging);
                    }
                });
            }
            None => {
                let n = self.strip.tagging_targets().len();
                let label = format!("AI-tag this folder ({n})");
                let button = Button::new(RichText::new(label).color(t.on_accent)).fill(t.accent);
                if ui
                    .add_enabled(n > 0, button)
                    .on_hover_text(
                        "Suggest keywords for the photos without AI keywords (all of them when every photo has some)",
                    )
                    .on_disabled_hover_text("No photo in this folder")
                    .clicked()
                {
                    actions.push(LibraryAction::TagFolder);
                }
            }
        }
    }

    // ----- filmstrip -----

    /// The filmstrip: one card per photo that passes the filters, or one
    /// per variant, right after each other, for a photo that has several.
    pub fn filmstrip(
        &mut self,
        ui: &mut egui::Ui,
        current: Option<&Path>,
        open: OpenVariants<'_>,
        variant_blocks: VariantBlocks,
        actions: &mut Vec<LibraryAction>,
    ) {
        let t = Tokens::of(ui.ctx().theme());
        self.wanted.clear();
        self.strip_hovered = ui.rect_contains_pointer(ui.max_rect());
        if self.loading.is_some() {
            ui.add_space(SPACE[3]);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(RichText::new("Reading the folder\u{2026}").color(t.text_weak));
            });
            return;
        }
        let visible = self.strip.visible();
        if visible.is_empty() {
            ui.add_space(SPACE[3]);
            let text = if self.strip.folder.is_none() {
                "Choose a collection or a folder in the Library"
            } else if self.strip.photos.is_empty() {
                match &self.collection {
                    Some(Collection::Trash) => "The Trash is empty",
                    Some(Collection::Album(_)) => "No photos in this album: drag some onto its name",
                    Some(_) => "No photos here",
                    None => "No photos in this folder",
                }
            } else {
                "No photo matches the filters"
            };
            ui.add(Label::new(RichText::new(text).color(t.text_weak)).wrap());
            return;
        }
        // A row of cards under the photo, as tall as the strip: drag its top
        // edge to make them larger.
        let gap = ui.spacing().item_spacing.x;
        let card_h = ui.available_height().max(60.0);
        let thumb_h = (card_h - 2.0 * CARD_PAD - 4.0 - 2.0 * CARD_LINE).max(20.0);
        let card_w = thumb_h * THUMB_ASPECT + 2.0 * CARD_PAD;
        let step = card_w + gap;

        self.reload_variant_lists();
        let cards = editor::library::strip_cards(&visible, &self.strip.photos, |p| {
            // The open photo's list has its unsaved new variant too.
            if current == Some(p) && !open.list.is_empty() {
                Some(open.list)
            } else {
                self.variant_lists.get(p).map(Vec::as_slice)
            }
        });
        let mut area = egui::ScrollArea::horizontal().id_salt("filmstrip").auto_shrink([false, false]);
        if let Some(target) = self.scroll_to.take()
            && let Some(pos) = cards.iter().position(|c| self.strip.photos[c.index].path == target)
            && !(self.shown_cards.start < pos && pos + 1 < self.shown_cards.end)
        {
            // One card before the target stays in view.
            #[allow(clippy::cast_precision_loss)] // card counts
            let offset = pos.saturating_sub(1) as f32 * step;
            area = area.horizontal_scroll_offset(offset);
        }

        let mut card_actions = Vec::new();
        let several = self.strip.selection().len() > 1;
        let hovered = self.strip_hovered;
        let Self { strip, thumbs, wanted, shown_cards, presets, albums, collection, catalog, variant_thumbs, .. } =
            self;
        let presets: &[String] = presets;
        let (albums, collection): (&[Album], Option<&Collection>) = (albums, collection.as_ref());
        area.show_viewport(ui, |ui, viewport| {
            // The wheel moves along the strip.
            if hovered {
                let dy = ui.input(|i| i.smooth_scroll_delta.y);
                if dy != 0.0 {
                    ui.scroll_with_delta(vec2(dy, 0.0));
                }
            }
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let (start, end) = {
                let start = (viewport.min.x / step).floor().max(0.0) as usize;
                let end = ((viewport.max.x / step).ceil().max(0.0) as usize + 1).min(cards.len());
                (start.min(end), end)
            };
            let first = start.saturating_sub(PREFETCH_CARDS);
            let last = (end + PREFETCH_CARDS).min(cards.len());
            for c in &cards[first..last] {
                let path = &strip.photos[c.index].path;
                if wanted.last() != Some(path) {
                    wanted.push(path.clone());
                }
            }
            *shown_cards = start..end;
            ui.horizontal(|ui| {
                #[allow(clippy::cast_precision_loss)] // card counts
                let before = start as f32 * step;
                #[allow(clippy::cast_precision_loss)] // card counts
                let after = (cards.len() - end) as f32 * step;
                ui.add_space(before);
                for c in &cards[start..end] {
                    let photo = &strip.photos[c.index];
                    let is_current = current == Some(photo.path.as_path());
                    let state = CardState {
                        open: is_current && c.variant.as_ref().is_none_or(|v| Some(v.number) == open.number),
                        selected: strip.is_selected(&photo.path),
                        several,
                        variants: variant_blocks,
                        presets,
                        albums,
                        collection,
                        variant: c.variant.as_ref(),
                    };
                    // A variant shows its own stored preview when there is one.
                    let own = c
                        .variant
                        .as_ref()
                        .and_then(|v| variant_thumb(ui.ctx(), catalog.as_ref(), variant_thumbs, &photo.path, v.number));
                    let own = own.map(ThumbState::Ready);
                    let thumb = own.as_ref().or_else(|| thumbs.get(&photo.path));
                    card(ui, &t, photo, thumb, state, [card_w, card_h], &mut card_actions);
                }
                ui.add_space(after);
            });
        });
        self.drag_hint(ui.ctx(), &t);
        // The overview of a multiple selection shows its thumbnails too.
        let extra: Vec<PathBuf> =
            self.strip.selection().into_iter().filter(|p| !self.wanted.contains(p)).take(MAX_OVERVIEW_THUMBS).collect();
        self.wanted.extend(extra);

        for action in card_actions {
            match action {
                CardAction::Click { path, ctrl, shift, variant } => {
                    if let Some(open) = self.strip.click(&path, ctrl, shift, current) {
                        actions.push(match variant {
                            Some(number) if open == path => LibraryAction::OpenVariant(open, number),
                            _ => LibraryAction::Open(open),
                        });
                    }
                }
                CardAction::Export(p) => actions.push(LibraryAction::Export(self.strip.targets(&p))),
                CardAction::ExportAgain(p) => actions.push(LibraryAction::ExportAgain(self.strip.targets(&p))),
                CardAction::Command(c) => actions.push(LibraryAction::Command(c)),
                CardAction::ApplyPreset(p, i) => actions.push(LibraryAction::ApplyPreset(self.strip.targets(&p), i)),
                CardAction::SavePreset(p) => actions.push(LibraryAction::SavePreset(p)),
                // On a card of a multiple selection, stars and menus act on all of it.
                CardAction::Rate(p, r) => {
                    let targets = self.strip.targets(&p);
                    if let Err(e) = self.set_rating(&targets, r) {
                        actions.push(LibraryAction::Error(e));
                    }
                }
                CardAction::Label(p, l) => {
                    let targets = self.strip.targets(&p);
                    if let Err(e) = self.set_label(&targets, l) {
                        actions.push(LibraryAction::Error(e));
                    }
                }
                CardAction::Rename(p) => self.begin_rename(p),
                CardAction::DeleteVariant(p, info) => actions.push(LibraryAction::DeleteVariant(p, info)),
                CardAction::Trash(p) => {
                    let targets = self.strip.targets(&p);
                    self.trash(&targets, current, actions);
                }
                CardAction::Restore(p) => {
                    let targets = self.strip.targets(&p);
                    self.restore(&targets, actions);
                }
                CardAction::AddToAlbum(p, id) => {
                    let targets = self.strip.targets(&p);
                    self.add_to_album(id, &targets, actions);
                }
                CardAction::NewAlbum(p) => {
                    let targets = self.strip.targets(&p);
                    self.begin_new_album(targets, None);
                }
                CardAction::RemoveFromAlbum(p, id) => {
                    let targets = self.strip.targets(&p);
                    self.remove_from_album(id, &targets, actions);
                }
                CardAction::OpenWith(p) => {
                    if let Err(e) = launch(&platform::open_with_default_app(platform::Os::current(), &p)) {
                        actions.push(LibraryAction::Error(format!("Cannot open the file: {e}")));
                    }
                }
                CardAction::Reveal(p) => {
                    if let Err(e) = launch(&platform::reveal_in_file_manager(platform::Os::current(), &p)) {
                        actions.push(LibraryAction::Error(format!("Cannot open the folder: {e}")));
                    }
                }
            }
        }
    }

    /// The viewer while several photos are selected: one
    /// tile per photo; a click on a tile opens that
    /// photo alone.
    pub fn overview(&mut self, ui: &mut egui::Ui, current: Option<&Path>, actions: &mut Vec<LibraryAction>) {
        let t = Tokens::of(ui.ctx().theme());
        let selection = self.strip.selection();
        let rect = ui.max_rect();
        let painter = ui.painter_at(rect);
        let total = self.strip.visible().len();
        let active = current.map(editor::library::folder_name).unwrap_or_default();
        painter.text(
            rect.left_top() + vec2(SPACE[2], OVERVIEW_HEADER / 2.0),
            egui::Align2::LEFT_CENTER,
            format!("Selected: {} of {total} files  |  Active: {active}", selection.len()),
            FontId::proportional(FONT[1]),
            t.text_weak,
        );
        let grid = Rect::from_min_max(rect.min + vec2(0.0, OVERVIEW_HEADER), rect.max);
        let layout = editor::OverviewLayout::new([grid.width(), grid.height()], selection.len());
        self.wanted_previews = selection.iter().take(layout.shown).cloned().collect();
        let mut clicked = None;
        for (i, path) in selection.iter().take(layout.shown).enumerate() {
            let [x, y] = layout.slot(i);
            let tile = Rect::from_min_size(grid.min + vec2(x, y), vec2(layout.tile[0], layout.tile[1]));
            let response = ui.interact(tile, egui::Id::unique(("aktis-overview", path)), Sense::click_and_drag());
            // Dragged onto an album, as from the filmstrip (with the whole selection).
            response.dnd_set_drag_payload(collections::DraggedPhoto(path.clone()));
            let response = response
                .on_hover_text(format!("{}\nClick to open this photo alone, drag it onto an album", path.display()));
            let is_active = current == Some(path.as_path());
            // The photos sit on the viewer's background, as in Capture One;
            // the tile shows only under the pointer.
            if response.hovered() {
                painter.rect_filled(tile, CornerRadius::same(6), t.bg_overlay);
            }
            if is_active {
                painter.rect_stroke(tile, CornerRadius::same(6), Stroke::new(2.0, t.accent), StrokeKind::Inside);
            }
            let frame = Rect::from_min_max(tile.min + vec2(6.0, 6.0), tile.max - vec2(6.0, 6.0 + 2.0 * CARD_LINE));
            painter.rect_filled(frame, CornerRadius::same(3), t.canvas);
            // The large preview when it has arrived, the thumbnail meanwhile.
            let large = matches!(self.previews.get(path), Some(ThumbState::Ready(_)));
            let shown = if large { self.previews.get(path) } else { self.thumbs.get(path) };
            match shown {
                Some(ThumbState::Ready(texture)) => {
                    let size = texture.size_vec2();
                    let scale = (frame.width() / size.x.max(1.0)).min(frame.height() / size.y.max(1.0));
                    let image = Rect::from_center_size(frame.center(), size * scale);
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    painter.image(texture.id(), image, uv, Color32::WHITE);
                }
                Some(ThumbState::Failed(_)) => {
                    painter.text(
                        frame.center(),
                        egui::Align2::CENTER_CENTER,
                        "No preview",
                        FontId::proportional(FONT[0]),
                        t.text_weak,
                    );
                }
                None => {
                    painter.text(
                        frame.center(),
                        egui::Align2::CENTER_CENTER,
                        "\u{2026}",
                        FontId::proportional(FONT[2]),
                        t.text_weak,
                    );
                }
            }
            if let Some(meta) = self.meta_of(path) {
                badges(&painter, &t, frame, meta);
                for k in 0..MAX_RATING {
                    let c = pos2(frame.left() + 7.0 + f32::from(k) * STAR_CELL, frame.bottom() + CARD_LINE * 1.5);
                    if k < meta.rating {
                        icons::star(&painter, c, 5.5, t.star);
                    }
                }
            }
            painter.with_clip_rect(tile.shrink(4.0)).text(
                pos2(frame.left(), frame.bottom() + CARD_LINE * 0.5 + 2.0),
                egui::Align2::LEFT_CENTER,
                editor::library::folder_name(path),
                FontId::proportional(FONT[0]),
                if is_active { t.text_strong } else { t.text_weak },
            );
            if response.clicked() {
                clicked = Some(path.clone());
            }
        }
        if layout.has_more_tile() {
            let [x, y] = layout.slot(layout.shown);
            let tile = Rect::from_min_size(grid.min + vec2(x, y), vec2(layout.tile[0], layout.tile[1]));
            painter.rect_filled(tile, CornerRadius::same(6), t.bg_raised);
            painter.text(
                tile.center(),
                egui::Align2::CENTER_CENTER,
                format!("+{} more", layout.more),
                FontId::proportional(FONT[4]),
                t.text_weak,
            );
        }
        if let Some(path) = clicked {
            self.strip.select_only(&path);
            self.scroll_to = Some(path.clone());
            actions.push(LibraryAction::Open(path));
        }
    }

    /// The Rename dialog, when open.
    /// The Library's windows: Rename, New/Rename Album, the confirmations.
    pub fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<LibraryAction>) {
        self.rename_dialog(ctx, actions);
        self.album_dialogs(ctx, actions);
        self.disk_dialogs(ctx, actions);
    }

    fn rename_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<LibraryAction>) {
        let Some(d) = &mut self.rename else { return };
        let t = Tokens::of(ctx.theme());
        let (mut confirm, mut cancel) = (false, false);
        let extension = d.path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        dialog(ctx, "aktis-rename", |ui| {
            ui.label(RichText::new("Rename").size(FONT[4]).strong());
            ui.add_space(SPACE[1]);
            ui.label(RichText::new(editor::library::folder_name(&d.path)).color(t.text_weak));
            ui.add_space(SPACE[1]);
            ui.horizontal(|ui| {
                let edit = ui.add(TextEdit::singleline(&mut d.stem).desired_width(260.0));
                if !d.focused {
                    edit.request_focus();
                    d.focused = true;
                }
                if edit.changed() {
                    d.error = None;
                }
                ui.label(extension.as_str());
            });
            if let Some(e) = &d.error {
                ui.label(RichText::new(e.as_str()).color(t.error));
            }
            ui.add_space(SPACE[2]);
            match dialog_buttons(ui, &t, Some("Rename"), "Cancel", |_| {}) {
                Some(Choice::Action) => confirm = true,
                Some(Choice::Cancel) => cancel = true,
                None => {}
            }
        });
        if cancel {
            self.rename = None;
        } else if confirm {
            let (from, stem) = (d.path.clone(), d.stem.clone());
            match self.rename_file(&from, &stem) {
                Ok(Some(to)) => {
                    actions.push(LibraryAction::Renamed { from, to });
                    self.rename = None;
                }
                Ok(None) => self.rename = None,
                Err(e) => {
                    if let Some(d) = &mut self.rename {
                        d.error = Some(e);
                    }
                }
            }
        }
    }
}

/// Runs a program of the system (file manager, default app).
pub(crate) fn launch(l: &platform::Launch) -> std::io::Result<()> {
    l.command().spawn().map(drop)
}

/// The colour of a label.
pub fn label_color(label: ColorLabel) -> Color32 {
    let [_, r, g, b] = label.rgb().to_be_bytes();
    Color32::from_rgb(r, g, b)
}

/// A toggle chip ("All", "landscape 12"); `star` adds a star after the text.
fn chip(ui: &mut egui::Ui, t: &Tokens, on: bool, text: &str, star: bool) -> egui::Response {
    let color = if on { t.on_accent } else { t.text_strong };
    let galley = ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(FONT[0]), color);
    let star_w = if star { 12.0 } else { 0.0 };
    let size = vec2(galley.size().x + star_w + 16.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if on {
            t.accent
        } else if response.hovered() {
            t.bg_overlay
        } else {
            t.fill
        };
        let stroke = Stroke::new(1.0, if on { t.accent } else { t.stroke_weak });
        ui.painter().rect(rect, CornerRadius::same(11), fill, stroke, StrokeKind::Inside);
        let text_w = galley.size().x;
        let pos = pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, color);
        if star {
            icons::star(
                ui.painter(),
                pos2(pos.x + text_w + 7.0, rect.center().y),
                5.0,
                if on { t.on_accent } else { t.star },
            );
        }
    }
    response
}

/// A colour square that can be toggled.
fn swatch(ui: &mut egui::Ui, t: &Tokens, on: bool, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let inner = rect.shrink(4.0);
        ui.painter().rect_filled(inner, CornerRadius::same(3), color);
        if on {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(5),
                Stroke::new(2.0, t.text_strong),
                StrokeKind::Inside,
            );
        } else if response.hovered() {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                CornerRadius::same(5),
                Stroke::new(1.0, t.stroke_strong),
                StrokeKind::Inside,
            );
        }
    }
    response
}

/// How a card is shown.
#[derive(Debug, Clone, Copy)]
struct CardState<'a> {
    /// The photo is open in the editor.
    open: bool,
    /// The photo is selected.
    selected: bool,
    /// Several photos are selected (menus say how many they act on).
    several: bool,
    /// Why the variant commands are unavailable on the open photo.
    variants: VariantBlocks,
    /// The presets the menu offers.
    presets: &'a [String],
    /// The albums the menu offers.
    albums: &'a [Album],
    /// What the filmstrip shows: in the Trash photos are restored, in an
    /// album taken out of it.
    collection: Option<&'a Collection>,
    /// The variant the card shows, for a photo with several.
    variant: Option<&'a VariantInfo>,
}

/// `image` reduced so that its longer side is at most `side`, each pixel
/// the mean of the block it covers.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // pixel counts
fn shrink_rgb(image: &engine::Rgb8Image, side: u32) -> egui::ColorImage {
    let (w, h) = (image.width.max(1) as usize, image.height.max(1) as usize);
    let scale = (side.max(1) as f32 / w.max(h) as f32).min(1.0);
    let (ow, oh) = (((w as f32 * scale).round() as usize).max(1), ((h as f32 * scale).round() as usize).max(1));
    let mut out = Vec::with_capacity(ow * oh * 3);
    for oy in 0..oh {
        let (y0, y1) = (oy * h / oh, ((oy + 1) * h / oh).max(oy * h / oh + 1));
        for ox in 0..ow {
            let (x0, x1) = (ox * w / ow, ((ox + 1) * w / ow).max(ox * w / ow + 1));
            let mut sum = [0_u32; 3];
            for y in y0..y1 {
                let row = &image.pixels[(y * w + x0) * 3..(y * w + x1) * 3];
                for px in row.as_chunks::<3>().0 {
                    for (total, v) in sum.iter_mut().zip(px) {
                        *total += u32::from(*v);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            out.extend(sum.map(|v| ((v + n / 2) / n) as u8));
        }
    }
    egui::ColorImage::from_rgb([ow, oh], &out)
}

/// The thumbnail of variant `number` of `path`, decoded once from the
/// preview the catalog keeps for it; `None` when there is none.
fn variant_thumb(
    ctx: &egui::Context,
    catalog: Option<&Catalog>,
    cache: &mut HashMap<(PathBuf, u32), Option<TextureHandle>>,
    path: &Path,
    number: u32,
) -> Option<TextureHandle> {
    cache
        .entry((path.to_owned(), number))
        .or_insert_with(|| {
            let jpeg = catalog?.variant_preview(path, number).ok()??;
            let t = raw_io::decode_jpeg(&jpeg, editor::worker::THUMBNAIL_SIDE).ok()?;
            let image = egui::ColorImage::from_rgb([t.width as usize, t.height as usize], &t.rgb);
            Some(ctx.load_texture(
                format!("variant:{}:{number}", path.display()),
                image,
                crate::photo_texture::smooth(),
            ))
        })
        .clone()
}

/// One filmstrip card: thumbnail with badges, name, stars, context menu.
fn card(
    ui: &mut egui::Ui,
    t: &Tokens,
    photo: &StripPhoto,
    thumb: Option<&ThumbState<TextureHandle>>,
    state: CardState<'_>,
    size: [f32; 2],
    actions: &mut Vec<CardAction>,
) {
    let is_open = state.open;
    let height = size[1];
    // Dragged onto an album to put it there (with the selection it is in).
    let (rect, response) = ui.allocate_exact_size(vec2(size[0], height), Sense::click_and_drag());
    response.dnd_set_drag_payload(collections::DraggedPhoto(photo.path.clone()));
    let mut hover = match thumb {
        Some(ThumbState::Failed(why)) => format!("{}\nNo preview: {why}", photo.path.display()),
        _ => photo.path.display().to_string(),
    };
    if let Some(v) = state.variant {
        hover = format!("{hover}\n{}", v.name);
    }
    let response = response.on_hover_text(hover);
    let painter = ui.painter_at(rect);
    let fill = if is_open || state.selected {
        t.bg_overlay
    } else if response.hovered() {
        t.bg_raised
    } else {
        t.bg
    };
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if is_open {
        painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(6), Stroke::new(2.0, t.accent), StrokeKind::Inside);
    } else if state.selected {
        painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(6), Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }

    // Thumbnail, letterboxed in a 3:2 frame.
    let frame = Rect::from_min_size(
        rect.min + vec2(CARD_PAD, CARD_PAD),
        vec2(rect.width() - 2.0 * CARD_PAD, height - 2.0 * CARD_PAD - 4.0 - 2.0 * CARD_LINE),
    );
    painter.rect_filled(frame, CornerRadius::same(3), t.canvas);
    match thumb {
        Some(ThumbState::Ready(texture)) => {
            let size = texture.size_vec2();
            let (w, h) = (size.x.max(1.0), size.y.max(1.0));
            let scale = (frame.width() / w).min(frame.height() / h);
            let image = Rect::from_center_size(frame.center(), vec2(w * scale, h * scale));
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(texture.id(), image, uv, Color32::WHITE);
        }
        Some(ThumbState::Failed(_)) => {
            painter.text(
                frame.center(),
                egui::Align2::CENTER_CENTER,
                "No preview",
                FontId::proportional(FONT[0]),
                t.text_weak,
            );
        }
        None => {
            painter.text(
                frame.center(),
                egui::Align2::CENTER_CENTER,
                "\u{2026}",
                FontId::proportional(FONT[2]),
                t.text_weak,
            );
        }
    }
    badges(&painter, t, frame, &photo.meta);

    // Name and stars.
    let name_rect = Rect::from_min_size(pos2(frame.left(), frame.bottom() + 4.0), vec2(frame.width(), CARD_LINE));
    let name_color = if is_open { t.text_strong } else { t.text_weak };
    // A variant's card adds its number: "IMG_0001.CR3 (2)".
    let name = match state.variant {
        Some(v) => format!("{} ({})", photo.name, v.number),
        None => photo.name.clone(),
    };
    ui.put(name_rect, Label::new(RichText::new(name).size(FONT[0]).color(name_color)).truncate().selectable(false));
    let stars_top = name_rect.bottom();
    let star_rect = |i: u8| {
        Rect::from_min_size(pos2(frame.left() + f32::from(i) * STAR_CELL, stars_top), vec2(STAR_CELL, CARD_LINE))
    };
    for i in 0..MAX_RATING {
        let cell = star_rect(i);
        if i < photo.meta.rating {
            icons::star(&painter, cell.center(), 5.5, t.star);
        } else {
            painter.text(
                cell.center(),
                egui::Align2::CENTER_CENTER,
                "\u{b7}",
                FontId::proportional(FONT[3]),
                t.text_weak,
            );
        }
    }

    if response.clicked() {
        let on_star = response.interact_pointer_pos().and_then(|p| (0..MAX_RATING).find(|&i| star_rect(i).contains(p)));
        match on_star {
            // Clicking the active star clears the rating.
            Some(i) => {
                let rating = i + 1;
                let next = if photo.meta.rating == rating { 0 } else { rating };
                actions.push(CardAction::Rate(photo.path.clone(), next));
            }
            None => {
                let m = ui.input(|i| i.modifiers);
                actions.push(CardAction::Click {
                    path: photo.path.clone(),
                    ctrl: m.command,
                    shift: m.shift,
                    variant: state.variant.map(|v| v.number),
                });
            }
        }
    }
    response.context_menu(|ui| card_menu(ui, photo, state, actions));
}

/// Rating, AI keyword and colour badges over a thumbnail.
fn badges(painter: &egui::Painter, t: &Tokens, frame: Rect, meta: &PhotoMeta) {
    let font = FontId::proportional(FONT[0]);
    if meta.rating > 0 {
        let galley = painter.layout_no_wrap(meta.rating.to_string(), font.clone(), t.text_strong);
        let size = vec2(galley.size().x + 22.0, 18.0);
        let badge = Rect::from_min_size(frame.min + vec2(4.0, 4.0), size);
        painter.rect(
            badge,
            CornerRadius::same(4),
            Color32::from_black_alpha(170),
            Stroke::new(1.0, BADGE_STAR),
            StrokeKind::Inside,
        );
        icons::star(painter, pos2(badge.left() + 9.0, badge.center().y), 5.0, BADGE_STAR);
        painter.galley(pos2(badge.left() + 17.0, badge.center().y - galley.size().y / 2.0), galley, t.text_strong);
    }
    if let Some(tag) = meta.best_ai_tag() {
        let text = format!("AI \u{b7} {}", tag.name);
        let galley = painter.layout_no_wrap(text, font, t.text_strong);
        let w = (galley.size().x + 10.0).min(frame.width() - 8.0);
        let badge = Rect::from_min_size(pos2(frame.left() + 4.0, frame.bottom() - 22.0), vec2(w, 18.0));
        painter.rect_filled(badge, CornerRadius::same(4), Color32::from_black_alpha(170));
        painter.with_clip_rect(badge.shrink(2.0)).galley(
            pos2(badge.left() + 5.0, badge.center().y - galley.size().y / 2.0),
            galley,
            t.text_strong,
        );
    }
    if let Some(label) = meta.label {
        let square = Rect::from_min_size(pos2(frame.right() - 16.0, frame.top() + 4.0), vec2(12.0, 12.0));
        painter.rect_filled(square, CornerRadius::same(2), label_color(label));
        painter.rect_stroke(
            square,
            CornerRadius::same(2),
            Stroke::new(1.0, Color32::from_black_alpha(160)),
            StrokeKind::Outside,
        );
    }
}

/// The right-click menu of a card.
fn card_menu(ui: &mut egui::Ui, photo: &StripPhoto, state: CardState<'_>, actions: &mut Vec<CardAction>) {
    let presets = state.presets;
    // Rating, colour and export apply to the whole selection when this card is in it.
    let group = if state.several && state.selected { " (selected photos)" } else { "" };
    let path = &photo.path;
    let soon = "Coming soon";
    let is_open = state.open;
    let open_first = "Open the photo first";
    if ui.add(Button::new("Rename").shortcut_text("F2")).clicked() {
        actions.push(CardAction::Rename(path.clone()));
        ui.close();
    }
    ui.add_enabled(false, Button::new("Batch Rename\u{2026}")).on_disabled_hover_text(soon);
    ui.separator();
    // Variants: on the open photo (the same commands as Image ▸ Variants).
    let why = |block: Option<&'static str>| if is_open { block } else { Some(open_first) };
    variant_item(ui, Command::NewVariant, "New Variant", why(state.variants.new), actions);
    variant_item(ui, Command::CloneVariant, "Clone Variant", why(state.variants.clone), actions);
    // The same presets as Image ▸ Presets ▸ Apply.
    ui.menu_button(format!("Apply Preset{group}"), |ui| {
        if presets.is_empty() {
            ui.add_enabled(false, Button::new("No presets saved yet"))
                .on_disabled_hover_text("Save one with Image \u{25b8} Presets \u{25b8} Save preset\u{2026}");
        }
        for (i, name) in presets.iter().enumerate() {
            if ui.button(name).clicked() {
                actions.push(CardAction::ApplyPreset(path.clone(), i));
                ui.close();
            }
        }
    });
    // The same window as Image ▸ Presets ▸ Save preset…, from this photo's edit.
    if ui.button("Save Preset\u{2026}").on_hover_text("Save this photo's adjustments as a preset").clicked() {
        actions.push(CardAction::SavePreset(path.clone()));
        ui.close();
    }
    ui.separator();
    ui.menu_button(format!("Export{group}"), |ui| {
        let keys = |c: Command| ui.ctx().format_shortcut(&c.shortcuts()[0]);
        let (export_keys, again_keys) = (keys(Command::Export), keys(Command::ExportAgain));
        if ui.add(Button::new(Command::Export.label()).shortcut_text(export_keys)).clicked() {
            actions.push(CardAction::Export(path.clone()));
            ui.close();
        }
        if ui.add(Button::new(Command::ExportAgain.label()).shortcut_text(again_keys)).clicked() {
            actions.push(CardAction::ExportAgain(path.clone()));
            ui.close();
        }
    });
    ui.menu_button("Edit With", |ui| {
        ui.add_enabled(false, Button::new("External editors")).on_disabled_hover_text(soon);
    });
    ui.menu_button("Open With", |ui| {
        if ui.button("Default application").clicked() {
            actions.push(CardAction::OpenWith(path.clone()));
            ui.close();
        }
    });
    ui.separator();
    if ui.button("Show in Explorer").clicked() {
        actions.push(CardAction::Reveal(path.clone()));
        ui.close();
    }
    ui.menu_button(format!("Rating{group}"), |ui| {
        for r in (1..=MAX_RATING).rev() {
            let text = format!("{r} star{}", if r == 1 { "" } else { "s" });
            if ui.add(Button::selectable(photo.meta.rating == r, text).shortcut_text(r.to_string())).clicked() {
                actions.push(CardAction::Rate(path.clone(), r));
                ui.close();
            }
        }
        if ui.add(Button::selectable(photo.meta.rating == 0, "No rating").shortcut_text("0")).clicked() {
            actions.push(CardAction::Rate(path.clone(), 0));
            ui.close();
        }
    });
    ui.menu_button(format!("Color Tag{group}"), |ui| {
        let keys = ["6", "7", "8", "9", ""];
        for (label, key) in ColorLabel::ALL.into_iter().zip(keys) {
            let text = RichText::new(label.name()).color(label_color(label));
            if ui.add(Button::selectable(photo.meta.label == Some(label), text).shortcut_text(key)).clicked() {
                actions.push(CardAction::Label(path.clone(), Some(label)));
                ui.close();
            }
        }
        if ui.add(Button::selectable(photo.meta.label.is_none(), "None")).clicked() {
            actions.push(CardAction::Label(path.clone(), None));
            ui.close();
        }
    });
    ui.separator();
    collections::card_menu_items(ui, path, group, state, actions);
    ui.separator();
    variant_item(ui, Command::DeleteVariant, "Delete Variant\u{2026}", why(state.variants.delete), actions);
}

/// One variant command of a card menu, disabled with the reason `why`.
fn variant_item(
    ui: &mut egui::Ui,
    command: Command,
    label: &str,
    why: Option<&'static str>,
    actions: &mut Vec<CardAction>,
) {
    let shortcut = ui.ctx().format_shortcut(&command.shortcuts()[0]);
    let button = Button::new(label).shortcut_text(shortcut);
    if ui.add_enabled(why.is_none(), button).on_disabled_hover_text(why.unwrap_or_default()).clicked() {
        actions.push(CardAction::Command(command));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_thumbnail_is_the_photo_reduced_by_block_means() {
        let image = engine::Rgb8Image {
            width: 4,
            height: 2,
            pixels: [[0, 0, 0], [100, 50, 10], [200, 200, 200], [255, 255, 255]]
                .iter()
                .cycle()
                .take(8)
                .flatten()
                .copied()
                .collect(),
        };
        let small = shrink_rgb(&image, 2);
        assert_eq!(small.size, [2, 1]);
        assert_eq!(small.pixels[0], egui::Color32::from_rgb(50, 25, 5));
        assert_eq!(small.pixels[1], egui::Color32::from_rgb(228, 228, 228));
        assert_eq!(shrink_rgb(&image, 320).size, [4, 2], "never enlarged");
    }
}
