//! The database: built-in lensfun data, optionally extended with newer
//! lensfun files from a folder, and the lookups the editor needs.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use crate::matching::{self, Query};
use crate::model::{self, Camera, Lens, Mount, loose_eq};
use crate::xml;

/// Largest database file read from disk (the whole lensfun database is ~5 MB).
pub const MAX_FILE_BYTES: u64 = 64 << 20;

/// Why database text could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbError {
    /// The text is not well-formed XML.
    Xml(xml::XmlError),
    /// The XML is not a lensfun database.
    NotADatabase(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(e) => write!(f, "not valid XML: {e}"),
            Self::NotADatabase(m) => write!(f, "not a lensfun database: {m}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(e) => Some(e),
            Self::NotADatabase(_) => None,
        }
    }
}

/// A lens found for a query, with lensfun's match score (higher is better).
#[derive(Debug, Clone, Copy)]
pub struct LensMatch<'a> {
    /// The lens.
    pub lens: &'a Lens,
    /// Match score; only the order matters.
    pub score: u32,
}

/// Cameras, lenses and mounts.
#[derive(Debug, Clone, Default)]
pub struct LensDatabase {
    mounts: Vec<Mount>,
    cameras: Vec<Camera>,
    lenses: Vec<Lens>,
    warnings: Vec<String>,
}

impl LensDatabase {
    /// An empty database.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The lensfun data built into the editor (major camera and lens makers),
    /// read once on first use.
    pub fn builtin() -> &'static Self {
        static DB: OnceLock<LensDatabase> = OnceLock::new();
        DB.get_or_init(|| {
            let mut db = Self::new();
            for (name, text) in crate::data::FILES {
                if let Err(e) = db.add_xml(text) {
                    // Covered by a test; never expected at run time.
                    db.warnings.push(format!("built-in {name}.xml: {e}"));
                }
            }
            db
        })
    }

    /// Reads one lensfun XML file into a new database.
    ///
    /// # Errors
    /// [`DbError`] when the text is not a lensfun database.
    pub fn from_xml(text: &str) -> Result<Self, DbError> {
        let mut db = Self::new();
        db.add_xml(text)?;
        Ok(db)
    }

    /// Adds the contents of a lensfun XML file. An entry for a camera or lens
    /// already present replaces it, so newer files override older data.
    /// Returns the number of cameras and lenses read.
    ///
    /// # Errors
    /// [`DbError`] when the text is not a lensfun database; nothing is added then.
    pub fn add_xml(&mut self, text: &str) -> Result<usize, DbError> {
        let root = xml::parse(text).map_err(DbError::Xml)?;
        let doc = model::read_document(&root).map_err(DbError::NotADatabase)?;
        let count = doc.cameras.len() + doc.lenses.len();
        for m in doc.mounts {
            match self.mounts.iter_mut().find(|x| loose_eq(&x.name, &m.name)) {
                Some(existing) => {
                    for c in m.compat {
                        if !existing.compat.iter().any(|e| loose_eq(e, &c)) {
                            existing.compat.push(c);
                        }
                    }
                }
                None => self.mounts.push(m),
            }
        }
        for c in doc.cameras {
            let same = |x: &Camera| {
                loose_eq(x.maker.primary(), c.maker.primary())
                    && loose_eq(x.model.primary(), c.model.primary())
                    && x.variant == c.variant
            };
            match self.cameras.iter_mut().find(|x| same(x)) {
                Some(existing) => *existing = c,
                None => self.cameras.push(c),
            }
        }
        for l in doc.lenses {
            let same = |x: &Lens| {
                loose_eq(x.maker.primary(), l.maker.primary())
                    && loose_eq(x.model.primary(), l.model.primary())
                    && (x.mounts.is_empty() || x.mounts.iter().any(|m| l.mounts.iter().any(|n| loose_eq(m, n))))
            };
            match self.lenses.iter_mut().find(|x| same(x)) {
                // The same lens measured on another sensor format is a second
                // entry in lensfun files: keep both sets. Data for a format
                // already present replaces it (newer files win).
                Some(existing) => merge_lens(existing, l),
                None => self.lenses.push(l),
            }
        }
        self.warnings.extend(doc.warnings);
        Ok(count)
    }

    /// Adds every `*.xml` file in `dir` (for newer lensfun data, e.g. from
    /// `lensfun-update-data`). Files that cannot be read are skipped; the
    /// returned list says which and why. A missing folder is not an error.
    pub fn add_dir(&mut self, dir: &Path) -> Vec<String> {
        let mut problems = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return problems };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")))
            .collect();
        files.sort();
        for path in files {
            let result = std::fs::metadata(&path)
                .map_err(|e| e.to_string())
                .and_then(|m| {
                    if m.len() > MAX_FILE_BYTES {
                        Err(format!("larger than {} MB", MAX_FILE_BYTES >> 20))
                    } else {
                        std::fs::read_to_string(&path).map_err(|e| e.to_string())
                    }
                })
                .and_then(|text| self.add_xml(&text).map_err(|e| e.to_string()));
            if let Err(e) = result {
                problems.push(format!("{}: {e}", path.display()));
            }
        }
        problems
    }

    /// All cameras.
    #[must_use]
    pub fn cameras(&self) -> &[Camera] {
        &self.cameras
    }

    /// All lenses.
    #[must_use]
    pub fn lenses(&self) -> &[Lens] {
        &self.lenses
    }

    /// All mounts.
    #[must_use]
    pub fn mounts(&self) -> &[Mount] {
        &self.mounts
    }

    /// Entries skipped while reading, with the reason.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The camera with this maker and model, as written in the EXIF data or
    /// in the short forms ("Nikon", "D3S").
    #[must_use]
    pub fn find_camera(&self, maker: &str, model: &str) -> Option<&Camera> {
        self.cameras.iter().find(|c| c.maker.matches(maker) && c.model.matches(model))
    }

    /// Mounts whose lenses fit `mount`, besides itself.
    #[must_use]
    pub fn compatible_mounts(&self, mount: &str) -> Vec<&str> {
        self.mounts
            .iter()
            .find(|m| loose_eq(&m.name, mount))
            .map_or_else(Vec::new, |m| m.compat.iter().map(String::as_str).collect())
    }

    /// Lenses that may be the one named `model` (and `maker`, if known) on
    /// `camera`, best first, following lensfun's rules: every word of the
    /// name must appear in a candidate's name. This is the lookup for
    /// automatic correction; a wrong lens would do more harm than none.
    #[must_use]
    pub fn find_lenses(&self, camera: Option<&Camera>, maker: Option<&str>, model: &str) -> Vec<LensMatch<'_>> {
        self.query(camera, maker, model, true)
    }

    /// Like [`Self::find_lenses`], but a candidate only needs to share words
    /// with the name: suggestions to choose from by hand when the automatic
    /// lookup finds nothing.
    #[must_use]
    pub fn suggest_lenses(&self, camera: Option<&Camera>, maker: Option<&str>, model: &str) -> Vec<LensMatch<'_>> {
        self.query(camera, maker, model, false)
    }

    fn query(&self, camera: Option<&Camera>, maker: Option<&str>, model: &str, all_words: bool) -> Vec<LensMatch<'_>> {
        let model = model.trim();
        if model.is_empty() {
            return Vec::new();
        }
        let query = Query {
            camera,
            compat_mounts: camera.map_or_else(Vec::new, |c| self.compatible_mounts(&c.mount)),
            maker: maker.map(str::trim).filter(|m| !m.is_empty()),
            words: matching::words(model),
            guess: matching::guess_from_name(model),
            all_words,
        };
        self.matches(&query)
    }

    fn matches(&self, q: &Query<'_>) -> Vec<LensMatch<'_>> {
        let mut found: Vec<LensMatch<'_>> = self
            .lenses
            .iter()
            .filter_map(|lens| {
                let score = matching::score(q, lens);
                (score > 0).then_some(LensMatch { lens, score })
            })
            .collect();
        // Stable: equal scores keep database order.
        found.sort_by_key(|m| std::cmp::Reverse(m.score));
        found
    }

    /// The lens whose full name ([`Lens::full_name`]) or model is `name`,
    /// preferring one that fits `camera`. Used for a lens chosen by hand.
    #[must_use]
    pub fn lens_named(&self, name: &str, camera: Option<&Camera>) -> Option<&Lens> {
        let named = |l: &&Lens| loose_eq(&l.full_name(), name) || l.model.matches(name);
        let fits = |l: &&Lens| match camera {
            Some(c) => {
                let compat = self.compatible_mounts(&c.mount);
                l.mounts.iter().any(|m| loose_eq(m, &c.mount) || compat.iter().any(|x| loose_eq(m, x)))
            }
            None => true,
        };
        self.lenses.iter().filter(named).find(fits).or_else(|| self.lenses.iter().find(named))
    }

    /// Lenses whose name contains every word of `text` (case-insensitive),
    /// for browsing. An empty text lists every lens.
    #[must_use]
    pub fn search_lenses(&self, text: &str) -> Vec<&Lens> {
        let wanted: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        self.lenses
            .iter()
            .filter(|l| {
                let name = format!("{} {}", l.full_name(), l.mounts.join(" ")).to_lowercase();
                wanted.iter().all(|w| name.contains(w.as_str()))
            })
            .collect()
    }
}

/// Folds `newer` into `existing` (same maker, model and mount).
fn merge_lens(existing: &mut Lens, newer: Lens) {
    for set in newer.calibrations {
        let same_format = |s: &&mut model::CalibrationSet| {
            (s.crop_factor - set.crop_factor).abs() < 1e-3 && (s.aspect_ratio - set.aspect_ratio).abs() < 1e-3
        };
        match existing.calibrations.iter_mut().find(same_format) {
            Some(old) => *old = set,
            None => existing.calibrations.push(set),
        }
    }
    for m in newer.mounts {
        if !existing.mounts.iter().any(|x| loose_eq(x, &m)) {
            existing.mounts.push(m);
        }
    }
    for (old, new) in existing.focal_range.iter_mut().zip(newer.focal_range) {
        if new > 0.0 {
            *old = new;
        }
    }
    for (old, new) in existing.aperture_range.iter_mut().zip(newer.aperture_range) {
        if new > 0.0 {
            *old = new;
        }
    }
    existing.lens_type = newer.lens_type;
    existing.center = newer.center;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_database_reads_without_errors() {
        let db = LensDatabase::builtin();
        assert!(db.cameras().len() > 800, "{} cameras", db.cameras().len());
        assert!(db.lenses().len() > 1200, "{} lenses", db.lenses().len());
        assert!(db.warnings().iter().all(|w| !w.starts_with("built-in")), "{:?}", db.warnings());
        let with_data = db.lenses().iter().filter(|l| l.has_distortion() || l.has_vignetting()).count();
        assert!(with_data > 1000, "{with_data} lenses with data");
    }

    #[test]
    fn popular_cameras_are_found_by_exif_and_by_short_names() {
        let db = LensDatabase::builtin();
        for (maker, model, mount) in [
            ("NIKON CORPORATION", "NIKON D3S", "Nikon F AF"),
            ("Nikon", "D3S", "Nikon F AF"),
            ("SONY", "ILCE-7RM5", "Sony E"),
            ("Canon", "Canon EOS 5D Mark IV", "Canon EF"),
            ("Canon", "EOS 5D Mark IV", "Canon EF"),
            ("FUJIFILM", "X-T5", "Fujifilm X"),
            ("OM Digital Solutions", "OM-1", "Micro 4/3 System"),
        ] {
            let c = db.find_camera(maker, model).unwrap_or_else(|| panic!("{maker} {model} not found"));
            assert_eq!(c.mount, mount, "{maker} {model}");
        }
        assert!(db.find_camera("Sony", "ILCE-9999").is_none());
    }

    #[test]
    fn lenses_are_found_from_the_names_cameras_write() {
        let db = LensDatabase::builtin();
        let a7 = db.find_camera("SONY", "ILCE-7RM5");
        let found = db.find_lenses(a7, Some("Sony"), "FE 24-70mm F2.8 GM");
        let best = found.first().expect("a lens");
        assert!(best.lens.model.primary().contains("24-70mm f/2.8 GM"), "{}", best.lens.full_name());

        let d850 = db.find_camera("Nikon", "D850");
        let found = db.find_lenses(d850, None, "AF-S Nikkor 24-70mm f/2.8G ED");
        assert!(found.first().is_some_and(|m| m.lens.full_name().contains("24-70mm f/2.8G")), "{found:?}");

        // A full-frame lens is a candidate on a Sony E body, a Canon EF lens is not.
        let found = db.find_lenses(a7, None, "Canon EF 50mm f/1.8 STM");
        assert!(found.iter().all(|m| !m.lens.mounts.iter().any(|x| x == "Canon EF")), "{found:?}");
    }

    #[test]
    fn newer_files_replace_entries_and_bad_files_change_nothing() {
        let mut db = LensDatabase::from_xml(
            r#"<lensdatabase><lens><maker>X</maker><model>X 50mm f/2</model><mount>M</mount>
               <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration></lens></lensdatabase>"#,
        )
        .expect("db");
        let newer = r#"<lensdatabase><lens><maker>X</maker><model>X 50mm f/2</model><mount>M</mount>
               <calibration><distortion model="poly3" focal="50" k1="-0.02"/></calibration></lens></lensdatabase>"#;
        assert_eq!(db.add_xml(newer), Ok(1));
        assert_eq!(db.lenses().len(), 1);
        assert!((db.lenses()[0].calibrations[0].distortion[0].terms[0] + 0.02).abs() < 1e-7);
        assert!(db.add_xml("<lensdatabase><lens>").is_err());
        assert!(matches!(db.add_xml("<other/>"), Err(DbError::NotADatabase(_))));
        assert_eq!(db.lenses().len(), 1);
        assert!(db.lens_named("X 50mm f/2", None).is_some());
        assert_eq!(db.search_lenses("50mm x").len(), 1);
    }

    #[test]
    fn the_same_lens_measured_on_two_sensor_formats_keeps_both() {
        let db = LensDatabase::builtin();
        let l = db.lens_named("Canon EF 24-105mm f/4L IS USM", None).expect("lens");
        let mut crops: Vec<f32> = l.calibrations.iter().map(|s| s.crop_factor).collect();
        crops.sort_by(f32::total_cmp);
        assert!(crops.len() >= 2 && (crops[0] - 1.0).abs() < 1e-6, "{crops:?}");
        let ff = db.find_camera("Canon", "Canon EOS 5D Mark IV");
        assert!(!db.find_lenses(ff, None, "EF 24-105mm f/4L IS USM").is_empty());
    }

    #[test]
    fn a_missing_folder_adds_nothing_and_reports_nothing() {
        let mut db = LensDatabase::new();
        assert!(db.add_dir(Path::new("this/folder/does/not/exist")).is_empty());
        assert!(db.lenses().is_empty());
    }
}
