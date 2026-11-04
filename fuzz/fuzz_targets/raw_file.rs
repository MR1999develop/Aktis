//! A photo file as the editor opens it: RAW and DNG through the decoder,
//! JPEG and TIFF through the picture readers, the DNG opcodes and the Exif
//! block. The first byte picks the file name extension.
//!
//! The RAW decoder is a third-party crate with known panics on rare files;
//! `raw-io` catches those, as the editor does, so only a panic that gets
//! past it (in this project's code) is reported.

#![no_main]

use std::io::Write;
use std::sync::Once;

use libfuzzer_sys::fuzz_target;

const EXTENSIONS: [&str; 6] = ["dng", "cr3", "nef", "arw", "jpg", "tif"];

static QUIET_CAUGHT_PANICS: Once = Once::new();

fuzz_target!(|data: &[u8]| {
    let Some((&pick, file)) = data.split_first() else { return };
    // Panics that raw-io catches are not findings; one that escapes still
    // reaches libFuzzer, which treats the unwinding run as a crash.
    QUIET_CAUGHT_PANICS.call_once(|| std::panic::set_hook(Box::new(|_| {})));
    let ext = EXTENSIONS[usize::from(pick) % EXTENSIONS.len()];
    let path = std::env::temp_dir().join(format!("aktis-fuzz-{}.{ext}", std::process::id()));
    let Ok(mut out) = std::fs::File::create(&path) else { return };
    if out.write_all(file).is_err() {
        return;
    }
    drop(out);
    let _ = raw_io::read_info(&path);
    let _ = raw_io::read_raw(&path);
    let _ = raw_io::read_thumbnail(&path, 256);
    let _ = raw_io::decode_jpeg(file, 256);
});
