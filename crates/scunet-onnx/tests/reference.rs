//! Byte-for-byte comparison with the Python reference converter on the real
//! checkpoint (72 MB, not in the repository).
//!
//! ```sh
//! RNE_SCUNET_PTH=scunet_color_real_psnr.pth RNE_SCUNET_REF448=scunet_color_real_448.onnx \
//!     cargo test --release -p scunet-onnx -- --ignored --nocapture
//! ```
//!
//! `RNE_SCUNET_REF960`, if set, adds the 960 px tile model.

#![allow(clippy::unwrap_used)]

use std::time::Instant;

fn compare(tile: u32, reference_var: &str) {
    let Ok(reference) = std::env::var(reference_var) else {
        println!("{reference_var} not set: tile {tile} skipped");
        return;
    };
    let pth = std::fs::read(std::env::var("RNE_SCUNET_PTH").expect("RNE_SCUNET_PTH")).unwrap();
    let reference = std::fs::read(reference).unwrap();
    let start = Instant::now();
    let model = scunet_onnx::convert(&pth, tile).unwrap();
    println!("tile {tile}: {} bytes in {:.2?}", model.len(), start.elapsed());
    assert_eq!(model.len(), reference.len(), "size differs from the Python model");
    if let Some(at) = model.iter().zip(&reference).position(|(a, b)| a != b) {
        panic!("first difference at byte {at}");
    }
}

#[test]
#[ignore = "needs the KAIR checkpoint and the Python-made models (RNE_SCUNET_PTH, RNE_SCUNET_REF448)"]
fn identical_to_python() {
    compare(448, "RNE_SCUNET_REF448");
    compare(960, "RNE_SCUNET_REF960");
}

#[test]
#[ignore = "needs the KAIR checkpoint (RNE_SCUNET_PTH)"]
fn reads_every_weight() {
    let pth = std::fs::read(std::env::var("RNE_SCUNET_PTH").expect("RNE_SCUNET_PTH")).unwrap();
    let weights = scunet_onnx::load_state_dict(&pth).unwrap();
    assert_eq!(weights.len(), 540);
    assert_eq!(weights.values().map(|t| t.data.len()).sum::<usize>(), 17_946_072);
    assert_eq!(weights["m_down1.0.trans_block.msa.relative_position_params"].shape, [1, 15, 15]);
}
