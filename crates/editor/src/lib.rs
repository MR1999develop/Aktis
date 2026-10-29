//! The editor without the window.
//!
//! Everything the desktop app does that does not need a screen lives here,
//! so it is tested on every CI runner without a display:
//!
//! - [`document`]: an open image, its recipe and undo history;
//! - [`worker`]: opening, previews and exports on background threads, and
//!   the plan that decides which preview to render next;
//! - [`export_job`]: exports with the export window's options, one photo
//!   or many, with progress and cancel;
//! - [`view`]: zoom and pan geometry of the viewer;
//! - [`library`]: the Explorer's folder tree and the filmstrip with its filters;
//! - [`graph_layout`] and [`graph_view`]: where the node graph's nodes are
//!   drawn, moved and hit;
//! - [`overview`]: the viewer's grid when several photos are selected;
//! - [`thumbs`]: the filmstrip's thumbnail cache;
//! - [`variants`]: saving edits in the catalog and reopening them, per variant;
//! - [`history_order`]: History-tab gestures as step orders;
//! - [`histogram`]: levels of the displayed image;
//! - [`custom_lenses`]: lens corrections saved under a name for a lens;
//! - [`catalogs`]: catalog folders, new catalogs and what an import adds;
//! - [`camera_defaults`]: tool values saved as the defaults of a camera model;
//! - [`retouch`]: where the spots of the Heal and Clone tools go;
//! - [`adjust`] and [`color_wheel`]: what the adjustment panels' handles,
//!   pickers and colour wheels set.
//!
//! The `app` crate draws these with egui and turns gestures into calls here.

pub mod adjust;
pub mod camera_defaults;
pub mod catalogs;
pub mod clipping;
pub mod color_wheel;
pub mod crop;
pub mod custom_lenses;
pub mod document;
pub mod export_job;
pub mod graph_layout;
pub mod graph_view;
pub mod histogram;
pub mod history_order;
pub mod import_job;
pub mod inspect;
pub mod lenses;
pub mod library;
pub mod masking;
pub mod overview;
pub mod presets;
pub mod retouch;
pub mod thumbs;
pub mod variants;
pub mod view;
pub mod worker;

pub use document::{DevelopedImage, Document, NodeRow, Opened, ParamRow, RawData};
pub use export_job::{ExportHandle, ExportJob, ExportPhoto, ExportSummary, OpenPhoto};
pub use graph_layout::{NodePlace, layout};
pub use graph_view::GraphView;
pub use histogram::Histogram;
pub use library::{
    Filmstrip, FolderListing, FolderNode, FolderTree, RenameError, StripPhoto, file_stem, rename_target,
};
pub use overview::OverviewLayout;
pub use thumbs::{ThumbCache, ThumbState};
pub use view::{Rect, ViewState, Viewport, Zoom, needs_full_resolution};
pub use worker::{Event, OpenTiming, Quality, RenderPlan, RenderRequest, RenderTiming, ThumbJob, Worker};
