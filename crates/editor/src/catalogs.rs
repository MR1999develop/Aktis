//! Catalogs on disk: each catalog is a folder ("Travel.rnecatalog") holding
//! its database, so it can be moved or copied whole, as Capture One's
//! catalogs. Photos are imported into a catalog by reference (the RAW files
//! stay where they are); this module finds catalogs, makes new ones, brings
//! over the single catalog of earlier versions and lists the photos an
//! import adds.

use std::io;
use std::path::{Path, PathBuf};

use catalog::{CATALOG_EXTENSION, FILE_NAME};
use engine::export;

/// Name of the catalog made the first time (and from an earlier version's data).
pub const DEFAULT_NAME: &str = "Default";

/// The folder new catalogs go in when the user has not chosen one.
#[must_use]
pub fn default_folder() -> Option<PathBuf> {
    platform::editor_data_dir().map(|d| d.join("Catalogs"))
}

/// The database of the catalog in folder `catalog`.
#[must_use]
pub fn database(catalog: &Path) -> PathBuf {
    catalog.join(FILE_NAME)
}

/// The catalog's name, for titles ("Travel" for "…/Travel.rnecatalog").
#[must_use]
pub fn name(catalog: &Path) -> String {
    catalog.file_stem().map_or_else(|| catalog.display().to_string(), |s| s.to_string_lossy().into_owned())
}

/// `true` when `path` is a catalog: a folder named `*.rnecatalog`, or a
/// folder that holds a catalog database.
#[must_use]
pub fn is_catalog(path: &Path) -> bool {
    path.is_dir()
        && (path.extension().is_some_and(|e| e.eq_ignore_ascii_case(CATALOG_EXTENSION)) || database(path).is_file())
}

/// The folder of a new catalog called `name` in `folder`.
///
/// # Errors
/// A message when the name is empty or a catalog of that name is already there.
pub fn new_catalog_path(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let stem = export::sanitize_stem(name.trim());
    if stem.is_empty() {
        return Err("a catalog needs a name".to_owned());
    }
    let path = folder.join(format!("{stem}.{CATALOG_EXTENSION}"));
    if path.exists() {
        return Err(format!("there is already a catalog called \u{201c}{stem}\u{201d} in {}", folder.display()));
    }
    Ok(path)
}

/// The catalog to open at start when none was open: `Default` in `folder`.
/// The first time, the single catalog of earlier versions (`legacy`, the
/// database in the editor's data folder) is copied into it, so ratings,
/// keywords and edits carry over; returns whether that happened.
///
/// # Errors
/// The folder cannot be made or the old database copied.
pub fn default_catalog(folder: &Path, legacy: Option<&Path>) -> io::Result<(PathBuf, bool)> {
    let catalog = folder.join(format!("{DEFAULT_NAME}.{CATALOG_EXTENSION}"));
    let db = database(&catalog);
    if db.exists() {
        return Ok((catalog, false));
    }
    std::fs::create_dir_all(&catalog)?;
    let Some(old) = legacy.filter(|p| p.is_file()) else { return Ok((catalog, false)) };
    // The database and, when SQLite left them, its journal files.
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", old.display()));
        if from.is_file() {
            std::fs::copy(&from, PathBuf::from(format!("{}{suffix}", db.display())))?;
        }
    }
    Ok((catalog, true))
}

/// The photos (RAW, JPEG, TIFF) an import of `dir` adds: those in it, and in its
/// sub-folders when `subfolders`, in natural order.
#[must_use]
pub fn raw_files(dir: &Path, subfolders: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(d) = pending.pop() {
        let Ok(listing) = crate::library::list_folder(&d) else { continue };
        out.extend(listing.photos);
        if subfolders {
            pending.extend(listing.subfolders.into_iter().rev());
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rne-catalogs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_catalogs_are_named_folders_and_never_replace_one() {
        let dir = temp("new");
        let travel = new_catalog_path(&dir, " Travel ").unwrap();
        assert_eq!(travel, dir.join("Travel.rnecatalog"));
        assert_eq!(name(&travel), "Travel");
        assert!(!is_catalog(&travel), "not made yet");
        std::fs::create_dir_all(&travel).unwrap();
        assert!(is_catalog(&travel));
        assert!(new_catalog_path(&dir, "Travel").is_err());
        assert!(new_catalog_path(&dir, "  ").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_default_catalog_takes_over_the_old_database_once() {
        let dir = temp("default");
        let old = dir.join("catalog.sqlite");
        std::fs::write(&old, b"old data").unwrap();
        let (catalog, moved) = default_catalog(&dir.join("Catalogs"), Some(&old)).unwrap();
        assert!(moved);
        assert_eq!(std::fs::read(database(&catalog)).unwrap(), b"old data");
        std::fs::write(database(&catalog), b"newer").unwrap();
        let (again, moved) = default_catalog(&dir.join("Catalogs"), Some(&old)).unwrap();
        assert!(!moved && again == catalog);
        assert_eq!(std::fs::read(database(&catalog)).unwrap(), b"newer", "never copied over again");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_import_lists_the_raw_files_and_optionally_the_sub_folders() {
        let dir = temp("import");
        std::fs::create_dir_all(dir.join("day2")).unwrap();
        for f in ["a.NEF", "notes.txt", "day2/b.CR3"] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        assert_eq!(raw_files(&dir, false), vec![dir.join("a.NEF")]);
        assert_eq!(raw_files(&dir, true), vec![dir.join("a.NEF"), dir.join("day2").join("b.CR3")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
