//! Keyword lists (Lightroom's text format), the keywords typed at import,
//! and the folder pattern of the import, which must stay below the folder
//! the user chose.

#![no_main]

use std::path::Component;

use catalog::{Day, FolderPattern, ImportMetadata};
use libfuzzer_sys::fuzz_target;
use tagging::Vocabulary;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    if let Ok(v) = Vocabulary::parse_lightroom(text) {
        let written = v.to_lightroom();
        let again = Vocabulary::parse_lightroom(&written).expect("own output reads back");
        assert_eq!(again.to_lightroom(), written);
    }
    let _ = ImportMetadata::parse_keywords(text);
    let folder = FolderPattern::new(text).folder_for(Some(Day { year: 2026, month: 3, day: 7 }));
    assert!(folder.components().all(|c| matches!(c, Component::Normal(_))), "{}", folder.display());
});
