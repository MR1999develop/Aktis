//! What the app remembers between sessions: theme, folders, recent files.
//!
//! Saved by eframe's storage (a RON file in the system's settings folder).
//! Every field has a default and unknown or missing fields are ignored, so
//! older and newer versions of the app read each other's settings.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Key under which the settings are stored.
pub const STORAGE_KEY: &str = "settings";

/// Files kept in File ▸ Open recent.
pub const MAX_RECENT: usize = 10;

/// The interface theme chosen by the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the system.
    System,
    /// Light.
    Light,
    /// Dark: the default for a photo editor, so colours are judged on a dark surround.
    #[default]
    Dark,
}

/// Remembered between sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Interface theme.
    pub theme: ThemeChoice,
    /// Folder the Open dialog starts in.
    pub open_dir: Option<PathBuf>,
    /// Folder exports go to; `None` = next to the RAW file.
    pub export_dir: Option<PathBuf>,
    /// Recently opened files, newest first.
    pub recent: Vec<PathBuf>,
}

impl Settings {
    /// Records an opened file: first in the recent list (no duplicates) and
    /// its folder as the next Open folder.
    pub fn opened(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_owned());
        self.recent.truncate(MAX_RECENT);
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            self.open_dir = Some(dir.to_owned());
        }
    }

    /// Removes a file from the recent list (it could not be opened).
    pub fn forget(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
    }

    /// Folder an export of `raw` goes to.
    #[must_use]
    pub fn export_folder(&self, raw: &Path) -> PathBuf {
        self.export_dir
            .clone()
            .or_else(|| raw.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_are_newest_first_unique_and_limited() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.opened(Path::new(&format!("dir/{i}.NEF")));
        }
        s.opened(Path::new("dir/5.NEF"));
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], Path::new("dir/5.NEF"));
        assert_eq!(s.recent.iter().filter(|p| p.ends_with("5.NEF")).count(), 1);
        assert_eq!(s.open_dir.as_deref(), Some(Path::new("dir")));
        s.forget(Path::new("dir/5.NEF"));
        assert!(!s.recent.iter().any(|p| p.ends_with("5.NEF")));
    }

    #[test]
    fn exports_go_next_to_the_raw_unless_a_folder_was_chosen() {
        let mut s = Settings::default();
        assert_eq!(s.export_folder(Path::new("shots/a.NEF")), Path::new("shots"));
        assert_eq!(s.export_folder(Path::new("a.NEF")), Path::new("."));
        s.export_dir = Some(PathBuf::from("out"));
        assert_eq!(s.export_folder(Path::new("shots/a.NEF")), Path::new("out"));
    }

    #[test]
    fn the_default_theme_is_dark() {
        assert_eq!(Settings::default().theme, ThemeChoice::Dark);
    }
}
