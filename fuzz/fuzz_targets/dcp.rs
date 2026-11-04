//! A DNG camera profile (`.dcp`) from the profiles folder. A profile that
//! is accepted must also give its colour transform.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(profile) = core_color::read_dcp(data, core_color::ProfileSource::Builtin) {
        let _ = profile.calibrations();
        let _ = core_color::write_dcp(&profile);
    }
});
