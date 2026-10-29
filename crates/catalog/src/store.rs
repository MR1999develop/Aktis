//! The catalog file: one SQLite database for every folder the user browses.
//!
//! Photos are keyed by their full path. Writes that touch several photos
//! run in one transaction, so a crash never leaves half of a change.
//!
//! Layout versions: 1 had ratings, labels and keywords; 2 adds the edits
//! (table `variants`, one row per variant of a photo) and the variant shown
//! (column `photos.variant`); 3 adds the photos imported into the catalog
//! (table `imported`), every photo an older file knew about counting as
//! imported. Older files are upgraded on open.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{Collection, ColorLabel, MAX_RATING, PhotoMeta, StoredVariant, Tag, TagSource, VariantInfo};

/// Layout version of the database; older files are upgraded on open.
const SCHEMA_VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS photos (
    path    TEXT PRIMARY KEY NOT NULL,
    rating  INTEGER NOT NULL DEFAULT 0,
    label   TEXT,
    variant INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS variants (
    path    TEXT NOT NULL,
    number  INTEGER NOT NULL,
    name    TEXT NOT NULL,
    format  INTEGER NOT NULL,
    data    TEXT NOT NULL,
    preview BLOB,
    updated INTEGER NOT NULL,
    PRIMARY KEY (path, number)
);
CREATE TABLE IF NOT EXISTS tags (
    path       TEXT NOT NULL,
    name       TEXT NOT NULL COLLATE NOCASE,
    source     TEXT NOT NULL,
    confidence REAL,
    PRIMARY KEY (path, name)
);
CREATE INDEX IF NOT EXISTS tags_by_name ON tags(name);
CREATE TABLE IF NOT EXISTS imported (
    path     TEXT PRIMARY KEY NOT NULL,
    folder   TEXT NOT NULL,
    imported INTEGER NOT NULL,
    batch    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS imported_by_folder ON imported(folder);
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

    fn init(mut conn: Connection) -> Result<Self, CatalogError> {
        // Write-ahead logging: readers never wait for a writer. The pragma answers
        // with the mode in use ("memory" for an in-memory database).
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CatalogError::NewerVersion(version));
        }
        // Create and upgrade in one transaction: a crash leaves the old layout whole.
        let tx = conn.transaction()?;
        tx.execute_batch(SCHEMA)?;
        // Version 1 had no variant column (its table existed, so CREATE left it as it was).
        let has_variant: i64 =
            tx.query_row("SELECT COUNT(*) FROM pragma_table_info('photos') WHERE name = 'variant'", [], |r| r.get(0))?;
        if has_variant == 0 {
            tx.execute_batch("ALTER TABLE photos ADD COLUMN variant INTEGER NOT NULL DEFAULT 1;")?;
        }
        // Before version 3 every photo the catalog knew about was browsed, not
        // imported: they all count as imported, in one first batch.
        if version < 3 {
            let mut known = std::collections::BTreeSet::new();
            for table in ["photos", "variants", "tags"] {
                let mut stmt = tx.prepare(&format!("SELECT DISTINCT path FROM {table}"))?;
                let paths = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                known.extend(paths);
            }
            let now = now();
            for p in known {
                let folder = Path::new(&p).parent().map(key).unwrap_or_default();
                tx.execute(
                    "INSERT OR IGNORE INTO imported (path, folder, imported, batch) VALUES (?1, ?2, ?3, 0)",
                    params![p, folder, now],
                )?;
            }
        }
        tx.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        tx.commit()?;
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
        tx.execute("DELETE FROM variants WHERE path = ?1", params![t])?;
        tx.execute("UPDATE photos SET path = ?2 WHERE path = ?1", params![f, t])?;
        tx.execute("UPDATE tags SET path = ?2 WHERE path = ?1", params![f, t])?;
        tx.execute("UPDATE variants SET path = ?2 WHERE path = ?1", params![f, t])?;
        tx.execute("DELETE FROM imported WHERE path = ?1", params![t])?;
        tx.execute("UPDATE imported SET path = ?2 WHERE path = ?1", params![f, t])?;
        tx.commit()?;
        Ok(())
    }

    // ----- imported photos -----

    /// Adds `paths` to the catalog (those already in it are left as they
    /// are), as one import; returns how many were new.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn import(&mut self, paths: &[PathBuf]) -> Result<usize, CatalogError> {
        let tx = self.conn.transaction()?;
        let batch: i64 = tx.query_row("SELECT COALESCE(MAX(batch), 0) + 1 FROM imported", [], |r| r.get(0))?;
        let now = now();
        let mut added = 0;
        for p in paths {
            let folder = p.parent().map(key).unwrap_or_default();
            added += tx.execute(
                "INSERT OR IGNORE INTO imported (path, folder, imported, batch) VALUES (?1, ?2, ?3, ?4)",
                params![key(p), folder, now, batch],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// `true` when `path` was imported into the catalog.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn is_imported(&self, path: &Path) -> Result<bool, CatalogError> {
        let n: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM imported WHERE path = ?1", params![key(path)], |r| r.get(0))?;
        Ok(n > 0)
    }

    /// The photos of `collection`, by path.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn collection(&self, collection: &Collection) -> Result<Vec<PathBuf>, CatalogError> {
        let rows = |sql: &str, arg: Option<String>| -> Result<Vec<PathBuf>, CatalogError> {
            let mut stmt = self.conn.prepare(sql)?;
            let map = |r: &rusqlite::Row<'_>| r.get::<_, String>(0);
            let paths = match arg {
                Some(a) => stmt.query_map(params![a], map)?.collect::<Result<Vec<_>, _>>()?,
                None => stmt.query_map([], map)?.collect::<Result<Vec<_>, _>>()?,
            };
            Ok(paths.into_iter().map(PathBuf::from).collect())
        };
        let mut paths = match collection {
            Collection::All => rows("SELECT path FROM imported", None)?,
            Collection::LastImport => {
                rows("SELECT path FROM imported WHERE batch = (SELECT MAX(batch) FROM imported)", None)?
            }
            Collection::Folder(dir) => rows("SELECT path FROM imported WHERE folder = ?1", Some(key(dir)))?,
        };
        crate::model::sort_by_name(&mut paths);
        Ok(paths)
    }

    /// The folders with imported photos and how many each has, by path.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn imported_folders(&self) -> Result<Vec<(PathBuf, usize)>, CatalogError> {
        let mut stmt = self.conn.prepare("SELECT folder, COUNT(*) FROM imported GROUP BY folder ORDER BY folder")?;
        let rows = stmt
            .query_map([], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|(p, n)| (p, usize::try_from(n).unwrap_or(0))).collect())
    }

    /// How many photos `collection` holds.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn count(&self, collection: &Collection) -> Result<usize, CatalogError> {
        self.collection(collection).map(|p| p.len())
    }

    /// Removes `paths` from the catalog: their ratings, keywords and edits
    /// go too; the files are not touched.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn remove(&mut self, paths: &[PathBuf]) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        for p in paths {
            let k = key(p);
            for table in ["imported", "photos", "tags", "variants"] {
                tx.execute(&format!("DELETE FROM {table} WHERE path = ?1"), params![k])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ----- edits -----

    /// The variants stored for `path`, by number; empty for a photo never edited.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn variants(&self, path: &Path) -> Result<Vec<VariantInfo>, CatalogError> {
        let mut stmt =
            self.conn.prepare("SELECT number, name, format FROM variants WHERE path = ?1 ORDER BY number")?;
        let rows = stmt
            .query_map(params![key(path)], |r| {
                Ok(VariantInfo { number: r.get(0)?, name: r.get(1)?, format: r.get(2)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The variant shown for `path`: the one chosen last when it still
    /// exists, else the lowest; `None` for a photo never edited.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn current_variant(&self, path: &Path) -> Result<Option<u32>, CatalogError> {
        let sql = format!("SELECT number FROM variants WHERE path = ?1 ORDER BY {CHOSEN_FIRST}, number LIMIT 1");
        let number = self.conn.query_row(&sql, params![key(path)], |r| r.get(0)).optional()?;
        Ok(number)
    }

    /// Remembers which variant of `path` is shown.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn set_current_variant(&mut self, path: &Path, number: u32) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO photos (path, variant) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET variant = excluded.variant",
            params![key(path), number],
        )?;
        Ok(())
    }

    /// The stored edit of one variant, if there is one.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn load_variant(&self, path: &Path, number: u32) -> Result<Option<StoredVariant>, CatalogError> {
        let row = self
            .conn
            .query_row(
                "SELECT name, format, data FROM variants WHERE path = ?1 AND number = ?2",
                params![key(path), number],
                |r| {
                    let info = VariantInfo { number, name: r.get(0)?, format: r.get(1)? };
                    Ok(StoredVariant { info, data: r.get(2)? })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Stores (creates or replaces) the edit of variant `info.number` of
    /// `path`. `preview`, a small JPEG of the edited image, replaces the
    /// stored one when given; `None` keeps it.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn save_variant(
        &mut self,
        path: &Path,
        info: &VariantInfo,
        data: &str,
        preview: Option<&[u8]>,
    ) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO variants (path, number, name, format, data, preview, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path, number) DO UPDATE SET
                 name = excluded.name,
                 format = excluded.format,
                 data = excluded.data,
                 preview = COALESCE(excluded.preview, variants.preview),
                 updated = excluded.updated",
            params![key(path), info.number, info.name, info.format, data, preview, now()],
        )?;
        Ok(())
    }

    /// Replaces the preview of a stored variant (nothing happens when the
    /// variant is not stored).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn set_variant_preview(&mut self, path: &Path, number: u32, jpeg: &[u8]) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE variants SET preview = ?3 WHERE path = ?1 AND number = ?2",
            params![key(path), number, jpeg],
        )?;
        Ok(())
    }

    /// Forgets the preview of a stored variant (its edit changed without
    /// being shown): the camera's preview is shown until a new one is stored.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn clear_variant_preview(&mut self, path: &Path, number: u32) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE variants SET preview = NULL WHERE path = ?1 AND number = ?2",
            params![key(path), number],
        )?;
        Ok(())
    }

    /// Deletes one variant of `path`.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn delete_variant(&mut self, path: &Path, number: u32) -> Result<(), CatalogError> {
        self.conn.execute("DELETE FROM variants WHERE path = ?1 AND number = ?2", params![key(path), number])?;
        Ok(())
    }

    /// The preview of the shown variant of each photo, in the same order;
    /// `None` for a photo never edited (or saved without a preview).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn previews(&self, paths: &[PathBuf]) -> Result<Vec<Option<Vec<u8>>>, CatalogError> {
        let sql = format!("SELECT preview FROM variants WHERE path = ?1 ORDER BY {CHOSEN_FIRST}, number LIMIT 1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            let preview: Option<Option<Vec<u8>>> = stmt.query_row(params![key(p)], |r| r.get(0)).optional()?;
            out.push(preview.flatten());
        }
        Ok(out)
    }
}

/// `ORDER BY` term putting first the variant chosen for the photo `?1`.
const CHOSEN_FIRST: &str = "number = COALESCE((SELECT variant FROM photos WHERE path = ?1), 1) DESC";

/// Seconds since 1970, for the `updated` column.
fn now() -> i64 {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    i64::try_from(secs).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn photos_are_imported_in_batches_listed_by_collection_and_removed() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b, x) = (p("trip/b.NEF"), p("trip/a.NEF"), p("home/x.NEF"));
        assert_eq!(c.import(&[a.clone(), b.clone()]).unwrap(), 2);
        assert_eq!(c.import(&[a.clone(), x.clone()]).unwrap(), 1, "a was already in");
        assert!(c.is_imported(&a).unwrap() && !c.is_imported(&p("elsewhere.NEF")).unwrap());
        assert_eq!(c.collection(&Collection::All).unwrap(), vec![b.clone(), a.clone(), x.clone()], "by name");
        assert_eq!(c.collection(&Collection::LastImport).unwrap(), vec![x.clone()], "only the new one");
        assert_eq!(c.collection(&Collection::Folder(p("trip"))).unwrap(), vec![b.clone(), a.clone()]);
        assert_eq!(c.imported_folders().unwrap(), vec![(p("home"), 1), (p("trip"), 2)]);
        c.set_rating(std::slice::from_ref(&x), 3).unwrap();
        c.remove(std::slice::from_ref(&x)).unwrap();
        assert!(!c.is_imported(&x).unwrap());
        assert_eq!(c.meta(&x).unwrap().rating, 0, "its data went with it");
        c.rename(&a, &p("trip/c.NEF")).unwrap();
        assert!(c.is_imported(&p("trip/c.NEF")).unwrap() && !c.is_imported(&a).unwrap());
    }

    #[test]
    fn an_older_catalog_counts_every_known_photo_as_imported() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE photos (path TEXT PRIMARY KEY NOT NULL, rating INTEGER NOT NULL DEFAULT 0, label TEXT,
               variant INTEGER NOT NULL DEFAULT 1);
             CREATE TABLE tags (path TEXT NOT NULL, name TEXT NOT NULL COLLATE NOCASE, source TEXT NOT NULL,
               confidence REAL, PRIMARY KEY (path, name));
             INSERT INTO photos (path, rating) VALUES ('d/rated.NEF', 4);
             INSERT INTO tags (path, name, source) VALUES ('d/tagged.NEF', 'sea', 'user');
             PRAGMA user_version = 2;",
        )
        .unwrap();
        let c = Catalog::init(conn).unwrap();
        assert_eq!(c.collection(&Collection::All).unwrap(), vec![p("d/rated.NEF"), p("d/tagged.NEF")]);
        assert_eq!(c.meta(&p("d/rated.NEF")).unwrap().rating, 4);
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

    fn info(number: u32) -> VariantInfo {
        VariantInfo { number, name: crate::model::variant_name(number), format: 1 }
    }

    #[test]
    fn variants_are_stored_listed_and_deleted() {
        let mut c = Catalog::open_in_memory().unwrap();
        let a = p("a.NEF");
        assert!(c.variants(&a).unwrap().is_empty());
        assert_eq!(c.current_variant(&a).unwrap(), None);
        c.save_variant(&a, &info(1), "{\"one\":1}", Some(b"jpeg-1".as_slice())).unwrap();
        c.save_variant(&a, &info(2), "{\"two\":2}", None).unwrap();
        assert_eq!(c.variants(&a).unwrap(), vec![info(1), info(2)]);
        assert_eq!(c.current_variant(&a).unwrap(), Some(1), "the first when none was chosen");
        let stored = c.load_variant(&a, 2).unwrap().unwrap();
        assert_eq!((stored.info, stored.data.as_str()), (info(2), "{\"two\":2}"));
        assert_eq!(c.load_variant(&a, 3).unwrap(), None);

        c.set_current_variant(&a, 2).unwrap();
        assert_eq!(c.current_variant(&a).unwrap(), Some(2));
        assert_eq!(c.previews(&[a.clone(), p("b.NEF")]).unwrap(), vec![None, None], "variant 2 has no preview");
        c.set_variant_preview(&a, 2, b"jpeg-2").unwrap();
        assert_eq!(c.previews(std::slice::from_ref(&a)).unwrap(), vec![Some(b"jpeg-2".to_vec())]);

        // Saving again without a preview keeps it; with one replaces it.
        c.save_variant(&a, &info(2), "{\"two\":3}", None).unwrap();
        assert_eq!(c.previews(std::slice::from_ref(&a)).unwrap(), vec![Some(b"jpeg-2".to_vec())]);
        assert_eq!(c.load_variant(&a, 2).unwrap().unwrap().data, "{\"two\":3}");

        c.delete_variant(&a, 2).unwrap();
        assert_eq!(c.current_variant(&a).unwrap(), Some(1), "the chosen one is gone: the lowest");
        assert_eq!(c.previews(std::slice::from_ref(&a)).unwrap(), vec![Some(b"jpeg-1".to_vec())]);
        assert_eq!(c.meta(&a).unwrap(), PhotoMeta::default(), "choosing a variant leaves rating and label alone");
    }

    #[test]
    fn renaming_moves_the_edits_too() {
        let mut c = Catalog::open_in_memory().unwrap();
        c.save_variant(&p("old.NEF"), &info(1), "old", None).unwrap();
        c.save_variant(&p("new.NEF"), &info(4), "stale", None).unwrap();
        c.rename(&p("old.NEF"), &p("new.NEF")).unwrap();
        assert!(c.variants(&p("old.NEF")).unwrap().is_empty());
        assert_eq!(c.variants(&p("new.NEF")).unwrap(), vec![info(1)]);
    }

    #[test]
    fn a_version_1_catalog_is_upgraded_and_keeps_its_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE photos (path TEXT PRIMARY KEY NOT NULL, rating INTEGER NOT NULL DEFAULT 0, label TEXT);
             CREATE TABLE tags (path TEXT NOT NULL, name TEXT NOT NULL COLLATE NOCASE, source TEXT NOT NULL,
                                confidence REAL, PRIMARY KEY (path, name));
             INSERT INTO photos (path, rating, label) VALUES ('a.NEF', 4, 'Green');
             PRAGMA user_version = 1;",
        )
        .unwrap();
        let mut c = Catalog::init(conn).unwrap();
        let m = c.meta(&p("a.NEF")).unwrap();
        assert_eq!((m.rating, m.label), (4, Some(ColorLabel::Green)));
        c.save_variant(&p("a.NEF"), &info(1), "x", None).unwrap();
        c.set_current_variant(&p("a.NEF"), 1).unwrap();
        assert_eq!(c.current_variant(&p("a.NEF")).unwrap(), Some(1));
        let version: i64 = c.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn a_catalog_from_a_newer_editor_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 99;").unwrap();
        assert!(matches!(Catalog::init(conn), Err(CatalogError::NewerVersion(99))));
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
