//! A custom lens ([`CustomLens`]) as a small versioned text file: name, the
//! lens it is for as the files name it, and its lens-correction settings.

use core_model::CustomLens;

use crate::json::{self, Value};
use crate::{DecodeError, MAX_TEXT_BYTES, codec};

/// File extension of custom lenses.
pub const CUSTOM_LENS_EXTENSION: &str = "aktislens";

/// Version of the custom lens format written by this build.
pub const CUSTOM_LENS_FORMAT: u32 = 1;

/// The custom lens as text, in format [`CUSTOM_LENS_FORMAT`].
#[must_use]
pub fn encode_custom_lens(lens: &CustomLens) -> String {
    let Value::Object(mut fields) = codec::custom_lens(lens) else {
        unreachable!("a custom lens is written as an object")
    };
    fields.insert(0, ("custom_lens_format".to_owned(), Value::uint(CUSTOM_LENS_FORMAT)));
    Value::Object(fields).to_text()
}

/// Reads a custom lens written by [`encode_custom_lens`] of this or an
/// older version; names are trimmed, settings brought into their ranges.
///
/// # Errors
/// [`DecodeError::NewerVersion`] for a newer format, [`DecodeError::Malformed`]
/// when the text is not a custom lens or names nothing.
pub fn decode_custom_lens(text: &str) -> Result<CustomLens, DecodeError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(DecodeError::Malformed("the text is too large".to_owned()));
    }
    let value = json::parse(text).map_err(DecodeError::Malformed)?;
    let fields = value.as_object("custom lens").map_err(DecodeError::Malformed)?;
    let format = json::required(fields, "custom_lens_format")
        .and_then(|v| v.as_u32("custom_lens_format"))
        .map_err(DecodeError::Malformed)?;
    if format > CUSTOM_LENS_FORMAT {
        return Err(DecodeError::NewerVersion(format));
    }
    let mut lens = codec::read_custom_lens(&value).map_err(DecodeError::Malformed)?;
    lens.name = lens.name.trim().to_owned();
    lens.exif_lens = lens.exif_lens.trim().to_owned();
    if lens.name.is_empty() || lens.exif_lens.is_empty() {
        return Err(DecodeError::Malformed("a custom lens needs a name and a lens".to_owned()));
    }
    lens.settings = lens.settings.sanitized();
    lens.settings.custom = Some(lens.name.clone());
    Ok(lens)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use core_model::{LensAmounts, LensSettings, ManualLens};

    fn sample() -> CustomLens {
        CustomLens {
            name: "My 24-70".to_owned(),
            exif_lens: "Sony FE 24-70mm F2.8 GM".to_owned(),
            settings: LensSettings {
                lens: Some("Sony FE 24-70mm f/2.8 GM".to_owned()),
                tca: false,
                zoom: Some(1.02),
                amount: LensAmounts { distortion: 0.75, tca: 1.0, vignetting: 1.5 },
                manual: ManualLens { distortion: -0.2, vignetting: 0.3 },
                purple_fringing: 0.6,
                custom: Some("My 24-70".to_owned()),
                ..LensSettings::default()
            },
        }
    }

    #[test]
    fn a_custom_lens_round_trips() {
        let lens = sample();
        assert_eq!(decode_custom_lens(&encode_custom_lens(&lens)).unwrap(), lens);
    }

    #[test]
    fn newer_empty_and_out_of_range_custom_lenses_are_handled() {
        let text = encode_custom_lens(&sample()).replace("\"custom_lens_format\":1", "\"custom_lens_format\":9");
        assert!(text.contains(":9"), "{text}");
        assert_eq!(decode_custom_lens(&text), Err(DecodeError::NewerVersion(9)));
        let mut nameless = sample();
        nameless.name = "  ".to_owned();
        assert!(matches!(decode_custom_lens(&encode_custom_lens(&nameless)), Err(DecodeError::Malformed(_))));
        let mut wild = sample();
        wild.settings.amount.vignetting = 7.0;
        let back = decode_custom_lens(&encode_custom_lens(&wild)).unwrap();
        assert!((back.settings.amount.vignetting - LensAmounts::MAX).abs() < 1e-6);
        assert!(decode_custom_lens("not json").is_err());
    }
}
