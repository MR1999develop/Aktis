//! The user's keywords: a hierarchy with synonyms, read and written in the
//! plain-text format of Lightroom's "Export Keywords" so an existing keyword
//! list can be brought in unchanged.
//!
//! Format: one keyword per line; each leading tab is one level below the
//! keyword above; `{name}` is a synonym of the keyword one level up;
//! `[name]` is a keyword that is not written into exported files (a
//! category such as `[Luoghi]`). Such categories are never suggested.

use std::collections::HashMap;
use std::fmt;

/// Identifier of a keyword inside one [`Vocabulary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeywordId(u32);

impl KeywordId {
    /// The raw number, for storage.
    #[must_use]
    pub fn get(self) -> u32 {
        self.0
    }

    /// Rebuilds an id read back from storage; only the vocabulary decides
    /// whether it exists.
    #[must_use]
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    fn index(self) -> usize {
        self.0 as usize
    }
}

/// One keyword.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    name: String,
    parent: Option<KeywordId>,
    synonyms: Vec<String>,
    exported: bool,
}

impl Keyword {
    /// The keyword as the user wrote it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The keyword one level up, if any.
    #[must_use]
    pub fn parent(&self) -> Option<KeywordId> {
        self.parent
    }

    /// Other words for the same thing; each is also used as a prompt.
    #[must_use]
    pub fn synonyms(&self) -> &[String] {
        &self.synonyms
    }

    /// False for categories (`[name]`): they organise the list, are not
    /// written into files and are never suggested.
    #[must_use]
    pub fn is_exported(&self) -> bool {
        self.exported
    }
}

/// Why a keyword or a keyword file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocabularyError {
    /// Empty after trimming, longer than 255 bytes, or containing `|` or a
    /// control character.
    InvalidName {
        /// The refused text.
        name: String,
    },
    /// The same name already exists under the same parent.
    Duplicate {
        /// The full path of the existing keyword.
        path: String,
    },
    /// The parent id is not in this vocabulary.
    UnknownKeyword,
    /// More than [`MAX_KEYWORDS`] keywords.
    TooMany,
    /// A line of a keyword file could not be read.
    Line {
        /// 1-based line number.
        line: usize,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for VocabularyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName { name } => write!(f, "'{name}' is not a valid keyword"),
            Self::Duplicate { path } => write!(f, "keyword '{path}' already exists"),
            Self::UnknownKeyword => write!(f, "keyword not found"),
            Self::TooMany => write!(f, "more than {MAX_KEYWORDS} keywords"),
            Self::Line { line, reason } => write!(f, "line {line}: {reason}"),
        }
    }
}

impl std::error::Error for VocabularyError {}

/// Largest vocabulary accepted.
pub const MAX_KEYWORDS: usize = 100_000;

/// Separator of hierarchical paths, as in Lightroom (`Animali|Uccelli|Airone`).
pub const PATH_SEPARATOR: char = '|';

/// The keyword hierarchy. Ids are dense and never reused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Vocabulary {
    keywords: Vec<Keyword>,
    /// (parent, lowercase name) → id, to refuse duplicates and find paths.
    by_parent_and_name: HashMap<(Option<KeywordId>, String), KeywordId>,
}

fn check_name(raw: &str) -> Result<String, VocabularyError> {
    let name = trim(raw);
    let ok = !name.is_empty()
        && name.len() <= 255
        && !name.contains(PATH_SEPARATOR)
        && !name.chars().any(|c| c.is_control() || is_invisible_mark(c));
    if ok { Ok(name.to_owned()) } else { Err(VocabularyError::InvalidName { name: raw.to_owned() }) }
}

/// `text` without white space or byte-order marks at either end.
fn trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// Invisible characters that change how a name reads without being seen:
/// the byte-order mark and the bidirectional marks, overrides and isolates
/// (a name could otherwise show its letters in another order).
fn is_invisible_mark(c: char) -> bool {
    matches!(c, '\u{feff}' | '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

impl Vocabulary {
    /// An empty vocabulary.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of keywords.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keywords.len()
    }

    /// True when there are no keywords.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty()
    }

    /// Adds a keyword under `parent` (or at the top).
    ///
    /// # Errors
    /// [`VocabularyError`] for an invalid name, an unknown parent, a
    /// duplicate (same name, ignoring case, under the same parent) or a full
    /// vocabulary.
    pub fn add(&mut self, name: &str, parent: Option<KeywordId>) -> Result<KeywordId, VocabularyError> {
        let name = check_name(name)?;
        if parent.is_some_and(|p| self.get(p).is_none()) {
            return Err(VocabularyError::UnknownKeyword);
        }
        let key = (parent, name.to_lowercase());
        if let Some(&existing) = self.by_parent_and_name.get(&key) {
            return Err(VocabularyError::Duplicate { path: self.path(existing) });
        }
        if self.keywords.len() >= MAX_KEYWORDS {
            return Err(VocabularyError::TooMany);
        }
        let id = KeywordId(u32::try_from(self.keywords.len()).map_err(|_| VocabularyError::TooMany)?);
        self.keywords.push(Keyword { name, parent, synonyms: Vec::new(), exported: true });
        self.by_parent_and_name.insert(key, id);
        Ok(id)
    }

    /// Adds a synonym; an existing one (ignoring case) is not repeated.
    ///
    /// # Errors
    /// [`VocabularyError`] for an invalid name or an unknown keyword.
    pub fn add_synonym(&mut self, id: KeywordId, synonym: &str) -> Result<(), VocabularyError> {
        let synonym = check_name(synonym)?;
        let kw = self.keywords.get_mut(id.index()).ok_or(VocabularyError::UnknownKeyword)?;
        let lower = synonym.to_lowercase();
        if kw.name.to_lowercase() != lower && !kw.synonyms.iter().any(|s| s.to_lowercase() == lower) {
            kw.synonyms.push(synonym);
        }
        Ok(())
    }

    /// Marks a keyword as a category (`false`) or a normal keyword (`true`).
    ///
    /// # Errors
    /// [`VocabularyError::UnknownKeyword`].
    pub fn set_exported(&mut self, id: KeywordId, exported: bool) -> Result<(), VocabularyError> {
        self.keywords.get_mut(id.index()).ok_or(VocabularyError::UnknownKeyword)?.exported = exported;
        Ok(())
    }

    /// The keyword with this id.
    #[must_use]
    pub fn get(&self, id: KeywordId) -> Option<&Keyword> {
        self.keywords.get(id.index())
    }

    /// Every keyword, parents before children.
    pub fn iter(&self) -> impl Iterator<Item = (KeywordId, &Keyword)> {
        // Ids are indices that fit in u32 (checked in `add`).
        self.keywords.iter().enumerate().filter_map(|(i, k)| Some((KeywordId(u32::try_from(i).ok()?), k)))
    }

    /// Parent, grandparent, … up to the top. Empty for a top-level keyword.
    #[must_use]
    pub fn ancestors(&self, id: KeywordId) -> Vec<KeywordId> {
        let mut out = Vec::new();
        let mut cur = self.get(id).and_then(Keyword::parent);
        // Parents always have smaller ids, so this cannot loop.
        while let Some(p) = cur {
            out.push(p);
            cur = self.get(p).and_then(Keyword::parent);
        }
        out
    }

    /// True if `ancestor` is a parent, grandparent, … of `id`.
    #[must_use]
    pub fn is_ancestor(&self, ancestor: KeywordId, id: KeywordId) -> bool {
        self.ancestors(id).contains(&ancestor)
    }

    /// The full path, `Animali|Uccelli|Airone`. Empty for an unknown id.
    #[must_use]
    pub fn path(&self, id: KeywordId) -> String {
        let mut parts: Vec<&str> =
            self.ancestors(id).iter().rev().filter_map(|&a| self.get(a)).map(Keyword::name).collect();
        match self.get(id) {
            Some(k) => parts.push(k.name()),
            None => return String::new(),
        }
        parts.join(&PATH_SEPARATOR.to_string())
    }

    /// The keyword at a full path, ignoring case.
    #[must_use]
    pub fn find_path(&self, path: &str) -> Option<KeywordId> {
        let mut parent = None;
        for part in path.split(PATH_SEPARATOR) {
            parent = Some(*self.by_parent_and_name.get(&(parent, part.trim().to_lowercase()))?);
        }
        parent
    }

    /// Keywords whose name or synonym equals `name`, ignoring case. More
    /// than one when the same word sits under different parents.
    #[must_use]
    pub fn find_name(&self, name: &str) -> Vec<KeywordId> {
        let lower = name.trim().to_lowercase();
        self.iter()
            .filter(|(_, k)| k.name.to_lowercase() == lower || k.synonyms.iter().any(|s| s.to_lowercase() == lower))
            .map(|(id, _)| id)
            .collect()
    }

    /// Reads a keyword list in Lightroom's text format (see the module
    /// documentation). A byte-order mark and Windows line ends are accepted.
    ///
    /// # Errors
    /// [`VocabularyError::Line`] naming the first line that cannot be read.
    pub fn parse_lightroom(text: &str) -> Result<Self, VocabularyError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut vocab = Self::new();
        // The keyword most recently read at each depth.
        let mut stack: Vec<KeywordId> = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line_no = n + 1;
            let fail = |reason: String| VocabularyError::Line { line: line_no, reason };
            let line = raw.trim_end_matches(['\r', ' ']);
            let depth = line.chars().take_while(|&c| c == '\t').count();
            let content = trim(&line[depth..]);
            if content.is_empty() {
                continue;
            }
            if let Some(syn) = content.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                let owner = depth.checked_sub(1).and_then(|d| stack.get(d)).copied();
                let owner = owner.ok_or_else(|| fail("synonym without a keyword above it".to_owned()))?;
                vocab.add_synonym(owner, syn).map_err(|e| fail(e.to_string()))?;
                continue;
            }
            if depth > stack.len() {
                return Err(fail("indented more than one level below the keyword above".to_owned()));
            }
            stack.truncate(depth);
            let content = trim(content.trim_start_matches('~'));
            let (name, exported) = match content.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                Some(inner) => (inner, false),
                None => (content, true),
            };
            let parent = stack.last().copied();
            // A repeated keyword (same parent) is the same keyword: reuse it.
            let id = match vocab.add(name, parent) {
                Ok(id) => id,
                Err(VocabularyError::Duplicate { .. }) => {
                    let key = (parent, name.trim().to_lowercase());
                    *vocab.by_parent_and_name.get(&key).ok_or_else(|| fail("duplicate keyword".to_owned()))?
                }
                Err(e) => return Err(fail(e.to_string())),
            };
            if !exported {
                vocab.set_exported(id, false).map_err(|e| fail(e.to_string()))?;
            }
            stack.push(id);
        }
        Ok(vocab)
    }

    /// Writes the vocabulary in Lightroom's text format, parents before
    /// children; [`Self::parse_lightroom`] reads back the same hierarchy.
    #[must_use]
    pub fn to_lightroom(&self) -> String {
        let mut children: HashMap<Option<KeywordId>, Vec<KeywordId>> = HashMap::new();
        for (id, k) in self.iter() {
            children.entry(k.parent).or_default().push(id);
        }
        let mut out = String::new();
        let mut todo: Vec<(KeywordId, usize)> =
            children.get(&None).map(|v| v.iter().rev().map(|&id| (id, 0)).collect()).unwrap_or_default();
        while let Some((id, depth)) = todo.pop() {
            let Some(k) = self.get(id) else { continue };
            let tabs = "\t".repeat(depth);
            if k.exported {
                out.push_str(&format!("{tabs}{}\n", k.name));
            } else {
                out.push_str(&format!("{tabs}[{}]\n", k.name));
            }
            for s in &k.synonyms {
                out.push_str(&format!("{tabs}\t{{{s}}}\n"));
            }
            if let Some(kids) = children.get(&Some(id)) {
                todo.extend(kids.iter().rev().map(|&c| (c, depth + 1)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}[Luoghi]\r\n\tItalia\r\n\t\tMilano\r\n\t\t\t{Milan}\r\nAnimali\n\tUccelli\n\t\tAirone\n\t\t\t{Heron}\n\t\t\t{airone cenerino}\n\tCane\n\t\t{dog}\n\nTramonto\n";

    #[test]
    fn a_lightroom_export_is_read_with_hierarchy_synonyms_and_categories() {
        let v = Vocabulary::parse_lightroom(SAMPLE).expect("valid file");
        assert_eq!(v.len(), 8);
        let heron = v.find_path("animali|uccelli|AIRONE").expect("path found ignoring case");
        assert_eq!(v.path(heron), "Animali|Uccelli|Airone");
        let k = v.get(heron).expect("exists");
        assert_eq!(k.synonyms(), ["Heron", "airone cenerino"]);
        let places = v.find_path("Luoghi").expect("category");
        assert!(!v.get(places).expect("exists").is_exported());
        assert!(v.get(heron).expect("exists").is_exported());
        let milano = v.find_path("Luoghi|Italia|Milano").expect("nested under a category");
        assert_eq!(v.ancestors(milano).len(), 2);
        assert!(v.is_ancestor(places, milano));
        assert!(!v.is_ancestor(milano, places));
        assert_eq!(v.find_name("dog"), vec![v.find_path("Animali|Cane").expect("exists")]);
    }

    #[test]
    fn damaged_keyword_files_never_panic_and_what_is_read_writes_back() {
        mutate::check_seed("keywords", "lightroom.txt", SAMPLE.as_bytes());
        for copy in mutate::text_copies(SAMPLE, 71, 3000) {
            // A branch written twice is merged, so the ids may change order on
            // the way back; the tree written out stays the same.
            if let Ok(v) = Vocabulary::parse_lightroom(&copy) {
                let written = v.to_lightroom();
                let again = Vocabulary::parse_lightroom(&written).expect("own output reads back");
                assert_eq!(again.to_lightroom(), written, "read: {copy:?}");
                assert_eq!(again.len(), v.len());
            }
        }
    }

    #[test]
    fn writing_and_reading_back_gives_the_same_vocabulary() {
        let v = Vocabulary::parse_lightroom(SAMPLE).expect("valid file");
        let again = Vocabulary::parse_lightroom(&v.to_lightroom()).expect("own output reads back");
        assert_eq!(again, v);
    }

    #[test]
    fn bad_lines_are_reported_with_their_number() {
        let jump = Vocabulary::parse_lightroom("Animali\n\t\tAirone\n");
        assert!(matches!(jump, Err(VocabularyError::Line { line: 2, .. })), "{jump:?}");
        let orphan = Vocabulary::parse_lightroom("{solo}\n");
        assert!(matches!(orphan, Err(VocabularyError::Line { line: 1, .. })), "{orphan:?}");
        let pipe = Vocabulary::parse_lightroom("a|b\n");
        assert!(matches!(pipe, Err(VocabularyError::Line { line: 1, .. })), "{pipe:?}");
    }

    #[test]
    fn the_same_word_can_live_under_different_parents_but_not_twice_under_one() {
        let mut v = Vocabulary::new();
        let fr = v.add("Francia", None).expect("valid");
        let us = v.add("Texas", None).expect("valid");
        let p1 = v.add("Parigi", Some(fr)).expect("valid");
        let p2 = v.add("Parigi", Some(us)).expect("same name, other parent");
        assert_ne!(p1, p2);
        assert_eq!(v.find_name("parigi").len(), 2);
        assert!(matches!(v.add("PARIGI", Some(fr)), Err(VocabularyError::Duplicate { .. })));
        // A repeated line in a file is the same keyword, not an error.
        let file = Vocabulary::parse_lightroom("Cane\n\t{dog}\nCane\n\t{doggo}\n").expect("valid");
        assert_eq!(file.len(), 1);
        assert_eq!(file.get(KeywordId(0)).expect("exists").synonyms(), ["dog", "doggo"]);
    }

    #[test]
    fn the_sample_keyword_list_reads() {
        let v = Vocabulary::parse_lightroom(include_str!("../testdata/keywords-it.txt")).expect("valid file");
        let monkey = v.find_path("Soggetti|Animali|Scimmia").expect("exists");
        assert_eq!(v.find_name("mandrillo"), vec![monkey]);
        assert!(!v.get(v.find_path("Luoghi").expect("exists")).expect("exists").is_exported());
    }

    #[test]
    fn invalid_names_and_parents_are_refused() {
        let mut v = Vocabulary::new();
        assert!(matches!(v.add("  ", None), Err(VocabularyError::InvalidName { .. })));
        assert!(matches!(v.add(&"x".repeat(256), None), Err(VocabularyError::InvalidName { .. })));
        assert_eq!(v.add("a", Some(KeywordId(7))), Err(VocabularyError::UnknownKeyword));
        // A name that shows its letters in another order than it has them.
        for mark in ['\u{202e}', '\u{2066}', '\u{200f}', '\u{feff}'] {
            assert!(
                matches!(v.add(&format!("ab{mark}cd"), None), Err(VocabularyError::InvalidName { .. })),
                "{mark:?}"
            );
        }
        assert!(v.add("x".repeat(255).as_str(), None).is_ok(), "255 bytes is the limit, not beyond");
    }

    #[test]
    fn a_second_byte_order_mark_or_a_space_after_the_tilde_is_not_part_of_the_name() {
        let v = Vocabulary::parse_lightroom("\u{feff}\u{feff}[Luoghi]\n~ [Casa]\n").expect("valid");
        let places = v.find_path("Luoghi").expect("the mark is dropped");
        assert!(!v.get(places).expect("exists").is_exported());
        assert!(!v.get(v.find_path("Casa").expect("exists")).expect("exists").is_exported());
    }
}
