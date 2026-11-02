//! The catalog file: one SQLite database for every folder the user browses.
//!
//! Photos are keyed by their full path. Writes that touch several photos
//! run in one transaction, so a crash never leaves half of a change.
//!
//! Layout versions: 1 had ratings, labels and keywords; 2 adds the edits
//! (table `variants`, one row per variant of a photo) and the variant shown
//! (column `photos.variant`); 3 adds the photos imported into the catalog
//! (table `imported`), every photo an older file knew about counting as
//! imported; 4 adds each imported photo's file name and capture time
//! (duplicate checks) and the photographer and copyright given at import
//! (table `texts`); 5 adds the Trash (column `imported.trashed`) and the
//! albums (tables `albums` and `album_photos`); 6 adds sub-albums (column
//! `albums.parent`). Older files are upgraded on open.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use crate::import::{ImportMetadata, ImportedPhoto, ShotKey};
use crate::model::{
    Album, Collection, ColorLabel, ImportBatch, MAX_RATING, MergeUndo, PhotoMeta, StoredVariant, Tag, TagSource,
    VariantInfo,
};

/// Layout version of the database; older files are upgraded on open.
const SCHEMA_VERSION: i64 = 6;

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
    batch    INTEGER NOT NULL,
    name     TEXT,
    captured TEXT
);
CREATE INDEX IF NOT EXISTS imported_by_folder ON imported(folder);
CREATE TABLE IF NOT EXISTS albums (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    name    TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS album_photos (
    album INTEGER NOT NULL,
    path  TEXT NOT NULL,
    added INTEGER NOT NULL,
    PRIMARY KEY (album, path)
);
CREATE INDEX IF NOT EXISTS album_photos_by_path ON album_photos(path);
CREATE TABLE IF NOT EXISTS texts (
    path  TEXT NOT NULL,
    field TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (path, field)
);
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
    /// The operation makes no sense (the message says why).
    Refused(String),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "catalog database error: {e}"),
            Self::Io(e) => write!(f, "cannot create the catalog folder: {e}"),
            Self::NewerVersion(v) => {
                write!(f, "the catalog was written by a newer version of the editor (layout {v}); update the editor")
            }
            Self::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for CatalogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::NewerVersion(_) | Self::Refused(_) => None,
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

    /// Writes a complete copy of the catalog to the new file `file` (its
    /// folder must exist), consistent even while the catalog is in use:
    /// SQLite's `VACUUM INTO`.
    ///
    /// # Errors
    /// [`CatalogError::Refused`] when `file` exists; [`CatalogError::Sqlite`]
    /// when it cannot be written (a partial file is removed).
    pub fn save_copy(&self, file: &Path) -> Result<(), CatalogError> {
        if file.exists() {
            return Err(CatalogError::Refused(format!("{} exists already", file.display())));
        }
        let target = file.to_string_lossy().into_owned();
        if let Err(e) = self.conn.execute("VACUUM INTO ?1", params![target]) {
            let _ = std::fs::remove_file(file);
            return Err(e.into());
        }
        Ok(())
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
        // Version 3 had no name and capture time of the imported photos,
        // version 4 no Trash.
        for (column, kind) in [("name", "TEXT"), ("captured", "TEXT"), ("trashed", "INTEGER")] {
            let has: i64 = tx.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('imported') WHERE name = ?1",
                params![column],
                |r| r.get(0),
            )?;
            if has == 0 {
                tx.execute_batch(&format!("ALTER TABLE imported ADD COLUMN {column} {kind};"))?;
            }
        }
        // Version 5 had no sub-albums.
        let has_parent: i64 =
            tx.query_row("SELECT COUNT(*) FROM pragma_table_info('albums') WHERE name = 'parent'", [], |r| r.get(0))?;
        if has_parent == 0 {
            tx.execute_batch("ALTER TABLE albums ADD COLUMN parent INTEGER;")?;
        }
        // The index needs the name column, so it comes after the upgrade.
        tx.execute_batch("CREATE INDEX IF NOT EXISTS imported_by_shot ON imported(name, captured);")?;
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
        let tx = self.conn.transaction()?;
        rename_rows(&tx, from, to)?;
        tx.commit()?;
        Ok(())
    }

    /// Follows folder `from` moved or renamed to `to`: every photo inside
    /// it (and its sub-folders) keeps its data at its new path. Returns how
    /// many photos moved.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn move_folder(&mut self, from: &Path, to: &Path) -> Result<usize, CatalogError> {
        let inside: Vec<PathBuf> = self.paths()?.into_iter().filter(|p| p.starts_with(from)).collect();
        let tx = self.conn.transaction()?;
        for p in &inside {
            let Ok(rest) = p.strip_prefix(from) else { continue };
            rename_rows(&tx, p, &to.join(rest))?;
        }
        tx.commit()?;
        Ok(inside.len())
    }

    /// How many photos of the catalog are in folder `dir` or below.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn count_below(&self, dir: &Path) -> Result<usize, CatalogError> {
        Ok(self.paths()?.iter().filter(|p| p.starts_with(dir)).count())
    }

    /// Removes from the catalog every photo in folder `dir` or below;
    /// returns them.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn remove_below(&mut self, dir: &Path) -> Result<Vec<PathBuf>, CatalogError> {
        let gone: Vec<PathBuf> = self.paths()?.into_iter().filter(|p| p.starts_with(dir)).collect();
        self.remove(&gone)?;
        Ok(gone)
    }

    // ----- imported photos -----

    /// Adds `paths` to the catalog (those already in it are left as they
    /// are), as one import; returns how many were new. Their capture times
    /// are not known: [`Self::add_imported`] keeps them.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn import(&mut self, paths: &[PathBuf]) -> Result<usize, CatalogError> {
        let batch = self.next_import()?;
        let photos: Vec<ImportedPhoto> =
            paths.iter().map(|p| ImportedPhoto { path: p.clone(), captured: None }).collect();
        self.add_imported(batch, &photos)
    }

    /// The number of the next import, for [`Self::add_imported`]: an import
    /// that adds photos over time (copying them) keeps one number.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn next_import(&self) -> Result<i64, CatalogError> {
        Ok(self.conn.query_row("SELECT COALESCE(MAX(batch), 0) + 1 FROM imported", [], |r| r.get(0))?)
    }

    /// Adds `photos` to import `batch` (photos already in the catalog are
    /// left as they are); returns how many were new.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn add_imported(&mut self, batch: i64, photos: &[ImportedPhoto]) -> Result<usize, CatalogError> {
        let tx = self.conn.transaction()?;
        let now = now();
        let mut added = 0;
        for photo in photos {
            let p = &photo.path;
            let folder = p.parent().map(key).unwrap_or_default();
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase());
            added += tx.execute(
                "INSERT OR IGNORE INTO imported (path, folder, imported, batch, name, captured)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key(p), folder, now, batch, name, photo.captured.as_deref().map(str::trim)],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// The shots in the catalog (file name and capture time), to leave out
    /// duplicates from an import.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn known_shots(&self) -> Result<std::collections::HashSet<ShotKey>, CatalogError> {
        let mut stmt = self.conn.prepare("SELECT path, captured FROM imported WHERE captured IS NOT NULL")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter_map(|(p, c)| ShotKey::new(Path::new(&p), Some(&c))).collect())
    }

    /// Gives every photo in `paths` what an import adds: rating, keywords
    /// (the user's own), photographer and copyright.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn apply_metadata(&mut self, paths: &[PathBuf], metadata: &ImportMetadata) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        for p in paths {
            let k = key(p);
            if metadata.rating > 0 {
                tx.execute(
                    "INSERT INTO photos (path, rating) VALUES (?1, ?2)
                     ON CONFLICT(path) DO UPDATE SET rating = excluded.rating",
                    params![k, metadata.rating.min(MAX_RATING)],
                )?;
            }
            for name in metadata.keywords.iter().map(|n| n.trim()).filter(|n| !n.is_empty()) {
                tx.execute(
                    "INSERT INTO tags (path, name, source, confidence) VALUES (?1, ?2, ?3, NULL)
                     ON CONFLICT(path, name) DO UPDATE SET source = excluded.source, confidence = NULL",
                    params![k, name, source_name(TagSource::User)],
                )?;
            }
            for (field, value) in [(TEXT_ARTIST, &metadata.artist), (TEXT_COPYRIGHT, &metadata.copyright)] {
                if let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                    tx.execute(
                        "INSERT INTO texts (path, field, value) VALUES (?1, ?2, ?3)
                         ON CONFLICT(path, field) DO UPDATE SET value = excluded.value",
                        params![k, field, v],
                    )?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The photographer and copyright given to `path` in the catalog.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn credits(&self, path: &Path) -> Result<(Option<String>, Option<String>), CatalogError> {
        let get = |field: &str| -> Result<Option<String>, CatalogError> {
            Ok(self
                .conn
                .query_row("SELECT value FROM texts WHERE path = ?1 AND field = ?2", params![key(path), field], |r| {
                    r.get(0)
                })
                .optional()?)
        };
        Ok((get(TEXT_ARTIST)?, get(TEXT_COPYRIGHT)?))
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

    /// The photos of `collection`, by name (Recent Edits: the latest edit
    /// first). Photos in the Trash are only in [`Collection::Trash`].
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn collection(&self, collection: &Collection) -> Result<Vec<PathBuf>, CatalogError> {
        let rows = |sql: &str, arg: Option<rusqlite::types::Value>| -> Result<Vec<PathBuf>, CatalogError> {
            let mut stmt = self.conn.prepare(sql)?;
            let map = |r: &rusqlite::Row<'_>| r.get::<_, String>(0);
            let paths = match arg {
                Some(a) => stmt.query_map(params![a], map)?.collect::<Result<Vec<_>, _>>()?,
                None => stmt.query_map([], map)?.collect::<Result<Vec<_>, _>>()?,
            };
            Ok(paths.into_iter().map(PathBuf::from).collect())
        };
        use rusqlite::types::Value;
        let live = "trashed IS NULL";
        let mut paths = match collection {
            Collection::All => rows(&format!("SELECT path FROM imported WHERE {live}"), None)?,
            Collection::LastImport => rows(
                &format!("SELECT path FROM imported WHERE {live} AND batch = (SELECT MAX(batch) FROM imported)"),
                None,
            )?,
            Collection::Import(n) => {
                rows(&format!("SELECT path FROM imported WHERE {live} AND batch = ?1"), Some(Value::Integer(*n)))?
            }
            Collection::RecentEdits => {
                let since = now() - crate::model::RECENT_EDIT_DAYS * 86_400;
                // Newest first: not sorted by name.
                return rows(
                    "SELECT v.path FROM variants v JOIN imported i ON i.path = v.path
                     WHERE i.trashed IS NULL GROUP BY v.path HAVING MAX(v.updated) >= ?1
                     ORDER BY MAX(v.updated) DESC",
                    Some(Value::Integer(since)),
                );
            }
            Collection::Trash => rows("SELECT path FROM imported WHERE trashed IS NOT NULL", None)?,
            Collection::Folder(dir) => {
                rows(&format!("SELECT path FROM imported WHERE {live} AND folder = ?1"), Some(Value::Text(key(dir))))?
            }
            Collection::Search(text) => {
                let text = text.trim().to_lowercase();
                let all = rows(&format!("SELECT path FROM imported WHERE {live}"), None)?;
                all.into_iter()
                    .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&text)))
                    .collect()
            }
            Collection::Album(id) => {
                // In the order they were put there: not sorted by name.
                return rows(
                    "SELECT a.path FROM album_photos a JOIN imported i ON i.path = a.path
                     WHERE i.trashed IS NULL AND a.album = ?1 ORDER BY a.added, a.rowid",
                    Some(Value::Integer(*id)),
                );
            }
        };
        crate::model::sort_by_name(&mut paths);
        Ok(paths)
    }

    /// The folders with imported photos and how many each has, by path
    /// (the Trash apart).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn imported_folders(&self) -> Result<Vec<(PathBuf, usize)>, CatalogError> {
        let mut stmt = self
            .conn
            .prepare("SELECT folder, COUNT(*) FROM imported WHERE trashed IS NULL GROUP BY folder ORDER BY folder")?;
        let rows = stmt
            .query_map([], |r| Ok((PathBuf::from(r.get::<_, String>(0)?), r.get::<_, i64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|(p, n)| (p, usize::try_from(n).unwrap_or(0))).collect())
    }

    /// Every photo of the catalog, the Trash included.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn paths(&self) -> Result<Vec<PathBuf>, CatalogError> {
        let mut stmt = self.conn.prepare("SELECT path FROM imported")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(PathBuf::from).collect())
    }

    /// The last `limit` imports that still have photos, the latest first.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn recent_imports(&self, limit: usize) -> Result<Vec<ImportBatch>, CatalogError> {
        // SQLite knows the computer's time zone: the local offset at the
        // time of each import (summer time included).
        let mut stmt = self.conn.prepare(
            "SELECT batch, MAX(imported), COUNT(*),
                    CAST(strftime('%s', MAX(imported), 'unixepoch', 'localtime') AS INTEGER) - MAX(imported)
             FROM imported WHERE trashed IS NULL
             GROUP BY batch ORDER BY batch DESC LIMIT ?1",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<i64>>(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(number, when, count, offset)| ImportBatch {
                number,
                when,
                offset: offset.unwrap_or(0),
                count: usize::try_from(count).unwrap_or(0),
            })
            .collect())
    }

    // ----- Trash -----

    /// Moves `paths` to the Trash: they leave every other collection but
    /// stay in the catalog, with their edits, until the Trash is emptied.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn trash(&mut self, paths: &[PathBuf]) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        let now = now();
        for p in paths {
            tx.execute("UPDATE imported SET trashed = ?2 WHERE path = ?1", params![key(p), now])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Takes `paths` out of the Trash, back where they were.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn restore(&mut self, paths: &[PathBuf]) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        for p in paths {
            tx.execute("UPDATE imported SET trashed = NULL WHERE path = ?1", params![key(p)])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes every photo of the Trash from the catalog (the files are not
    /// touched); returns them.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn empty_trash(&mut self) -> Result<Vec<PathBuf>, CatalogError> {
        let gone = self.collection(&Collection::Trash)?;
        self.remove(&gone)?;
        Ok(gone)
    }

    // ----- albums -----

    /// The albums, by name, with how many photos each holds.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn albums(&self) -> Result<Vec<Album>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT al.id, al.name,
                    (SELECT COUNT(*) FROM album_photos a JOIN imported i ON i.path = a.path
                     WHERE a.album = al.id AND i.trashed IS NULL),
                    al.parent
             FROM albums al ORDER BY al.name COLLATE NOCASE, al.id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<i64>>(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, name, n, parent)| Album { id, name, count: usize::try_from(n).unwrap_or(0), parent })
            .collect())
    }

    /// Makes an album called `name` (trimmed), inside album `parent` when
    /// given; returns its number.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn create_album(&mut self, name: &str, parent: Option<i64>) -> Result<i64, CatalogError> {
        self.conn.execute(
            "INSERT INTO albums (name, created, parent) VALUES (?1, ?2, ?3)",
            params![name.trim(), now(), parent],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Merges album `from` into album `into`: `into` gets its photos and
    /// its sub-albums, `from` is deleted. Returns what is needed to undo it
    /// ([`Self::undo_merge`]).
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then. Merging an album
    /// into itself or into one of its own sub-albums is refused as
    /// [`CatalogError::Refused`].
    pub fn merge_albums(&mut self, from: i64, into: i64) -> Result<MergeUndo, CatalogError> {
        let albums = self.albums()?;
        if from == into || crate::model::album_descendants(&albums, from).contains(&into) {
            return Err(CatalogError::Refused("an album cannot be merged into itself or its sub-albums".to_owned()));
        }
        let album = albums
            .iter()
            .find(|a| a.id == from)
            .cloned()
            .ok_or_else(|| CatalogError::Refused("the album is gone".to_owned()))?;
        let photos = self.album_paths(from)?;
        let had: std::collections::HashSet<PathBuf> = self.album_paths(into)?.into_iter().collect();
        let added: Vec<PathBuf> = photos.iter().filter(|p| !had.contains(*p)).cloned().collect();
        let children: Vec<i64> = albums.iter().filter(|a| a.parent == Some(from)).map(|a| a.id).collect();
        let tx = self.conn.transaction()?;
        let now = now();
        for p in &added {
            tx.execute(
                "INSERT OR IGNORE INTO album_photos (album, path, added) VALUES (?1, ?2, ?3)",
                params![into, key(p), now],
            )?;
        }
        tx.execute("UPDATE albums SET parent = ?2 WHERE parent = ?1", params![from, into])?;
        tx.execute("DELETE FROM album_photos WHERE album = ?1", params![from])?;
        tx.execute("DELETE FROM albums WHERE id = ?1", params![from])?;
        tx.commit()?;
        Ok(MergeUndo { album, photos, into, added, children })
    }

    /// Undoes a merge: the merged album comes back (same number, name and
    /// place) with its photos and sub-albums, and the other album gives
    /// back the photos it got.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn undo_merge(&mut self, undo: &MergeUndo) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        let now = now();
        let a = &undo.album;
        tx.execute(
            "INSERT OR REPLACE INTO albums (id, name, created, parent) VALUES (?1, ?2, ?3, ?4)",
            params![a.id, a.name, now, a.parent],
        )?;
        for p in &undo.photos {
            tx.execute(
                "INSERT OR IGNORE INTO album_photos (album, path, added) VALUES (?1, ?2, ?3)",
                params![a.id, key(p), now],
            )?;
        }
        for p in &undo.added {
            tx.execute("DELETE FROM album_photos WHERE album = ?1 AND path = ?2", params![undo.into, key(p)])?;
        }
        for child in &undo.children {
            tx.execute("UPDATE albums SET parent = ?2 WHERE id = ?1", params![child, a.id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The photos put in album `id` (the Trash included).
    fn album_paths(&self, id: i64) -> Result<Vec<PathBuf>, CatalogError> {
        let mut stmt = self.conn.prepare("SELECT path FROM album_photos WHERE album = ?1 ORDER BY added, rowid")?;
        let rows = stmt.query_map(params![id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(PathBuf::from).collect())
    }

    /// Renames album `id`.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`].
    pub fn rename_album(&mut self, id: i64, name: &str) -> Result<(), CatalogError> {
        self.conn.execute("UPDATE albums SET name = ?2 WHERE id = ?1", params![id, name.trim()])?;
        Ok(())
    }

    /// Deletes album `id`; its photos stay in the catalog and its
    /// sub-albums move up to its place.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn delete_album(&mut self, id: i64) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE albums SET parent = (SELECT parent FROM albums WHERE id = ?1) WHERE parent = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM album_photos WHERE album = ?1", params![id])?;
        tx.execute("DELETE FROM albums WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Puts `paths` in album `id` (those already there stay once); returns
    /// how many were added.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn add_to_album(&mut self, id: i64, paths: &[PathBuf]) -> Result<usize, CatalogError> {
        let tx = self.conn.transaction()?;
        let now = now();
        let mut added = 0;
        for p in paths {
            added += tx.execute(
                "INSERT OR IGNORE INTO album_photos (album, path, added) VALUES (?1, ?2, ?3)",
                params![id, key(p), now],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    /// Takes `paths` out of album `id`; they stay in the catalog.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`]; nothing is changed then.
    pub fn remove_from_album(&mut self, id: i64, paths: &[PathBuf]) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        for p in paths {
            tx.execute("DELETE FROM album_photos WHERE album = ?1 AND path = ?2", params![id, key(p)])?;
        }
        tx.commit()?;
        Ok(())
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
            for table in ["imported", "photos", "tags", "variants", "texts", "album_photos"] {
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

    /// The preview stored for variant `number` of `path`, if any.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn variant_preview(&self, path: &Path, number: u32) -> Result<Option<Vec<u8>>, CatalogError> {
        let preview: Option<Option<Vec<u8>>> = self
            .conn
            .query_row(
                "SELECT preview FROM variants WHERE path = ?1 AND number = ?2",
                params![key(path), number],
                |r| r.get(0),
            )
            .optional()?;
        Ok(preview.flatten())
    }

    /// The variants of every photo that has more than one, by number: the
    /// filmstrip shows each of them as a card of its own.
    ///
    /// # Errors
    /// [`CatalogError::Sqlite`] when the database cannot be read.
    pub fn multi_variants(&self) -> Result<Vec<(PathBuf, Vec<VariantInfo>)>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT path, number, name, format FROM variants
             WHERE path IN (SELECT path FROM variants GROUP BY path HAVING COUNT(*) > 1)
             ORDER BY path, number",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let path: String = r.get(0)?;
                Ok((PathBuf::from(path), VariantInfo { number: r.get(1)?, name: r.get(2)?, format: r.get(3)? }))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out: Vec<(PathBuf, Vec<VariantInfo>)> = Vec::new();
        for (path, info) in rows {
            match out.last_mut() {
                Some((last, list)) if *last == path => list.push(info),
                _ => out.push((path, vec![info])),
            }
        }
        Ok(out)
    }
}

/// `ORDER BY` term putting first the variant chosen for the photo `?1`.
const CHOSEN_FIRST: &str = "number = COALESCE((SELECT variant FROM photos WHERE path = ?1), 1) DESC";

/// Seconds since 1970, for the `updated` column.
/// Moves the rows of photo `from` to `to` in every table (replacing
/// whatever `to` had), inside transaction `tx`.
fn rename_rows(tx: &rusqlite::Transaction<'_>, from: &Path, to: &Path) -> Result<(), CatalogError> {
    let (f, t) = (key(from), key(to));
    if f == t {
        return Ok(());
    }
    for table in ["photos", "tags", "variants", "imported", "texts", "album_photos"] {
        tx.execute(&format!("DELETE FROM {table} WHERE path = ?1"), params![t])?;
        tx.execute(&format!("UPDATE {table} SET path = ?2 WHERE path = ?1"), params![f, t])?;
    }
    let folder = to.parent().map(key).unwrap_or_default();
    tx.execute("UPDATE imported SET folder = ?2 WHERE path = ?1", params![t, folder])?;
    Ok(())
}

/// Fields of table `texts`.
const TEXT_ARTIST: &str = "artist";
const TEXT_COPYRIGHT: &str = "copyright";

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
    fn an_import_keeps_capture_times_for_duplicates_and_gives_its_metadata() {
        let mut c = Catalog::open_in_memory().unwrap();
        let batch = c.next_import().unwrap();
        let shot = ImportedPhoto { path: p("D/2026/IMG_1.CR3"), captured: Some("2026:10:29 10:00:00".into()) };
        let plain = ImportedPhoto { path: p("D/2026/IMG_2.CR3"), captured: None };
        assert_eq!(c.add_imported(batch, &[shot.clone(), plain.clone()]).unwrap(), 2);
        assert_eq!(c.add_imported(batch, std::slice::from_ref(&shot)).unwrap(), 0);
        assert_eq!(c.next_import().unwrap(), batch + 1);
        let known = c.known_shots().unwrap();
        assert!(known.contains(&ShotKey::new(Path::new("card/img_1.cr3"), Some("2026:10:29 10:00:00")).unwrap()));
        assert_eq!(known.len(), 1, "a photo without a capture time is never a duplicate");
        let metadata = ImportMetadata {
            artist: Some("Ada".into()),
            copyright: Some(" ".into()),
            keywords: vec!["sea".into(), "boat".into()],
            rating: 4,
        };
        let paths = [shot.path.clone()];
        c.apply_metadata(&paths, &metadata).unwrap();
        let meta = c.meta(&shot.path).unwrap();
        assert_eq!(meta.rating, 4);
        assert_eq!(meta.tags.len(), 2);
        assert_eq!(c.credits(&shot.path).unwrap(), (Some("Ada".into()), None), "a blank copyright is not kept");
        c.rename(&shot.path, &p("D/2026/IMG_1b.CR3")).unwrap();
        assert_eq!(c.credits(&p("D/2026/IMG_1b.CR3")).unwrap().0.as_deref(), Some("Ada"));
    }

    #[test]
    fn the_trash_hides_photos_until_restored_or_emptied() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b) = (p("d/a.NEF"), p("d/b.NEF"));
        c.import(&[a.clone(), b.clone()]).unwrap();
        c.set_rating(std::slice::from_ref(&a), 5).unwrap();
        c.trash(std::slice::from_ref(&a)).unwrap();
        assert_eq!(c.collection(&Collection::All).unwrap(), std::slice::from_ref(&b));
        assert_eq!(c.collection(&Collection::Folder(p("d"))).unwrap(), std::slice::from_ref(&b));
        assert_eq!(c.collection(&Collection::Trash).unwrap(), std::slice::from_ref(&a));
        assert_eq!(c.imported_folders().unwrap(), [(p("d"), 1)]);
        assert!(c.is_imported(&a).unwrap(), "still in the catalog");
        c.restore(std::slice::from_ref(&a)).unwrap();
        assert_eq!(c.count(&Collection::All).unwrap(), 2);
        assert_eq!(c.meta(&a).unwrap().rating, 5, "nothing lost on the way");
        c.trash(std::slice::from_ref(&a)).unwrap();
        assert_eq!(c.empty_trash().unwrap(), std::slice::from_ref(&a));
        assert!(!c.is_imported(&a).unwrap() && c.collection(&Collection::Trash).unwrap().is_empty());
    }

    #[test]
    fn a_copy_of_the_catalog_has_everything_and_is_separate() {
        let dir = std::env::temp_dir().join(format!("aktis-catalog-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = Catalog::open(&dir.join("a.sqlite")).unwrap();
        let a = p("d/a.NEF");
        c.import(std::slice::from_ref(&a)).unwrap();
        c.set_rating(std::slice::from_ref(&a), 3).unwrap();
        let album = c.create_album("Best", None).unwrap();
        c.add_to_album(album, std::slice::from_ref(&a)).unwrap();
        let copy = dir.join("b.sqlite");
        c.save_copy(&copy).unwrap();
        assert!(matches!(c.save_copy(&copy), Err(CatalogError::Refused(_))), "never over a file");
        let mut b = Catalog::open(&copy).unwrap();
        assert_eq!(b.meta(&a).unwrap().rating, 3);
        assert_eq!(b.collection(&Collection::Album(album)).unwrap(), std::slice::from_ref(&a));
        b.set_rating(std::slice::from_ref(&a), 5).unwrap();
        assert_eq!(c.meta(&a).unwrap().rating, 3, "the copy is a catalog of its own");
        drop((b, c));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn albums_hold_photos_by_hand_and_imports_are_listed() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b, x) = (p("d/a.NEF"), p("d/b.NEF"), p("e/x.NEF"));
        c.import(&[a.clone(), b.clone()]).unwrap();
        c.import(std::slice::from_ref(&x)).unwrap();
        let imports = c.recent_imports(10).unwrap();
        assert_eq!(imports.iter().map(|i| (i.number, i.count)).collect::<Vec<_>>(), [(2, 1), (1, 2)]);
        assert_eq!(c.collection(&Collection::Import(1)).unwrap(), [a.clone(), b.clone()]);
        let best = c.create_album(" Best ", None).unwrap();
        let other = c.create_album("Another", None).unwrap();
        assert_eq!(c.add_to_album(best, &[x.clone(), a.clone()]).unwrap(), 2);
        assert_eq!(c.add_to_album(best, std::slice::from_ref(&a)).unwrap(), 0);
        assert_eq!(c.collection(&Collection::Album(best)).unwrap(), [x.clone(), a.clone()], "in the order added");
        let offset = imports[0].offset;
        assert!(offset.abs() <= 14 * 3600 && offset % 900 == 0, "a time zone offset: {offset}");
        let names: Vec<(String, usize)> = c.albums().unwrap().into_iter().map(|al| (al.name, al.count)).collect();
        assert_eq!(names, [("Another".to_owned(), 0), ("Best".to_owned(), 2)]);
        c.trash(std::slice::from_ref(&x)).unwrap();
        assert_eq!(c.albums().unwrap()[1].count, 1, "the Trash is not counted");
        c.remove_from_album(best, std::slice::from_ref(&a)).unwrap();
        assert!(c.collection(&Collection::Album(best)).unwrap().is_empty());
        c.rename_album(other, "Kept").unwrap();
        c.delete_album(best).unwrap();
        assert_eq!(c.albums().unwrap().len(), 1);
        assert!(c.is_imported(&a).unwrap(), "deleting an album keeps its photos");
    }

    #[test]
    fn sub_albums_merges_undo_and_search() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b, x) = (p("d/IMG_1.NEF"), p("d/IMG_2.NEF"), p("e/other.NEF"));
        c.import(&[a.clone(), b.clone(), x.clone()]).unwrap();
        let trips = c.create_album("Trips", None).unwrap();
        let italy = c.create_album("Italy", Some(trips)).unwrap();
        let como = c.create_album("Como", Some(italy)).unwrap();
        c.add_to_album(italy, &[a.clone(), b.clone()]).unwrap();
        c.add_to_album(trips, std::slice::from_ref(&a)).unwrap();
        assert!(matches!(c.merge_albums(trips, como), Err(CatalogError::Refused(_))), "not into its own sub-album");
        let undo = c.merge_albums(italy, trips).unwrap();
        assert_eq!(undo.added, std::slice::from_ref(&b), "a was in Trips already");
        assert_eq!(c.collection(&Collection::Album(trips)).unwrap(), [a.clone(), b.clone()]);
        let albums = c.albums().unwrap();
        assert!(albums.iter().all(|al| al.id != italy));
        assert_eq!(albums.iter().find(|al| al.id == como).unwrap().parent, Some(trips), "Como moved up");
        c.undo_merge(&undo).unwrap();
        assert_eq!(c.collection(&Collection::Album(trips)).unwrap(), std::slice::from_ref(&a));
        assert_eq!(c.collection(&Collection::Album(italy)).unwrap(), [a.clone(), b.clone()]);
        let albums = c.albums().unwrap();
        assert_eq!(albums.iter().find(|al| al.id == italy).unwrap().parent, Some(trips));
        assert_eq!(albums.iter().find(|al| al.id == como).unwrap().parent, Some(italy));
        // Deleting an album moves its sub-albums up.
        c.delete_album(italy).unwrap();
        assert_eq!(c.albums().unwrap().iter().find(|al| al.id == como).unwrap().parent, Some(trips));
        assert_eq!(c.collection(&Collection::Search("img".into())).unwrap(), [a.clone(), b.clone()]);
        c.trash(std::slice::from_ref(&a)).unwrap();
        assert_eq!(c.collection(&Collection::Search(" IMG_".into())).unwrap(), std::slice::from_ref(&b));
        assert!(!x.as_os_str().is_empty());
    }

    #[test]
    fn a_moved_folder_takes_its_photos_along() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b, other) = (p("d/trip/a.NEF"), p("d/trip/day2/b.NEF"), p("d/tripper/c.NEF"));
        c.import(&[a.clone(), b.clone(), other.clone()]).unwrap();
        c.set_rating(std::slice::from_ref(&b), 4).unwrap();
        let album = c.create_album("Best", None).unwrap();
        c.add_to_album(album, std::slice::from_ref(&b)).unwrap();
        assert_eq!(c.count_below(&p("d/trip")).unwrap(), 2, "a folder with a longer name is not inside");
        assert_eq!(c.move_folder(&p("d/trip"), &p("e/2026 trip")).unwrap(), 2);
        // Built as the catalog does: the new folder and the rest of the old path.
        let moved = p("e/2026 trip").join(b.strip_prefix("d/trip").unwrap());
        assert!(c.is_imported(&moved).unwrap() && !c.is_imported(&b).unwrap());
        assert_eq!(c.meta(&moved).unwrap().rating, 4);
        assert_eq!(c.collection(&Collection::Album(album)).unwrap(), std::slice::from_ref(&moved));
        assert_eq!(
            c.collection(&Collection::Folder(moved.parent().unwrap().to_path_buf())).unwrap(),
            std::slice::from_ref(&moved)
        );
        assert_eq!(c.remove_below(&p("e/2026 trip")).unwrap().len(), 2);
        assert_eq!(c.collection(&Collection::All).unwrap(), std::slice::from_ref(&other));
    }

    #[test]
    fn recent_edits_are_the_photos_saved_lately_latest_first() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b, old) = (p("d/a.NEF"), p("d/b.NEF"), p("d/old.NEF"));
        c.import(&[a.clone(), b.clone(), old.clone()]).unwrap();
        for (path, updated) in [(&a, now() - 100), (&b, now() - 10), (&old, now() - 40 * 86_400)] {
            c.conn
                .execute(
                    "INSERT INTO variants (path, number, name, format, data, preview, updated)
                     VALUES (?1, 1, 'Variant 1', 1, '', NULL, ?2)",
                    params![key(path), updated],
                )
                .unwrap();
        }
        assert_eq!(c.collection(&Collection::RecentEdits).unwrap(), [b, a]);
    }

    #[test]
    fn a_version_3_catalog_gets_the_shot_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE imported (path TEXT PRIMARY KEY NOT NULL, folder TEXT NOT NULL,
               imported INTEGER NOT NULL, batch INTEGER NOT NULL);
             INSERT INTO imported VALUES ('d/a.NEF', 'd', 0, 1);
             PRAGMA user_version = 3;",
        )
        .unwrap();
        let mut c = Catalog::init(conn).unwrap();
        assert!(c.is_imported(&p("d/a.NEF")).unwrap());
        let shot = ImportedPhoto { path: p("d/b.NEF"), captured: Some("2026:01:01 00:00:00".into()) };
        assert_eq!(c.add_imported(2, &[shot]).unwrap(), 1);
        assert_eq!(c.known_shots().unwrap().len(), 1);
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
    fn photos_with_several_variants_are_listed_with_each_preview() {
        let mut c = Catalog::open_in_memory().unwrap();
        let (a, b) = (p("a.NEF"), p("b.NEF"));
        c.save_variant(&a, &info(1), "1", Some(b"jpeg-a1".as_slice())).unwrap();
        c.save_variant(&b, &info(1), "1", None).unwrap();
        assert!(c.multi_variants().unwrap().is_empty(), "one variant each");
        c.save_variant(&a, &info(3), "3", Some(b"jpeg-a3".as_slice())).unwrap();
        c.save_variant(&a, &info(2), "2", None).unwrap();
        assert_eq!(c.multi_variants().unwrap(), vec![(a.clone(), vec![info(1), info(2), info(3)])]);
        assert_eq!(c.variant_preview(&a, 3).unwrap(), Some(b"jpeg-a3".to_vec()));
        assert_eq!(c.variant_preview(&a, 2).unwrap(), None);
        assert_eq!(c.variant_preview(&b, 9).unwrap(), None);
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
        let dir = std::env::temp_dir().join(format!("aktis-catalog-{}", std::process::id()));
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
