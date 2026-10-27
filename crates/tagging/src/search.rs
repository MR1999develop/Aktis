//! Similarity search over embeddings: "more like this" from an image, or a
//! free-text search from the embedding of a sentence ("barca al tramonto").

use crate::Embedding;

/// An item with its similarity to the query.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked<T> {
    /// The item.
    pub item: T,
    /// Cosine similarity to the query, in `[-1, 1]`.
    pub similarity: f32,
}

/// The `k` items most similar to `query`, most similar first. Items whose
/// embedding has another length (another model) are skipped. Ties keep the
/// input order.
pub fn nearest<T: Clone>(query: &Embedding, items: &[(T, &Embedding)], k: usize) -> Vec<Ranked<T>> {
    let mut scored: Vec<(usize, f32)> =
        items.iter().enumerate().filter_map(|(i, (_, e))| query.cosine(e).ok().map(|s| (i, s))).collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(k);
    scored.into_iter().map(|(i, similarity)| Ranked { item: items[i].0.clone(), similarity }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_items_come_first_and_other_models_are_skipped() {
        let e = |v: Vec<f32>| Embedding::new(v).expect("valid");
        let (a, b, c, other) = (e(vec![1.0, 0.0]), e(vec![0.6, 0.8]), e(vec![-1.0, 0.0]), e(vec![1.0, 0.0, 0.0]));
        let items = [("c", &c), ("other", &other), ("b", &b), ("a", &a)];
        let r = nearest(&e(vec![1.0, 0.1]), &items, 2);
        assert_eq!(r.iter().map(|r| r.item).collect::<Vec<_>>(), vec!["a", "b"]);
        assert!(r[0].similarity > r[1].similarity);
        assert_eq!(nearest(&a, &items, 10).len(), 3);
    }
}
