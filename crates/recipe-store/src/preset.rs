//! Preset files: a style (a chosen part of a photo's edits) kept in a file
//! of its own, in a folder the user chooses, to apply to other photos; or a
//! style brush, the same with the brush it paints with.
//!
//! There is no universal format for presets: Lightroom and Camera Raw keep
//! theirs as XMP (`crs:` settings), Capture One as `.costyle` and
//! `.costylebrush`, each tied to its own tools. Ours are versioned JSON,
//! like the stored edits, with the extension [`PRESET_EXTENSION`] or
//! [`BRUSH_EXTENSION`].

use core_model::Style;

use crate::json::{self, Value};
use crate::{DecodeError, MAX_TEXT_BYTES, codec};

/// Extension of a preset file (a style applied to the whole photo).
pub const PRESET_EXTENSION: &str = "aktispreset";

/// Extension of a style brush file.
pub const BRUSH_EXTENSION: &str = "aktisbrush";

/// Version of the preset format written by this build; a newer one is refused.
pub const PRESET_FORMAT: u32 = 1;

/// How a style brush paints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    /// Radius, as a fraction of the image's long side.
    pub radius: f32,
    /// Share of the radius painted at full strength, 0–1.
    pub hardness: f32,
    /// Strength of a stroke, 0–1.
    pub opacity: f32,
    /// Strokes follow the photo's edges.
    pub edge_aware: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        Self { radius: 0.03, hardness: 0.5, opacity: 1.0, edge_aware: false }
    }
}

/// A preset or style brush file.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// Name shown in menus.
    pub name: String,
    /// What it applies.
    pub style: Style,
    /// `Some` for a style brush: how it paints.
    pub brush: Option<BrushSettings>,
}

impl Preset {
    /// The file extension for this preset ([`PRESET_EXTENSION`] or [`BRUSH_EXTENSION`]).
    #[must_use]
    pub fn extension(&self) -> &'static str {
        if self.brush.is_some() { BRUSH_EXTENSION } else { PRESET_EXTENSION }
    }
}

/// The preset as text, in format [`PRESET_FORMAT`].
#[must_use]
pub fn encode_preset(preset: &Preset) -> String {
    let brush = preset.brush.map_or(Value::Null, |b| {
        Value::object([
            ("radius", Value::f32(b.radius)),
            ("hardness", Value::f32(b.hardness)),
            ("opacity", Value::f32(b.opacity)),
            ("edge_aware", Value::Bool(b.edge_aware)),
        ])
    });
    Value::object([
        ("preset_format", Value::uint(PRESET_FORMAT)),
        ("name", Value::text(&preset.name)),
        ("style", codec::style(&preset.style)),
        ("brush", brush),
    ])
    .to_text()
}

/// Reads a preset written by [`encode_preset`] of this or an older version.
///
/// # Errors
/// [`DecodeError::NewerVersion`] for a newer format, [`DecodeError::Malformed`]
/// when the text is not a preset.
pub fn decode_preset(text: &str) -> Result<Preset, DecodeError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(DecodeError::Malformed("the text is too large".to_owned()));
    }
    let value = json::parse(text).map_err(DecodeError::Malformed)?;
    read(&value).map_err(|e| match e {
        Read::Newer(v) => DecodeError::NewerVersion(v),
        Read::Bad(why) => DecodeError::Malformed(why),
    })
}

enum Read {
    Newer(u32),
    Bad(String),
}

impl From<String> for Read {
    fn from(why: String) -> Self {
        Self::Bad(why)
    }
}

fn read(value: &Value) -> Result<Preset, Read> {
    let f = value.as_object("preset")?;
    let format = json::required(f, "preset_format")?.as_u32("preset_format")?;
    if format == 0 {
        return Err(Read::Bad("preset format 0 does not exist".to_owned()));
    }
    if format > PRESET_FORMAT {
        return Err(Read::Newer(format));
    }
    let name = json::required(f, "name")?.as_str("name")?.trim().to_owned();
    let style = codec::read_style(json::required(f, "style")?).map_err(|e| format!("style: {e}"))?;
    let brush = match json::field(f, "brush") {
        None | Some(Value::Null) => None,
        Some(b) => {
            let bf = b.as_object("brush")?;
            let num = |key: &str, max: f32| -> Result<f32, String> {
                let v = json::required(bf, key)?.as_f32(key)?;
                if v.is_finite() { Ok(v.clamp(0.0, max)) } else { Err(format!("{key} is not a number")) }
            };
            Some(BrushSettings {
                radius: num("radius", core_model::MAX_BRUSH_RADIUS)?.max(0.001),
                hardness: num("hardness", 1.0)?,
                opacity: num("opacity", 1.0)?,
                edge_aware: json::required(bf, "edge_aware")?.as_bool("edge_aware")?,
            })
        }
    };
    Ok(Preset { name, style, brush })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use core_model::{Geometry, ParamId, ParamValue, Params, ProfileChoice, StyleTool, ToolId, WhiteBalance};

    use super::*;

    fn sample(brush: bool) -> Preset {
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(-0.333_333_34));
        p.set(ParamId::new("method").unwrap(), ParamValue::Choice(2));
        p.set(ParamId::new("enable").unwrap(), ParamValue::Bool(true));
        let style = Style {
            white_balance: Some(WhiteBalance::Multipliers([2.0, 1.0, 1.5])),
            profile: Some(ProfileChoice::Named("Portrait".into())),
            lens: None,
            geometry: Some(Geometry { angle: 2.5, ..Geometry::default() }),
            tools: vec![
                StyleTool { tool: ToolId::new("exposure").unwrap(), params: p },
                StyleTool { tool: ToolId::new("clarity").unwrap(), params: Params::new() },
            ],
        };
        let brush = brush.then_some(BrushSettings { radius: 0.05, hardness: 0.25, opacity: 0.6, edge_aware: true });
        Preset { name: "Warm \u{b7} soft".into(), style, brush }
    }

    #[test]
    fn the_fuzzers_start_from_a_preset_of_the_current_format() {
        mutate::check_seed("recipe", "preset.json", encode_preset(&sample(true)).as_bytes());
    }

    #[test]
    fn damaged_presets_never_panic_and_what_is_accepted_round_trips() {
        for copy in mutate::text_copies(&encode_preset(&sample(true)), 42, 3000) {
            if let Ok(p) = decode_preset(&copy) {
                let written = encode_preset(&p);
                assert_eq!(decode_preset(&written).map(|again| encode_preset(&again)), Ok(written), "accepted: {copy}");
            }
        }
    }

    #[test]
    fn presets_and_brushes_round_trip() {
        for b in [false, true] {
            let p = sample(b);
            let text = encode_preset(&p);
            assert!(text.starts_with("{\"preset_format\":1,"));
            assert_eq!(decode_preset(&text), Ok(p.clone()));
            assert_eq!(p.extension(), if b { BRUSH_EXTENSION } else { PRESET_EXTENSION });
        }
    }

    #[test]
    fn newer_or_broken_presets_are_refused() {
        let text = encode_preset(&sample(true));
        let newer = text.replacen("\"preset_format\":1", "\"preset_format\":4", 1);
        assert_eq!(decode_preset(&newer), Err(DecodeError::NewerVersion(4)));
        for bad in
            [String::new(), "{}".into(), text.replacen("\"tools\"", "\"toolz\"", 1), text[..text.len() - 2].into()]
        {
            assert!(matches!(decode_preset(&bad), Err(DecodeError::Malformed(_))), "{bad}");
        }
    }

    #[test]
    fn presets_saved_before_the_layers_still_open() {
        let p = sample(false);
        let old = encode_preset(&p).replacen("\"tools\"", "\"nodes\"", 1);
        assert_eq!(decode_preset(&old), Ok(p));
    }

    #[test]
    fn brush_values_are_kept_in_range() {
        let text = encode_preset(&sample(true)).replacen("\"opacity\":0.6", "\"opacity\":7", 1);
        let p = decode_preset(&text).unwrap();
        assert_eq!(p.brush.map(|b| b.opacity), Some(1.0));
    }
}
