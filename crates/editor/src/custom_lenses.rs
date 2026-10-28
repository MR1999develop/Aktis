//! Custom lenses: lens-correction settings saved under a name for the lens
//! that took a photo, one `.rnelens` file each in the editor's `Lenses`
//! folder. They are offered in the lens chooser, can be saved again with
//! new values, and a photo never edited starts with the one saved last for
//! its lens.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use core_model::{CustomLens, LensSettings};
use engine::export;
use recipe_store::{CUSTOM_LENS_EXTENSION, decode_custom_lens, encode_custom_lens};

/// Sub-folder of the editor's data folder.
pub const FOLDER_NAME: &str = "Lenses";

/// The folder used on this computer.
#[must_use]
pub fn default_folder() -> Option<PathBuf> {
    platform::editor_data_dir().map(|d| d.join(FOLDER_NAME))
}

/// The custom lenses of a folder, in the order they were saved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CustomLenses {
    folder: Option<PathBuf>,
    /// Oldest first, with the file each came from.
    lenses: Vec<(CustomLens, PathBuf)>,
    /// Files that could not be read.
    problems: Vec<String>,
}

impl CustomLenses {
    /// Reads every custom lens of `folder` (none when it is `None` or missing).
    #[must_use]
    pub fn load(folder: Option<PathBuf>) -> Self {
        let mut found: Vec<(SystemTime, CustomLens, PathBuf)> = Vec::new();
        let mut problems = Vec::new();
        let entries = folder.as_deref().and_then(|d| std::fs::read_dir(d).ok());
        for entry in entries.into_iter().flatten().flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(CUSTOM_LENS_EXTENSION)) {
                continue;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| decode_custom_lens(&t).map_err(|e| e.to_string()))
            {
                Ok(lens) => {
                    let saved = entry.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                    found.push((saved, lens, path));
                }
                Err(e) => problems.push(format!("{name}: {e}")),
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
        Self { folder, lenses: found.into_iter().map(|(_, l, p)| (l, p)).collect(), problems }
    }

    /// Every custom lens, by name.
    #[must_use]
    pub fn by_name(&self) -> Vec<&CustomLens> {
        let mut out: Vec<&CustomLens> = self.lenses.iter().map(|(l, _)| l).collect();
        out.sort_by_key(|l| l.name.to_lowercase());
        out
    }

    /// The custom lens called `name` (case apart).
    #[must_use]
    pub fn named(&self, name: &str) -> Option<&CustomLens> {
        self.lenses.iter().map(|(l, _)| l).find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The custom lens a photo taken with `exif_lens` starts with: the one
    /// saved last for that lens.
    #[must_use]
    pub fn for_lens(&self, exif_lens: &str) -> Option<&CustomLens> {
        self.lenses.iter().rev().map(|(l, _)| l).find(|l| l.fits(exif_lens))
    }

    /// The lens settings a photo never edited starts with: those of its
    /// custom lens, if any.
    #[must_use]
    pub fn start_settings(&self, exif_lens: Option<&str>) -> Option<LensSettings> {
        exif_lens.and_then(|k| self.for_lens(k)).map(|l| l.settings.clone())
    }

    /// Files that could not be read, to tell the user.
    #[must_use]
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// Saves `lens` (its settings then name it), replacing a custom lens of
    /// the same name; it becomes the one saved last for its lens.
    ///
    /// # Errors
    /// A message when there is no folder, the name is empty or the file
    /// cannot be written; nothing changes then.
    pub fn save(&mut self, mut lens: CustomLens) -> Result<(), String> {
        let folder = self.folder.clone().ok_or("there is no folder for custom lenses on this computer")?;
        lens.name = lens.name.trim().to_owned();
        if lens.name.is_empty() {
            return Err("a custom lens needs a name".to_owned());
        }
        if lens.exif_lens.trim().is_empty() {
            return Err("the photo does not name its lens".to_owned());
        }
        lens.settings = lens.settings.sanitized();
        lens.settings.custom = Some(lens.name.clone());
        std::fs::create_dir_all(&folder).map_err(|e| format!("the folder {} cannot be made: {e}", folder.display()))?;
        let old = self.position(&lens.name);
        let path = match old {
            Some(i) => self.lenses[i].1.clone(),
            None => free_path(&folder, &lens.name),
        };
        export::write_atomic(&path, encode_custom_lens(&lens).as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(i) = old {
            self.lenses.remove(i);
        }
        self.lenses.push((lens, path));
        Ok(())
    }

    /// Deletes the custom lens called `name` and its file.
    ///
    /// # Errors
    /// A message when the file cannot be removed; nothing changes then.
    pub fn delete(&mut self, name: &str) -> Result<(), String> {
        let Some(i) = self.position(name) else { return Ok(()) };
        let path = &self.lenses[i].1;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
        self.lenses.remove(i);
        Ok(())
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.lenses.iter().position(|(l, _)| l.name.eq_ignore_ascii_case(name.trim()))
    }
}

/// A file for a new custom lens called `name` that replaces nothing.
fn free_path(folder: &Path, name: &str) -> PathBuf {
    let stem = Some(export::sanitize_stem(name)).filter(|s| !s.is_empty()).unwrap_or_else(|| "Lens".into());
    export::non_destructive_path(&folder.join(format!("{stem}.{CUSTOM_LENS_EXTENSION}")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use core_model::ManualLens;

    fn lens(name: &str, exif: &str, vignetting: f32) -> CustomLens {
        CustomLens {
            name: name.to_owned(),
            exif_lens: exif.to_owned(),
            settings: LensSettings { manual: ManualLens { vignetting, distortion: 0.0 }, ..LensSettings::default() },
        }
    }

    #[test]
    fn custom_lenses_are_saved_replaced_found_by_lens_and_deleted() {
        let dir = std::env::temp_dir().join(format!("rne-custom-lenses-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut all = CustomLenses::load(Some(dir.clone()));
        assert!(all.by_name().is_empty());
        all.save(lens("Zoom soft", "Sony FE 24-70mm F2.8 GM", 0.2)).unwrap();
        all.save(lens("Prime", "Sigma 35mm F1.4 DG", 0.1)).unwrap();
        all.save(lens("Zoom strong", "sony fe 24-70mm  f2.8 gm", 0.6)).unwrap();
        assert_eq!(all.for_lens("Sony FE 24-70mm F2.8 GM").unwrap().name, "Zoom strong", "the one saved last");
        // Saving again under the same name replaces it and makes it the last.
        all.save(lens("zoom SOFT", "Sony FE 24-70mm F2.8 GM", 0.3)).unwrap();
        let start = all.start_settings(Some("Sony FE 24-70mm F2.8 GM")).unwrap();
        assert!((start.manual.vignetting - 0.3).abs() < 1e-6);
        assert_eq!(start.custom.as_deref(), Some("zoom SOFT"));
        assert_eq!(all.by_name().len(), 3);
        assert!(all.start_settings(Some("Canon EF 50mm")).is_none() && all.start_settings(None).is_none());

        let again = CustomLenses::load(Some(dir.clone()));
        assert_eq!(
            again.by_name().iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["Prime", "zoom SOFT", "Zoom strong"]
        );
        assert!(again.named("prime").is_some());

        all.delete("Prime").unwrap();
        assert!(all.named("Prime").is_none());
        assert_eq!(CustomLenses::load(Some(dir.clone())).by_name().len(), 2);
        assert!(all.save(lens(" ", "x", 0.0)).is_err());
        std::fs::write(dir.join("broken.rnelens"), "{").unwrap();
        assert_eq!(CustomLenses::load(Some(dir.clone())).problems().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
