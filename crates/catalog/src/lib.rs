//! The photo catalog: what the editor remembers about each photo — rating,
//! colour label and keywords (suggested by the AI or added by the user),
//! and its edits: one stored edit per variant, with a small preview of the
//! edited image — and the Explorer's filters over them.
//!
//! [`model`] is pure (filters, keyword counts, variant numbers) and always
//! built; the SQLite store ([`Catalog`]) comes with the default `sqlite`
//! feature. The edits are stored as text written by the `recipe-store`
//! crate; the catalog does not read them.

pub mod import;
pub mod model;
#[cfg(feature = "sqlite")]
mod store;

pub use import::{
    Day, Destination, FolderPattern, ImportMetadata, ImportedPhoto, ShotKey, free_name, without_jpeg_twins,
};
pub use model::{
    Album, Collection, ColorLabel, FIRST_VARIANT, Filter, FolderNode, ImportBatch, LabelSet, MAX_RATING, PhotoMeta,
    RECENT_EDIT_DAYS, RECENT_IMPORTS, RatingSet, StoredVariant, Tag, TagSource, VariantInfo, folder_tree,
    next_variant_number, sort_by_name, tag_counts, variant_name,
};
#[cfg(feature = "sqlite")]
pub use store::{Catalog, CatalogError};

/// File name of the database, inside a catalog's folder (and, before
/// catalogs, in the editor's data folder).
pub const FILE_NAME: &str = "catalog.sqlite";

/// Extension of a catalog's folder ("Travel.rnecatalog").
pub const CATALOG_EXTENSION: &str = "rnecatalog";
