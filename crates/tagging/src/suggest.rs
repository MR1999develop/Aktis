//! The semi-automatic part: what the user decided for an image, and which
//! keywords to propose next.

use std::collections::BTreeMap;

use crate::{KeywordId, Vocabulary};

/// The state of one keyword on one image. Absent means undecided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    /// The user applied the keyword (accepted a suggestion or typed it).
    /// A positive example for learning.
    Accepted,
    /// The user rejected a suggestion. Never proposed again for this image;
    /// a negative example for learning.
    Rejected,
    /// Applied automatically above the auto-apply threshold and not yet
    /// reviewed. Counts as applied, but is never a learning example.
    Auto,
}

/// The decisions taken on one image. Keyed by keyword; parents of an applied
/// keyword are implied and not stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageTags {
    decisions: BTreeMap<KeywordId, Decision>,
}

impl ImageTags {
    /// No decisions yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a decision, replacing any earlier one for that keyword.
    pub fn set(&mut self, keyword: KeywordId, decision: Decision) {
        self.decisions.insert(keyword, decision);
    }

    /// Forgets the decision for `keyword` (back to undecided).
    pub fn clear(&mut self, keyword: KeywordId) {
        self.decisions.remove(&keyword);
    }

    /// The decision for `keyword`, if any.
    #[must_use]
    pub fn get(&self, keyword: KeywordId) -> Option<Decision> {
        self.decisions.get(&keyword).copied()
    }

    /// Every decision.
    pub fn iter(&self) -> impl Iterator<Item = (KeywordId, Decision)> + '_ {
        self.decisions.iter().map(|(&k, &d)| (k, d))
    }

    /// Keywords applied to the image (accepted or auto), without the
    /// implied parents.
    pub fn applied(&self) -> impl Iterator<Item = KeywordId> + '_ {
        self.iter().filter(|(_, d)| matches!(d, Decision::Accepted | Decision::Auto)).map(|(k, _)| k)
    }

    /// Applied keywords with their parents, as written into files
    /// (categories excluded), sorted and without repetition.
    #[must_use]
    pub fn applied_with_parents(&self, vocab: &Vocabulary) -> Vec<KeywordId> {
        let mut out: Vec<KeywordId> =
            self.applied().flat_map(|k| std::iter::once(k).chain(vocab.ancestors(k))).collect();
        out.retain(|&k| vocab.get(k).is_some_and(crate::Keyword::is_exported));
        out.sort_unstable();
        out.dedup();
        out
    }

    /// What this image says about `keyword` as a learning example:
    /// `Some(true)` if the user applied it or a keyword below it,
    /// `Some(false)` if the user rejected it or a keyword above it, `None`
    /// otherwise. Auto-applied keywords say nothing.
    #[must_use]
    pub fn label_for(&self, keyword: KeywordId, vocab: &Vocabulary) -> Option<bool> {
        let mut rejected = false;
        for (k, d) in self.iter() {
            match d {
                Decision::Accepted if k == keyword || vocab.is_ancestor(keyword, k) => return Some(true),
                Decision::Rejected if k == keyword || vocab.is_ancestor(k, keyword) => rejected = true,
                _ => {}
            }
        }
        rejected.then_some(false)
    }
}

/// How many keywords to propose and when to apply them without asking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuggestPolicy {
    /// Keywords below this probability are not proposed.
    pub min_probability: f32,
    /// At most this many proposals per image, most likely first.
    pub max_suggestions: usize,
    /// Apply without asking at or above this probability; `None` (the
    /// default) never applies anything on its own.
    pub auto_apply_above: Option<f32>,
    /// Auto-apply only keywords the user has confirmed on at least this
    /// many images, so a zero-shot guess alone is never applied silently.
    pub auto_apply_min_examples: usize,
}

impl Default for SuggestPolicy {
    fn default() -> Self {
        Self { min_probability: 0.05, max_suggestions: 12, auto_apply_above: None, auto_apply_min_examples: 10 }
    }
}

/// A proposed keyword.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Suggestion {
    /// The keyword.
    pub keyword: KeywordId,
    /// Estimated probability that it applies, in `[0, 1]`.
    pub probability: f32,
    /// True when the policy says to apply it without asking.
    pub auto_apply: bool,
}

/// Picks the proposals from per-keyword probabilities.
///
/// A keyword is proposed when it is a normal (exported) keyword, nothing has
/// been decided for it on this image, it is not implied by an applied
/// keyword below it, no keyword above it was rejected, and its probability
/// reaches the policy minimum. Of a keyword and one below it, only the more
/// specific is proposed: accepting it applies the parent as well.
pub(crate) fn select(
    probabilities: &[(KeywordId, f32)],
    tags: &ImageTags,
    vocab: &Vocabulary,
    policy: &SuggestPolicy,
    confirmed_examples: impl Fn(KeywordId) -> usize,
) -> Vec<Suggestion> {
    let applied: Vec<KeywordId> = tags.applied().collect();
    let rejected: Vec<KeywordId> = tags.iter().filter(|(_, d)| *d == Decision::Rejected).map(|(k, _)| k).collect();

    let mut candidates: Vec<(KeywordId, f32)> = probabilities
        .iter()
        .copied()
        .filter(|&(k, p)| {
            p >= policy.min_probability
                && vocab.get(k).is_some_and(crate::Keyword::is_exported)
                && tags.get(k).is_none()
                && !applied.iter().any(|&a| vocab.is_ancestor(k, a))
                && !rejected.iter().any(|&r| vocab.is_ancestor(r, k))
        })
        .collect();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    let specific: Vec<(KeywordId, f32)> = candidates
        .iter()
        .copied()
        .filter(|&(k, _)| !candidates.iter().any(|&(other, _)| vocab.is_ancestor(k, other)))
        .take(policy.max_suggestions)
        .collect();

    specific
        .into_iter()
        .map(|(keyword, probability)| Suggestion {
            keyword,
            probability,
            auto_apply: policy.auto_apply_above.is_some_and(|t| probability >= t)
                && confirmed_examples(keyword) >= policy.auto_apply_min_examples,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> (Vocabulary, [KeywordId; 5]) {
        let mut v = Vocabulary::new();
        let animals = v.add("Animali", None).expect("valid");
        let birds = v.add("Uccelli", Some(animals)).expect("valid");
        let heron = v.add("Airone", Some(birds)).expect("valid");
        let sunset = v.add("Tramonto", None).expect("valid");
        let places = v.add("Luoghi", None).expect("valid");
        v.set_exported(places, false).expect("exists");
        (v, [animals, birds, heron, sunset, places])
    }

    #[test]
    fn only_the_most_specific_keyword_of_a_branch_is_proposed() {
        let (v, [animals, birds, heron, sunset, places]) = vocab();
        let probs = [(animals, 0.9), (birds, 0.7), (heron, 0.4), (sunset, 0.3), (places, 0.99)];
        let s = select(&probs, &ImageTags::new(), &v, &SuggestPolicy::default(), |_| 0);
        let got: Vec<_> = s.iter().map(|s| s.keyword).collect();
        assert_eq!(got, vec![heron, sunset], "categories never, parents folded into the child");
    }

    #[test]
    fn decided_and_implied_keywords_are_not_proposed_again() {
        let (v, [animals, birds, heron, sunset, _]) = vocab();
        let probs = [(animals, 0.9), (birds, 0.7), (heron, 0.4), (sunset, 0.3)];
        let mut tags = ImageTags::new();
        tags.set(birds, Decision::Accepted);
        tags.set(sunset, Decision::Rejected);
        let s = select(&probs, &tags, &v, &SuggestPolicy::default(), |_| 0);
        assert_eq!(s.iter().map(|s| s.keyword).collect::<Vec<_>>(), vec![heron]);

        // Rejecting a parent silences everything below it.
        let mut tags = ImageTags::new();
        tags.set(animals, Decision::Rejected);
        let s = select(&probs, &tags, &v, &SuggestPolicy::default(), |_| 0);
        assert_eq!(s.iter().map(|s| s.keyword).collect::<Vec<_>>(), vec![sunset]);
    }

    #[test]
    fn auto_apply_needs_both_the_threshold_and_confirmed_examples() {
        let (v, [_, _, heron, sunset, _]) = vocab();
        let probs = [(heron, 0.97), (sunset, 0.98)];
        let policy = SuggestPolicy { auto_apply_above: Some(0.95), ..SuggestPolicy::default() };
        let s = select(&probs, &ImageTags::new(), &v, &policy, |k| if k == heron { 25 } else { 2 });
        let auto: Vec<_> = s.iter().filter(|s| s.auto_apply).map(|s| s.keyword).collect();
        assert_eq!(auto, vec![heron]);
        let off = select(&probs, &ImageTags::new(), &v, &SuggestPolicy::default(), |_| 1000);
        assert!(off.iter().all(|s| !s.auto_apply), "off by default");
    }

    #[test]
    fn threshold_and_limit_are_respected_most_likely_first() {
        let (v, [animals, _, heron, sunset, _]) = vocab();
        let probs = [(sunset, 0.2), (heron, 0.6), (animals, 0.01)];
        let policy = SuggestPolicy { max_suggestions: 1, ..SuggestPolicy::default() };
        let s = select(&probs, &ImageTags::new(), &v, &policy, |_| 0);
        assert_eq!(s.iter().map(|s| s.keyword).collect::<Vec<_>>(), vec![heron]);
    }

    #[test]
    fn labels_follow_the_hierarchy_and_ignore_auto() {
        let (v, [animals, birds, heron, sunset, _]) = vocab();
        let mut tags = ImageTags::new();
        tags.set(heron, Decision::Accepted);
        tags.set(sunset, Decision::Auto);
        assert_eq!(tags.label_for(animals, &v), Some(true), "a heron is an animal");
        assert_eq!(tags.label_for(heron, &v), Some(true));
        assert_eq!(tags.label_for(sunset, &v), None, "auto tags teach nothing");

        let mut tags = ImageTags::new();
        tags.set(birds, Decision::Rejected);
        assert_eq!(tags.label_for(heron, &v), Some(false), "no bird, so no heron");
        assert_eq!(tags.label_for(animals, &v), None, "could still be another animal");
    }

    #[test]
    fn applied_keywords_include_parents_but_not_categories() {
        let (mut v, [animals, birds, heron, _, places]) = vocab();
        let milano = v.add("Milano", Some(places)).expect("valid");
        let mut tags = ImageTags::new();
        tags.set(heron, Decision::Accepted);
        tags.set(milano, Decision::Auto);
        assert_eq!(tags.applied_with_parents(&v), vec![animals, birds, heron, milano]);
    }
}
