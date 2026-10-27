//! The desktop window of the RAW node editor, built with egui.
//!
//! This crate only draws and turns gestures into calls: documents, edits,
//! background jobs and viewer geometry live in the `editor` crate, which is
//! tested without a display.

mod actions;
mod app;
mod settings;
mod theme;

pub use app::{EditorApp, Status};

/// Name shown in the title bar and the About text.
pub const APP_NAME: &str = "RAW Node Editor";

/// Application id (Wayland app id, settings folder name).
pub const APP_ID: &str = "raw-node-editor";

/// Version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Window options: size, minimum size, drag and drop, remembered position.
#[must_use]
pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id(APP_ID)
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 500.0])
            .with_drag_and_drop(true),
        persist_window: true,
        ..Default::default()
    }
}
