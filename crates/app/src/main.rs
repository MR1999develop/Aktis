//! Starts the editor window. A RAW file given on the command line is opened.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // no console window in release builds

use std::path::PathBuf;

fn main() -> eframe::Result {
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    eframe::run_native(
        app::APP_ID,
        app::native_options(),
        Box::new(move |cc| Ok(Box::new(app::EditorApp::new(cc, open)))),
    )
}
