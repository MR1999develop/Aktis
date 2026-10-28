//! Names and folders of exported files: a pattern with tokens such as
//! `{name}` and `{counter}`, and where the files go.

use std::path::{Path, PathBuf};

/// The tokens a name pattern understands, with what they stand for.
pub const NAME_TOKENS: [(&str, &str); 4] = [
    ("{name}", "original file name"),
    ("{counter}", "sequence number"),
    ("{date}", "capture date"),
    ("{camera}", "camera"),
];

/// How exported files are named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePattern {
    /// Free text with tokens, such as `{date}_{name}`.
    pub pattern: String,
    /// The `{counter}` of the first photo.
    pub counter_start: u32,
    /// Digits of `{counter}`, padded with zeros (1–9).
    pub counter_digits: u8,
}

impl Default for NamePattern {
    fn default() -> Self {
        Self { pattern: "{name}".into(), counter_start: 1, counter_digits: 3 }
    }
}

/// What the tokens of one photo are replaced with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameFields<'a> {
    /// The original file name without its extension.
    pub stem: &'a str,
    /// Position of the photo in the export, from 0.
    pub index: u32,
    /// Capture date as cameras write it ("2026:08:14 18:42:07").
    pub captured: Option<&'a str>,
    /// Camera ("Nikon D3S").
    pub camera: Option<&'a str>,
}

/// "2026-08-14" from a camera date, when it starts with one.
fn date_part(captured: &str) -> Option<String> {
    let d = captured.trim().get(..10)?;
    let ok = d.char_indices().all(|(i, c)| if i == 4 || i == 7 { matches!(c, ':' | '-') } else { c.is_ascii_digit() });
    ok.then(|| d.replace(':', "-"))
}

/// A file name stem the file systems of Windows, macOS and Linux all
/// accept: no `< > : " / \ | ? *` or control characters (replaced by `_`),
/// no trailing dots or spaces, not a reserved device name, at most 200
/// characters. Empty when nothing usable is left.
#[must_use]
pub fn sanitize_stem(s: &str) -> String {
    let cleaned: String =
        s.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).take(200).collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']).trim_start();
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    let upper = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = reserved.contains(&upper.as_str())
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    if device { format!("_{trimmed}") } else { trimmed.to_owned() }
}

impl NamePattern {
    /// The file stem for one photo: tokens replaced, unknown tokens left as
    /// typed, made safe for every file system; the original stem when the
    /// result is empty.
    #[must_use]
    pub fn stem(&self, fields: &NameFields<'_>) -> String {
        let digits = usize::from(self.counter_digits.clamp(1, 9));
        let counter = u64::from(self.counter_start) + u64::from(fields.index);
        let date = fields.captured.and_then(date_part).unwrap_or_default();
        let text = self
            .pattern
            .replace("{name}", fields.stem)
            .replace("{counter}", &format!("{counter:0digits$}"))
            .replace("{date}", &date)
            .replace("{camera}", fields.camera.unwrap_or_default().trim());
        let safe = sanitize_stem(&text);
        if safe.is_empty() {
            let original = sanitize_stem(fields.stem);
            if original.is_empty() { "image".into() } else { original }
        } else {
            safe
        }
    }
}

/// Where exported files go.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Destination {
    /// The folder of the original.
    #[default]
    NextToOriginal,
    /// A folder of this name inside the original's folder.
    Subfolder(String),
    /// This folder.
    Folder(PathBuf),
}

impl Destination {
    /// The folder the export of `original` goes to (it may not exist yet).
    #[must_use]
    pub fn folder_for(&self, original: &Path) -> PathBuf {
        let parent = original.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        match self {
            Self::NextToOriginal => parent.to_owned(),
            Self::Subfolder(name) => {
                let name = sanitize_stem(name);
                if name.is_empty() { parent.to_owned() } else { parent.join(name) }
            }
            Self::Folder(dir) => dir.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> NameFields<'static> {
        NameFields { stem: "DSC_0042", index: 4, captured: Some("2026:08:14 18:42:07"), camera: Some("Nikon D3S") }
    }

    #[test]
    fn tokens_are_replaced() {
        let p =
            NamePattern { pattern: "{date}_{camera}_{name}-{counter}".into(), counter_start: 10, counter_digits: 4 };
        assert_eq!(p.stem(&fields()), "2026-08-14_Nikon D3S_DSC_0042-0014");
        assert_eq!(NamePattern::default().stem(&fields()), "DSC_0042");
        let free = NamePattern { pattern: "Holiday {counter}".into(), ..NamePattern::default() };
        assert_eq!(free.stem(&fields()), "Holiday 005");
    }

    #[test]
    fn missing_values_and_unsafe_characters_give_usable_names() {
        let f = NameFields { captured: None, camera: None, ..fields() };
        let p = NamePattern { pattern: "{date}{camera}".into(), ..NamePattern::default() };
        assert_eq!(p.stem(&f), "DSC_0042", "empty result falls back to the original name");
        let p = NamePattern { pattern: "a/b:c*{unknown}?. ".into(), ..NamePattern::default() };
        assert_eq!(p.stem(&fields()), "a_b_c_{unknown}_");
        assert_eq!(sanitize_stem("con"), "_con");
        assert_eq!(sanitize_stem("COM1.backup"), "_COM1.backup");
        assert_eq!(sanitize_stem("Console"), "Console");
        assert_eq!(sanitize_stem(" . "), "");
    }

    #[test]
    fn dates_that_are_not_dates_are_left_out() {
        assert_eq!(date_part("2026:08:14 18:42:07").as_deref(), Some("2026-08-14"));
        assert_eq!(date_part("2026-08-14").as_deref(), Some("2026-08-14"));
        assert_eq!(date_part("14/08/2026 18:42"), None);
        assert_eq!(date_part("short"), None);
    }

    #[test]
    fn destinations_resolve_against_the_original() {
        let raw = Path::new("shots").join("a.NEF");
        assert_eq!(Destination::NextToOriginal.folder_for(&raw), Path::new("shots"));
        assert_eq!(Destination::Subfolder("export".into()).folder_for(&raw), Path::new("shots").join("export"));
        assert_eq!(Destination::Subfolder(" ".into()).folder_for(&raw), Path::new("shots"));
        assert_eq!(Destination::Subfolder("a/b".into()).folder_for(&raw), Path::new("shots").join("a_b"));
        assert_eq!(Destination::Folder("out".into()).folder_for(&raw), Path::new("out"));
        assert_eq!(Destination::NextToOriginal.folder_for(Path::new("a.NEF")), Path::new("."));
    }
}
