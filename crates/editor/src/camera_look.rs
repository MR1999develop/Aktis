//! The camera's own look of a RAW file: the base curve fitted to the JPEG
//! the camera embedded in it, which is what Auto shows for RAW files
//! ([`core_model::BaseCurve::with_camera_fit`]), as RawTherapee's
//! "Auto-Matched Curve" does.

use std::path::Path;

use core_model::{CurveFit, ImageF32};

/// Longer side the two pictures are compared at: enough for their
/// distributions, quick to read and to fit.
pub const FIT_SIDE: u32 = 512;

/// The curve that makes `developed` (the photo as developed, before any
/// tool) look like the camera's picture inside `path`, or `None` when the
/// file has no picture or no film curve comes close.
#[must_use]
pub fn camera_curve(path: &Path, developed: &ImageF32) -> Option<CurveFit> {
    let picture = raw_io::read_thumbnail(path, FIT_SIDE).ok()?;
    let small = engine::downscale(developed, FIT_SIDE).ok()?;
    let scene = scene_luminance(&small);
    let shown = picture_luminance(&picture.rgb);
    render_cpu::tone::fit_camera_curve(&scene, &shown)
}

/// Luminance of a developed image (linear, in its working space).
fn scene_luminance(image: &ImageF32) -> Vec<f32> {
    let w = luminance_weights(image.space());
    image.pixels().iter().map(|p| w[0] * p[0] + w[1] * p[1] + w[2] * p[2]).collect()
}

/// sRGB-encoded luminance of 8-bit sRGB pixels.
fn picture_luminance(rgb: &[u8]) -> Vec<f32> {
    const W: [f32; 3] = [0.2126, 0.7152, 0.0722];
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|c| {
            let y: f32 = c.iter().zip(W).map(|(v, w)| w * core_color::srgb_decode(f32::from(*v) / 255.0)).sum();
            core_color::srgb_encode(y)
        })
        .collect()
}

fn luminance_weights(space: core_model::WorkingSpace) -> [f32; 3] {
    match space {
        core_model::WorkingSpace::LinearRec2020 => [0.2627, 0.6780, 0.0593],
        core_model::WorkingSpace::LinearSrgb => [0.2126, 0.7152, 0.0722],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_pixel_has_its_own_luminance() {
        let y = picture_luminance(&[128, 128, 128]);
        assert!((y[0] - 128.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn a_file_without_a_picture_has_no_camera_curve() {
        let image = ImageF32::new(4, 4, core_model::WorkingSpace::LinearRec2020).expect("an image");
        assert!(camera_curve(Path::new("no/such/file.NEF"), &image).is_none());
    }
}
