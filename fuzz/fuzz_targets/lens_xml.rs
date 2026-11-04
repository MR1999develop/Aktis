//! A lens database file (lensfun XML) from the lenses folder, and the
//! correction of an image with each lens it describes.

#![no_main]

use lens_db::{Adjustments, CorrectionRequest, LensCorrection, LensDatabase};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let Ok(db) = LensDatabase::from_xml(text) else { return };
    let request = CorrectionRequest {
        crop_factor: 1.5,
        focal: 24.0,
        aperture: 4.0,
        distance: 1000.0,
        width: 64,
        height: 48,
        distortion: true,
        tca: true,
        vignetting: true,
        adjust: Adjustments::default(),
    };
    for lens in db.lenses() {
        if let Ok(c) = LensCorrection::new(lens, &request) {
            let _ = (c.source(0.0, 0.0), c.source(63.0, 47.0), c.vignetting_gain(31.5, 23.5));
        }
    }
    let _ = db.find_lenses(db.cameras().first(), None, "24-70");
});
