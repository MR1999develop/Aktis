//! Starts the editor window. A RAW file given on the command line is opened.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release builds
#![allow(
    linker_messages,
    reason = "a C dependency of the tagging tokenizer exports functions, so the MSVC linker reports the .lib/.exp it writes"
)]

use std::path::PathBuf;

fn main() -> eframe::Result {
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    // Before eframe reads the window settings from the settings folder.
    let problems = platform::move_legacy_dirs();
    eframe::run_native(
        app::APP_ID,
        app::native_options(),
        Box::new(move |cc| {
            let mut app = app::EditorApp::new(cc, open);
            app.report_startup_problems(&problems);
            Ok(Box::new(app))
        }),
    )
}
