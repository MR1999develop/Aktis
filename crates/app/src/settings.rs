//! What the app remembers between sessions: theme, folders, recent files,
//! the folder shown in the filmstrip, the export window's choices.
//!
//! Saved by eframe's storage (a RON file in the system's settings folder).
//! Every field has a default and unknown or missing fields are ignored, so
//! older and newer versions of the app read each other's settings.

use std::path::{Path, PathBuf};

use engine::export;
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

/// The colour around the photo in the viewer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewerBackground {
    /// The interface's own dark (or light) canvas.
    #[default]
    Interface,
    /// White, as Capture One offers, for more contrast around the photo.
    White,
    /// A light neutral grey.
    LightGrey,
    /// Middle grey, for judging tones.
    MiddleGrey,
    /// Black.
    Black,
}

impl ViewerBackground {
    /// Every choice offered, in menu order: the interface's colour or white,
    /// as in Capture One. The greys and black of earlier versions are read
    /// back as the interface's colour.
    pub const ALL: [Self; 2] = [Self::Interface, Self::White];

    /// The choice as used: the ones no longer offered become [`Self::Interface`].
    #[must_use]
    pub fn offered(self) -> Self {
        if self == Self::White { Self::White } else { Self::Interface }
    }

    /// Name in the Preferences.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Interface => "Like the interface",
            Self::White => "White",
            Self::LightGrey => "Light grey",
            Self::MiddleGrey => "Middle grey",
            Self::Black => "Black",
        }
    }
}

/// The sizes of the window's panels (points) and its other view choices;
/// `None`: the default size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Explorer, left.
    pub explorer_width: Option<f32>,
    /// Filmstrip, next to the Explorer.
    pub filmstrip_width: Option<f32>,
    /// Settings, right.
    pub inspector_width: Option<f32>,
    /// History and Metadata tabs under the Explorer.
    pub tabs_height: Option<f32>,
    /// Node graph under the photo.
    pub graph_height: Option<f32>,
    /// Scope above the settings: histogram, waveform or parade.
    pub scope: crate::scopes::ScopeKind,
    /// Its height; `None`: the default.
    pub scope_height: Option<f32>,
    /// The left tab shown: the history (`false`) or the metadata.
    pub metadata_tab: bool,
    /// The exposure warning is on.
    pub exposure_warning: bool,
}

/// A layout saved under a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Its name in the menu.
    pub name: String,
    /// What it restores.
    pub layout: Layout,
}

impl Settings {
    /// Saves the current layout as workspace `name`, replacing one of the
    /// same name (any case); the list stays sorted by name.
    pub fn save_workspace(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.workspaces.retain(|w| !w.name.eq_ignore_ascii_case(name));
        self.workspaces.push(Workspace { name: name.to_owned(), layout: self.layout });
        self.workspaces.sort_by_key(|w| w.name.to_lowercase());
    }

    /// Makes workspace `index` the current layout (`None`: the default
    /// layout); the panels take its sizes at the next frame.
    pub fn apply_workspace(&mut self, index: Option<usize>) {
        let layout = match index {
            Some(i) => match self.workspaces.get(i) {
                Some(w) => w.layout,
                None => return,
            },
            None => Layout::default(),
        };
        self.layout = layout;
        self.layout_generation = self.layout_generation.wrapping_add(1);
    }
}

/// The exposure warning's settings, as in Capture One's Preferences ▸
/// Exposure: the highlight threshold (250 by default) and a shadow warning,
/// off by default, with its own threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WarningSettings {
    /// A displayed channel at this value (0–255) or above is burnt.
    pub highlight: u8,
    /// Blocked shadows are shown too.
    pub shadow_enabled: bool,
    /// All displayed channels at this value or below are blocked.
    pub shadow: u8,
}

impl Default for WarningSettings {
    fn default() -> Self {
        Self { highlight: 250, shadow_enabled: false, shadow: 5 }
    }
}

impl WarningSettings {
    /// The thresholds the warning is computed with.
    #[must_use]
    pub fn thresholds(self) -> editor::clipping::Thresholds {
        editor::clipping::Thresholds { highlight: self.highlight, shadow: self.shadow_enabled.then_some(self.shadow) }
    }
}

/// The Import Images window's options, as they were last used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    /// Copy the files into `copy_dir` instead of adding them where they are.
    pub copy: bool,
    /// The folder the copies go in.
    pub copy_dir: Option<PathBuf>,
    /// Sub-folders below it ([`catalog::FolderPattern`]).
    pub pattern: String,
    /// Make a second copy of the originals in `backup_dir`.
    pub backup: bool,
    /// The backup folder.
    pub backup_dir: Option<PathBuf>,
    /// Leave out photos already in the catalog (file name and capture time).
    pub skip_duplicates: bool,
    /// Import the JPEG of a RAW+JPEG pair too (Capture One's default).
    pub jpeg_twins: bool,
    /// Preset applied to every photo ("Group / Name"), if any.
    pub preset: Option<String>,
    /// Photographer given to every photo.
    pub artist: String,
    /// Copyright notice given to every photo.
    pub copyright: String,
    /// Keywords, separated by commas.
    pub keywords: String,
    /// Rating given to every photo (0: none).
    pub rating: u8,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            copy: false,
            copy_dir: None,
            pattern: catalog::import::DEFAULT_PATTERN.to_owned(),
            backup: false,
            backup_dir: None,
            skip_duplicates: true,
            jpeg_twins: true,
            preset: None,
            artist: String::new(),
            copyright: String::new(),
            keywords: String::new(),
            rating: 0,
        }
    }
}

impl ImportSettings {
    /// Where the photos go.
    ///
    /// # Errors
    /// A message when copying and no folder is chosen.
    pub fn destination(&self) -> Result<catalog::Destination, String> {
        if !self.copy {
            return Ok(catalog::Destination::InPlace);
        }
        let folder = self.copy_dir.clone().ok_or("Choose the folder the copies go in")?;
        Ok(catalog::Destination::Copy { folder, pattern: catalog::FolderPattern::new(&self.pattern) })
    }

    /// The backup folder, when there is a backup.
    ///
    /// # Errors
    /// A message when the backup is on and no folder is chosen, or when it
    /// is the copies' folder.
    pub fn backup_folder(&self) -> Result<Option<PathBuf>, String> {
        if !self.backup {
            return Ok(None);
        }
        let dir = self.backup_dir.clone().ok_or("Choose the backup folder")?;
        if self.copy && self.copy_dir.as_ref() == Some(&dir) {
            return Err("The backup needs a folder of its own, apart from the copies".to_owned());
        }
        Ok(Some(dir))
    }

    /// Why the import cannot start yet, if it cannot.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        self.destination().err().or_else(|| self.backup_folder().err())
    }

    /// The photos imported out of those `found`: without the JPEG of each
    /// RAW+JPEG pair unless the pairs come in whole.
    #[must_use]
    pub fn files(&self, found: &[PathBuf]) -> Vec<PathBuf> {
        if self.jpeg_twins {
            found.to_vec()
        } else {
            catalog::without_jpeg_twins(found, raw_io::has_raw_extension, raw_io::has_jpeg_extension)
        }
    }

    /// What the import gives to every photo.
    #[must_use]
    pub fn metadata(&self) -> catalog::ImportMetadata {
        let text = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
        catalog::ImportMetadata {
            artist: text(&self.artist),
            copyright: text(&self.copyright),
            keywords: catalog::ImportMetadata::parse_keywords(&self.keywords),
            rating: self.rating.min(catalog::MAX_RATING),
        }
    }
}

/// Remembered between sessions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Interface theme.
    pub theme: ThemeChoice,
    /// Folder the Open dialog starts in.
    pub open_dir: Option<PathBuf>,
    /// The folder last chosen in the export window ("Choose…").
    pub export_dir: Option<PathBuf>,
    /// The export window's last choices.
    pub export: ExportSettings,
    /// Recently opened files, newest first.
    pub recent: Vec<PathBuf>,
    /// Its thresholds and whether it shows the shadows too (Preferences ▸ Exposure).
    pub warning: WarningSettings,
    /// Sizes of the panels and the left tab shown, as the window was left.
    pub layout: Layout,
    /// Layouts saved under a name (Window ▸ Workspace), as in Capture One.
    pub workspaces: Vec<Workspace>,
    /// Bumped when a workspace is applied, so the panels start again from its sizes.
    pub layout_generation: u32,
    /// Colour around the photo in the viewer.
    pub viewer_background: ViewerBackground,
    /// Folder of the presets and style brushes; `None`: `Presets` in the
    /// editor's data folder.
    pub presets_dir: Option<PathBuf>,
    /// The node selected in the photo open when the editor was closed (the
    /// first of `recent`), selected again when it reopens.
    pub last_node: Option<u32>,
    /// The catalog folder (`*.rnecatalog`) open when the editor was closed.
    pub catalog: Option<PathBuf>,
    /// Folder where new catalogs are made (Preferences ▸ Catalog); `None`:
    /// `Catalogs` in the editor's data folder.
    pub catalogs_dir: Option<PathBuf>,
    /// The folder of the last import (File ▸ Import Images…).
    pub import_dir: Option<PathBuf>,
    /// The Import Images window's options.
    pub import: ImportSettings,
    /// Recently opened catalogs, newest first.
    pub recent_catalogs: Vec<PathBuf>,
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

    /// Records an opened catalog: the one to open at start-up and first in
    /// the recent catalogs.
    pub fn catalog_opened(&mut self, catalog: &Path) {
        self.recent_catalogs.retain(|p| p != catalog);
        self.recent_catalogs.insert(0, catalog.to_owned());
        self.recent_catalogs.truncate(MAX_RECENT);
        self.catalog = Some(catalog.to_owned());
    }

    /// Removes a catalog from the recent list (it could not be opened).
    pub fn forget_catalog(&mut self, catalog: &Path) {
        self.recent_catalogs.retain(|p| p != catalog);
    }

    /// The folder where new catalogs are made.
    #[must_use]
    pub fn catalogs_folder(&self) -> Option<PathBuf> {
        self.catalogs_dir.clone().or_else(editor::catalogs::default_folder)
    }

    /// The folder of the presets and style brushes.
    #[must_use]
    pub fn presets_folder(&self) -> Option<PathBuf> {
        self.presets_dir.clone().or_else(editor::presets::default_folder)
    }

    /// The export options the export window last used.
    #[must_use]
    pub fn export_options(&self) -> export::ExportOptions {
        self.export.to_options(self.export_dir.as_deref())
    }
}

/// File format of exports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// JPEG.
    #[default]
    Jpeg,
    /// PNG.
    Png,
    /// TIFF.
    Tiff,
}

/// How the size of exports is chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResizeMode {
    /// Full size.
    #[default]
    Original,
    /// Longer side in pixels.
    LongEdge,
    /// Width in pixels.
    Width,
    /// Height in pixels.
    Height,
    /// A percentage.
    Percent,
}

/// Where exports go.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DestinationMode {
    /// The folder of the RAW file.
    #[default]
    NextToOriginal,
    /// A sub-folder of the RAW file's folder.
    Subfolder,
    /// The folder chosen with "Choose…" (`Settings::export_dir`).
    Folder,
}

/// Which metadata exports keep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataMode {
    /// Everything.
    #[default]
    All,
    /// Everything but the GPS position.
    AllButGps,
    /// The copyright notice alone.
    CopyrightOnly,
    /// Nothing.
    None,
}

/// The export window's choices, remembered between sessions. Every field
/// has a default, so settings saved by older versions still load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    /// File format.
    pub format: ExportFormat,
    /// JPEG quality, 1–100.
    pub jpeg_quality: u8,
    /// 16 bits per channel for PNG and TIFF.
    pub sixteen_bit: bool,
    /// How the size is chosen.
    pub resize: ResizeMode,
    /// Pixels for the long edge, width or height.
    pub size_pixels: u32,
    /// Percentage for [`ResizeMode::Percent`].
    pub size_percent: f32,
    /// Allow making images larger.
    pub upscale: bool,
    /// File name pattern with tokens.
    pub name_pattern: String,
    /// First `{counter}` value.
    pub counter_start: u32,
    /// Digits of `{counter}`.
    pub counter_digits: u8,
    /// Where files go.
    pub destination: DestinationMode,
    /// Name of the sub-folder for [`DestinationMode::Subfolder`].
    pub subfolder: String,
    /// Draw a text watermark.
    pub watermark: bool,
    /// Watermark text.
    pub watermark_text: String,
    /// Watermark capital height in % of the long edge.
    pub watermark_size: f32,
    /// Watermark opacity, 0–1.
    pub watermark_opacity: f32,
    /// Watermark position: 0 top left … 8 bottom right, row by row.
    pub watermark_position: u8,
    /// Watermark margin in % of the long edge.
    pub watermark_margin: f32,
    /// Which metadata to write.
    pub metadata: MetadataMode,
    /// Copyright notice; empty keeps the camera's.
    pub copyright: String,
}

impl Default for ExportSettings {
    fn default() -> Self {
        let mark = export::Watermark::default();
        Self {
            format: ExportFormat::Jpeg,
            jpeg_quality: export::Format::default().jpeg_quality,
            sixteen_bit: false,
            resize: ResizeMode::Original,
            size_pixels: 2048,
            size_percent: 50.0,
            upscale: false,
            name_pattern: "{name}".into(),
            counter_start: 1,
            counter_digits: 3,
            destination: DestinationMode::NextToOriginal,
            subfolder: "Export".into(),
            watermark: false,
            watermark_text: String::new(),
            watermark_size: mark.size_percent,
            watermark_opacity: mark.opacity,
            watermark_position: 8,
            watermark_margin: mark.margin_percent,
            metadata: MetadataMode::All,
            copyright: String::new(),
        }
    }
}

impl ExportSettings {
    /// The export options these settings describe; `folder` is the folder
    /// chosen for [`DestinationMode::Folder`] (without one, files go next
    /// to the originals).
    #[must_use]
    pub fn to_options(&self, folder: Option<&Path>) -> export::ExportOptions {
        use export::{BitDepth, Destination, FileFormat, MetadataChoice, Resize};
        let kind = match self.format {
            ExportFormat::Jpeg => FileFormat::Jpeg,
            ExportFormat::Png => FileFormat::Png,
            ExportFormat::Tiff => FileFormat::Tiff,
        };
        let depth = if self.sixteen_bit { BitDepth::Sixteen } else { BitDepth::Eight };
        let n = self.size_pixels.max(1);
        let resize = match self.resize {
            ResizeMode::Original => Resize::Original,
            ResizeMode::LongEdge => Resize::LongEdge(n),
            ResizeMode::Width => Resize::Width(n),
            ResizeMode::Height => Resize::Height(n),
            ResizeMode::Percent => Resize::Percent(self.size_percent.clamp(1.0, 400.0)),
        };
        let destination = match (self.destination, folder) {
            (DestinationMode::Subfolder, _) => Destination::Subfolder(self.subfolder.clone()),
            (DestinationMode::Folder, Some(dir)) => Destination::Folder(dir.to_owned()),
            _ => Destination::NextToOriginal,
        };
        let watermark = (self.watermark && !self.watermark_text.trim().is_empty()).then(|| export::Watermark {
            text: self.watermark_text.clone(),
            size_percent: self.watermark_size.clamp(0.5, 20.0),
            opacity: self.watermark_opacity.clamp(0.05, 1.0),
            position: export::Position::ALL[usize::from(self.watermark_position.min(8))],
            margin_percent: self.watermark_margin.clamp(0.0, 20.0),
        });
        let metadata = match self.metadata {
            MetadataMode::All => MetadataChoice::All,
            MetadataMode::AllButGps => MetadataChoice::AllButGps,
            MetadataMode::CopyrightOnly => MetadataChoice::CopyrightOnly,
            MetadataMode::None => MetadataChoice::None,
        };
        export::ExportOptions {
            format: export::Format { kind, jpeg_quality: self.jpeg_quality.clamp(1, 100), depth },
            size: export::Size { resize, upscale: self.upscale },
            name: export::NamePattern {
                pattern: self.name_pattern.clone(),
                counter_start: self.counter_start,
                counter_digits: self.counter_digits.clamp(1, 9),
            },
            destination,
            watermark,
            metadata,
            copyright: self.copyright.clone(),
        }
    }

    /// One line for the Output node: "JPEG, quality 92, full size".
    #[must_use]
    pub fn summary(&self) -> String {
        let format = match self.format {
            ExportFormat::Jpeg => format!("JPEG, quality {}", self.jpeg_quality),
            ExportFormat::Png => format!("PNG, {} bit", if self.sixteen_bit { 16 } else { 8 }),
            ExportFormat::Tiff => format!("TIFF, {} bit", if self.sixteen_bit { 16 } else { 8 }),
        };
        let size = match self.resize {
            ResizeMode::Original => "full size".to_owned(),
            ResizeMode::LongEdge => format!("long edge {} px", self.size_pixels),
            ResizeMode::Width => format!("width {} px", self.size_pixels),
            ResizeMode::Height => format!("height {} px", self.size_pixels),
            ResizeMode::Percent => format!("{} %", self.size_percent.round()),
        };
        format!("{format}, {size}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_options_say_what_is_missing_before_an_import() {
        let mut o = ImportSettings::default();
        assert_eq!(o.destination(), Ok(catalog::Destination::InPlace));
        assert!(o.problem().is_none() && o.skip_duplicates);
        o.copy = true;
        assert!(o.problem().is_some(), "no folder for the copies");
        o.copy_dir = Some(PathBuf::from("D:/Photos"));
        assert!(matches!(o.destination(), Ok(catalog::Destination::Copy { .. })));
        o.backup = true;
        assert!(o.problem().is_some(), "no backup folder");
        o.backup_dir = Some(PathBuf::from("D:/Photos"));
        assert!(o.problem().is_some(), "the backup is the copies' folder");
        o.backup_dir = Some(PathBuf::from("E:/Backup"));
        assert_eq!(o.backup_folder(), Ok(Some(PathBuf::from("E:/Backup"))));
        let found = [PathBuf::from("d/a.NEF"), PathBuf::from("d/a.JPG")];
        assert_eq!(o.files(&found).len(), 2);
        o.jpeg_twins = false;
        assert_eq!(o.files(&found), [PathBuf::from("d/a.NEF")]);
        o.artist = "  Ann ".into();
        o.keywords = "sea, boat".into();
        o.rating = 9;
        let m = o.metadata();
        assert_eq!((m.artist.as_deref(), m.copyright, m.keywords.len(), m.rating), (Some("Ann"), None, 2, 5));
    }

    #[test]
    fn recent_catalogs_are_newest_first_unique_and_the_last_is_reopened() {
        let mut s = Settings::default();
        s.catalog_opened(Path::new("a.rnecatalog"));
        s.catalog_opened(Path::new("b.rnecatalog"));
        s.catalog_opened(Path::new("a.rnecatalog"));
        assert_eq!(s.recent_catalogs, [PathBuf::from("a.rnecatalog"), PathBuf::from("b.rnecatalog")]);
        assert_eq!(s.catalog.as_deref(), Some(Path::new("a.rnecatalog")));
        s.forget_catalog(Path::new("b.rnecatalog"));
        assert_eq!(s.recent_catalogs.len(), 1);
    }

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
    fn export_settings_describe_the_export_options() {
        let mut s = Settings::default();
        let o = s.export_options();
        assert_eq!(o.format.kind, export::FileFormat::Jpeg);
        assert_eq!(o.destination, export::Destination::NextToOriginal);
        assert_eq!(o.watermark, None);
        s.export = ExportSettings {
            format: ExportFormat::Tiff,
            sixteen_bit: true,
            resize: ResizeMode::LongEdge,
            size_pixels: 2000,
            destination: DestinationMode::Folder,
            watermark: true,
            watermark_text: "\u{a9} Me".into(),
            watermark_position: 0,
            metadata: MetadataMode::AllButGps,
            ..ExportSettings::default()
        };
        // No folder chosen yet: next to the originals.
        assert_eq!(s.export_options().destination, export::Destination::NextToOriginal);
        s.export_dir = Some(PathBuf::from("out"));
        let o = s.export_options();
        assert_eq!(o.destination, export::Destination::Folder(PathBuf::from("out")));
        assert_eq!(o.format.depth, export::BitDepth::Sixteen);
        assert_eq!(o.size.resize, export::Resize::LongEdge(2000));
        assert_eq!(o.watermark.map(|w| w.position), Some(export::Position::TopLeft));
        assert_eq!(o.metadata, export::MetadataChoice::AllButGps);
        assert_eq!(s.export.summary(), "TIFF, 16 bit, long edge 2000 px");
    }

    #[test]
    fn the_default_theme_is_dark() {
        assert_eq!(Settings::default().theme, ThemeChoice::Dark);
    }

    #[test]
    fn workspaces_are_saved_by_name_replaced_and_applied() {
        let mut s = Settings::default();
        s.layout.inspector_width = Some(400.0);
        s.save_workspace(" Retouch ");
        s.layout.inspector_width = Some(300.0);
        s.save_workspace("colour");
        s.save_workspace("RETOUCH"); // same name: replaced, not added
        let names: Vec<&str> = s.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["colour", "RETOUCH"]);
        s.save_workspace("   ");
        assert_eq!(s.workspaces.len(), 2, "no empty names");
        let generation = s.layout_generation;
        s.apply_workspace(Some(0));
        assert_eq!(s.layout.inspector_width, Some(300.0));
        assert_ne!(s.layout_generation, generation, "the panels start again");
        s.apply_workspace(None);
        assert_eq!(s.layout, Layout::default());
        s.apply_workspace(Some(9)); // gone: nothing changes
        assert_eq!(s.layout, Layout::default());
    }
}
