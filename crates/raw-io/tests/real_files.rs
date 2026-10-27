//! Decodes and develops every RAW file found in `testdata/` at the workspace root.
//!
//! RAW samples are not in the repository (size and licences), so this test is
//! ignored by default. Put files in `testdata/` and run:
//! `cargo test -p raw-io --release -- --ignored --nocapture`

use std::path::PathBuf;

use core_model::{DevelopSettings, WorkingSpace};

#[test]
#[ignore = "needs RAW samples in testdata/"]
fn every_sample_in_testdata_decodes_and_develops() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
    let mut tested = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|e| e.eq_ignore_ascii_case("md")) {
            continue;
        }
        let raw = raw_io::read_raw(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let s = &raw.sensor;
        println!(
            "{}: {} {} — {}×{} {:?}, wb {:?}, {:?}",
            path.display(),
            raw.make,
            raw.model,
            s.width(),
            s.height(),
            s.cfa(),
            s.as_shot_wb(),
            raw.orientation
        );
        let img = develop::develop(s, &DevelopSettings::default(), WorkingSpace::LinearRec2020)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mean_g = img.pixels().iter().map(|p| f64::from(p[1])).sum::<f64>() / img.pixels().len() as f64;
        println!("    mean green (linear): {mean_g:.4}");
        assert!(mean_g.is_finite() && mean_g > 0.0, "{}: implausible mean {mean_g}", path.display());
        tested += 1;
    }
    assert!(tested > 0, "no RAW files in {}", dir.display());
}
