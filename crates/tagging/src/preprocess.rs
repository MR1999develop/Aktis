//! From an editor image to the tensor an image model expects.
//!
//! Vision models are trained on sRGB-encoded photos scaled to a small fixed
//! size. The editor's images are linear, scene-referred and possibly in
//! Rec.2020, so the conversion is: area-average down to the model size in
//! linear light (exact box filter, no aliasing), convert to linear sRGB,
//! clip to the displayable range, apply the sRGB curve, then normalise with
//! the model's mean and standard deviation. Resampling before the colour
//! conversion is valid because both the average and the matrix are linear,
//! and it keeps the per-pixel work at the model size.

use core_model::{ImageF32, WorkingSpace};

/// Size and normalisation of a model's pixel input. The output is planar
/// (all red, then all green, then all blue: CHW), rows top to bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelLayout {
    width: u32,
    height: u32,
    mean: [f32; 3],
    std: [f32; 3],
}

impl PixelLayout {
    /// SigLIP / SigLIP 2 at 224 px: plain resize (no crop), values scaled
    /// to `[-1, 1]` (mean 0.5, std 0.5).
    pub const SIGLIP_224: Self = Self { width: 224, height: 224, mean: [0.5; 3], std: [0.5; 3] };

    /// A layout of `width × height` (1 to 4096 each) with per-channel mean
    /// and standard deviation applied to sRGB values in `[0, 1]`.
    #[must_use]
    pub fn new(width: u32, height: u32, mean: [f32; 3], std: [f32; 3]) -> Option<Self> {
        let size_ok = (1..=4096).contains(&width) && (1..=4096).contains(&height);
        let stats_ok = mean.iter().all(|m| m.is_finite()) && std.iter().all(|s| s.is_finite() && *s > 0.0);
        (size_ok && stats_ok).then_some(Self { width, height, mean, std })
    }

    /// Model input width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Model input height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Number of `f32` values of one image: `3 × width × height`.
    #[must_use]
    pub fn len(&self) -> usize {
        3 * self.width as usize * self.height as usize
    }

    /// Always false: a layout has at least one pixel.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }
}

/// Converts `image` to the planar, normalised input described by `layout`.
/// The aspect ratio is not kept (SigLIP is trained on squashed images); the
/// image should already be upright. Alpha is ignored.
#[must_use]
pub fn to_model_input(image: &ImageF32, layout: &PixelLayout) -> Vec<f32> {
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let (tw, th) = (layout.width as usize, layout.height as usize);

    // Horizontal pass: sh rows × tw columns, then vertical: th × tw.
    let wx = box_weights(sw, tw);
    let wy = box_weights(sh, th);
    let src = image.pixels();
    let mut rows = vec![[0.0_f32; 3]; sh * tw];
    for y in 0..sh {
        let line = &src[y * sw..(y + 1) * sw];
        for (ox, taps) in wx.iter().enumerate() {
            let mut acc = [0.0_f32; 3];
            for &(i, w) in taps {
                let p = line[i];
                acc[0] += p[0] * w;
                acc[1] += p[1] * w;
                acc[2] += p[2] * w;
            }
            rows[y * tw + ox] = acc;
        }
    }

    let to_srgb = core_color::rgb_to_rgb(image.space(), WorkingSpace::LinearSrgb);
    let plane = tw * th;
    let mut out = vec![0.0_f32; 3 * plane];
    for (oy, taps) in wy.iter().enumerate() {
        for ox in 0..tw {
            let mut acc = [0.0_f32; 3];
            for &(i, w) in taps {
                let p = rows[i * tw + ox];
                acc[0] += p[0] * w;
                acc[1] += p[1] * w;
                acc[2] += p[2] * w;
            }
            let rgb = to_srgb.apply(acc);
            for c in 0..3 {
                // NaN from a broken pixel becomes 0 instead of poisoning the model.
                let linear = if rgb[c].is_nan() { 0.0 } else { rgb[c].clamp(0.0, 1.0) };
                let encoded = core_color::srgb_encode(linear);
                out[c * plane + oy * tw + ox] = (encoded - layout.mean[c]) / layout.std[c];
            }
        }
    }
    out
}

/// For each of `dst` output cells, the source indices it overlaps and the
/// fraction of the cell each covers (weights sum to 1). Upscaling falls back
/// to the same rule, which then picks one or two neighbours.
fn box_weights(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    #[allow(clippy::cast_precision_loss)] // image sizes are far below 2^52
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|o| {
            #[allow(clippy::cast_precision_loss)]
            let (start, end) = (o as f64 * scale, (o + 1) as f64 * scale);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // non-negative, ≤ src
            let (first, last) = (start.floor() as usize, (end.ceil() as usize).min(src));
            let width = end - start;
            (first..last.max(first + 1))
                .filter_map(|i| {
                    #[allow(clippy::cast_precision_loss)]
                    let lo = start.max(i as f64);
                    #[allow(clippy::cast_precision_loss)]
                    let hi = end.min((i + 1) as f64);
                    #[allow(clippy::cast_possible_truncation)] // a fraction in [0, 1]
                    let w = ((hi - lo) / width) as f32;
                    (w > 0.0 && i < src).then_some((i, w))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn weights_cover_each_output_cell_exactly_once() {
        for (src, dst) in [(10, 3), (4256, 224), (3, 7), (5, 5), (1, 4)] {
            for taps in box_weights(src, dst) {
                let sum: f32 = taps.iter().map(|t| t.1).sum();
                assert!(close(sum, 1.0, 1e-5), "{src}->{dst}: weights sum to {sum}");
                assert!(taps.iter().all(|t| t.0 < src));
            }
        }
    }

    #[test]
    fn a_flat_grey_maps_to_its_srgb_value_everywhere() {
        // Linear 0.2140 is sRGB-encoded 0.5, which SigLIP normalises to 0.
        let img = ImageF32::from_pixels(
            37,
            23,
            WorkingSpace::LinearSrgb,
            vec![[0.214_041, 0.214_041, 0.214_041, 1.0]; 37 * 23],
        )
        .expect("valid image");
        let layout = PixelLayout::new(8, 4, [0.5; 3], [0.5; 3]).expect("valid layout");
        let t = to_model_input(&img, &layout);
        assert_eq!(t.len(), layout.len());
        assert!(t.iter().all(|&v| close(v, 0.0, 1e-3)), "{t:?}");
    }

    #[test]
    fn rec2020_white_stays_white_and_out_of_range_is_clipped() {
        let mut px = vec![[1.0, 1.0, 1.0, 1.0]; 16];
        px[0] = [50.0, -3.0, f32::NAN, 1.0];
        let img = ImageF32::from_pixels(4, 4, WorkingSpace::LinearRec2020, px).expect("valid image");
        let layout = PixelLayout::new(4, 4, [0.0; 3], [1.0; 3]).expect("valid layout");
        let t = to_model_input(&img, &layout);
        let plane = 16;
        assert!(t.iter().all(|v| (0.0..=1.0).contains(v)), "everything in [0, 1]");
        // Pixel 1 is pure white in any RGB space with a D65 white.
        assert!((0..3).all(|c| close(t[c * plane + 1], 1.0, 1e-3)));
    }

    #[test]
    fn channels_are_planar_and_rows_keep_their_order() {
        // Left half red, right half blue, in linear sRGB.
        let px: Vec<_> =
            (0..4 * 2).map(|i| if i % 4 < 2 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 0.0, 1.0, 1.0] }).collect();
        let img = ImageF32::from_pixels(4, 2, WorkingSpace::LinearSrgb, px).expect("valid image");
        let layout = PixelLayout::new(2, 1, [0.0; 3], [1.0; 3]).expect("valid layout");
        let t = to_model_input(&img, &layout);
        // [R plane: left, right][G plane][B plane]
        assert!(close(t[0], 1.0, 1e-4) && close(t[1], 0.0, 1e-4));
        assert!(close(t[4], 0.0, 1e-4) && close(t[5], 1.0, 1e-4));
    }

    #[test]
    fn invalid_layouts_are_refused() {
        assert!(PixelLayout::new(0, 224, [0.5; 3], [0.5; 3]).is_none());
        assert!(PixelLayout::new(224, 224, [0.5; 3], [0.5, 0.0, 0.5]).is_none());
        assert!(PixelLayout::new(224, 224, [f32::NAN; 3], [0.5; 3]).is_none());
    }
}
