//! Moving through the filmstrip with the arrow keys while photos are still
//! developing: the window must stay responsive (no long frames) and the
//! last photo must reach the screen soon. Drops `testdata/` on the window
//! (its RAW files are imported, the first opened) and presses → five
//! times, one every 150 ms.
//!
//! It opens the window with the editor's settings and catalog, so run it
//! with folders of its own:
//!
//! ```text
//! set APPDATA=%TEMP%\aktis-test\roaming
//! set LOCALAPPDATA=%TEMP%\aktis-test\local
//! set AKTIS_DATA_DIR=%TEMP%\aktis-test\data
//! cargo test -p app --release --test photo_switching -- --ignored --nocapture
//! ```
#![allow(
    linker_messages,
    reason = "a C dependency of the tagging tokenizer exports functions, so the MSVC linker reports the .lib/.exp it writes"
)]

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

/// A folder dropped on the window, as the system hands it over.
#[derive(Debug)]
struct Dropped(PathBuf);

impl eframe::egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("a folder".to_owned())
    }
}

/// One frame, and how long the window's thread spent on it.
fn timed_step(harness: &mut Harness<'_, app::EditorApp>, frames: &mut Vec<Duration>) {
    let started = Instant::now();
    harness.step();
    frames.push(started.elapsed());
}

/// Steps until `done` holds, for at most two minutes; the time it took.
fn step_until(
    harness: &mut Harness<'_, app::EditorApp>,
    frames: &mut Vec<Duration>,
    done: impl Fn(&app::Status) -> bool,
) -> Duration {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(120) {
        timed_step(harness, frames);
        if done(&harness.state().status()) {
            return started.elapsed();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("not done in two minutes: {:?}", harness.state().status());
}

#[test]
#[ignore = "needs RAW files in testdata/, a GPU and settings folders of its own (see the top of the file)"]
fn moving_through_photos_keeps_the_window_responsive() {
    let folder = first_raw_file().and_then(|p| p.parent().map(Path::to_owned)).expect("put RAW files in testdata/");
    let mut harness =
        Harness::builder().with_size([1600.0, 960.0]).wgpu().build_eframe(|cc| app::EditorApp::new(cc, None));
    // The folder dropped on the window: its photos are imported, the first opened.
    harness.step();
    harness.input_mut().dropped_files.push(std::sync::Arc::new(Dropped(folder)));
    let mut frames = Vec::new();
    let settled = |s: &app::Status| s.has_document && s.image_current && !s.busy;
    let first = step_until(&mut harness, &mut frames, settled);
    // Let the first photo's full render and the thumbnails get going.
    for _ in 0..20 {
        timed_step(&mut harness, &mut frames);
        std::thread::sleep(Duration::from_millis(10));
    }
    frames.clear();
    let opens = harness.state().status().opens;

    let started = Instant::now();
    for _ in 0..5 {
        harness.key_press(eframe::egui::Key::ArrowRight);
        let pressed = Instant::now();
        while pressed.elapsed() < Duration::from_millis(150) {
            timed_step(&mut harness, &mut frames);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    step_until(&mut harness, &mut frames, settled);
    let last = started.elapsed();
    let moved = harness.state().status().opens - opens;
    assert_eq!(moved, 5, "each arrow opens the next photo");
    frames.sort();
    let n = frames.len();
    let pct = |p: usize| frames[(n * p / 100).min(n - 1)].as_secs_f32() * 1000.0;
    eprintln!(
        "first photo {:.2} s; five → and the last on screen {:.2} s; frames {n}: p50 {:.0} ms, p99 {:.0} ms, max {:.0} ms",
        first.as_secs_f32(),
        last.as_secs_f32(),
        pct(50),
        pct(99),
        frames[n - 1].as_secs_f32() * 1000.0
    );
    // Before the arrows went on from the photo being opened and the worker
    // opened one photo at a time: 3.35 s and frames of 84 ms, for a first
    // photo of 1.6 s; after: 2.0 s and 30 ms.
    assert!(last < first + Duration::from_millis(1500), "the last photo waited for the ones passed over");
    assert!(frames[n - 1] < Duration::from_millis(100), "a frame took {:?}", frames[n - 1]);
}
