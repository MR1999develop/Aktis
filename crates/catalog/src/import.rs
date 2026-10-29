//! Planning an import, without touching the disk: which photos are already
//! in the catalog (same file name and capture time, as Capture One's
//! "exclude duplicates"), the dated sub-folder a copy goes in, a free name
//! next to files already there, and what an import adds to the photos.
//!
//! Pure: the import job reads the files and copies them, the store keeps
//! the result.

use std::path::{Path, PathBuf};

/// Sub-folder pattern of copies when the user has not chosen one: year,
/// then day (`2026/2026-10-29`).
pub const DEFAULT_PATTERN: &str = "{yyyy}/{yyyy}-{mm}-{dd}";

/// Sub-folder of copies whose capture day is unknown.
pub const UNDATED_FOLDER: &str = "Undated";

/// What makes two files the same shot: the file name (case apart) and the
/// capture time written by the camera. A copy of a photo on another disk
/// has the same key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShotKey {
    name: String,
    captured: String,
}

impl ShotKey {
    /// The key of the photo at `path` taken at `captured` (as the camera
    /// writes it, "2026:08:14 18:42:07"); `None` without a name or a capture
    /// time, as such files cannot be told apart from others.
    #[must_use]
    pub fn new(path: &Path, captured: Option<&str>) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let captured = captured.map(str::trim).filter(|c| !c.is_empty())?.to_owned();
        Some(Self { name, captured })
    }

    /// The file name, lower case.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The capture time.
    #[must_use]
    pub fn captured(&self) -> &str {
        &self.captured
    }
}

/// A calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day {
    /// Year (2026).
    pub year: i32,
    /// Month, 1–12.
    pub month: u32,
    /// Day of the month, 1–31.
    pub day: u32,
}

impl Day {
    /// The day of an Exif date ("2026:08:14 18:42:07", also with `-`);
    /// `None` when it is not a real date (cameras write "0000:00:00").
    #[must_use]
    pub fn from_exif(text: &str) -> Option<Self> {
        let date = text.trim().get(..10)?;
        let mut parts = date.split([':', '-']);
        let year: i32 = parts.next()?.parse().ok()?;
        let month: u32 = parts.next()?.parse().ok()?;
        let day: u32 = parts.next()?.parse().ok()?;
        ((1..=9999).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day)).then_some(Self {
            year,
            month,
            day,
        })
    }

    /// The day (UTC) of `secs` seconds after 1970-01-01, for a file's
    /// modification time when the photo has no capture date.
    #[must_use]
    pub fn from_unix(secs: i64) -> Self {
        // Howard Hinnant's days-to-civil algorithm.
        let z = secs.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        Self {
            year: i32::try_from(year).unwrap_or(i32::MAX),
            month: u32::try_from(month).unwrap_or(1),
            day: u32::try_from(day).unwrap_or(1),
        }
    }
}

/// Where the copies of an import go below the chosen folder: a pattern of
/// sub-folders separated by `/`, with `{yyyy}`, `{yy}`, `{mm}` and `{dd}`
/// replaced by the capture day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderPattern(String);

impl Default for FolderPattern {
    fn default() -> Self {
        Self(DEFAULT_PATTERN.to_owned())
    }
}

impl FolderPattern {
    /// The pattern `text` (empty: every copy straight in the chosen folder).
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self(text.trim().to_owned())
    }

    /// The pattern as typed.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.0
    }

    /// Whether the pattern uses the capture day.
    #[must_use]
    pub fn is_dated(&self) -> bool {
        ["{yyyy}", "{yy}", "{mm}", "{dd}"].iter().any(|t| self.0.contains(t))
    }

    /// The sub-folder for a photo taken on `day` (`None`: unknown, the
    /// dated parts become [`UNDATED_FOLDER`]). Characters a folder name
    /// cannot hold are taken out, and so are `.` and `..`, so the result
    /// always stays below the chosen folder.
    #[must_use]
    pub fn folder_for(&self, day: Option<Day>) -> PathBuf {
        let mut out = PathBuf::new();
        for part in self.0.split(['/', '\\']) {
            let text = match day {
                Some(d) => part
                    .replace("{yyyy}", &format!("{:04}", d.year))
                    .replace("{yy}", &format!("{:02}", d.year.rem_euclid(100)))
                    .replace("{mm}", &format!("{:02}", d.month))
                    .replace("{dd}", &format!("{:02}", d.day)),
                None if ["{yyyy}", "{yy}", "{mm}", "{dd}"].iter().any(|t| part.contains(t)) => {
                    UNDATED_FOLDER.to_owned()
                }
                None => part.to_owned(),
            };
            let clean: String = text
                .chars()
                .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') && !c.is_control())
                .collect();
            let clean = clean.trim().trim_end_matches('.');
            if clean.is_empty() || clean == "." || clean == ".." {
                continue;
            }
            // An undated photo gets one "Undated" folder, not one per dated part.
            if day.is_none() && clean == UNDATED_FOLDER && out.ends_with(UNDATED_FOLDER) {
                continue;
            }
            out.push(clean);
        }
        out
    }
}

/// What an import does with the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// The photos are added where they are (referenced).
    InPlace,
    /// The files are copied below `folder`, in the sub-folders of
    /// `pattern`; the copies are what the catalog holds.
    Copy {
        /// The chosen folder.
        folder: PathBuf,
        /// Sub-folders below it.
        pattern: FolderPattern,
    },
}

/// Where to copy `name` in `dir` without replacing a different file:
/// `name` itself when free, else "IMG_1 (2).CR3", "IMG_1 (3).CR3"…
/// `taken` says whether a path is already used.
#[must_use]
pub fn free_name(dir: &Path, name: &str, mut taken: impl FnMut(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    (2..)
        .map(|n| {
            let candidate = match ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            };
            dir.join(candidate)
        })
        .find(|p| !taken(p))
        .unwrap_or(first)
}

/// What an import gives to each photo it adds, as Capture One's "Metadata"
/// of the import window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportMetadata {
    /// Photographer, when given.
    pub artist: Option<String>,
    /// Copyright notice, when given.
    pub copyright: Option<String>,
    /// Keywords added to every photo.
    pub keywords: Vec<String>,
    /// Rating given to every photo (0: none).
    pub rating: u8,
}

impl ImportMetadata {
    /// The keywords of `text`, separated by commas or semicolons, blanks
    /// and repeats left out.
    #[must_use]
    pub fn parse_keywords(text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for k in text.split([',', ';']).map(str::trim).filter(|k| !k.is_empty()) {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(k)) {
                out.push(k.to_owned());
            }
        }
        out
    }

    /// Nothing to give.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.artist.is_none() && self.copyright.is_none() && self.keywords.is_empty() && self.rating == 0
    }
}

/// A photo an import added: where the catalog finds it and when it was
/// taken (for later duplicate checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedPhoto {
    /// The file in the catalog (the copy, when the import copies).
    pub path: PathBuf,
    /// The camera's capture time, when the file has one.
    pub captured: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_shot_is_known_by_name_and_capture_time_wherever_it_is() {
        let a = ShotKey::new(Path::new("card/DCIM/IMG_0001.CR3"), Some("2026:10:29 10:00:00"));
        let b = ShotKey::new(Path::new("D:/Photos/img_0001.cr3"), Some(" 2026:10:29 10:00:00"));
        assert_eq!(a, b);
        assert_ne!(a, ShotKey::new(Path::new("IMG_0001.CR3"), Some("2026:10:29 10:00:01")));
        assert_eq!(ShotKey::new(Path::new("IMG_0001.CR3"), None), None);
        assert_eq!(ShotKey::new(Path::new("IMG_0001.CR3"), Some("  ")), None);
    }

    #[test]
    fn capture_days_come_from_exif_or_from_the_file_time() {
        assert_eq!(Day::from_exif("2026:08:14 18:42:07"), Some(Day { year: 2026, month: 8, day: 14 }));
        assert_eq!(Day::from_exif("2026-08-14T18:42"), Some(Day { year: 2026, month: 8, day: 14 }));
        assert_eq!(Day::from_exif("0000:00:00 00:00:00"), None);
        assert_eq!(Day::from_exif("bad"), None);
        assert_eq!(Day::from_unix(0), Day { year: 1970, month: 1, day: 1 });
        // 2026-10-29 10:00 UTC.
        assert_eq!(Day::from_unix(1_793_268_000), Day { year: 2026, month: 10, day: 29 });
        // 2024-02-29, a leap day.
        assert_eq!(Day::from_unix(1_709_164_800), Day { year: 2024, month: 2, day: 29 });
    }

    #[test]
    fn the_pattern_makes_dated_folders_that_stay_below_the_chosen_one() {
        let day = Some(Day { year: 2026, month: 3, day: 7 });
        assert_eq!(FolderPattern::default().folder_for(day), Path::new("2026").join("2026-03-07"));
        assert_eq!(FolderPattern::new("Trip {yy}\\{mm}").folder_for(day), Path::new("Trip 26").join("03"));
        assert_eq!(FolderPattern::new("../{yyyy}/./x:y").folder_for(day), Path::new("2026").join("xy"));
        assert_eq!(FolderPattern::default().folder_for(None), Path::new(UNDATED_FOLDER));
        assert_eq!(FolderPattern::new("Wedding/{yyyy}").folder_for(None), Path::new("Wedding").join(UNDATED_FOLDER));
        assert_eq!(FolderPattern::new("").folder_for(day), PathBuf::new());
        assert!(FolderPattern::default().is_dated() && !FolderPattern::new("Wedding").is_dated());
    }

    #[test]
    fn a_copy_never_replaces_another_file() {
        let dir = Path::new("out");
        let taken = |p: &Path| p == dir.join("a.NEF") || p == dir.join("a (2).NEF");
        assert_eq!(free_name(dir, "b.NEF", taken), dir.join("b.NEF"));
        assert_eq!(free_name(dir, "a.NEF", taken), dir.join("a (3).NEF"));
        assert_eq!(free_name(dir, "README", |p| p == dir.join("README")), dir.join("README (2)"));
    }

    #[test]
    fn keywords_are_split_trimmed_and_unique() {
        assert_eq!(ImportMetadata::parse_keywords(" sea, Sea ;boat,, "), ["sea", "boat"]);
        assert!(ImportMetadata::default().is_empty());
        assert!(!ImportMetadata { rating: 3, ..ImportMetadata::default() }.is_empty());
    }
}
