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
//! - [`graph_layout`] and [`graph_view`]: where the node graph's nodes are
//!   drawn, moved and hit;
//! - [`overview`]: the viewer's grid when several photos are selected;
//! - [`thumbs`]: the filmstrip's thumbnail cache;
//! - [`variants`]: saving edits in the catalog and reopening them, per variant;
//! - [`history_order`]: History-tab gestures as step orders;
//! - [`histogram`]: levels of the displayed image.
//!
//! The `app` crate draws these with egui and turns gestures into calls here.

pub mod adjust;
pub mod document;
pub mod graph_layout;
pub mod graph_view;
pub mod histogram;
pub mod history_order;
pub mod inspect;
pub mod lenses;
pub mod library;
pub mod overview;
pub mod thumbs;
pub mod variants;
pub mod view;
pub mod worker;

pub use document::{DevelopedImage, Document, NodeRow, Opened, ParamRow, RawData};
pub use graph_layout::{NodePlace, layout};
pub use graph_view::GraphView;
pub use histogram::Histogram;
pub use library::{
    Filmstrip, FolderListing, FolderNode, FolderTree, RenameError, StripPhoto, file_stem, rename_target,
};
pub use overview::OverviewLayout;
pub use thumbs::{ThumbCache, ThumbState};
pub use view::{Rect, ViewState, Viewport, Zoom, needs_full_resolution};
pub use worker::{Event, Quality, RenderPlan, RenderRequest, RenderTiming, ThumbJob, Worker};
