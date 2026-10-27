//! The histogram shown above the tool settings: how many pixels of the
//! displayed image fall at each 8-bit level, per channel and for luminance.

/// Counts per level (0–255) for red, green, blue and luminance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Histogram {
    /// Red channel.
    pub red: [u32; 256],
    /// Green channel.
    pub green: [u32; 256],
    /// Blue channel.
    pub blue: [u32; 256],
    /// Luminance (Rec. 709 weights on the encoded values).
    pub luma: [u32; 256],
}

impl Default for Histogram {
    fn default() -> Self {
        Self { red: [0; 256], green: [0; 256], blue: [0; 256], luma: [0; 256] }
    }
}

impl Histogram {
    /// Counts packed `R G B` pixels (a trailing partial pixel is ignored).
    #[must_use]
    pub fn of_rgb8(rgb: &[u8]) -> Self {
        let mut h = Self::default();
        for p in rgb.chunks_exact(3) {
            let (r, g, b) = (p[0], p[1], p[2]);
            h.red[usize::from(r)] += 1;
            h.green[usize::from(g)] += 1;
            h.blue[usize::from(b)] += 1;
            // Integer Rec. 709 weights (sum 10 000): no floating point per pixel.
            let l = (2126 * u32::from(r) + 7152 * u32::from(g) + 722 * u32::from(b) + 5000) / 10_000;
            h.luma[usize::try_from(l.min(255)).unwrap_or(255)] += 1;
        }
        h
    }

    /// Number of pixels counted.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.luma.iter().map(|&c| u64::from(c)).sum()
    }

    /// The largest count of any channel, ignoring the two extreme levels
    /// (clipped pixels would otherwise flatten the rest of the curve).
    #[must_use]
    pub fn peak(&self) -> u32 {
        [&self.red, &self.green, &self.blue, &self.luma]
            .iter()
            .flat_map(|c| c[1..255].iter().copied())
            .max()
            .unwrap_or(0)
    }

    /// Share of pixels with any channel at 0 and at 255 (shadow and highlight clipping), per channel max.
    #[must_use]
    pub fn clipped(&self) -> (f32, f32) {
        let total = self.total();
        if total == 0 {
            return (0.0, 0.0);
        }
        let low = [self.red[0], self.green[0], self.blue[0]].into_iter().max().unwrap_or(0);
        let high = [self.red[255], self.green[255], self.blue[255]].into_iter().max().unwrap_or(0);
        #[allow(clippy::cast_precision_loss)] // a share for display
        let share = |n: u32| f64::from(n) / total as f64;
        #[allow(clippy::cast_possible_truncation)] // between 0 and 1
        let pair = (share(low) as f32, share(high) as f32);
        pair
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_counted_per_channel_and_for_luminance() {
        let h = Histogram::of_rgb8(&[255, 255, 255, 0, 0, 0, 255, 0, 0, 7]);
        assert_eq!(h.total(), 3, "the trailing byte is not a pixel");
        assert_eq!(h.red[255], 2);
        assert_eq!(h.green[0], 2);
        assert_eq!(h.luma[255], 1);
        assert_eq!(h.luma[0], 1);
        assert_eq!(h.luma[54], 1, "pure red has 21 % luminance");
    }

    #[test]
    fn clipping_and_peak_ignore_nothing_else() {
        let mut px = vec![128_u8; 3 * 8];
        px.extend_from_slice(&[255, 255, 255, 255, 255, 255]);
        let h = Histogram::of_rgb8(&px);
        assert_eq!(h.peak(), 8);
        let (low, high) = h.clipped();
        assert_eq!(low, 0.0);
        assert!((high - 0.2).abs() < 1e-6);
        assert_eq!(Histogram::default().clipped(), (0.0, 0.0));
    }
}
