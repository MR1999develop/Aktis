//! The adjustment tools, modelled on Capture One's.
//!
//! Each module is one tool. The per-pixel ones have a CPU reference
//! ([`tool_api::Tool::apply_cpu`]) and a WGSL kernel computing the same
//! thing; [`clarity`], [`dehaze`], [`sharpening`] and [`noise_reduction`]
//! look at neighbouring pixels, and
//! [`retouch`] copies pixels from elsewhere in the image: they run on the
//! CPU ([`tool_api::GpuKernel::Host`]).
//!
//! Tone tools that shape a curve (levels, curve) work on values encoded with
//! a 2.2 gamma, as their histograms and handles are shown; the others work
//! on linear light, in stops around middle grey (0.18), so they treat every
//! part of the range alike.

pub mod black_white;
pub mod clarity;
pub mod color_balance;
pub mod color_editor;
mod common;
pub mod curve;
pub mod dehaze;
mod filters;
pub mod hdr;
pub mod levels;
pub mod noise_reduction;
pub mod retouch;
pub mod sharpening;
pub mod vignetting;
pub mod white_balance;

pub use common::hue_chroma;

use std::sync::Arc;

use tool_api::Tool;

/// Every tool of this crate, ready to register.
#[must_use]
pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(white_balance::WhiteBalance),
        Arc::new(hdr::Hdr),
        Arc::new(levels::Levels),
        Arc::new(curve::Curve),
        Arc::new(color_balance::ColorBalance),
        Arc::new(color_editor::ColorEditor),
        Arc::new(black_white::BlackWhite),
        Arc::new(clarity::Clarity),
        Arc::new(dehaze::Dehaze),
        Arc::new(sharpening::Sharpening),
        Arc::new(noise_reduction::NoiseReduction),
        Arc::new(vignetting::Vignetting),
        Arc::new(retouch::Retouch),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_descriptor_is_valid_and_ids_are_unique() {
        let mut ids = std::collections::BTreeSet::new();
        for t in tools() {
            let d = t.descriptor();
            assert_eq!(d.validate(), Ok(()), "{}", d.id);
            assert!(ids.insert(d.id.as_str().to_owned()), "duplicate {}", d.id);
        }
    }

    #[test]
    fn every_tool_but_black_and_white_and_the_details_tools_leaves_an_image_unchanged_at_its_defaults() {
        let pixels: Vec<[f32; 4]> = (0..64)
            .map(|i| {
                let f = i as f32 / 63.0;
                [f * 1.5, (1.0 - f) * 0.7 + 0.01, (f * 7.0).sin().abs() * 0.4, 1.0]
            })
            .collect();
        for t in tools()
            .into_iter()
            .filter(|t| !["black_white", "sharpening", "noise_reduction"].contains(&t.descriptor().id.as_str()))
        {
            let mut img =
                core_model::ImageF32::from_pixels(8, 8, core_model::WorkingSpace::LinearRec2020, pixels.clone())
                    .expect("8×8");
            let params = tool_api::ResolvedParams::resolve(t.descriptor(), &tool_api::Params::new());
            t.apply_cpu(&mut img, &params);
            for (a, b) in img.pixels().iter().zip(&pixels) {
                for c in 0..4 {
                    assert!((a[c] - b[c]).abs() <= 2e-5 * (1.0 + b[c].abs()), "{}: {a:?} vs {b:?}", t.descriptor().id);
                }
            }
        }
    }
}
