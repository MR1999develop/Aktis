//! What the catalog knows about a photo, and the Explorer's filters.
//!
//! Pure data and rules, no storage: tested without a database.

use std::collections::BTreeMap;
use std::fmt;

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
/// history, node positions and preview. A photo never edited has none
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

/// A set of ratings to show, as in a photo viewer's "Rating (multi-select)":
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
