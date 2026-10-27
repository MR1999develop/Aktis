//! Browsing photos on disk: the Explorer's folder tree, the photos of a
//! folder, and the filmstrip with its filters.
//!
//! Reading folders is I/O, so it happens in the worker
//! ([`crate::Worker::list_folder`]); this module only holds the results and
//! answers what to show.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};

use catalog::{Filter, PhotoMeta};

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
        } else if raw_io::has_raw_extension(&path) {
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
}

impl Filmstrip {
    /// Shows `folder` with `photos` (paths and catalog data, in order).
    pub fn set_folder(&mut self, folder: PathBuf, photos: Vec<(PathBuf, PhotoMeta)>) {
        self.folder = Some(folder);
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

    /// Updates the catalog data of `path` after a change.
    pub fn set_meta(&mut self, path: &Path, meta: PhotoMeta) {
        if let Some(p) = self.photos.iter_mut().find(|p| p.path == path) {
            p.meta = meta;
        }
    }

    /// Replaces a renamed file, keeping its place.
    pub fn rename(&mut self, from: &Path, to: PathBuf) {
        if let Some(p) = self.photos.iter_mut().find(|p| p.path == from) {
            p.name = folder_name(&to);
            p.path = to;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
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
    fn listing_a_folder_finds_raw_files_and_visible_subfolders() {
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
        assert_eq!(names(&l.photos), vec!["A.arw", "b.NEF"]);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(list_folder(&dir).is_err());
    }
}
