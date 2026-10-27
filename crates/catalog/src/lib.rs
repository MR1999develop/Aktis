//! The photo catalog: what the editor remembers about each photo besides
//! its edits — rating, colour label and keywords (suggested by the AI or
//! added by the user) — and the Explorer's filters over them.
//!
//! [`model`] is pure (filters, keyword counts) and always built; the
//! SQLite store ([`Catalog`]) comes with the default `sqlite` feature.

pub mod model;
#[cfg(feature = "sqlite")]
mod store;

pub use model::{ColorLabel, Filter, LabelSet, MAX_RATING, PhotoMeta, RatingSet, Tag, TagSource, tag_counts};
#[cfg(feature = "sqlite")]
pub use store::{Catalog, CatalogError};

/// File name of the catalog in the editor's data folder.
pub const FILE_NAME: &str = "catalog.sqlite";
