//! The desktop window of Aktis, built with egui.
//!
//! This crate only draws and turns gestures into calls: documents, edits,
//! background jobs, the folder tree, the filmstrip model and viewer geometry
//! live in the `editor` crate, ratings and keywords in `catalog`; both are
//! tested without a display.
// Only the test build of the library is linked (the window itself is main.rs).
#![cfg_attr(
    test,
    allow(
        linker_messages,
        reason = "a C dependency of the tagging tokenizer exports functions, so the MSVC linker reports the .lib/.exp it writes"
    )
)]

mod actions;
mod adjust_ui;
mod ai_mask_job;
mod app;
mod catalog_ui;
mod color_ui;
mod dialog;
mod export_ui;
mod geometry_ui;
mod graph_ui;
mod history_ui;
mod icons;
mod import_window;
mod inspector;
mod library_ui;
mod mask_ui;
mod models_ui;
mod photo_texture;
mod prefs_ui;
mod presets_ui;
mod retouch_ui;
mod scopes;
mod settings;
mod tagging_job;
mod theme;
mod toolbar;
mod zoom_bar;

pub use app::{EditorApp, Status};

/// Name shown in the title bar and the About text.
pub const APP_NAME: &str = "Aktis";

/// Application id (Wayland app id, settings folder name).
pub const APP_ID: &str = "aktis";

/// Version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Window options: size, minimum size, drag and drop, remembered position.
#[must_use]
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id(APP_ID)
            .with_inner_size([1600.0, 960.0])
            .with_min_inner_size([1100.0, 640.0])
            .with_drag_and_drop(true),
        persist_window: true,
        ..Default::default()
    }
}
