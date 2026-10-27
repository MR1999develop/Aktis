//! Embeddings: unit vectors produced by an image or text model.

use std::fmt;

/// Identifies the model that produced an embedding, including its version
/// (`"siglip2-base-patch16-224"`). Embeddings from different models are
/// never compared.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(String);

impl ModelId {
    /// Wraps a model name. Any non-empty text without control characters.
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        let ok = !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control);
        ok.then(|| Self(name.trim().to_owned()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Why a vector was refused as an embedding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddingError {
    /// No components.
    Empty,
    /// A component is NaN or infinite.
    NotFinite,
    /// All components are zero: there is no direction to keep.
    Zero,
    /// Two embeddings that must be compared have different lengths.
    DimensionMismatch {
        /// Length expected.
        expected: usize,
        /// Length found.
        actual: usize,
    },
}

impl fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "embedding has no components"),
            Self::NotFinite => write!(f, "embedding contains NaN or infinite values"),
            Self::Zero => write!(f, "embedding is the zero vector"),
            Self::DimensionMismatch { expected, actual } => {
                write!(f, "embedding has {actual} components, expected {expected}")
            }
        }
    }
}

impl std::error::Error for EmbeddingError {}

/// A unit-length vector. Exists only if finite and non-zero; the constructor
/// normalises it, so the dot product of two embeddings is their cosine.
#[derive(Clone, PartialEq)]
pub struct Embedding(Vec<f32>);

impl fmt::Debug for Embedding {
    // Hundreds of floats are noise in a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Embedding({} dims)", self.0.len())
    }
}

impl Embedding {
    /// Normalises `v` to unit length.
    ///
    /// # Errors
    /// [`EmbeddingError`] when `v` is empty, not finite or zero.
    pub fn new(mut v: Vec<f32>) -> Result<Self, EmbeddingError> {
        if v.is_empty() {
            return Err(EmbeddingError::Empty);
        }
        if v.iter().any(|x| !x.is_finite()) {
            return Err(EmbeddingError::NotFinite);
        }
        // Accumulate in f64: 1000+ squares in f32 lose digits.
        let norm = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>().sqrt();
        if norm == 0.0 || !norm.is_finite() {
            return Err(EmbeddingError::Zero);
        }
        for x in &mut v {
            #[allow(clippy::cast_possible_truncation)] // a unit vector's components fit in f32
            let n = (f64::from(*x) / norm) as f32;
            *x = n;
        }
        Ok(Self(v))
    }

    /// Number of components.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.0.len()
    }

    /// The components (unit length).
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// Cosine similarity, in `[-1, 1]`.
    ///
    /// # Errors
    /// [`EmbeddingError::DimensionMismatch`] when the lengths differ.
    pub fn cosine(&self, other: &Self) -> Result<f32, EmbeddingError> {
        if self.dim() != other.dim() {
            return Err(EmbeddingError::DimensionMismatch { expected: self.dim(), actual: other.dim() });
        }
        Ok(dot(&self.0, &other.0))
    }

    /// The normalised mean of several embeddings of the same length: the
    /// direction they share. `None` if the list is empty, the lengths differ
    /// or they cancel out.
    #[must_use]
    pub fn mean(items: &[&Self]) -> Option<Self> {
        let first = items.first()?;
        let mut sum = vec![0.0_f32; first.dim()];
        for e in items {
            if e.dim() != sum.len() {
                return None;
            }
            for (s, x) in sum.iter_mut().zip(&e.0) {
                *s += x;
            }
        }
        Self::new(sum).ok()
    }
}

/// Plain dot product of two slices of the same length.
pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn embeddings_are_normalised() {
        let e = Embedding::new(vec![3.0, 4.0]).expect("valid");
        assert!(close(e.as_slice()[0], 0.6) && close(e.as_slice()[1], 0.8));
        assert!(close(e.cosine(&e).expect("same dim"), 1.0));
    }

    #[test]
    fn invalid_vectors_are_refused() {
        assert_eq!(Embedding::new(vec![]), Err(EmbeddingError::Empty));
        assert_eq!(Embedding::new(vec![0.0, 0.0]), Err(EmbeddingError::Zero));
        assert_eq!(Embedding::new(vec![1.0, f32::NAN]), Err(EmbeddingError::NotFinite));
        assert_eq!(Embedding::new(vec![f32::INFINITY]), Err(EmbeddingError::NotFinite));
    }

    #[test]
    fn huge_components_do_not_overflow_the_norm() {
        let e = Embedding::new(vec![f32::MAX, f32::MAX]).expect("finite");
        assert!(close(e.as_slice()[0], std::f32::consts::FRAC_1_SQRT_2));
    }

    #[test]
    fn cosine_refuses_different_lengths() {
        let a = Embedding::new(vec![1.0, 0.0]).expect("valid");
        let b = Embedding::new(vec![1.0, 0.0, 0.0]).expect("valid");
        assert_eq!(a.cosine(&b), Err(EmbeddingError::DimensionMismatch { expected: 2, actual: 3 }));
    }

    #[test]
    fn mean_is_the_shared_direction() {
        let a = Embedding::new(vec![1.0, 0.0]).expect("valid");
        let b = Embedding::new(vec![0.0, 1.0]).expect("valid");
        let m = Embedding::mean(&[&a, &b]).expect("not opposite");
        assert!(close(m.as_slice()[0], m.as_slice()[1]));
        let minus_a = Embedding::new(vec![-1.0, 0.0]).expect("valid");
        assert!(Embedding::mean(&[&a, &minus_a]).is_none());
        assert!(Embedding::mean(&[]).is_none());
    }

    #[test]
    fn model_ids_need_visible_text() {
        assert_eq!(ModelId::new(" siglip2 ").map(|m| m.to_string()), Some("siglip2".to_owned()));
        assert!(ModelId::new("  ").is_none());
        assert!(ModelId::new("a\nb").is_none());
    }
}
