//! What the catalog knows about a photo, and the Explorer's filters.
//!
//! Pure data and rules, no storage: tested without a database.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// A group of the catalog's photos, as the Library lists them (Capture
/// One's catalog collections, folders and albums).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Collection {
    /// Every photo of the catalog, those in the Trash apart.
    All,
    /// The photos of the last import.
    LastImport,
    /// The photos of one import (Recent Imports), by its number.
    Import(i64),
    /// Photos whose edit was saved in the last [`RECENT_EDIT_DAYS`] days,
    /// the latest first.
    RecentEdits,
    /// Photos moved to the Trash: still in the catalog until it is emptied.
    Trash,
    /// The photos imported from one folder (not its sub-folders).
    Folder(PathBuf),
    /// The photos of an album, by its number.
    Album(i64),
    /// Photos of the catalog whose file name holds this text (case apart),
    /// the Trash apart.
    Search(String),
}

/// How far back Recent Edits goes.
pub const RECENT_EDIT_DAYS: i64 = 30;

/// How many imports Recent Imports lists.
pub const RECENT_IMPORTS: usize = 10;

impl Collection {
    /// Name shown in the Library; an import and an album are named by
    /// their [`ImportBatch`] and [`Album`].
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::All => "All Images".to_owned(),
            Self::LastImport => "Last Import".to_owned(),
            Self::Import(n) => format!("Import {n}"),
            Self::RecentEdits => "Recent Edits".to_owned(),
            Self::Trash => "Trash".to_owned(),
            Self::Folder(dir) => {
                dir.file_name().map_or_else(|| dir.display().to_string(), |n| n.to_string_lossy().into_owned())
            }
            Self::Album(n) => format!("Album {n}"),
            Self::Search(text) => format!("Search \u{201c}{text}\u{201d}"),
        }
    }

    /// Whether the photos shown are in the Trash (actions there restore or
    /// remove instead of trashing).
    #[must_use]
    pub fn is_trash(&self) -> bool {
        matches!(self, Self::Trash)
    }
}

/// One import of the catalog, for Recent Imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportBatch {
    /// Its number (the import's order).
    pub number: i64,
    /// When it was made, in seconds since 1970 (UTC).
    pub when: i64,
    /// The computer's time zone then, in seconds east of UTC.
    pub offset: i64,
    /// Photos it added that are still in the catalog (the Trash apart).
    pub count: usize,
}

impl ImportBatch {
    /// "29 Oct 2026, 14:05": the day and time of the import, local time.
    #[must_use]
    pub fn when_text(&self) -> String {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let local = self.when.saturating_add(self.offset);
        let d = crate::import::Day::from_unix(local);
        let month = MONTHS.get(usize::try_from(d.month).unwrap_or(1).saturating_sub(1)).copied().unwrap_or("");
        let minutes = local.rem_euclid(86_400) / 60;
        format!("{} {month} {}, {:02}:{:02}", d.day, d.year, minutes / 60, minutes % 60)
    }
}

/// An album: photos put together by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    /// Its number, never reused.
    pub id: i64,
    /// Name shown.
    pub name: String,
    /// Photos in it (the Trash apart), its sub-albums apart.
    pub count: usize,
    /// The album it is a sub-album of, if any.
    pub parent: Option<i64>,
}

/// What [`Album`]s look like as a tree: each album with its depth, parents
/// before their sub-albums, by name at every level. An album whose parent
/// is missing is shown at the top.
#[must_use]
pub fn album_tree(albums: &[Album]) -> Vec<(usize, &Album)> {
    fn add<'a>(albums: &'a [Album], parent: Option<i64>, depth: usize, out: &mut Vec<(usize, &'a Album)>) {
        // Depth is bounded by the number of albums (a loop in the data stops here).
        if depth > albums.len() {
            return;
        }
        for a in albums.iter().filter(|a| a.parent == parent) {
            out.push((depth, a));
            add(albums, Some(a.id), depth + 1, out);
        }
    }
    let mut out = Vec::new();
    add(albums, None, 0, &mut out);
    // Orphans (their parent was deleted outside the editor) at the top.
    let ids: std::collections::HashSet<i64> = albums.iter().map(|a| a.id).collect();
    for a in albums.iter().filter(|a| a.parent.is_some_and(|p| !ids.contains(&p))) {
        out.push((0, a));
        add(albums, Some(a.id), 1, &mut out);
    }
    out
}

/// The albums inside album `id` (its sub-albums, theirs, …), `id` apart.
#[must_use]
pub fn album_descendants(albums: &[Album], id: i64) -> Vec<i64> {
    let mut out = Vec::new();
    let mut todo = vec![id];
    while let Some(p) = todo.pop() {
        for a in albums.iter().filter(|a| a.parent == Some(p)) {
            if !out.contains(&a.id) && a.id != id {
                out.push(a.id);
                todo.push(a.id);
            }
        }
    }
    out
}

/// What merging an album into another changed, to undo it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeUndo {
    /// The album merged (gone after the merge).
    pub album: Album,
    /// Its photos.
    pub photos: Vec<PathBuf>,
    /// The album it went into.
    pub into: i64,
    /// Its photos the other album did not have before.
    pub added: Vec<PathBuf>,
    /// Its sub-albums, moved into the other album.
    pub children: Vec<i64>,
}

/// One folder of the catalog's folder tree, as the Library shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderNode {
    /// The folder.
    pub path: PathBuf,
    /// Name shown: the folder's name, or several names joined by the
    /// separator when folders without photos of their own lead to a
    /// single sub-folder ("DATI\Photos"), or the drive.
    pub name: String,
    /// Photos imported from the folder itself.
    pub own: usize,
    /// Photos in it and in its sub-folders.
    pub total: usize,
    /// Its sub-folders that hold photos, by name.
    pub children: Vec<FolderNode>,
}

/// The tree of the folders photos were imported from, with their counts
/// (`folders`: each folder and the photos imported from it). Folders on
/// the way that hold no photos of their own and lead to a single
/// sub-folder are shown as one ("D:\DATI\Photos"), so the tree starts
/// where the photos are.
#[must_use]
pub fn folder_tree(folders: &[(PathBuf, usize)]) -> Vec<FolderNode> {
    let mut nodes: BTreeMap<PathBuf, (usize, Vec<PathBuf>)> = BTreeMap::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    for (dir, n) in folders {
        if dir.as_os_str().is_empty() {
            continue;
        }
        nodes.entry(dir.clone()).or_default().0 += n;
        // Link every folder on the way up to its parent.
        let mut child = dir.clone();
        loop {
            match child.parent().filter(|p| !p.as_os_str().is_empty()) {
                Some(parent) => {
                    let parent = parent.to_path_buf();
                    let entry = nodes.entry(parent.clone()).or_default();
                    if !entry.1.contains(&child) {
                        entry.1.push(child.clone());
                    }
                    child = parent;
                }
                None => {
                    if !roots.contains(&child) {
                        roots.push(child.clone());
                    }
                    break;
                }
            }
        }
    }
    fn build(path: &Path, nodes: &BTreeMap<PathBuf, (usize, Vec<PathBuf>)>) -> FolderNode {
        let (own, kids) = nodes.get(path).cloned().unwrap_or_default();
        let mut children: Vec<FolderNode> = kids.iter().map(|k| build(k, nodes)).collect();
        children.sort_by_key(|c| c.name.to_lowercase());
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let total = own + children.iter().map(|c| c.total).sum::<usize>();
        let mut node = FolderNode { path: path.to_path_buf(), name, own, total, children };
        // One step that only leads on: shown with its single sub-folder.
        while node.own == 0 && node.children.len() == 1 {
            let only = node.children.remove(0);
            let sep =
                if node.name.ends_with(['/', '\\']) { String::new() } else { std::path::MAIN_SEPARATOR.to_string() };
            node = FolderNode { name: format!("{}{sep}{}", node.name, only.name), ..only };
        }
        node
    }
    let mut out: Vec<FolderNode> = roots.iter().map(|r| build(r, &nodes)).collect();
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

/// Sorts `paths` by file name (case apart), then by folder.
pub fn sort_by_name(paths: &mut [PathBuf]) {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    paths.sort_by(|a, b| name(a).cmp(&name(b)).then_with(|| a.cmp(b)));
}

/// Highest rating.
pub const MAX_RATING: u8 = 5;

/// Colour labels, as in Lightroom and Capture One.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColorLabel {
    /// Red.
    Red,
    /// Yellow.
    Yellow,
    /// Green.
    Green,
    /// Blue.
    Blue,
    /// Purple.
    Purple,
}

impl ColorLabel {
    /// Every label, in menu order.
    pub const ALL: [Self; 5] = [Self::Red, Self::Yellow, Self::Green, Self::Blue, Self::Purple];

    /// Name shown in menus and stored in the catalog ("Red").
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
        }
    }

    /// The label with this name, any case.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Display colour as `0xRRGGBB` (the Interface design's swatches).
    #[must_use]
    pub fn rgb(self) -> u32 {
        match self {
            Self::Red => 0x00c4_534a,
            Self::Yellow => 0x00d6_a54a,
            Self::Green => 0x006a_a36a,
            Self::Blue => 0x005b_8fd6,
            Self::Purple => 0x009a_7ad0,
        }
    }

    fn bit(self) -> u8 {
        match self {
            Self::Red => 1,
            Self::Yellow => 2,
            Self::Green => 4,
            Self::Blue => 8,
            Self::Purple => 16,
        }
    }
}

impl fmt::Display for ColorLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Who gave a photo a keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TagSource {
    /// Suggested by the AI tagger (replaced when the folder is tagged again).
    Ai,
    /// Added by the user (never replaced by the AI).
    User,
}

/// A keyword on a photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag {
    /// The keyword ("lake").
    pub name: String,
    /// Who added it.
    pub source: TagSource,
    /// The AI's probability, for AI keywords.
    pub confidence: Option<f32>,
}

/// What the catalog stores for one photo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoMeta {
    /// 0 (no rating) to 5 stars.
    pub rating: u8,
    /// Colour label, if any.
    pub label: Option<ColorLabel>,
    /// Keywords, user ones first, then by name.
    pub tags: Vec<Tag>,
}

impl PhotoMeta {
    /// `true` when the photo has keyword `name` (any case).
    #[must_use]
    pub fn has_tag(&self, name: &str) -> bool {
        self.tags.iter().any(|t| t.name.eq_ignore_ascii_case(name))
    }

    /// The AI keyword with the highest confidence, for the filmstrip badge.
    #[must_use]
    pub fn best_ai_tag(&self) -> Option<&Tag> {
        self.tags
            .iter()
            .filter(|t| t.source == TagSource::Ai)
            .max_by(|a, b| a.confidence.unwrap_or(0.0).total_cmp(&b.confidence.unwrap_or(0.0)))
    }
}

/// One variant of a photo, as the catalog lists it. A variant is one
/// version of the photo's edit (as in Capture One): its own recipe,
/// history and preview. A photo never edited has none
/// stored; it shows as variant 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantInfo {
    /// Number, unique for the photo, starting at 1.
    pub number: u32,
    /// Name shown in menus ("Variant 2").
    pub name: String,
    /// Version of the text format the edit was saved in (by the editor
    /// that saved it; a newer one than this build reads is shown read-only).
    pub format: u32,
}

/// A variant's stored edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVariant {
    /// Number, name and format.
    pub info: VariantInfo,
    /// The edit as text (written by the `recipe-store` crate).
    pub data: String,
}

/// Number of the first variant of every photo.
pub const FIRST_VARIANT: u32 = 1;

/// The number a new variant gets: one above the highest, 1 for the first.
#[must_use]
pub fn next_variant_number(variants: &[VariantInfo]) -> u32 {
    variants.iter().map(|v| v.number).max().map_or(FIRST_VARIANT, |n| n.saturating_add(1))
}

/// The name a variant gets: "Variant 2".
#[must_use]
pub fn variant_name(number: u32) -> String {
    format!("Variant {number}")
}

/// A set of ratings to show ("Rating", a multi-select):
/// any mix of 0 (unrated) to 5 stars; empty means all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RatingSet(u8);

impl RatingSet {
    /// No rating selected: every photo passes.
    pub const ALL: Self = Self(0);

    /// Adds or removes `rating` (values above 5 are ignored).
    pub fn toggle(&mut self, rating: u8) {
        if rating <= MAX_RATING {
            self.0 ^= 1 << rating;
        }
    }

    /// `true` when `rating` is selected.
    #[must_use]
    pub fn contains(self, rating: u8) -> bool {
        rating <= MAX_RATING && self.0 & (1 << rating) != 0
    }

    /// `true` when nothing is selected (or everything is): no filtering.
    #[must_use]
    pub fn is_all(self) -> bool {
        self.0 == 0 || self.0 == 0b11_1111
    }

    /// `true` when a photo with `rating` passes.
    #[must_use]
    pub fn matches(self, rating: u8) -> bool {
        self.is_all() || self.contains(rating.min(MAX_RATING))
    }

    /// Selects nothing (all photos pass).
    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// A set of colour labels to show; empty means all. "No label" is selectable too.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LabelSet(u8);

const NO_LABEL_BIT: u8 = 32;

impl LabelSet {
    /// Adds or removes a label; `None` stands for "no label".
    pub fn toggle(&mut self, label: Option<ColorLabel>) {
        self.0 ^= label.map_or(NO_LABEL_BIT, ColorLabel::bit);
    }

    /// `true` when `label` (or "no label" for `None`) is selected.
    #[must_use]
    pub fn contains(self, label: Option<ColorLabel>) -> bool {
        self.0 & label.map_or(NO_LABEL_BIT, ColorLabel::bit) != 0
    }

    /// `true` when nothing is selected: no filtering.
    #[must_use]
    pub fn is_all(self) -> bool {
        self.0 == 0
    }

    /// `true` when a photo with `label` passes.
    #[must_use]
    pub fn matches(self, label: Option<ColorLabel>) -> bool {
        self.is_all() || self.contains(label)
    }

    /// Selects nothing (all photos pass).
    pub fn clear(&mut self) {
        self.0 = 0;
    }
}

/// The Explorer's filters. Every part must pass; an empty part passes all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Ratings to show.
    pub ratings: RatingSet,
    /// Colour labels to show.
    pub labels: LabelSet,
    /// Keywords a photo must all have (any case).
    pub tags: Vec<String>,
    /// Text the file name must contain (any case).
    pub text: String,
}

impl Filter {
    /// `true` when nothing is filtered out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ratings.is_all() && self.labels.is_all() && self.tags.is_empty() && self.text.trim().is_empty()
    }

    /// `true` when a photo named `file_name` with `meta` passes.
    #[must_use]
    pub fn matches(&self, file_name: &str, meta: &PhotoMeta) -> bool {
        let text = self.text.trim().to_lowercase();
        self.ratings.matches(meta.rating)
            && self.labels.matches(meta.label)
            && self.tags.iter().all(|t| meta.has_tag(t))
            && (text.is_empty() || file_name.to_lowercase().contains(&text))
    }

    /// Adds keyword `name` to the filter, or removes it when present.
    pub fn toggle_tag(&mut self, name: &str) {
        if let Some(i) = self.tags.iter().position(|t| t.eq_ignore_ascii_case(name)) {
            self.tags.remove(i);
        } else {
            self.tags.push(name.to_owned());
        }
    }
}

/// How many photos have each keyword, most frequent first (then by name).
/// Keywords differing only in case count as one.
pub fn tag_counts<'a>(metas: impl IntoIterator<Item = &'a PhotoMeta>) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for meta in metas {
        let mut seen: Vec<String> = Vec::new();
        for tag in &meta.tags {
            let key = tag.name.to_lowercase();
            if seen.contains(&key) {
                continue;
            }
            let entry = counts.entry(key.clone()).or_insert_with(|| (tag.name.clone(), 0));
            entry.1 += 1;
            seen.push(key);
        }
    }
    let mut list: Vec<(String, usize)> = counts.into_values().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn albums_make_a_tree_by_name_with_their_depths() {
        let al = |id: i64, name: &str, parent: Option<i64>| Album { id, name: name.into(), count: 0, parent };
        let albums = [
            al(1, "Trips", None),
            al(2, "Italy", Some(1)),
            al(3, "Best", None),
            al(4, "Como", Some(2)),
            al(5, "Lost", Some(99)),
        ];
        let tree: Vec<(usize, i64)> = album_tree(&albums).into_iter().map(|(d, a)| (d, a.id)).collect();
        assert_eq!(tree, [(0, 1), (1, 2), (2, 4), (0, 3), (0, 5)], "parents first, the orphan at the top level");
        assert_eq!(album_descendants(&albums, 1), [2, 4]);
        assert!(album_descendants(&albums, 3).is_empty());
    }

    #[test]
    fn the_folder_tree_sums_its_sub_folders_and_skips_empty_steps() {
        let p = |s: &str| PathBuf::from(s);
        let folders =
            [(p("/data/photos/2026/a"), 3), (p("/data/photos/2026/b"), 2), (p("/data/photos"), 1), (p("/x"), 4)];
        let tree = folder_tree(&folders);
        // The root holds two branches, so it stays.
        assert_eq!(tree.len(), 1);
        assert_eq!((tree[0].total, tree[0].children.len()), (10, 2));
        let data = &tree[0].children[0];
        assert_eq!(data.path, p("/data/photos"), "data leads only to photos: one step");
        assert_eq!(data.name, format!("data{}photos", std::path::MAIN_SEPARATOR));
        assert_eq!((data.own, data.total), (1, 6));
        let year = &data.children[0];
        assert_eq!((year.name.as_str(), year.total, year.children.len()), ("2026", 5, 2));
        assert_eq!(year.children[0].name, "a");
        assert_eq!((tree[0].children[1].name.as_str(), tree[0].children[1].own), ("x", 4));
        assert!(folder_tree(&[]).is_empty());
    }

    #[test]
    fn collections_have_names_and_paths_sort_by_file_name() {
        assert!(Collection::Trash.is_trash() && !Collection::All.is_trash());
        // 29 Oct 2026, 10:00 UTC.
        let batch = ImportBatch { number: 3, when: 1_793_268_000, offset: 0, count: 4 };
        assert_eq!(batch.when_text(), "29 Oct 2026, 10:00");
        // An hour east of UTC, and a time that passes midnight there.
        assert_eq!(ImportBatch { offset: 3600, ..batch.clone() }.when_text(), "29 Oct 2026, 11:00");
        assert_eq!(ImportBatch { when: 1_793_314_800, offset: 3600, ..batch }.when_text(), "30 Oct 2026, 00:00");
        assert_eq!(Collection::All.label(), "All Images");
        assert_eq!(Collection::Folder(PathBuf::from("D:/Photos/Trip")).label(), "Trip");
        let mut paths = vec![PathBuf::from("b/A2.NEF"), PathBuf::from("a/a1.NEF"), PathBuf::from("a/A2.NEF")];
        sort_by_name(&mut paths);
        assert_eq!(paths, vec![PathBuf::from("a/a1.NEF"), PathBuf::from("a/A2.NEF"), PathBuf::from("b/A2.NEF")]);
    }

    #[test]
    fn a_new_variant_gets_the_number_above_the_highest() {
        let v = |number| VariantInfo { number, name: variant_name(number), format: 1 };
        assert_eq!(next_variant_number(&[]), FIRST_VARIANT);
        assert_eq!(next_variant_number(&[v(1), v(4), v(2)]), 5);
        assert_eq!(next_variant_number(&[v(u32::MAX)]), u32::MAX);
        assert_eq!(variant_name(3), "Variant 3");
    }

    fn meta(rating: u8, label: Option<ColorLabel>, tags: &[&str]) -> PhotoMeta {
        PhotoMeta {
            rating,
            label,
            tags: tags
                .iter()
                .map(|t| Tag { name: (*t).to_owned(), source: TagSource::Ai, confidence: Some(0.1) })
                .collect(),
        }
    }

    #[test]
    fn the_rating_filter_selects_exact_star_counts_not_a_minimum() {
        let mut r = RatingSet::ALL;
        assert!((0..=5).all(|n| r.matches(n)));
        r.toggle(2);
        r.toggle(5);
        assert!(r.matches(2) && r.matches(5));
        assert!(!r.matches(3) && !r.matches(4) && !r.matches(0));
        r.toggle(2);
        assert!(!r.matches(2) && r.matches(5));
        r.toggle(0);
        assert!(r.matches(0), "unrated can be selected on its own");
        r.toggle(9);
        assert!(!r.contains(9));
        r.clear();
        assert!(r.is_all());
    }

    #[test]
    fn selecting_every_rating_is_the_same_as_none() {
        let mut r = RatingSet::ALL;
        for n in 0..=5 {
            r.toggle(n);
        }
        assert!(r.is_all());
    }

    #[test]
    fn label_filter_can_ask_for_unlabelled_photos() {
        let mut l = LabelSet::default();
        assert!(l.matches(None) && l.matches(Some(ColorLabel::Red)));
        l.toggle(Some(ColorLabel::Red));
        assert!(l.matches(Some(ColorLabel::Red)) && !l.matches(Some(ColorLabel::Blue)) && !l.matches(None));
        l.toggle(None);
        assert!(l.matches(None));
    }

    #[test]
    fn every_part_of_the_filter_must_pass() {
        let photo = meta(4, Some(ColorLabel::Green), &["lake", "Sunset"]);
        let mut f = Filter::default();
        assert!(f.is_empty() && f.matches("DSC_0001.NEF", &photo));
        f.toggle_tag("sunset");
        assert!(f.matches("DSC_0001.NEF", &photo));
        f.toggle_tag("boat");
        assert!(!f.matches("DSC_0001.NEF", &photo));
        f.toggle_tag("BOAT");
        assert_eq!(f.tags, vec!["sunset".to_owned()]);
        f.text = "dsc_".into();
        assert!(f.matches("DSC_0001.NEF", &photo));
        f.text = "img".into();
        assert!(!f.matches("DSC_0001.NEF", &photo));
        f.text.clear();
        f.ratings.toggle(5);
        assert!(!f.matches("DSC_0001.NEF", &photo));
    }

    #[test]
    fn keyword_counts_are_sorted_and_ignore_case() {
        let photos =
            [meta(0, None, &["lake", "sky"]), meta(0, None, &["Lake"]), meta(0, None, &["sky", "SKY", "boat"])];
        let counts = tag_counts(&photos);
        assert_eq!(counts, vec![("lake".into(), 2), ("sky".into(), 2), ("boat".into(), 1)]);
    }

    #[test]
    fn labels_round_trip_by_name() {
        for l in ColorLabel::ALL {
            assert_eq!(ColorLabel::from_name(&l.name().to_uppercase()), Some(l));
        }
        assert_eq!(ColorLabel::from_name("orange"), None);
    }

    #[test]
    fn the_best_ai_keyword_has_the_highest_confidence() {
        let mut m = meta(0, None, &[]);
        m.tags.push(Tag { name: "pier".into(), source: TagSource::Ai, confidence: Some(0.02) });
        m.tags.push(Tag { name: "lake".into(), source: TagSource::Ai, confidence: Some(0.4) });
        m.tags.push(Tag { name: "mine".into(), source: TagSource::User, confidence: None });
        assert_eq!(m.best_ai_tag().map(|t| t.name.as_str()), Some("lake"));
    }
}
