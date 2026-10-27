//! The catalog file: one SQLite database for every folder the user browses.
//!
//! Photos are keyed by their full path. Writes that touch several photos
//! run in one transaction, so a crash never leaves half of a change.

use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{ColorLabel, MAX_RATING, PhotoMeta, Tag, TagSource};

/// Layout version of the database; older files are upgraded on open.
const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS photos (
    path   TEXT PRIMARY KEY NOT NULL,
    rating INTEGER NOT NULL DEFAULT 0,
    label  TEXT
);
CREATE TABLE IF NOT EXISTS tags (
    path       TEXT NOT NULL,
    name       TEXT NOT NULL COLLATE NOCASE,
    source     TEXT NOT NULL,
    confidence REAL,
    PRIMARY KEY (path, name)
);
CREATE INDEX IF NOT EXISTS tags_by_name ON tags(name);
";

/// Why the catalog could not be read or written.
#[derive(Debug)]
pub enum CatalogError {
    /// The database refused the operation.
    Sqlite(rusqlite::Error),
    /// The catalog's folder could not be created.
    Io(std::io::Error),
    /// The file was written by a newer version of the editor.
    NewerVersion(i64),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "catalog database error: {e}"),
            Self::Io(e) => write!(f, "cannot create the catalog folder: {e}"),
            Self::NewerVersion(v) => {
                write!(f, "the catalog was written by a newer version of the editor (layout {v}); update the editor")
            }
        }
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::NewerVersion(_) => None,
        }
    }
}

impl From<rusqlite::Error> for CatalogError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

/// The catalog database.
pub struct Catalog {
    conn: Connection,
}

impl fmt::Debug for Catalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Catalog").field("path", &self.conn.path()).finish()
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn source_name(source: TagSource) -> &'static str {
    match source {
        TagSource::Ai => "ai",
        TagSource::User => "user",
    }
}

impl Catalog {
    /// Opens (or creates) the catalog at `path`, creating its folder.
    ///
    /// # Errors
    /// [`CatalogError`] when the file cannot be opened or is from a newer editor.
    pub fn open(path: &Path) -> Result<Self, CatalogError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(CatalogError::Io)?;
        }
        Self::init(Connection::open(path)?)
    }

    /// A catalog that lives in memory only (tests, or when the file cannot be opened).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] only if SQLite cannot start.
    pub fn open_in_memory() -> Result<Self, CatalogError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, CatalogError> {
        // Write-ahead logging: readers never wait for a writer. The pragma answers
        // with the mode in use ("memory" for an in-memory database).
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CatalogError::NewerVersion(version));
        }
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        Ok(Self { conn })
    }

    /// What the catalog knows about `path` (defaults for an unknown photo).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn meta(&self, path: &Path) -> Result<PhotoMeta, CatalogError> {
        let k = key(path);
        let row: Option<(i64, Option<String>)> = self
            .conn
            .query_row("SELECT rating, label FROM photos WHERE path = ?1", params![k], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let (rating, label) = row.unwrap_or((0, None));
        let mut stmt = self
            .conn
            .prepare("SELECT name, source, confidence FROM tags WHERE path = ?1 ORDER BY source DESC, name")?;
        let tags = stmt
            .query_map(params![k], |r| {
                let source: String = r.get(1)?;
                let confidence: Option<f64> = r.get(2)?;
                #[allow(clippy::cast_possible_truncation)] // probabilities between 0 and 1
                let confidence = confidence.map(|c| c as f32);
                Ok(Tag {
                    name: r.get(0)?,
                    source: if source == "user" { TagSource::User } else { TagSource::Ai },
                    confidence,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PhotoMeta {
            rating: u8::try_from(rating.clamp(0, i64::from(MAX_RATING))).unwrap_or(0),
            label: label.as_deref().and_then(ColorLabel::from_name),
            tags,
        })
    }

    /// [`Self::meta`] for several photos, in the same order.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn metas(&self, paths: &[PathBuf]) -> Result<Vec<PhotoMeta>, CatalogError> {
        paths.iter().map(|p| self.meta(p)).collect()
    }

    /// Sets the rating (0–5, higher values are capped) of every photo in `paths`.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn set_rating(&mut self, paths: &[PathBuf], rating: u8) -> Result<(), CatalogError> {
        let rating = rating.min(MAX_RATING);
        let tx = self.conn.transaction()?;
        for p in paths {
            tx.execute(
                "INSERT INTO photos (path, rating) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET rating = excluded.rating",
                params![key(p), rating],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets (or with `None` removes) the colour label of every photo in `paths`.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn set_label(&mut self, paths: &[PathBuf], label: Option<ColorLabel>) -> Result<(), CatalogError> {
        let name = label.map(ColorLabel::name);
        let tx = self.conn.transaction()?;
        for p in paths {
            tx.execute(
                "INSERT INTO photos (path, label) VALUES (?1, ?2)
                 ON CONFLICT(path) DO UPDATE SET label = excluded.label",
                params![key(p), name],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Replaces the AI keywords of `path` with `tags` (name, probability).
    /// The user's own keywords are kept, and win over an AI keyword of the same name.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn set_ai_tags(&mut self, path: &Path, tags: &[(String, f32)]) -> Result<(), CatalogError> {
        let k = key(path);
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM tags WHERE path = ?1 AND source = 'ai'", params![k])?;
        for (name, confidence) in tags {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO tags (path, name, source, confidence) VALUES (?1, ?2, 'ai', ?3)",
                params![k, name, f64::from(*confidence)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Adds keyword `name` as the user's own (an AI keyword of that name becomes the user's).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn add_user_tag(&mut self, path: &Path, name: &str) -> Result<(), CatalogError> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "INSERT INTO tags (path, name, source, confidence) VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(path, name) DO UPDATE SET source = excluded.source, confidence = NULL",
            params![key(path), name, source_name(TagSource::User)],
        )?;
        Ok(())
    }

    /// Removes keyword `name` (any case) from `path`.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn remove_tag(&mut self, path: &Path, name: &str) -> Result<(), CatalogError> {
        self.conn.execute("DELETE FROM tags WHERE path = ?1 AND name = ?2", params![key(path), name.trim()])?;
        Ok(())
    }

    /// Moves everything known about `from` to `to` (the file was renamed).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn rename(&mut self, from: &Path, to: &Path) -> Result<(), CatalogError> {
        let (f, t) = (key(from), key(to));
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM photos WHERE path = ?1", params![t])?;
        tx.execute("DELETE FROM tags WHERE path = ?1", params![t])?;
        tx.execute("UPDATE photos SET path = ?2 WHERE path = ?1", params![f, t])?;
        tx.execute("UPDATE tags SET path = ?2 WHERE path = ?1", params![f, t])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn an_unknown_photo_has_no_rating_label_or_keywords() {
        let c = Catalog::open_in_memory().unwrap();
        assert_eq!(c.meta(&p("a.NEF")).unwrap(), PhotoMeta::default());
    }

    #[test]
    fn ratings_and_labels_are_stored_for_several_photos_at_once() {
        let mut c = Catalog::open_in_memory().unwrap();
        c.set_rating(&[p("a.NEF"), p("b.NEF")], 4).unwrap();
        c.set_label(&[p("b.NEF")], Some(ColorLabel::Green)).unwrap();
        c.set_rating(&[p("c.NEF")], 9).unwrap();
        let m = c.metas(&[p("a.NEF"), p("b.NEF"), p("c.NEF")]).unwrap();
        assert_eq!((m[0].rating, m[0].label), (4, None));
        assert_eq!((m[1].rating, m[1].label), (4, Some(ColorLabel::Green)));
        assert_eq!(m[2].rating, 5, "capped at five stars");
        c.set_label(&[p("b.NEF")], None).unwrap();
        assert_eq!(c.meta(&p("b.NEF")).unwrap().label, None);
        assert_eq!(c.meta(&p("b.NEF")).unwrap().rating, 4, "the label change keeps the rating");
    }

    #[test]
    fn ai_keywords_are_replaced_but_the_users_are_kept() {
        let mut c = Catalog::open_in_memory().unwrap();
        let a = p("a.NEF");
        c.add_user_tag(&a, "Holiday").unwrap();
        c.set_ai_tags(&a, &[("lake".into(), 0.4), ("holiday".into(), 0.9)]).unwrap();
        let m = c.meta(&a).unwrap();
        assert_eq!(m.tags.len(), 2);
        assert_eq!(m.tags[0].source, TagSource::User, "user keywords first");
        assert_eq!(m.tags[0].name, "Holiday");
        assert!((m.tags[1].confidence.unwrap() - 0.4).abs() < 1e-6);

        c.set_ai_tags(&a, &[("sky".into(), 0.2)]).unwrap();
        let names: Vec<String> = c.meta(&a).unwrap().tags.into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["Holiday".to_owned(), "sky".to_owned()]);

        c.remove_tag(&a, "HOLIDAY").unwrap();
        assert_eq!(c.meta(&a).unwrap().tags.len(), 1);
    }

    #[test]
    fn renaming_moves_everything_and_replaces_stale_data_of_the_new_name() {
        let mut c = Catalog::open_in_memory().unwrap();
        c.set_rating(&[p("old.NEF")], 3).unwrap();
        c.add_user_tag(&p("old.NEF"), "pier").unwrap();
        c.set_rating(&[p("new.NEF")], 1).unwrap();
        c.rename(&p("old.NEF"), &p("new.NEF")).unwrap();
        assert_eq!(c.meta(&p("old.NEF")).unwrap(), PhotoMeta::default());
        let m = c.meta(&p("new.NEF")).unwrap();
        assert_eq!(m.rating, 3);
        assert!(m.has_tag("pier"));
    }

    #[test]
    fn the_catalog_survives_closing_and_reopening() {
        let dir = std::env::temp_dir().join(format!("rne-catalog-{}", std::process::id()));
        let file = dir.join("sub").join("catalog.sqlite");
        {
            let mut c = Catalog::open(&file).unwrap();
            c.set_rating(&[p("x.NEF")], 2).unwrap();
        }
        let c = Catalog::open(&file).unwrap();
        assert_eq!(c.meta(&p("x.NEF")).unwrap().rating, 2);
        drop(c);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
