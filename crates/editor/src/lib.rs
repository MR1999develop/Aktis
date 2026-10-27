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
//! - [`graph_layout`]: where the node graph's nodes are drawn;
//! - [`thumbs`]: the filmstrip's thumbnail cache;
//! - [`history_order`]: History-tab gestures as step orders;
//! - [`histogram`]: levels of the displayed image.
//!
//! The `app` crate draws these with egui and turns gestures into calls here.

pub mod document;
pub mod graph_layout;
pub mod histogram;
pub mod history_order;
pub mod library;
pub mod thumbs;
pub mod view;
pub mod worker;

pub use document::{DevelopedImage, Document, NodeRow, Opened, ParamRow, RawData};
pub use graph_layout::{NodePlace, layout};
pub use histogram::Histogram;
pub use library::{
    Filmstrip, FolderListing, FolderNode, FolderTree, RenameError, StripPhoto, file_stem, rename_target,
};
pub use thumbs::{ThumbCache, ThumbState};
pub use view::{Rect, ViewState, Viewport, Zoom};
pub use worker::{Event, Quality, RenderPlan, RenderRequest, Worker};
