//! Opens the first RAW file in `testdata/`, waits until the full-size image
//! is on screen, and saves a picture of the whole window to the folder in
//! `RNE_SCREENSHOT_DIR`. Run by CI to look at the window without a display:
//!
//! ```text
//! RNE_SCREENSHOT_DIR=out cargo test -p app --release --test screenshot -- --ignored
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;

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

#[test]
#[ignore = "needs a RAW file in testdata/ and a GPU (a software one is fine)"]
fn window_with_a_developed_raw_file() {
    let out = PathBuf::from(std::env::var_os("RNE_SCREENSHOT_DIR").expect("set RNE_SCREENSHOT_DIR"));
    let raw = first_raw_file().expect("put a RAW file in testdata/");

    let mut harness =
        Harness::builder().with_size([1280.0, 800.0]).wgpu().build_eframe(|cc| app::EditorApp::new(cc, Some(raw)));

    // The empty window first, while the file is opening.
    harness.step();
    let started = Instant::now();
    let mut settled_frames = 0;
    while started.elapsed() < Duration::from_secs(120) {
        harness.step();
        let s = harness.state().status();
        settled_frames = if s.has_document && s.image_current && !s.busy { settled_frames + 1 } else { 0 };
        // A few calm frames: the preview, then the full render, have arrived.
        if settled_frames > 40 {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let status = harness.state().status();
    assert!(status.has_document && status.image_current, "the image did not appear in time: {status:?}");

    std::fs::create_dir_all(&out).expect("create the screenshot folder");
    let image = harness.render().expect("render the window");
    image.save(out.join("window.png")).expect("save the screenshot");
}
