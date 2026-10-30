//! Browsing photos on disk: the Explorer's folder tree, the photos of a
//! folder, and the filmstrip with its filters.
//!
//! Reading folders is I/O, so it happens in the worker
//! ([`crate::Worker::list_folder`]); this module only holds the results and
//! answers what to show.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

use catalog::{Filter, PhotoMeta, VariantInfo};

/// One folder in the Explorer tree.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderNode {
    /// Full path.
    pub path: PathBuf,
    /// Name shown ("Bellagio", or "C:\" for a drive).
    pub name: String,
    /// Sub-folders once listed; `None` until the folder is first expanded.
    pub children: Option<Vec<FolderNode>>,
    /// Shown open.
    pub expanded: bool,
}

impl FolderNode {
    /// A folder not listed yet.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        let name = folder_name(&path);
        Self { path, name, children: None, expanded: false }
    }
}

/// Name shown for a folder: its last component, or the whole path for a
/// root such as `C:\` or `/`.
#[must_use]
pub fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The Explorer's folder tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderTree {
    /// Top-level folders (Pictures, drives…).
    pub roots: Vec<FolderNode>,
}

impl FolderTree {
    /// A tree with these roots, none listed yet.
    #[must_use]
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots: roots.into_iter().map(FolderNode::new).collect() }
    }

    /// The node for `path`, if it is in the tree.
    #[must_use]
    pub fn find(&self, path: &Path) -> Option<&FolderNode> {
        fn walk<'a>(nodes: &'a [FolderNode], path: &Path) -> Option<&'a FolderNode> {
            for n in nodes {
                if n.path == path {
                    return Some(n);
                }
                if path.starts_with(&n.path)
                    && let Some(found) = n.children.as_deref().and_then(|c| walk(c, path))
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&self.roots, path)
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut FolderNode> {
        fn walk<'a>(nodes: &'a mut [FolderNode], path: &Path) -> Option<&'a mut FolderNode> {
            for n in nodes {
                if n.path == path {
                    return Some(n);
                }
                if path.starts_with(&n.path)
                    && let Some(children) = n.children.as_deref_mut()
                    && let Some(found) = walk(children, path)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&mut self.roots, path)
    }

    /// Records the sub-folders of `path` (from the worker). Expansion state
    /// of sub-folders already known is kept.
    pub fn set_children(&mut self, path: &Path, mut subfolders: Vec<PathBuf>) {
        let Some(node) = self.find_mut(path) else { return };
        sort_paths(&mut subfolders);
        let old = node.children.take().unwrap_or_default();
        node.children = Some(
            subfolders
                .into_iter()
                .map(|p| old.iter().find(|o| o.path == p).cloned().unwrap_or_else(|| FolderNode::new(p)))
                .collect(),
        );
    }

    /// Opens or closes `path`; returns `true` when it must be listed first.
    pub fn toggle(&mut self, path: &Path) -> bool {
        let Some(node) = self.find_mut(path) else { return false };
        node.expanded = !node.expanded;
        node.expanded && node.children.is_none()
    }

    /// Opens every folder from a root down to `path` (to show the current
    /// folder at start-up); returns the folders that must be listed first.
    pub fn reveal(&mut self, path: &Path) -> Vec<PathBuf> {
        let mut to_list = Vec::new();
        let ancestors: Vec<PathBuf> = path.ancestors().map(Path::to_path_buf).collect();
        for dir in ancestors.iter().rev() {
            if let Some(node) = self.find_mut(dir) {
                if dir.as_path() != path {
                    node.expanded = true;
                }
                if node.children.is_none() {
                    to_list.push(dir.clone());
                }
            }
        }
        to_list
    }

    /// Rows to draw, depth first: (depth, node).
    #[must_use]
    pub fn visible_rows(&self) -> Vec<(usize, &FolderNode)> {
        fn walk<'a>(nodes: &'a [FolderNode], depth: usize, out: &mut Vec<(usize, &'a FolderNode)>) {
            for n in nodes {
                out.push((depth, n));
                if n.expanded
                    && let Some(children) = &n.children
                {
                    walk(children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, 0, &mut out);
        out
    }
}

/// Sorts paths by file name, ignoring case, numbers in natural order
/// ("IMG_2" before "IMG_10").
pub fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| natural_cmp(&folder_name(a), &folder_name(b)));
}

/// Compares names ignoring case, with runs of digits compared as numbers.
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut s = String::new();
                    while let Some(ch) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(ch);
                        it.next();
                    }
                    s
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (mt, nt) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let order = mt.len().cmp(&nt.len()).then_with(|| mt.cmp(nt));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(c), Some(d)) => {
                let order = c.to_lowercase().cmp(d.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// What a folder contains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderListing {
    /// Sub-folders (not hidden), sorted.
    pub subfolders: Vec<PathBuf>,
    /// RAW files, sorted.
    pub photos: Vec<PathBuf>,
}

/// Lists `dir`: visible sub-folders and RAW files, both in natural order.
///
/// # Errors
/// The folder cannot be read. Entries that cannot be read are skipped.
pub fn list_folder(dir: &Path) -> io::Result<FolderListing> {
    let mut listing = FolderListing::default();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name();
        let hidden = name.to_string_lossy().starts_with('.') || name.to_string_lossy().starts_with('$');
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        // Follow links to folders and files like the system file manager does.
        let is_dir = kind.is_dir() || (kind.is_symlink() && path.is_dir());
        if is_dir {
            if !hidden {
                listing.subfolders.push(path);
            }
        } else if raw_io::has_photo_extension(&path) {
            listing.photos.push(path);
        }
    }
    sort_paths(&mut listing.subfolders);
    sort_paths(&mut listing.photos);
    Ok(listing)
}

/// A photo in the filmstrip.
#[derive(Debug, Clone, PartialEq)]
pub struct StripPhoto {
    /// The file.
    pub path: PathBuf,
    /// File name shown under the thumbnail.
    pub name: String,
    /// Rating, label and keywords from the catalog.
    pub meta: PhotoMeta,
}

/// The photos of the current folder and the Explorer's filter over them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filmstrip {
    /// The folder shown.
    pub folder: Option<PathBuf>,
    /// Every photo of the folder, in order.
    pub photos: Vec<StripPhoto>,
    /// The Explorer's filters.
    pub filter: Filter,
    /// Selected photos (several with Ctrl or Shift), in no particular order.
    selected: Vec<PathBuf>,
    /// Where a Shift+click range starts.
    anchor: Option<PathBuf>,
}

impl Filmstrip {
    /// Shows `folder` with `photos` (paths and catalog data, in order).
    pub fn set_folder(&mut self, folder: PathBuf, photos: Vec<(PathBuf, PhotoMeta)>) {
        if self.folder.as_deref() != Some(folder.as_path()) {
            self.selected.clear();
            self.anchor = None;
        }
        self.folder = Some(folder);
        self.selected.retain(|s| photos.iter().any(|(p, _)| p == s));
        if self.anchor.as_ref().is_some_and(|a| !photos.iter().any(|(p, _)| p == a)) {
            self.anchor = None;
        }
        self.photos = photos
            .into_iter()
            .map(|(path, meta)| {
                let name = folder_name(&path);
                StripPhoto { path, name, meta }
            })
            .collect();
    }

    /// Indices of the photos that pass the filter, in order.
    #[must_use]
    pub fn visible(&self) -> Vec<usize> {
        self.photos.iter().enumerate().filter(|(_, p)| self.filter.matches(&p.name, &p.meta)).map(|(i, _)| i).collect()
    }

    /// Index of `path` among all photos.
    #[must_use]
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.photos.iter().position(|p| p.path == path)
    }

    /// The visible photo after (`step` = 1) or before (`step` = -1) `path`;
    /// the first visible one when `path` is not visible.
    #[must_use]
    pub fn neighbour(&self, path: Option<&Path>, step: isize) -> Option<&Path> {
        let visible = self.visible();
        let at = path.and_then(|p| visible.iter().position(|&i| self.photos[i].path == p));
        let next = match at {
            Some(pos) => pos.checked_add_signed(step).filter(|n| *n < visible.len())?,
            None => 0,
        };
        visible.get(next).map(|&i| self.photos[i].path.as_path())
    }

    /// The photo to show once `gone` leave the filmstrip while `current` is
    /// open: the next visible photo after `current` that stays, else the
    /// one before it; `None` when `current` stays (or nothing is left).
    #[must_use]
    pub fn after_removing(&self, gone: &[PathBuf], current: &Path) -> Option<PathBuf> {
        if !gone.iter().any(|g| g == current) {
            return None;
        }
        let visible: Vec<&Path> = self.visible().into_iter().map(|i| self.photos[i].path.as_path()).collect();
        let at = visible.iter().position(|p| *p == current)?;
        let stays = |p: &Path| !gone.iter().any(|g| g == p);
        let after = visible[at + 1..].iter().copied().find(|p| stays(p));
        after.or_else(|| visible[..at].iter().rev().copied().find(|p| stays(p))).map(Path::to_path_buf)
    }

    /// Updates the catalog data of `path` after a change.
    pub fn set_meta(&mut self, path: &Path, meta: PhotoMeta) {
        if let Some(p) = self.photos.iter_mut().find(|p| p.path == path) {
            p.meta = meta;
        }
    }

    /// Keywords of the folder's photos, with how many photos have each,
    /// most frequent first.
    #[must_use]
    pub fn tag_counts(&self) -> Vec<(String, usize)> {
        catalog::tag_counts(self.photos.iter().map(|p| &p.meta))
    }

    /// The photos "AI-tag this folder" works on: those without AI keywords
    /// yet, or every photo when all have some (tag again).
    #[must_use]
    pub fn tagging_targets(&self) -> Vec<PathBuf> {
        let untagged: Vec<PathBuf> =
            self.photos.iter().filter(|p| p.meta.best_ai_tag().is_none()).map(|p| p.path.clone()).collect();
        if untagged.is_empty() { self.photos.iter().map(|p| p.path.clone()).collect() } else { untagged }
    }

    /// Replaces a renamed file, keeping its place.
    pub fn rename(&mut self, from: &Path, to: PathBuf) {
        for s in self.selected.iter_mut().chain(self.anchor.iter_mut()).filter(|s| s.as_path() == from) {
            to.clone_into(s);
        }
        if let Some(p) = self.photos.iter_mut().find(|p| p.path == from) {
            p.name = folder_name(&to);
            p.path = to;
        }
    }

    // ----- selection, as in a photo viewer -----

    /// A click on a filmstrip card, with Ctrl (Cmd on macOS) and Shift as
    /// held. Returns the photo to open, if the click opens one:
    ///
    /// - click: only this photo is selected and opened;
    /// - Ctrl+click: adds the photo (and opens it), or removes it when it
    ///   was selected and others remain;
    /// - Shift+click: selects from the last Ctrl/plain click (or the open
    ///   photo) to this one and opens it; with Ctrl the range is added.
    pub fn click(&mut self, path: &Path, ctrl: bool, shift: bool, current: Option<&Path>) -> Option<PathBuf> {
        self.index_of(path)?;
        if shift {
            let visible: Vec<&Path> = self.visible().into_iter().map(|i| self.photos[i].path.as_path()).collect();
            let anchor = self.anchor.as_deref().or(current).filter(|a| visible.contains(a)).unwrap_or(path);
            let (Some(a), Some(b)) =
                (visible.iter().position(|p| *p == anchor), visible.iter().position(|p| *p == path))
            else {
                return None;
            };
            let range: Vec<PathBuf> = visible[a.min(b)..=a.max(b)].iter().map(|p| p.to_path_buf()).collect();
            if !ctrl {
                self.selected.clear();
            }
            for p in range {
                if !self.selected.contains(&p) {
                    self.selected.push(p);
                }
            }
            return Some(path.to_owned());
        }
        if ctrl {
            if self.is_selected(path) {
                if self.selected.len() > 1 {
                    self.selected.retain(|p| p != path);
                }
                return None;
            }
            self.selected.push(path.to_owned());
            self.anchor = Some(path.to_owned());
            return Some(path.to_owned());
        }
        self.select_only(path);
        Some(path.to_owned())
    }

    /// Only `path` selected (arrow keys, a photo opened from elsewhere).
    pub fn select_only(&mut self, path: &Path) {
        self.selected = vec![path.to_owned()];
        self.anchor = Some(path.to_owned());
    }

    /// Every visible photo selected (Ctrl+A).
    pub fn select_all(&mut self) {
        self.selected = self.visible().into_iter().map(|i| self.photos[i].path.clone()).collect();
    }

    /// `true` when `path` is selected.
    #[must_use]
    pub fn is_selected(&self, path: &Path) -> bool {
        self.selected.iter().any(|p| p == path)
    }

    /// The selected photos that pass the filter, in filmstrip order.
    #[must_use]
    pub fn selection(&self) -> Vec<PathBuf> {
        self.visible().into_iter().map(|i| &self.photos[i].path).filter(|p| self.is_selected(p)).cloned().collect()
    }

    /// What an action on the card of `clicked` (stars, its menu) applies
    /// to: the whole selection when the card is part of a selection of
    /// several, else that photo alone.
    #[must_use]
    pub fn targets(&self, clicked: &Path) -> Vec<PathBuf> {
        let selection = self.selection();
        if selection.len() > 1 && selection.iter().any(|p| p == clicked) { selection } else { vec![clicked.to_owned()] }
    }

    /// What a key (rating, label) applies to: the selection, else the open photo.
    #[must_use]
    pub fn selected_or(&self, current: Option<&Path>) -> Vec<PathBuf> {
        let selection = self.selection();
        if selection.is_empty() { current.map(Path::to_owned).into_iter().collect() } else { selection }
    }
}

/// Why a new file name was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// The name is empty.
    Empty,
    /// The name holds a character file systems refuse (`/ \ : * ? " < > |`).
    BadCharacter(char),
    /// Another file already has that name.
    Exists,
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Type a name"),
            Self::BadCharacter(c) => write!(f, "A file name cannot contain \u{201c}{c}\u{201d}"),
            Self::Exists => f.write_str("A file with that name already exists in this folder"),
        }
    }
}

impl std::error::Error for RenameError {}

/// Characters refused in file names on at least one supported system.
const FORBIDDEN_IN_NAMES: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// The new path of `from` renamed to `stem`, keeping its extension and
/// folder. `exists` tells whether a path is taken (the file system, in the
/// window). Renaming to the same name gives `from` back.
///
/// # Errors
/// [`RenameError`] for an empty name, a forbidden character or a name
/// already taken.
pub fn rename_target(from: &Path, stem: &str, exists: impl Fn(&Path) -> bool) -> Result<PathBuf, RenameError> {
    let stem = stem.trim();
    if stem.is_empty() || stem.chars().all(|c| c == '.') {
        return Err(RenameError::Empty);
    }
    if let Some(c) = stem.chars().find(|c| FORBIDDEN_IN_NAMES.contains(c) || c.is_control()) {
        return Err(RenameError::BadCharacter(c));
    }
    let name = match from.extension() {
        Some(ext) => format!("{stem}.{}", ext.to_string_lossy()),
        None => stem.to_owned(),
    };
    let to = from.with_file_name(name);
    if to == from {
        return Ok(to);
    }
    // A change of case only is allowed: on Windows and macOS the old file
    // is "found" under the new name. Only the names are compared: the folder
    // is the same, and on Windows `with_file_name` writes `\` where `from`
    // may have `/`.
    let lower = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let same_file_other_case = lower(&to) == lower(from);
    if exists(&to) && !same_file_other_case {
        return Err(RenameError::Exists);
    }
    Ok(to)
}

/// The name without its extension, to edit in the Rename dialog.
#[must_use]
pub fn file_stem(path: &Path) -> String {
    path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

/// Widest a filmstrip card grows before the filmstrip makes another column.
pub const MAX_CARD_WIDTH: f32 = 240.0;

/// Columns and card width of a filmstrip `width` points wide, with `gap`
/// between cards: one column whose card grows with the strip up to
/// [`MAX_CARD_WIDTH`], then as many columns as keep every card at most
/// that wide (as Capture One's browser does).
#[must_use]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // small counts
pub fn filmstrip_columns(width: f32, gap: f32) -> (usize, f32) {
    let width = width.max(1.0);
    let columns = (((width + gap) / (MAX_CARD_WIDTH + gap)).ceil() as usize).max(1);
    let card = (width - gap * (columns - 1) as f32) / columns as f32;
    (columns, card)
}

/// One card of the filmstrip: a photo, or one of its variants when it has
/// several (each variant is a card of its own, right after the one before,
/// as in Capture One).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripCard {
    /// Index of the photo in [`Filmstrip::photos`].
    pub index: usize,
    /// The variant the card shows; `None` for a photo with one variant (or
    /// none stored), shown as the photo itself.
    pub variant: Option<VariantInfo>,
}

/// The cards for the `visible` photos (indices into `photos`), in order:
/// one per photo, or one per variant, in number order, for a photo whose
/// `variants_of` lists more than one.
#[must_use]
pub fn strip_cards<'a>(
    visible: &[usize],
    photos: &[StripPhoto],
    variants_of: impl Fn(&Path) -> Option<&'a [VariantInfo]>,
) -> Vec<StripCard> {
    let mut cards = Vec::with_capacity(visible.len());
    for &index in visible {
        match variants_of(&photos[index].path).filter(|v| v.len() > 1) {
            Some(list) => cards.extend(list.iter().map(|v| StripCard { index, variant: Some(v.clone()) })),
            None => cards.push(StripCard { index, variant: None }),
        }
    }
    cards
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn the_filmstrip_grows_then_makes_columns() {
        assert_eq!(filmstrip_columns(160.0, 6.0), (1, 160.0));
        assert_eq!(filmstrip_columns(MAX_CARD_WIDTH, 6.0).0, 1);
        let (c, w) = filmstrip_columns(300.0, 6.0);
        assert_eq!(c, 2);
        assert!((w - 147.0).abs() < 1e-4);
        let (c, w) = filmstrip_columns(1200.0, 6.0);
        assert!(c >= 5 && w <= MAX_CARD_WIDTH, "{c} {w}");
    }

    use super::*;

    #[test]
    fn a_photo_with_several_variants_gets_a_card_for_each_right_after_the_one_before() {
        let photos: Vec<StripPhoto> = ["a.NEF", "b.NEF", "c.NEF"]
            .iter()
            .map(|n| StripPhoto { path: PathBuf::from(n), name: (*n).to_owned(), meta: PhotoMeta::default() })
            .collect();
        let info = |n: u32| VariantInfo { number: n, name: format!("Variant {n}"), format: 1 };
        let b = vec![info(1), info(3)];
        let c = vec![info(1)];
        let lists = |p: &Path| match p.to_str() {
            Some("b.NEF") => Some(b.as_slice()),
            Some("c.NEF") => Some(c.as_slice()),
            _ => None,
        };
        let cards = strip_cards(&[0, 1, 2], &photos, lists);
        let shown: Vec<(usize, Option<u32>)> =
            cards.iter().map(|c| (c.index, c.variant.as_ref().map(|v| v.number))).collect();
        assert_eq!(shown, vec![(0, None), (1, Some(1)), (1, Some(3)), (2, None)]);
        // A filtered-out photo has no card, whatever its variants.
        assert_eq!(strip_cards(&[0, 2], &photos, lists).len(), 2);
    }
    use catalog::ColorLabel;

    #[test]
    fn names_sort_naturally_and_ignore_case() {
        let mut v = vec!["IMG_10.NEF", "img_2.NEF", "IMG_1.NEF", "b", "A", "IMG_002.NEF"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["A", "b", "IMG_1.NEF", "IMG_002.NEF", "img_2.NEF", "IMG_10.NEF"]);
    }

    #[test]
    fn the_tree_lists_folders_on_demand_and_keeps_open_state() {
        let mut t = FolderTree::new(vec![PathBuf::from("/photos")]);
        assert!(t.toggle(Path::new("/photos")), "first opening needs a listing");
        t.set_children(Path::new("/photos"), vec!["/photos/2026".into(), "/photos/2025".into()]);
        let rows: Vec<(usize, String)> = t.visible_rows().into_iter().map(|(d, n)| (d, n.name.clone())).collect();
        assert_eq!(rows, vec![(0, "photos".into()), (1, "2025".into()), (1, "2026".into())]);

        assert!(t.toggle(Path::new("/photos/2026")));
        t.set_children(Path::new("/photos/2026"), vec!["/photos/2026/Como".into()]);
        // Listing the parent again keeps 2026 open.
        t.set_children(Path::new("/photos"), vec!["/photos/2026".into(), "/photos/2025".into()]);
        assert_eq!(t.visible_rows().len(), 4);
        assert!(!t.toggle(Path::new("/photos")), "closing needs no listing");
        assert_eq!(t.visible_rows().len(), 1);
    }

    #[test]
    fn revealing_a_folder_opens_its_parents_and_asks_for_listings() {
        let mut t = FolderTree::new(vec![PathBuf::from("/photos")]);
        let wanted = t.reveal(Path::new("/photos/2026/Como"));
        assert_eq!(wanted, vec![PathBuf::from("/photos")]);
        t.set_children(Path::new("/photos"), vec!["/photos/2026".into()]);
        let wanted = t.reveal(Path::new("/photos/2026/Como"));
        assert_eq!(wanted, vec![PathBuf::from("/photos/2026")]);
        assert!(t.find(Path::new("/photos/2026")).is_some_and(|n| n.expanded));
    }

    fn strip() -> Filmstrip {
        let mut s = Filmstrip::default();
        let meta = |rating, label| PhotoMeta { rating, label, tags: vec![] };
        s.set_folder(
            "/f".into(),
            vec![
                ("/f/a.NEF".into(), meta(5, None)),
                ("/f/b.NEF".into(), meta(2, Some(ColorLabel::Red))),
                ("/f/c.NEF".into(), meta(5, Some(ColorLabel::Red))),
            ],
        );
        s
    }

    #[test]
    fn the_filter_hides_photos_and_navigation_skips_them() {
        let mut s = strip();
        assert_eq!(s.visible(), vec![0, 1, 2]);
        s.filter.ratings.toggle(5);
        assert_eq!(s.visible(), vec![0, 2]);
        assert_eq!(s.neighbour(Some(Path::new("/f/a.NEF")), 1), Some(Path::new("/f/c.NEF")));
        assert_eq!(s.neighbour(Some(Path::new("/f/c.NEF")), 1), None);
        assert_eq!(s.neighbour(Some(Path::new("/f/c.NEF")), -1), Some(Path::new("/f/a.NEF")));
        assert_eq!(s.neighbour(Some(Path::new("/f/b.NEF")), 1), Some(Path::new("/f/a.NEF")), "hidden → first");
        s.filter.labels.toggle(Some(ColorLabel::Red));
        assert_eq!(s.visible(), vec![2]);
    }

    #[test]
    fn metadata_and_renames_update_in_place() {
        let mut s = strip();
        s.set_meta(Path::new("/f/a.NEF"), PhotoMeta { rating: 1, ..PhotoMeta::default() });
        assert_eq!(s.photos[0].meta.rating, 1);
        s.rename(Path::new("/f/b.NEF"), "/f/z.NEF".into());
        assert_eq!(s.photos[1].name, "z.NEF");
        assert_eq!(s.index_of(Path::new("/f/z.NEF")), Some(1));
    }

    #[test]
    fn after_a_removal_the_next_photo_shows_else_the_previous() {
        let mut s = Filmstrip::default();
        let photos: Vec<(PathBuf, PhotoMeta)> = ["/f/a.NEF", "/f/b.NEF", "/f/c.NEF", "/f/d.NEF"]
            .iter()
            .map(|p| (PathBuf::from(p), PhotoMeta::default()))
            .collect();
        s.set_folder(PathBuf::from("/f"), photos);
        let p = |n: &str| PathBuf::from(format!("/f/{n}.NEF"));
        assert_eq!(s.after_removing(&[p("b"), p("c")], &p("b")), Some(p("d")));
        assert_eq!(s.after_removing(&[p("d")], &p("d")), Some(p("c")), "the last one: the previous");
        assert_eq!(s.after_removing(&[p("a")], &p("b")), None, "the open photo stays");
        assert_eq!(s.after_removing(&[p("a"), p("b"), p("c"), p("d")], &p("a")), None);
    }

    #[test]
    fn listing_a_folder_finds_photos_and_visible_subfolders() {
        let dir = std::env::temp_dir().join(format!("rne-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub 2")).unwrap();
        std::fs::create_dir_all(dir.join("sub 10")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        for f in ["b.NEF", "A.arw", "notes.txt", "c.jpg"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let l = list_folder(&dir).unwrap();
        let names = |v: &[PathBuf]| v.iter().map(|p| folder_name(p)).collect::<Vec<_>>();
        assert_eq!(names(&l.subfolders), vec!["Sub 2", "sub 10"]);
        assert_eq!(names(&l.photos), vec!["A.arw", "b.NEF", "c.jpg"], "RAW files and pictures, not other files");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(list_folder(&dir).is_err());
    }

    #[test]
    fn renaming_keeps_the_folder_and_extension_and_refuses_bad_names() {
        let from = Path::new("/f/DSC_0001.NEF");
        let taken = |p: &Path| p == Path::new("/f/Taken.NEF");
        assert_eq!(rename_target(from, " Como ", taken), Ok(PathBuf::from("/f/Como.NEF")));
        assert_eq!(rename_target(from, "DSC_0001", taken), Ok(from.to_path_buf()));
        assert_eq!(rename_target(from, "", taken), Err(RenameError::Empty));
        assert_eq!(rename_target(from, "..", taken), Err(RenameError::Empty));
        assert_eq!(rename_target(from, "a/b", taken), Err(RenameError::BadCharacter('/')));
        assert_eq!(rename_target(from, "a?", taken), Err(RenameError::BadCharacter('?')));
        assert_eq!(rename_target(from, "Taken", taken), Err(RenameError::Exists));
        let everything = |_: &Path| true;
        assert_eq!(rename_target(from, "dsc_0001", everything), Ok(PathBuf::from("/f/dsc_0001.NEF")), "case only");
        assert_eq!(file_stem(from), "DSC_0001");
        assert_eq!(rename_target(Path::new("/f/noext"), "x", taken), Ok(PathBuf::from("/f/x")));
    }

    #[test]
    fn tagging_works_on_untagged_photos_first() {
        use catalog::{Tag, TagSource};
        let mut s = strip();
        assert_eq!(s.tagging_targets().len(), 3);
        let ai = |name: &str| Tag { name: name.into(), source: TagSource::Ai, confidence: Some(0.1) };
        s.set_meta(Path::new("/f/a.NEF"), PhotoMeta { tags: vec![ai("lake")], ..PhotoMeta::default() });
        assert_eq!(s.tagging_targets(), vec![PathBuf::from("/f/b.NEF"), PathBuf::from("/f/c.NEF")]);
        for p in ["/f/b.NEF", "/f/c.NEF"] {
            s.set_meta(Path::new(p), PhotoMeta { tags: vec![ai("lake")], ..PhotoMeta::default() });
        }
        assert_eq!(s.tagging_targets().len(), 3, "all tagged: tag again");
        assert_eq!(s.tag_counts(), vec![("lake".to_owned(), 3)]);
    }

    #[test]
    fn clicks_select_like_a_photo_viewer() {
        let mut s = Filmstrip::default();
        let p = |n: &str| PathBuf::from(format!("/f/{n}.NEF"));
        let photos = ["a", "b", "c", "d", "e"].map(|n| (p(n), PhotoMeta::default())).to_vec();
        s.set_folder("/f".into(), photos.clone());
        let path = |n: &str| p(n);

        // Plain click: one photo, opened.
        assert_eq!(s.click(&path("b"), false, false, None), Some(path("b")));
        assert_eq!(s.selection(), vec![path("b")]);
        // Shift+click: the range from the anchor, the clicked one opened.
        assert_eq!(s.click(&path("d"), false, true, Some(&path("b"))), Some(path("d")));
        assert_eq!(s.selection(), vec![path("b"), path("c"), path("d")]);
        // Ctrl+click on a selected photo removes it, without opening anything.
        assert_eq!(s.click(&path("c"), true, false, Some(&path("d"))), None);
        assert_eq!(s.selection(), vec![path("b"), path("d")]);
        // Ctrl+click adds and becomes the anchor; Ctrl+Shift adds a range.
        assert_eq!(s.click(&path("a"), true, false, None), Some(path("a")));
        assert_eq!(s.click(&path("c"), true, true, None), Some(path("c")));
        assert_eq!(s.selection(), vec![path("a"), path("b"), path("c"), path("d")]);
        // Shift without Ctrl replaces the selection with the range.
        s.click(&path("e"), false, true, None);
        assert_eq!(s.selection(), vec![path("a"), path("b"), path("c"), path("d"), path("e")]);
        s.click(&path("b"), false, false, None);
        s.click(&path("b"), true, false, None);
        assert_eq!(s.selection(), vec![path("b")], "the last selected photo stays");

        // Actions on a card of the selection apply to all of it.
        s.click(&path("c"), false, true, None);
        assert_eq!(s.targets(&path("c")), vec![path("b"), path("c")]);
        assert_eq!(s.targets(&path("e")), vec![path("e")]);
        assert_eq!(s.selected_or(Some(&path("e"))), vec![path("b"), path("c")]);

        // Renames follow; another folder clears; Ctrl+A takes every visible photo.
        s.rename(&path("c"), path("z"));
        assert!(s.is_selected(&path("z")));
        s.select_all();
        assert_eq!(s.selection().len(), 5);
        s.set_folder("/g".into(), photos);
        assert!(s.selection().is_empty());
        assert_eq!(s.selected_or(Some(&path("a"))), vec![path("a")]);
    }
}
