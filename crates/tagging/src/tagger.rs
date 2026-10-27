//! The tagger: zero-shot scores from the keyword prompts plus the learned
//! per-keyword corrections.

use std::collections::HashMap;
use std::fmt;

use crate::Embedding;
use crate::learn::{self, Label, Sample, logistic};
use crate::suggest::{self, ImageTags, SuggestPolicy, Suggestion};
use crate::{Adapter, EmbeddingError, EncodeError, KeywordId, LearnSettings, ModelId, TextEncoder, Vocabulary};

/// How a cosine similarity becomes a logit: `scale × cos + bias`. SigLIP
/// learns both during training, so the result is a real per-keyword
/// probability (sigmoid), not a share among competing labels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZeroShotHead {
    scale: f32,
    bias: f32,
}

impl ZeroShotHead {
    /// A head with a positive, finite scale and a finite bias.
    #[must_use]
    pub fn new(scale: f32, bias: f32) -> Option<Self> {
        (scale.is_finite() && scale > 0.0 && bias.is_finite()).then_some(Self { scale, bias })
    }

    /// The logit for a cosine similarity.
    #[must_use]
    pub fn logit(&self, cosine: f32) -> f32 {
        self.scale * cosine + self.bias
    }
}

/// Why the tagger refused an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggerError {
    /// An embedding does not match the others (length).
    Embedding(EmbeddingError),
    /// The text model failed.
    Encode(EncodeError),
    /// The text encoder belongs to another model than the tagger.
    ModelMismatch {
        /// The tagger's model.
        expected: ModelId,
        /// The encoder's model.
        actual: ModelId,
    },
    /// The keyword is not in the vocabulary.
    UnknownKeyword,
    /// The prompt template has no `{}` for the keyword.
    Template,
}

impl fmt::Display for TaggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Embedding(e) => write!(f, "{e}"),
            Self::Encode(e) => write!(f, "text model: {e}"),
            Self::ModelMismatch { expected, actual } => {
                write!(f, "text encoder is '{actual}' but the tagger uses '{expected}'")
            }
            Self::UnknownKeyword => write!(f, "keyword not found"),
            Self::Template => write!(f, "the prompt template must contain {{}} where the keyword goes"),
        }
    }
}

impl std::error::Error for TaggerError {}

impl From<EmbeddingError> for TaggerError {
    fn from(e: EmbeddingError) -> Self {
        Self::Embedding(e)
    }
}

/// Zero-shot logit for a keyword without prompts: "unlikely" until the
/// user's examples say otherwise (`σ(-8)` ≈ 0.03 %).
const NO_PROMPT_LOGIT: f32 = -8.0;

/// Background images sampled per keyword when learning.
const MAX_BACKGROUND: usize = 256;

/// Texts sent to the text model in one call.
const TEXT_BATCH: usize = 32;

/// Scores keywords for images and learns from the user's decisions.
#[derive(Debug, Clone)]
pub struct Tagger {
    model: ModelId,
    head: ZeroShotHead,
    vocab: Vocabulary,
    dim: Option<usize>,
    prompts: HashMap<KeywordId, Vec<Embedding>>,
    adapters: HashMap<KeywordId, Adapter>,
    settings: LearnSettings,
}

impl Tagger {
    /// A tagger for `model` over `vocab`, with no prompts encoded yet.
    #[must_use]
    pub fn new(model: ModelId, head: ZeroShotHead, vocab: Vocabulary) -> Self {
        Self {
            model,
            head,
            vocab,
            dim: None,
            prompts: HashMap::new(),
            adapters: HashMap::new(),
            settings: LearnSettings::default(),
        }
    }

    /// The model whose embeddings this tagger compares.
    #[must_use]
    pub fn model(&self) -> &ModelId {
        &self.model
    }

    /// The keywords.
    #[must_use]
    pub fn vocabulary(&self) -> &Vocabulary {
        &self.vocab
    }

    /// Changes how learning is tuned (takes effect at the next `learn`).
    pub fn set_learn_settings(&mut self, settings: LearnSettings) {
        self.settings = settings;
    }

    /// The prompts of every normal keyword: its name and each synonym, put
    /// into `template` at `{}` ("una foto di {}"). Categories get none.
    ///
    /// # Errors
    /// [`TaggerError::Template`] when `template` has no `{}`.
    pub fn prompt_texts(&self, template: &str) -> Result<Vec<(KeywordId, String)>, TaggerError> {
        if !template.contains("{}") {
            return Err(TaggerError::Template);
        }
        Ok(self
            .vocab
            .iter()
            .filter(|(_, k)| k.is_exported())
            .flat_map(|(id, k)| {
                std::iter::once(k.name()).chain(k.synonyms().iter().map(String::as_str)).map(move |w| (id, w))
            })
            .map(|(id, w)| (id, template.replace("{}", w)))
            .collect())
    }

    /// Encodes every prompt with the text model, replacing earlier prompts.
    ///
    /// # Errors
    /// [`TaggerError`] for a bad template, an encoder of another model, a
    /// model failure or embeddings of inconsistent length.
    pub fn encode_prompts(&mut self, encoder: &mut dyn TextEncoder, template: &str) -> Result<(), TaggerError> {
        if encoder.model() != &self.model {
            return Err(TaggerError::ModelMismatch { expected: self.model.clone(), actual: encoder.model().clone() });
        }
        let texts = self.prompt_texts(template)?;
        let mut by_keyword: HashMap<KeywordId, Vec<Embedding>> = HashMap::new();
        for chunk in texts.chunks(TEXT_BATCH) {
            let strs: Vec<&str> = chunk.iter().map(|(_, t)| t.as_str()).collect();
            let embs = encoder.encode_texts(&strs).map_err(TaggerError::Encode)?;
            if embs.len() != chunk.len() {
                return Err(TaggerError::Encode(EncodeError::new(format!(
                    "{} embeddings for {} texts",
                    embs.len(),
                    chunk.len()
                ))));
            }
            for ((id, _), e) in chunk.iter().zip(embs) {
                by_keyword.entry(*id).or_default().push(e);
            }
        }
        self.prompts.clear();
        for (id, embs) in by_keyword {
            self.set_prompts(id, embs)?;
        }
        Ok(())
    }

    /// Sets the prompt embeddings of one keyword directly (from a cache).
    /// An empty list means "no visual meaning": the keyword is then learned
    /// from the user's examples only.
    ///
    /// # Errors
    /// [`TaggerError`] for an unknown keyword or a length that differs from
    /// the embeddings already set.
    pub fn set_prompts(&mut self, keyword: KeywordId, prompts: Vec<Embedding>) -> Result<(), TaggerError> {
        if self.vocab.get(keyword).is_none() {
            return Err(TaggerError::UnknownKeyword);
        }
        for e in &prompts {
            self.check_dim(e)?;
        }
        self.prompts.insert(keyword, prompts);
        Ok(())
    }

    /// The learned correction of a keyword, if the user has given examples.
    #[must_use]
    pub fn adapter(&self, keyword: KeywordId) -> Option<&Adapter> {
        self.adapters.get(&keyword)
    }

    fn check_dim(&mut self, e: &Embedding) -> Result<(), EmbeddingError> {
        match self.dim {
            Some(d) if d != e.dim() => Err(EmbeddingError::DimensionMismatch { expected: d, actual: e.dim() }),
            Some(_) => Ok(()),
            None => {
                self.dim = Some(e.dim());
                Ok(())
            }
        }
    }

    fn require_dim(&self, e: &Embedding) -> Result<(), EmbeddingError> {
        match self.dim {
            Some(d) if d != e.dim() => Err(EmbeddingError::DimensionMismatch { expected: d, actual: e.dim() }),
            _ => Ok(()),
        }
    }

    /// The model's own opinion: the best of the keyword's prompts.
    fn zero_shot(&self, keyword: KeywordId, image: &Embedding) -> f32 {
        self.prompts.get(&keyword).filter(|p| !p.is_empty()).map_or(NO_PROMPT_LOGIT, |prompts| {
            prompts
                .iter()
                .filter_map(|p| p.cosine(image).ok())
                .map(|c| self.head.logit(c))
                .fold(f32::NEG_INFINITY, f32::max)
        })
    }

    fn logit(&self, keyword: KeywordId, image: &Embedding) -> f32 {
        let correction = self.adapters.get(&keyword).map_or(0.0, |a| a.correction(image.as_slice()));
        self.zero_shot(keyword, image) + correction
    }

    /// Probability that `keyword` applies to the image.
    ///
    /// # Errors
    /// [`TaggerError`] for an unknown keyword or an embedding of another length.
    pub fn probability(&self, keyword: KeywordId, image: &Embedding) -> Result<f32, TaggerError> {
        self.require_dim(image)?;
        if self.vocab.get(keyword).is_none() {
            return Err(TaggerError::UnknownKeyword);
        }
        Ok(logistic(self.logit(keyword, image)))
    }

    /// Probability of every normal keyword, in vocabulary order.
    ///
    /// # Errors
    /// [`TaggerError::Embedding`] for an embedding of another length.
    pub fn probabilities(&self, image: &Embedding) -> Result<Vec<(KeywordId, f32)>, TaggerError> {
        self.require_dim(image)?;
        Ok(self
            .vocab
            .iter()
            .filter(|(_, k)| k.is_exported())
            .map(|(id, _)| (id, logistic(self.logit(id, image))))
            .collect())
    }

    /// What to propose for an image, given what was already decided on it.
    ///
    /// # Errors
    /// [`TaggerError::Embedding`] for an embedding of another length.
    pub fn suggest(
        &self,
        image: &Embedding,
        tags: &ImageTags,
        policy: &SuggestPolicy,
    ) -> Result<Vec<Suggestion>, TaggerError> {
        let probs = self.probabilities(image)?;
        Ok(suggest::select(&probs, tags, &self.vocab, policy, |k| self.adapters.get(&k).map_or(0, Adapter::positives)))
    }

    /// Re-learns one keyword from every image's decisions. Without any
    /// positive or negative example the keyword goes back to zero-shot.
    ///
    /// # Errors
    /// [`TaggerError`] for an unknown keyword or embeddings of another length.
    pub fn learn(&mut self, keyword: KeywordId, images: &[(&Embedding, &ImageTags)]) -> Result<(), TaggerError> {
        if self.vocab.get(keyword).is_none() {
            return Err(TaggerError::UnknownKeyword);
        }
        let mut labelled = Vec::new();
        let mut unlabelled = Vec::new();
        for (e, tags) in images {
            self.check_dim(e)?;
            match tags.label_for(keyword, &self.vocab) {
                Some(true) => labelled.push((*e, Label::Positive)),
                Some(false) => labelled.push((*e, Label::Negative)),
                None => unlabelled.push(*e),
            }
        }
        // An even, deterministic sample of the rest of the catalogue.
        let stride = unlabelled.len().div_ceil(MAX_BACKGROUND).max(1);
        let background = unlabelled.iter().step_by(stride).map(|e| (*e, Label::Background));

        let samples: Vec<Sample<'_>> = labelled
            .iter()
            .copied()
            .chain(background)
            .map(|(e, label)| Sample { embedding: e.as_slice(), zero_shot: self.zero_shot(keyword, e), label })
            .collect();
        let dim = self.dim.unwrap_or(0);
        match learn::train(&samples, dim, &self.settings) {
            Some(a) => self.adapters.insert(keyword, a),
            None => self.adapters.remove(&keyword),
        };
        Ok(())
    }

    /// Re-learns every keyword that has at least one example; returns how
    /// many were learned.
    ///
    /// # Errors
    /// [`TaggerError::Embedding`] for embeddings of another length.
    pub fn learn_all(&mut self, images: &[(&Embedding, &ImageTags)]) -> Result<usize, TaggerError> {
        let ids: Vec<KeywordId> = self.vocab.iter().map(|(id, _)| id).collect();
        let mut learned = 0;
        for id in ids {
            let has_examples = images.iter().any(|(_, t)| t.label_for(id, &self.vocab).is_some());
            if has_examples {
                self.learn(id, images)?;
                learned += 1;
            } else {
                self.adapters.remove(&id);
            }
        }
        Ok(learned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decision;

    /// A fake text model: each known word is an axis of a 6-D space.
    struct AxisText(ModelId);
    impl TextEncoder for AxisText {
        fn model(&self) -> &ModelId {
            &self.0
        }
        fn encode_texts(&mut self, texts: &[&str]) -> Result<Vec<Embedding>, EncodeError> {
            texts
                .iter()
                .map(|t| {
                    let axis = ["cane", "dog", "gatto", "mare", "tramonto", "rossi"]
                        .iter()
                        .position(|w| t.to_lowercase().contains(w));
                    // An unknown word points nowhere in particular.
                    let mut v = vec![0.05; 6];
                    match axis {
                        Some(a) => v[a] = 1.0,
                        None => v.fill(1.0),
                    }
                    Embedding::new(v).map_err(|e| EncodeError::new(e.to_string()))
                })
                .collect()
        }
    }

    fn image(weights: [f32; 6]) -> Embedding {
        Embedding::new(weights.to_vec()).expect("valid")
    }

    fn setup() -> (Tagger, [KeywordId; 4]) {
        let v = Vocabulary::parse_lightroom("Animali\n\tCane\n\t\t{dog}\n\tGatto\nMare\nCliente Rossi\n")
            .expect("valid vocabulary");
        let ids = ["Animali|Cane", "Animali|Gatto", "Mare", "Cliente Rossi"].map(|p| v.find_path(p).expect("exists"));
        let model = ModelId::new("axis").expect("valid");
        // SigLIP-like head: only a close match gets a real probability.
        let mut t = Tagger::new(model.clone(), ZeroShotHead::new(10.0, -7.0).expect("valid"), v);
        t.encode_prompts(&mut AxisText(model), "foto di {}").expect("all words known");
        // "Cliente Rossi" has no visual meaning: learned from examples only.
        t.set_prompts(ids[3], Vec::new()).expect("exists");
        (t, ids)
    }

    #[test]
    fn zero_shot_suggests_what_the_image_shows() {
        let (t, [cane, _, mare, _]) = setup();
        let dog_at_sea = image([0.9, 0.0, 0.0, 0.8, 0.0, 0.0]);
        let s = t.suggest(&dog_at_sea, &ImageTags::new(), &SuggestPolicy::default()).expect("same dim");
        let mut got: Vec<_> = s.iter().map(|s| s.keyword).collect();
        got.sort();
        assert_eq!(got, vec![cane, mare]);
    }

    #[test]
    fn synonyms_are_prompts_too() {
        let (t, [cane, ..]) = setup();
        // An image that matches only the English synonym's axis.
        let p = t.probability(cane, &image([0.0, 1.0, 0.0, 0.0, 0.0, 0.0])).expect("valid");
        assert!(p > 0.9, "{p}");
    }

    #[test]
    fn a_keyword_without_visual_meaning_is_learned_from_examples() {
        let (mut t, [_, _, _, rossi]) = setup();
        // Rossi's photos share a look (axis 5) the text model cannot name.
        let rossi_imgs: Vec<_> = (0..4).map(|i| image([0.1 * i as f32, 0.0, 0.0, 0.1, 0.1, 1.0])).collect();
        let others: Vec<_> = (0..20).map(|i| image([1.0, 0.0, 0.1 * (i % 3) as f32, 0.5, 0.2, 0.0])).collect();
        let mut accepted = ImageTags::new();
        accepted.set(rossi, Decision::Accepted);
        let none = ImageTags::new();
        let mut catalog: Vec<(&Embedding, &ImageTags)> = rossi_imgs.iter().map(|e| (e, &accepted)).collect();
        catalog.extend(others.iter().map(|e| (e, &none)));

        let new_rossi = image([0.0, 0.0, 0.2, 0.0, 0.0, 1.0]);
        let new_other = image([0.9, 0.0, 0.0, 0.4, 0.3, 0.0]);
        assert!(t.probability(rossi, &new_rossi).expect("valid") < 0.01, "unknown before learning");
        assert_eq!(t.learn_all(&catalog).expect("valid"), 1);
        assert!(t.probability(rossi, &new_rossi).expect("valid") > 0.5, "learned");
        assert!(t.probability(rossi, &new_other).expect("valid") < 0.1, "not everywhere");
        assert_eq!(t.adapter(rossi).map(Adapter::positives), Some(4));
    }

    #[test]
    fn rejections_teach_the_tagger_to_stop_proposing_a_wrong_keyword() {
        let (mut t, [_, gatto, ..]) = setup();
        // The user's foxes look like cats to the model.
        let foxes: Vec<_> = (0..5).map(|i| image([0.0, 0.0, 0.9, 0.0, 0.3 + 0.05 * i as f32, 0.0])).collect();
        let cats: Vec<_> = (0..5).map(|i| image([0.0, 0.0, 0.9, 0.1 * i as f32, 0.0, 0.0])).collect();
        let mut rejected = ImageTags::new();
        rejected.set(gatto, Decision::Rejected);
        let mut accepted = ImageTags::new();
        accepted.set(gatto, Decision::Accepted);
        let mut catalog: Vec<(&Embedding, &ImageTags)> = foxes.iter().map(|e| (e, &rejected)).collect();
        catalog.extend(cats.iter().map(|e| (e, &accepted)));

        let new_fox = image([0.0, 0.0, 0.9, 0.0, 0.4, 0.0]);
        assert!(t.probability(gatto, &new_fox).expect("valid") > 0.5, "zero-shot is fooled");
        t.learn(gatto, &catalog).expect("valid");
        assert!(t.probability(gatto, &new_fox).expect("valid") < 0.5, "learned the difference");
        assert!(t.probability(gatto, &image([0.0, 0.0, 1.0, 0.1, 0.0, 0.0])).expect("valid") > 0.5);
    }

    #[test]
    fn mismatches_are_refused() {
        let (mut t, [cane, ..]) = setup();
        assert!(t.probability(cane, &image([1.0; 6])).is_ok());
        let short = Embedding::new(vec![1.0, 0.0]).expect("valid");
        assert!(matches!(t.probability(cane, &short), Err(TaggerError::Embedding(_))));
        let other = ModelId::new("other").expect("valid");
        assert!(matches!(t.encode_prompts(&mut AxisText(other), "{}"), Err(TaggerError::ModelMismatch { .. })));
        assert_eq!(t.prompt_texts("no placeholder"), Err(TaggerError::Template));
        assert_eq!(t.probability(KeywordId::from_raw(99), &image([1.0; 6])), Err(TaggerError::UnknownKeyword));
    }
}
