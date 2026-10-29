//! The tool defaults of one camera ([`CameraDefaults`]) as a small
//! versioned text file: the camera and the values of each tool.

use core_model::{CameraDefaults, Style};

use crate::json::{self, Value};
use crate::{DecodeError, MAX_TEXT_BYTES, codec};

/// File extension of camera defaults.
pub const CAMERA_DEFAULTS_EXTENSION: &str = "rnedefaults";

/// Version of the camera defaults format written by this build.
pub const CAMERA_DEFAULTS_FORMAT: u32 = 1;

/// The defaults as text, in format [`CAMERA_DEFAULTS_FORMAT`].
#[must_use]
pub fn encode_camera_defaults(defaults: &CameraDefaults) -> String {
    let style = Style { nodes: defaults.nodes.clone(), ..Style::default() };
    Value::object([
        ("camera_defaults_format", Value::uint(CAMERA_DEFAULTS_FORMAT)),
        ("camera", Value::text(&defaults.camera)),
        ("style", codec::style(&style)),
    ])
    .to_text()
}

/// Reads defaults written by [`encode_camera_defaults`] of this or an older
/// version; only the tool nodes of the stored style are kept.
///
/// # Errors
/// [`DecodeError::NewerVersion`] for a newer format, [`DecodeError::Malformed`]
/// when the text is not camera defaults or names no camera.
pub fn decode_camera_defaults(text: &str) -> Result<CameraDefaults, DecodeError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(DecodeError::Malformed("the text is too large".to_owned()));
    }
    let value = json::parse(text).map_err(DecodeError::Malformed)?;
    let bad = DecodeError::Malformed;
    let fields = value.as_object("camera defaults").map_err(bad)?;
    let format = json::required(fields, "camera_defaults_format")
        .and_then(|v| v.as_u32("camera_defaults_format"))
        .map_err(DecodeError::Malformed)?;
    if format > CAMERA_DEFAULTS_FORMAT {
        return Err(DecodeError::NewerVersion(format));
    }
    let camera = json::required(fields, "camera")
        .and_then(|v| v.as_str("camera").map(|s| s.trim().to_owned()))
        .map_err(DecodeError::Malformed)?;
    if camera.is_empty() {
        return Err(DecodeError::Malformed("camera defaults need a camera".to_owned()));
    }
    let style = json::required(fields, "style").and_then(codec::read_style).map_err(DecodeError::Malformed)?;
    Ok(CameraDefaults { camera, nodes: style.nodes })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use core_model::{ParamId, ParamValue, Params, ToolId};

    #[test]
    fn camera_defaults_round_trip_and_newer_ones_are_refused() {
        let mut d = CameraDefaults::new("Sony ILCE-7M3");
        let mut p = Params::new();
        p.set(ParamId::new("exposure").unwrap(), ParamValue::Float(0.35));
        d.set(ToolId::new("exposure").unwrap(), p);
        d.set(ToolId::new("dehaze").unwrap(), Params::new());
        let text = encode_camera_defaults(&d);
        assert_eq!(decode_camera_defaults(&text).unwrap(), d);
        let newer = text.replace("\"camera_defaults_format\":1", "\"camera_defaults_format\":4");
        assert_eq!(decode_camera_defaults(&newer), Err(DecodeError::NewerVersion(4)));
        let nameless = text.replace("Sony ILCE-7M3", " ");
        assert!(matches!(decode_camera_defaults(&nameless), Err(DecodeError::Malformed(_))));
    }
}
