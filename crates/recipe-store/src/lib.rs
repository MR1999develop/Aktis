//! The stored form of a photo's edit: one variant's recipe, its history
//! and its name, as versioned text.
//!
//! The editor keeps edits only in its catalog (no sidecar files); the
//! catalog stores the text written here, one row per variant. The text is
//! JSON written and read by hand (the workspace has no serde outside the
//! window), with a `format` number: a file written by a newer editor is
//! refused with [`DecodeError::NewerVersion`], so the window opens the photo
//! read-only instead of saving an older format over it. Edits of format 1
//! (before the layers) are not read: [`DecodeError::Obsolete`], and the
//! photo starts again from its first settings.
//!
//! Pure: no I/O, every type round-trips exactly (floats included).

mod camera_defaults;
mod codec;
mod custom_lens;
mod json;
mod preset;

use std::fmt;

pub use camera_defaults::{
    CAMERA_DEFAULTS_EXTENSION, CAMERA_DEFAULTS_FORMAT, decode_camera_defaults, encode_camera_defaults,
};
pub use custom_lens::{CUSTOM_LENS_EXTENSION, CUSTOM_LENS_FORMAT, decode_custom_lens, encode_custom_lens};
pub use preset::{
    BRUSH_EXTENSION, BrushSettings, PRESET_EXTENSION, PRESET_FORMAT, Preset, decode_preset, encode_preset,
};

use commands::SavedHistory;
use core_model::Recipe;

/// Version of the text format written by this build. Raised whenever a
/// field or a kind of layer, mask or edit is added, so older editors know
/// the text is newer than they are.
pub const FORMAT_VERSION: u32 = 2;

/// The oldest format this build reads: format 1 held a node graph, which
/// the layers replaced.
pub const OLDEST_FORMAT: u32 = 2;

/// Largest text accepted (a recipe with many brush strokes is still far smaller).
pub const MAX_TEXT_BYTES: usize = 64 << 20;

/// Everything stored for one variant of a photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// Name shown in menus ("Variant 2").
    pub name: String,
    /// The edit.
    pub recipe: Recipe,
    /// Undo and redo; `None` when not kept.
    pub history: Option<SavedHistory>,
}

/// Why a stored text could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Written by a newer editor, in this format version.
    NewerVersion(u32),
    /// Written in a format this build no longer reads (the node graph of
    /// format 1).
    Obsolete(u32),
    /// Not valid; the text says what is wrong.
    Malformed(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewerVersion(v) => write!(
                f,
                "the edit was saved by a newer version of the editor (format {v}; this version reads up to {FORMAT_VERSION})"
            ),
            Self::Obsolete(v) => write!(f, "the edit was saved in format {v}, which this version no longer reads"),
            Self::Malformed(why) => write!(f, "the saved edit cannot be read: {why}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// The variant as text, in format [`FORMAT_VERSION`].
#[must_use]
pub fn encode(variant: &Variant) -> String {
    json::Value::object([
        ("format", json::Value::uint(FORMAT_VERSION)),
        ("name", json::Value::text(&variant.name)),
        ("recipe", codec::recipe(&variant.recipe)),
        ("history", variant.history.as_ref().map_or(json::Value::Null, codec::history)),
    ])
    .to_text()
}

/// Parses the text and checks its format number.
fn parse_checked(text: &str) -> Result<(u32, json::Value), DecodeError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(DecodeError::Malformed("the text is too large".to_owned()));
    }
    let value = json::parse(text).map_err(DecodeError::Malformed)?;
    let format = {
        let fields = value.as_object("edit").map_err(DecodeError::Malformed)?;
        json::required(fields, "format").and_then(|v| v.as_u32("format")).map_err(DecodeError::Malformed)?
    };
    if format == 0 {
        return Err(DecodeError::Malformed("format 0 does not exist".to_owned()));
    }
    Ok((format, value))
}

/// Reads a variant written by [`encode`] of this or an older version.
///
/// # Errors
/// [`DecodeError::NewerVersion`] for a newer format, [`DecodeError::Obsolete`]
/// for one older than [`OLDEST_FORMAT`], [`DecodeError::Malformed`] when the
/// text is not a valid edit.
pub fn decode(text: &str) -> Result<Variant, DecodeError> {
    let (format, value) = parse_checked(text)?;
    if format > FORMAT_VERSION {
        return Err(DecodeError::NewerVersion(format));
    }
    if format < OLDEST_FORMAT {
        return Err(DecodeError::Obsolete(format));
    }
    read_variant(&value, true).map_err(DecodeError::Malformed)
}

/// The format number of a stored text, without reading the rest.
#[must_use]
pub fn format_of(text: &str) -> Option<u32> {
    parse_checked(text).ok().map(|(f, _)| f)
}

/// Best effort for a text of a newer format: its name and recipe, read with
/// the rules of this version (the history is left out). `None` when the
/// recipe uses something this version does not know, or the format is
/// older than [`OLDEST_FORMAT`].
#[must_use]
pub fn decode_recipe_lenient(text: &str) -> Option<(String, Recipe)> {
    let (format, value) = parse_checked(text).ok()?;
    if format < OLDEST_FORMAT {
        return None;
    }
    read_variant(&value, false).ok().map(|v| (v.name, v.recipe))
}

fn read_variant(value: &json::Value, everything: bool) -> Result<Variant, String> {
    let fields = value.as_object("edit")?;
    let name = json::required(fields, "name")?.as_str("name")?.to_owned();
    let recipe = codec::read_recipe(json::required(fields, "recipe")?).map_err(|e| format!("recipe: {e}"))?;
    if !everything {
        return Ok(Variant { name, recipe, history: None });
    }
    let present = |key: &str| json::field(fields, key).filter(|v| **v != json::Value::Null);
    let history = present("history").map(codec::read_history).transpose().map_err(|e| format!("history: {e}"))?;
    Ok(Variant { name, recipe, history })
}

#[cfg(test)]
mod tests;
