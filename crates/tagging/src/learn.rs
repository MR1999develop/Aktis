//! Learning from the user's decisions: a per-keyword correction on top of
//! the zero-shot score.
//!
//! For a keyword `k` and an image embedding `e`, the model's own opinion is
//! the zero-shot logit `z₀(e)`. The [`Adapter`] adds `Δ·e + δ`, a linear
//! correction learned by logistic regression on the user's examples:
//!
//! * positives: images where the user applied `k` (or a keyword below it);
//! * negatives: images where the user rejected `k`;
//! * background: a sample of other images, weakly treated as negatives, so
//!   that a keyword with positives only (a client's name) still learns what
//!   is *not* it instead of becoming likely everywhere.
//!
//! `Δ` and `δ` are pulled towards zero by a penalty that shrinks as examples
//! accumulate: with few examples the zero-shot opinion dominates, with many
//! the user's own usage does. Auto-applied keywords never count as examples,
//! so the system cannot reinforce its own mistakes.

use crate::embedding::dot;

/// Tuning of the logistic regression. The defaults suit unit-length
/// embeddings of a few hundred dimensions and SigLIP-sized logits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LearnSettings {
    /// Penalty on `|Δ|²`, divided by the number of labelled examples.
    pub weight_penalty: f32,
    /// Penalty on `δ²`, divided by the number of labelled examples. Larger
    /// than the weight penalty so corrections are specific to what the
    /// examples look like rather than raising the keyword everywhere.
    pub bias_penalty: f32,
    /// Total weight of the background sample relative to the positives.
    pub background_share: f32,
    /// Gradient steps (accelerated). Fixed, so training time is predictable.
    pub iterations: u32,
}

impl Default for LearnSettings {
    fn default() -> Self {
        Self { weight_penalty: 0.01, bias_penalty: 0.2, background_share: 0.5, iterations: 400 }
    }
}

/// The learned correction for one keyword.
#[derive(Debug, Clone, PartialEq)]
pub struct Adapter {
    delta: Vec<f32>,
    bias: f32,
    positives: usize,
    negatives: usize,
}

impl Adapter {
    /// `Δ·e + δ`, to add to the zero-shot logit.
    #[must_use]
    pub fn correction(&self, embedding: &[f32]) -> f32 {
        dot(&self.delta, embedding) + self.bias
    }

    /// Images the user applied the keyword to.
    #[must_use]
    pub fn positives(&self) -> usize {
        self.positives
    }

    /// Images the user rejected the keyword for.
    #[must_use]
    pub fn negatives(&self) -> usize {
        self.negatives
    }
}

/// One training example: the embedding, the zero-shot logit for the keyword
/// and the label.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sample<'a> {
    pub embedding: &'a [f32],
    pub zero_shot: f32,
    pub label: Label,
}

/// What an example says about the keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Label {
    Positive,
    Negative,
    Background,
}

/// The logistic function, without overflow at large |x|.
pub(crate) fn logistic(x: f32) -> f32 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

/// Fits an adapter. `None` without any positive or negative example (the
/// background alone teaches nothing about the keyword).
pub(crate) fn train(samples: &[Sample<'_>], dim: usize, settings: &LearnSettings) -> Option<Adapter> {
    let count = |l: Label| samples.iter().filter(|s| s.label == l).count();
    let (n_pos, n_neg, n_bg) = (count(Label::Positive), count(Label::Negative), count(Label::Background));
    let labelled = n_pos + n_neg;
    if labelled == 0 {
        return None;
    }

    // Each class gets a fixed total weight, so 500 background images do not
    // drown 3 positives.
    #[allow(clippy::cast_precision_loss)] // counts are far below 2^24
    let per = |total: f32, n: usize| if n == 0 { 0.0 } else { total / n as f32 };
    let w_pos = per(1.0, n_pos);
    let w_neg = per(1.0, n_neg);
    let w_bg = per(settings.background_share.max(0.0), n_bg);
    let weight = |l: Label| match l {
        Label::Positive => w_pos,
        Label::Negative => w_neg,
        Label::Background => w_bg,
    };
    let target = |l: Label| if l == Label::Positive { 1.0 } else { 0.0 };

    #[allow(clippy::cast_precision_loss)]
    let n = labelled as f32;
    let lambda_w = settings.weight_penalty.max(0.0) / n;
    let lambda_b = settings.bias_penalty.max(0.0) / n;
    let total_weight: f32 = samples.iter().map(|s| weight(s.label)).sum();
    // |e|² + 1 = 2 for a unit embedding plus the bias input; σ' ≤ 1/4.
    let lipschitz = 0.5 * total_weight + lambda_w.max(lambda_b);
    let step = 1.0 / lipschitz;

    // Nesterov-accelerated gradient descent on the convex loss.
    let mut x = vec![0.0_f32; dim + 1]; // Δ, then δ
    let mut prev = x.clone();
    let mut grad = vec![0.0_f32; dim + 1];
    let mut look = vec![0.0_f32; dim + 1];
    for it in 0..settings.iterations {
        #[allow(clippy::cast_precision_loss)]
        let momentum = it as f32 / (it as f32 + 3.0);
        for i in 0..=dim {
            look[i] = x[i] + momentum * (x[i] - prev[i]);
        }
        grad.iter_mut().for_each(|g| *g = 0.0);
        for s in samples {
            let z = s.zero_shot + dot(&look[..dim], s.embedding) + look[dim];
            let r = weight(s.label) * (logistic(z) - target(s.label));
            for (g, e) in grad[..dim].iter_mut().zip(s.embedding) {
                *g += r * e;
            }
            grad[dim] += r;
        }
        for i in 0..dim {
            grad[i] += lambda_w * look[i];
        }
        grad[dim] += lambda_b * look[dim];
        prev.copy_from_slice(&x);
        for i in 0..=dim {
            x[i] = look[i] - step * grad[i];
        }
    }

    let bias = x.pop().unwrap_or(0.0);
    Some(Adapter { delta: x, bias, positives: n_pos, negatives: n_neg })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit vector in 4-D mostly along axis `a`, with a little of axis `b`.
    fn emb(a: usize, b: usize, mix: f32) -> Vec<f32> {
        let mut v = [0.0; 4];
        v[a] = 1.0;
        v[b] += mix;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn positives_raise_similar_images_and_not_unrelated_ones() {
        // The zero-shot prior knows nothing (-6 everywhere): a keyword with
        // no visual meaning, learned from three examples near axis 0.
        let pos: Vec<_> = (0..3).map(|i| emb(0, 1 + i % 3, 0.2)).collect();
        let bg: Vec<_> = (0..12).map(|i| emb(1 + i % 3, 0, 0.1)).collect();
        let mut samples: Vec<_> =
            pos.iter().map(|e| Sample { embedding: e, zero_shot: -6.0, label: Label::Positive }).collect();
        samples.extend(bg.iter().map(|e| Sample { embedding: e, zero_shot: -6.0, label: Label::Background }));
        let a = train(&samples, 4, &LearnSettings::default()).expect("has positives");
        assert_eq!((a.positives(), a.negatives()), (3, 0));

        let p = |e: &[f32]| logistic(-6.0 + a.correction(e));
        let similar = emb(0, 3, 0.3);
        let unrelated = emb(2, 1, 0.2);
        assert!(p(&similar) > 0.5, "similar image: {}", p(&similar));
        assert!(p(&unrelated) < 0.2, "unrelated image: {}", p(&unrelated));
    }

    #[test]
    fn a_rejection_lowers_a_confident_zero_shot_guess() {
        // Zero-shot is sure (+4) about everything near axis 1, but the user
        // rejected it twice there; positives live near axis 0.
        let neg = [emb(1, 2, 0.1), emb(1, 3, 0.1)];
        let pos = [emb(0, 2, 0.1), emb(0, 3, 0.1)];
        let mut samples: Vec<_> =
            neg.iter().map(|e| Sample { embedding: e, zero_shot: 4.0, label: Label::Negative }).collect();
        samples.extend(pos.iter().map(|e| Sample { embedding: e, zero_shot: 4.0, label: Label::Positive }));
        let a = train(&samples, 4, &LearnSettings::default()).expect("has labels");
        let p = |e: &[f32]| logistic(4.0 + a.correction(e));
        assert!(p(&emb(1, 0, 0.0)) < 0.5, "rejected look: {}", p(&emb(1, 0, 0.0)));
        assert!(p(&emb(0, 1, 0.0)) > 0.9, "confirmed look: {}", p(&emb(0, 1, 0.0)));
    }

    #[test]
    fn background_alone_teaches_nothing() {
        let e = emb(0, 1, 0.0);
        let samples = [Sample { embedding: &e, zero_shot: 0.0, label: Label::Background }];
        assert!(train(&samples, 4, &LearnSettings::default()).is_none());
    }

    #[test]
    fn sigmoid_is_stable_at_the_extremes() {
        assert!(logistic(-1000.0) >= 0.0 && logistic(-1000.0) < 1e-30);
        assert!((logistic(1000.0) - 1.0).abs() < f32::EPSILON);
        assert!((logistic(0.0) - 0.5).abs() < f32::EPSILON);
    }
}
