//! From the editor's linear light to what the network expects, and back.
//!
//! SCUNet was trained on gamma-encoded photos in 0..=1. The editor's pixels
//! are linear and scene-referred, so they are encoded with the sRGB transfer
//! curve, keeping the working space's primaries (Rec.2020: no negative values
//! from out-of-gamut colours). The network sees values clamped to 0..=1.
//!
//! Its correction is then applied in the encoded domain, split
//! into a luma part (grain) and a colour part (colour blotches),
//! each weighted by its own amount, and the result is decoded back to linear
//! light. Two differences from applying it to a finished photo, both
//! because RAW data reach the tool unclipped:
//!
//! * values below 0 (noise below black) are denoised like the rest: the
//!   network sees them as 0, and its output replaces them. Tried on simulated
//!   sensor noise, this gives a lower error and less bias in the shadows than
//!   lifting the blacks with a pedestal or keeping the negative noise;
//! * highlights above 1 were never seen by the network: the correction fades
//!   out between encoded 1.0 and [`HIGHLIGHT_END`] instead of stopping at a
//!   hard edge, so no seam appears around blown areas.

use core_color::{luminance, srgb_decode, srgb_encode};
use core_model::{ImageF32, WorkingSpace};

use crate::tiling::RgbImage;

/// Encoded value above which the network's correction is no longer applied.
pub const HIGHLIGHT_END: f32 = 1.1;

/// Encodes the image for the network (alpha dropped). Values are not clamped
/// here: the tiling clamps what the network sees, the mixer needs the rest.
#[must_use]
pub fn encode(image: &ImageF32) -> RgbImage {
    let pixels = image.pixels().iter().map(|p| [srgb_encode(p[0]), srgb_encode(p[1]), srgb_encode(p[2])]).collect();
    RgbImage::new(image.width() as usize, image.height() as usize, pixels).unwrap_or_default()
}

/// How much of the network's correction to apply, split into luma and colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mixer {
    luma: f32,
    chroma: f32,
    space: WorkingSpace,
}

impl Mixer {
    /// `luma` and `chroma` are fractions 0..=1 (clamped); `space` gives the
    /// luma weights of the image's primaries.
    #[must_use]
    pub fn new(luma: f32, chroma: f32, space: WorkingSpace) -> Self {
        let unit = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        Self { luma: unit(luma), chroma: unit(chroma), space }
    }

    /// `true` when nothing of the correction is applied.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.luma <= 0.0 && self.chroma <= 0.0
    }

    /// Mixes one encoded pixel with the network's output for it.
    ///
    /// The difference `network − original` is split into its luma (equal on
    /// the three channels) and the rest (colour). With both amounts at 1 the
    /// result is the network output; with both at 0 it is the original.
    #[must_use]
    pub fn apply(&self, original: [f32; 3], network: [f32; 3]) -> [f32; 3] {
        let brightest = original[0].max(original[1]).max(original[2]);
        let trust = ((HIGHLIGHT_END - brightest) / (HIGHLIGHT_END - 1.0)).clamp(0.0, 1.0);
        if trust <= 0.0 {
            return original;
        }
        let d = [network[0] - original[0], network[1] - original[1], network[2] - original[2]];
        let dy = luminance(d, self.space);
        let mut out = original;
        for (o, dc) in out.iter_mut().zip(d) {
            *o += trust * (self.luma * dy + self.chroma * (dc - dy));
        }
        out
    }

    /// Applies the mix to `image` in place: `encoded` is the image as
    /// [`encode`] returned it and `network` the network's output for it.
    /// Alpha is untouched.
    pub fn apply_to(&self, image: &mut ImageF32, encoded: &RgbImage, network: &RgbImage) {
        for ((p, e), n) in image.pixels_mut().iter_mut().zip(encoded.pixels()).zip(network.pixels()) {
            let m = self.apply(*e, *n);
            p[0] = srgb_decode(m[0]);
            p[1] = srgb_decode(m[1]);
            p[2] = srgb_decode(m[2]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPACE: WorkingSpace = WorkingSpace::LinearRec2020;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn the_extremes_give_the_original_and_the_network() {
        let o = [0.2, 0.5, 0.7];
        let n = [0.3, 0.4, 0.75];
        assert!(close(Mixer::new(0.0, 0.0, SPACE).apply(o, n), o));
        assert!(close(Mixer::new(1.0, 1.0, SPACE).apply(o, n), n));
        assert!(Mixer::new(0.0, 0.0, SPACE).is_identity());
        assert!(!Mixer::new(0.0, 0.3, SPACE).is_identity());
    }

    #[test]
    fn luma_only_keeps_the_colour_of_the_original() {
        let o = [0.2, 0.5, 0.7];
        let n = [0.4, 0.4, 0.4];
        let m = Mixer::new(1.0, 0.0, SPACE).apply(o, n);
        assert!(((m[0] - m[1]) - (o[0] - o[1])).abs() < 1e-6);
        assert!(((m[2] - m[1]) - (o[2] - o[1])).abs() < 1e-6);
        assert!((luminance(m, SPACE) - luminance(n, SPACE)).abs() < 1e-6);
    }

    #[test]
    fn noise_below_black_is_replaced_by_the_network_output() {
        let m = Mixer::new(1.0, 1.0, SPACE).apply([-0.05, 0.01, -0.02], [0.01, 0.01, 0.01]);
        assert!(close(m, [0.01; 3]));
    }

    #[test]
    fn highlights_fade_out_instead_of_stopping_at_an_edge() {
        let mixer = Mixer::new(1.0, 1.0, SPACE);
        let net = [0.9, 0.9, 0.9];
        assert!(close(mixer.apply([1.0, 0.8, 0.8], net), net), "at 1.0 the network is fully trusted");
        let half = mixer.apply([1.05, 1.05, 1.05], net);
        assert!((half[0] - 0.975).abs() < 1e-5, "half way: {half:?}");
        assert!(close(mixer.apply([1.2, 0.5, 0.5], net), [1.2, 0.5, 0.5]), "beyond the end: untouched");
    }

    #[test]
    fn encode_then_identity_mix_returns_the_image_and_keeps_alpha() {
        let px = vec![[0.18, -0.01, 2.5, 0.5], [0.0, 1.0, 0.003, 1.0]];
        let mut img = ImageF32::from_pixels(2, 1, SPACE, px.clone()).expect("2\u{d7}1");
        let e = encode(&img);
        Mixer::new(1.0, 1.0, SPACE).apply_to(&mut img, &e, &e);
        for (a, b) in img.pixels().iter().zip(&px) {
            assert!(a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5), "{a:?} vs {b:?}");
        }
    }
}
