//! Finding the DCP profiles installed on this computer for a camera.
//!
//! Profiles are looked for in the editor's own `profiles` folder (see
//! `platform::camera_profile_dirs`); other programs' folders are not searched. File names are only a first filter: a
//! profile is offered for a camera when its `UniqueCameraModel` names it.

use std::path::{Path, PathBuf};

use crate::dcp::{self, DcpError, MAX_DCP_BYTES};
use crate::profile::{ColorProfile, ProfileSource};

/// How deep to look inside each folder (Adobe: `CameraProfiles/Camera/<model>/x.dcp`).
const MAX_DEPTH: usize = 4;

/// Files looked at per folder at most, so a wrong folder cannot stall the editor.
const MAX_FILES: usize = 50_000;

/// A profile found for a camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEntry {
    /// Profile name ("Adobe Standard").
    pub name: String,
    /// Camera as the profile names it.
    pub camera: String,
    /// The file.
    pub path: PathBuf,
}

/// Why a profile file could not be used.
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read.
    Io(std::io::Error),
    /// The file is not a usable profile.
    Dcp(DcpError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read the profile: {e}"),
            Self::Dcp(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Dcp(e) => Some(e),
        }
    }
}

/// Reads one DCP file.
///
/// # Errors
/// [`LoadError`] when the file cannot be read or is not a usable profile.
pub fn load_dcp(path: &Path) -> Result<ColorProfile, LoadError> {
    let len = std::fs::metadata(path).map_err(LoadError::Io)?.len();
    if !usize::try_from(len).is_ok_and(|l| l <= MAX_DCP_BYTES) {
        return Err(LoadError::Dcp(DcpError::TooLarge));
    }
    let bytes = std::fs::read(path).map_err(LoadError::Io)?;
    dcp::read_dcp(&bytes, ProfileSource::Dcp(path.to_owned())).map_err(LoadError::Dcp)
}

/// Lower case letters and digits only: "Canon EOS 5D Mark IV" → "canoneos5dmarkiv".
fn key(text: &str) -> String {
    text.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// The ways a camera may be named in a profile: "Sony ILCE-7RM5",
/// "SONY ILCE-7RM5", "Canon EOS 5D Mark IV", "NIKON D850"…
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraNames {
    keys: Vec<String>,
    /// Short model keys used to pre-filter file names.
    models: Vec<String>,
}

impl CameraNames {
    /// From the EXIF maker/model and the cleaned-up ones.
    #[must_use]
    pub fn new(make: &str, model: &str, clean_make: &str, clean_model: &str) -> Self {
        let mut keys: Vec<String> = [
            format!("{clean_make} {clean_model}"),
            format!("{make} {model}"),
            format!("{clean_make} {model}"),
            model.to_owned(),
            clean_model.to_owned(),
        ]
        .iter()
        .map(|s| key(s))
        .filter(|k| !k.is_empty())
        .collect();
        keys.sort();
        keys.dedup();
        let mut models: Vec<String> = [key(clean_model), key(model)].into_iter().filter(|k| k.len() >= 2).collect();
        models.dedup();
        Self { keys, models }
    }

    /// `true` when a profile's camera name is this camera.
    #[must_use]
    pub fn matches(&self, profile_camera: &str) -> bool {
        let k = key(profile_camera);
        !k.is_empty() && self.keys.contains(&k)
    }

    fn file_may_match(&self, path: &Path) -> bool {
        let stem = path.file_stem().map(|s| key(&s.to_string_lossy())).unwrap_or_default();
        let parent = path.parent().and_then(Path::file_name).map(|s| key(&s.to_string_lossy())).unwrap_or_default();
        self.models.iter().any(|m| stem.contains(m.as_str()) || parent.contains(m.as_str()))
    }
}

/// The DCP files under a set of folders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileLibrary {
    dirs: Vec<PathBuf>,
}

impl ProfileLibrary {
    /// A library over `dirs`, searched in order (earlier folders win on equal names).
    #[must_use]
    pub fn new(dirs: Vec<PathBuf>) -> Self {
        Self { dirs }
    }

    /// The folders searched.
    #[must_use]
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// Profiles for `camera`, sorted by name; files that look like a match
    /// but cannot be read are listed in the second vector with the reason.
    #[must_use]
    pub fn for_camera(&self, camera: &CameraNames) -> (Vec<ProfileEntry>, Vec<String>) {
        let mut found: Vec<ProfileEntry> = Vec::new();
        let mut problems = Vec::new();
        for dir in &self.dirs {
            let mut files = Vec::new();
            collect_dcp_files(dir, 0, &mut files);
            files.sort();
            for path in files.into_iter().filter(|p| camera.file_may_match(p)) {
                match load_dcp(&path) {
                    Ok(p) if camera.matches(&p.camera) => {
                        if !found.iter().any(|e| e.name.eq_ignore_ascii_case(&p.name)) {
                            found.push(ProfileEntry { name: p.name, camera: p.camera, path });
                        }
                    }
                    Ok(_) => {}
                    Err(e) => problems.push(format!("{}: {e}", path.display())),
                }
            }
        }
        found.sort_by_key(|e| e.name.to_lowercase());
        (found, problems)
    }

    /// The profile called `name` for `camera`, if installed.
    ///
    /// # Errors
    /// [`LoadError`] when the file is found but cannot be read.
    pub fn find(&self, camera: &CameraNames, name: &str) -> Option<Result<ColorProfile, LoadError>> {
        let (entries, _) = self.for_camera(camera);
        let entry = entries.into_iter().find(|e| e.name.eq_ignore_ascii_case(name.trim()))?;
        Some(load_dcp(&entry.path))
    }
}

fn collect_dcp_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            collect_dcp_files(&path, depth + 1, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dcp")) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{CalibratedMatrix, Illuminant, Matrix3};

    fn profile(camera: &str, name: &str) -> Vec<u8> {
        let m = Matrix3::from_rows([[0.8, -0.3, -0.07], [-0.43, 1.2, 0.25], [-0.04, 0.13, 0.58]]);
        let mut p =
            ColorProfile::from_matrices(camera, &[CalibratedMatrix { illuminant: Illuminant::D65, xyz_to_camera: m }])
                .expect("profile");
        p.name = name.to_owned();
        dcp::write_dcp(&p)
    }

    #[test]
    fn camera_names_match_however_the_profile_spells_them() {
        let c = CameraNames::new("SONY", "ILCE-7RM5", "Sony", "ILCE-7RM5");
        assert!(c.matches("Sony ILCE-7RM5") && c.matches("SONY ILCE-7RM5"));
        assert!(!c.matches("Sony ILCE-7RM4"));
        let n = CameraNames::new("NIKON CORPORATION", "NIKON D850", "Nikon", "D850");
        assert!(n.matches("Nikon D850") && n.matches("NIKON D850") && !n.matches("Nikon D8500"));
    }

    #[test]
    fn profiles_are_found_by_their_camera_not_their_file_name() {
        let dir = std::env::temp_dir().join(format!("rne-profiles-{}", std::process::id()));
        let sub = dir.join("Adobe Standard");
        std::fs::create_dir_all(&sub).expect("temp folder");
        std::fs::write(sub.join("Sony ILCE-7RM5 Adobe Standard.dcp"), profile("Sony ILCE-7RM5", "Adobe Standard"))
            .expect("write");
        // Right file name, other camera inside: not offered.
        std::fs::write(sub.join("Sony ILCE-7RM5 Other.dcp"), profile("Sony ILCE-7RM4", "Other")).expect("write");
        std::fs::write(dir.join("SONY ILCE-7RM5.dcp"), profile("SONY ILCE-7RM5", "RawTherapee")).expect("write");
        std::fs::write(dir.join("Sony ILCE-7RM5 broken.dcp"), b"IIRC\xff\xff\xff\xff").expect("write");

        let lib = ProfileLibrary::new(vec![dir.clone(), dir.join("missing")]);
        let cam = CameraNames::new("SONY", "ILCE-7RM5", "Sony", "ILCE-7RM5");
        let (found, problems) = lib.for_camera(&cam);
        let names: Vec<&str> = found.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Adobe Standard", "RawTherapee"]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(lib.find(&cam, "adobe standard").is_some_and(|r| r.is_ok()));
        assert!(lib.find(&cam, "Camera Vivid").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
