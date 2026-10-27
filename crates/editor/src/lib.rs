//! The editor without the window.
//!
//! Everything the desktop app does that does not need a screen lives here,
//! so it is tested on every CI runner without a display:
//!
//! - [`document`]: an open image, its recipe and undo history;
//! - [`worker`]: opening, previews and exports on background threads, and
//!   the plan that decides which preview to render next;
//! - [`view`]: zoom and pan geometry of the viewer;
//! - [`library`]: the Explorer's folder tree and the filmstrip with its filters;
//! - [`histogram`]: levels of the displayed image.
//!
//! The `app` crate draws these with egui and turns gestures into calls here.

pub mod document;
pub mod histogram;
pub mod library;
pub mod view;
pub mod worker;

pub use document::{DevelopedImage, Document, NodeRow, Opened, ParamRow, RawData};
pub use histogram::Histogram;
pub use library::{Filmstrip, FolderListing, FolderNode, FolderTree, StripPhoto};
pub use view::{Rect, ViewState, Viewport, Zoom};
pub use worker::{Event, Quality, RenderPlan, RenderRequest, Worker};
