//! Turning a developed image into model input.
//!
//! Segmentation models were trained on ordinary photos: display-referred
//! sRGB at a normal exposure. The editor's image is linear, scene-referred,
//! in a wide gamut, at whatever exposure the camera chose. So the image is
//! resized in linear light, given a neutral exposure, converted to sRGB
//! primaries and encoded with the sRGB curve before a model sees it. None of
//! this touches the edit: it only shapes what the model looks at.

use core_model::{ImageF32, WorkingSpace};

use crate::inference::Tensor;

/// ImageNet channel means, for models trained with torchvision normalisation.
pub(crate) const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
/// ImageNet channel standard deviations.
pub(crate) const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// An image as models see it: sRGB-encoded values in [0, 1], one plane per channel.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Planes {
    pub w: usize,
    pub h: usize,
    pub rgb: [Vec<f32>; 3],
}

/// Log-average luminance (linear) below which an image counts as underexposed
/// for the models: a RAW file developed without any brightening, a night shot.
const DARK: f32 = 0.03;
/// Most an underexposed image is brightened for the models.
const MAX_GAIN: f32 = 8.0;

/// `image` resized to `w × h` in linear light, in sRGB primaries,
/// sRGB-encoded: the picture as the user sees it. Only an underexposed image
/// is brightened (to [`DARK`]), since models were trained on ordinary photos.
pub(crate) fn srgb_planes(image: &ImageF32, w: usize, h: usize) -> Planes {
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    // One channel at a time: a full-size plane is the only large temporary.
    let mut small: [Vec<f32>; 3] = Default::default();
    for (c, plane) in small.iter_mut().enumerate() {
        let full: Vec<f32> = image.pixels().iter().map(|p| p[c]).collect();
        *plane = masks::resize_plane(&full, sw, sh, w, h);
    }
    let to_srgb = core_color::rgb_to_rgb(image.space(), WorkingSpace::LinearSrgb);
    let mut rgb: [Vec<f32>; 3] = [vec![0.0; w * h], vec![0.0; w * h], vec![0.0; w * h]];
    let mut log_sum = 0.0_f64;
    for i in 0..w * h {
        let v = to_srgb.apply([small[0][i], small[1][i], small[2][i]]);
        let y = core_color::luminance(v, WorkingSpace::LinearSrgb);
        log_sum += f64::from(y.max(1e-4).ln());
        for (plane, value) in rgb.iter_mut().zip(v) {
            plane[i] = value;
        }
    }
    let mean_log = log_sum / (w * h).max(1) as f64;
    #[allow(clippy::cast_possible_truncation)] // a log-luminance, small
    let geometric_mean = (mean_log as f32).exp();
    let gain = (DARK / geometric_mean).clamp(1.0, MAX_GAIN);
    for plane in &mut rgb {
        for v in plane.iter_mut() {
            *v = core_color::srgb_encode((*v * gain).clamp(0.0, 1.0));
        }
    }
    Planes { w, h, rgb }
}

/// `[1, 3, h, w]` tensor of `(value − mean) / std` per channel.
pub(crate) fn normalized_nchw(p: &Planes, mean: [f32; 3], std: [f32; 3]) -> Tensor {
    let mut data = Vec::with_capacity(3 * p.w * p.h);
    for c in 0..3 {
        data.extend(p.rgb[c].iter().map(|v| (v - mean[c]) / std[c]));
    }
    Tensor { shape: vec![1, 3, p.h, p.w], data }
}

/// The standard logistic function.
pub(crate) fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, v: [f32; 4]) -> ImageF32 {
        ImageF32::from_pixels(w, h, WorkingSpace::LinearSrgb, vec![v; (w * h) as usize]).expect("valid image")
    }

    #[test]
    fn a_middle_grey_image_becomes_srgb_118_over_255() {
        let p = srgb_planes(&image(8, 6, [0.18, 0.18, 0.18, 1.0]), 4, 3);
        assert_eq!((p.w, p.h), (4, 3));
        for plane in &p.rgb {
            assert!(plane.iter().all(|v| (v - 0.4613).abs() < 2e-3), "{plane:?}");
        }
    }

    #[test]
    fn only_an_underexposed_image_is_brightened() {
        let dark = srgb_planes(&image(4, 4, [0.0075, 0.0075, 0.0075, 1.0]), 2, 2);
        assert!((dark.rgb[1][0] - core_color::srgb_encode(0.03)).abs() < 2e-3, "{}", dark.rgb[1][0]);
        let bright = srgb_planes(&image(4, 4, [0.9, 0.9, 0.9, 1.0]), 2, 2);
        assert!((bright.rgb[1][0] - core_color::srgb_encode(0.9)).abs() < 2e-3);
    }

    #[test]
    fn normalisation_is_planar_and_per_channel() {
        let p = Planes { w: 2, h: 1, rgb: [vec![0.485, 1.0], vec![0.456, 0.0], vec![0.406, 0.5]] };
        let t = normalized_nchw(&p, IMAGENET_MEAN, IMAGENET_STD);
        assert_eq!(t.shape, vec![1, 3, 1, 2]);
        assert!(t.data[0].abs() < 1e-6 && t.data[2].abs() < 1e-6 && t.data[4].abs() < 1e-6);
        assert!((t.data[1] - (1.0 - 0.485) / 0.229).abs() < 1e-5);
    }

    #[test]
    fn sigmoid_is_centred() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-7);
        assert!(sigmoid(10.0) > 0.9999 && sigmoid(-10.0) < 1e-4);
    }
}
