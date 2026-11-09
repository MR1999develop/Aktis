//! Filters that no photo passes leave the viewer empty: opens the first RAW
//! file in `testdata/`, types a name no photo has into the Explorer's name
//! filter, and the photo closes; clearing the filter shows the photos again.
//!
//! It opens the window with the editor's settings and catalog, so run it
//! with folders of its own:
//!
//! ```text
//! set APPDATA=%TEMP%\aktis-test\roaming
//! set LOCALAPPDATA=%TEMP%\aktis-test\local
//! set AKTIS_DATA_DIR=%TEMP%\aktis-test\data
//! cargo test -p app --release --test filtered_out -- --ignored
//! ```
#![allow(
    linker_messages,
    reason = "a C dependency of the tagging tokenizer exports functions, so the MSVC linker reports the .lib/.exp it writes"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn first_raw_file() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| raw_io::has_raw_extension(p))
        .collect();
    files.sort();
    files.into_iter().next()
}

/// Steps the window until `done` holds, for at most two minutes.
fn step_until(harness: &mut Harness<'_, app::EditorApp>, done: impl Fn(&app::Status) -> bool) -> app::Status {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(120) {
        harness.step();
        let status = harness.state().status();
        if done(&status) {
            return status;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    harness.state().status()
}

#[test]
#[ignore = "needs a RAW file in testdata/, a GPU and settings folders of its own (see the top of the file)"]
fn a_filter_no_photo_passes_closes_the_open_photo() {
    let raw = first_raw_file().expect("put a RAW file in testdata/");
    let mut harness =
        Harness::builder().with_size([1600.0, 960.0]).wgpu().build_eframe(|cc| app::EditorApp::new(cc, Some(raw)));

    let status = step_until(&mut harness, |s| s.has_document && !s.busy);
    assert!(status.has_document, "the photo did not open: {status:?}");

    // The Explorer's name filter, by the hint it shows when empty.
    let name_filter = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.role() == eframe::egui::Role::TextInput && n.placeholder() == Some("Filter by name")
    };
    harness.get_by(name_filter).focus();
    harness.step();
    harness.get_by(name_filter).type_text("no photo is called this");
    let status = step_until(&mut harness, |s| !s.has_document);
    assert!(!status.has_document, "the photo stayed open: {status:?}");
    // The photo closes after a frame is drawn: the empty viewer comes in the next ones.
    harness.run_steps(2);

    harness.get_by_label("Clear Filters").click();
    harness.run_steps(3);
    assert!(harness.query_by_label("No photos match the filters").is_none(), "the filters are still on");
}
