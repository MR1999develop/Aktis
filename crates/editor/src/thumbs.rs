//! The filmstrip's thumbnails: which to ask the worker for, and which to
//! keep once made.
//!
//! The cache is generic over what it keeps (the window keeps GPU textures),
//! holds at most a fixed number of them and forgets the ones looked at
//! least recently first. Failures are remembered too, so a broken file is
//! not read again at every frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A thumbnail that was made, or why it could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbState<V> {
    /// Ready to draw.
    Ready(V),
    /// The file has no usable preview; the message says why.
    Failed(String),
}

#[derive(Debug, Clone)]
struct Entry<V> {
    state: ThumbState<V>,
    used: u64,
}

/// Thumbnails by file, least recently used forgotten first.
#[derive(Debug, Clone)]
pub struct ThumbCache<V> {
    entries: HashMap<PathBuf, Entry<V>>,
    clock: u64,
    capacity: usize,
    /// What was last asked of the worker, to ask again only when it changes.
    asked: Vec<PathBuf>,
}

impl<V> ThumbCache<V> {
    /// An empty cache keeping at most `capacity` thumbnails (at least one).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { entries: HashMap::new(), clock: 0, capacity: capacity.max(1), asked: Vec::new() }
    }

    /// Number of thumbnails (and failures) kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The thumbnail of `path`, marking it as just used.
    pub fn get(&mut self, path: &Path) -> Option<&ThumbState<V>> {
        self.clock += 1;
        let clock = self.clock;
        self.entries.get_mut(path).map(|e| {
            e.used = clock;
            &e.state
        })
    }

    /// Stores what the worker made for `path`, forgetting the least recently
    /// used thumbnails beyond the capacity.
    pub fn insert(&mut self, path: PathBuf, state: ThumbState<V>) {
        self.clock += 1;
        self.asked.retain(|p| *p != path);
        self.entries.insert(path, Entry { state, used: self.clock });
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.used).map(|(p, _)| p.clone()) else { break };
            self.entries.remove(&oldest);
        }
    }

    /// The photos of `wanted` (in order) that have no thumbnail yet, when
    /// they differ from what was asked last time; `None` when the worker
    /// already has the right list.
    pub fn to_request(&mut self, wanted: &[PathBuf]) -> Option<Vec<PathBuf>> {
        let missing: Vec<PathBuf> = wanted.iter().filter(|p| !self.entries.contains_key(*p)).cloned().collect();
        if missing == self.asked {
            return None;
        }
        self.asked.clone_from(&missing);
        Some(missing)
    }

    /// Follows a renamed file.
    pub fn rename(&mut self, from: &Path, to: PathBuf) {
        if let Some(e) = self.entries.remove(from) {
            self.entries.insert(to, e);
        }
    }

    /// Forgets one file (it changed on disk).
    pub fn forget(&mut self, path: &Path) {
        self.entries.remove(path);
        self.asked.retain(|p| p != path);
    }

    /// Forgets everything (another folder is shown).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.asked.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/f/{name}"))
    }

    #[test]
    fn the_least_recently_used_thumbnail_goes_first() {
        let mut c = ThumbCache::new(2);
        c.insert(p("a"), ThumbState::Ready(1));
        c.insert(p("b"), ThumbState::Ready(2));
        assert_eq!(c.get(&p("a")), Some(&ThumbState::Ready(1)));
        c.insert(p("c"), ThumbState::Failed("no preview".into()));
        assert_eq!(c.len(), 2);
        assert!(c.get(&p("b")).is_none(), "b was used least recently");
        assert!(c.get(&p("a")).is_some());
        assert!(c.get(&p("c")).is_some());
    }

    #[test]
    fn the_worker_is_asked_only_for_missing_thumbnails_and_only_on_change() {
        let mut c: ThumbCache<u8> = ThumbCache::new(10);
        c.insert(p("a"), ThumbState::Ready(1));
        let wanted = vec![p("a"), p("b"), p("c")];
        assert_eq!(c.to_request(&wanted), Some(vec![p("b"), p("c")]));
        assert_eq!(c.to_request(&wanted), None, "same list: nothing new to ask");
        c.insert(p("b"), ThumbState::Ready(2));
        assert_eq!(c.to_request(&wanted), None, "b arrived; the worker still has c");
        assert_eq!(c.to_request(&[p("a")]), Some(vec![]), "scrolled away: the queue is emptied");
        assert_eq!(c.to_request(&[p("a")]), None);
    }

    #[test]
    fn renames_and_clears_follow_the_files() {
        let mut c = ThumbCache::new(4);
        c.insert(p("a"), ThumbState::Ready(1));
        c.rename(&p("a"), p("z"));
        assert!(c.get(&p("a")).is_none());
        assert_eq!(c.get(&p("z")), Some(&ThumbState::Ready(1)));
        c.forget(&p("z"));
        assert!(c.is_empty());
        c.insert(p("b"), ThumbState::Ready(2));
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.to_request(&[p("b")]), Some(vec![p("b")]));
    }
}
