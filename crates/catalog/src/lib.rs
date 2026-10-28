//! The photo catalog: what the editor remembers about each photo — rating,
//! colour label and keywords (suggested by the AI or added by the user),
//! and its edits: one stored edit per variant, with a small preview of the
//! edited image — and the Explorer's filters over them.
//!
//! [`model`] is pure (filters, keyword counts, variant numbers) and always
//! built; the SQLite store ([`Catalog`]) comes with the default `sqlite`
//! feature. The edits are stored as text written by the `recipe-store`
//! crate; the catalog does not read them.

pub mod model;
#[cfg(feature = "sqlite")]
mod store;

pub use model::{
    ColorLabel, FIRST_VARIANT, Filter, LabelSet, MAX_RATING, PhotoMeta, RatingSet, StoredVariant, Tag, TagSource,
    VariantInfo, next_variant_number, tag_counts, variant_name,
};
#[cfg(feature = "sqlite")]
pub use store::{Catalog, CatalogError};

/// File name of the catalog in the editor's data folder.
pub const FILE_NAME: &str = "catalog.sqlite";
