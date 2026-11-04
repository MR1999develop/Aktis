//! The stored texts: a photo's edit (variant), presets and brushes, custom
//! lenses and camera defaults. What is accepted must write back to the
//! same text (compared as text: a stored "nan" is not equal to itself).

#![no_main]

use libfuzzer_sys::fuzz_target;
use recipe_store as rs;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    if let Ok(v) = rs::decode(text) {
        let written = rs::encode(&v);
        assert_eq!(rs::decode(&written).map(|again| rs::encode(&again)), Ok(written));
    }
    let _ = rs::decode_recipe_lenient(text);
    if let Ok(p) = rs::decode_preset(text) {
        let written = rs::encode_preset(&p);
        assert_eq!(rs::decode_preset(&written).map(|again| rs::encode_preset(&again)), Ok(written));
    }
    if let Ok(l) = rs::decode_custom_lens(text) {
        let written = rs::encode_custom_lens(&l);
        assert_eq!(rs::decode_custom_lens(&written).map(|again| rs::encode_custom_lens(&again)), Ok(written));
    }
    if let Ok(c) = rs::decode_camera_defaults(text) {
        let written = rs::encode_camera_defaults(&c);
        assert_eq!(rs::decode_camera_defaults(&written).map(|again| rs::encode_camera_defaults(&again)), Ok(written));
    }
});
