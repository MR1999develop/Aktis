//! The scopes shown above the tool settings: the histogram (how many
//! pixels of the displayed image fall at each 8-bit level, per channel and
//! for luminance) and the waveform and parade (the same counted per column
//! of the image, so a level can be traced back to where it is).

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
    /// Levels per column of the image; empty when not computed.
    pub waveform: Waveform,
}

impl Default for Histogram {
    fn default() -> Self {
        Self { red: [0; 256], green: [0; 256], blue: [0; 256], luma: [0; 256], waveform: Waveform::default() }
    }
}

/// Columns of the waveform and of each parade channel.
pub const WAVEFORM_COLUMNS: usize = 256;
/// Levels (rows) of the waveform.
pub const WAVEFORM_LEVELS: usize = 128;

/// Counts per (level, column): for each of [`WAVEFORM_COLUMNS`] vertical
/// strips of the image, how many pixels fall at each of
/// [`WAVEFORM_LEVELS`] levels; luminance for the waveform, red, green and
/// blue for the parade. Index `level * WAVEFORM_COLUMNS + column`, level 0
/// is black.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Waveform {
    /// Luminance.
    pub luma: Vec<u32>,
    /// Red.
    pub red: Vec<u32>,
    /// Green.
    pub green: Vec<u32>,
    /// Blue.
    pub blue: Vec<u32>,
}

impl Waveform {
    /// Counts packed `R G B` pixels of an image `width` pixels wide.
    #[must_use]
    pub fn of_rgb8(rgb: &[u8], width: usize) -> Self {
        let n = WAVEFORM_COLUMNS * WAVEFORM_LEVELS;
        let mut w = Self { luma: vec![0; n], red: vec![0; n], green: vec![0; n], blue: vec![0; n] };
        if width == 0 {
            return w;
        }
        for (i, p) in rgb.as_chunks::<3>().0.iter().enumerate() {
            let column = (i % width) * WAVEFORM_COLUMNS / width;
            let at = |v: u32| (v as usize * WAVEFORM_LEVELS / 256) * WAVEFORM_COLUMNS + column;
            let (r, g, b) = (u32::from(p[0]), u32::from(p[1]), u32::from(p[2]));
            w.red[at(r)] += 1;
            w.green[at(g)] += 1;
            w.blue[at(b)] += 1;
            w.luma[at(((2126 * r + 7152 * g + 722 * b + 5000) / 10_000).min(255))] += 1;
        }
        w
    }

    /// Whether nothing was counted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.luma.is_empty()
    }

    /// Brightness (0–1) of each cell of one plot, `columns` wide and
    /// [`WAVEFORM_LEVELS`] high, top row (white) first: `counts` scaled by
    /// [`Self::scale`] with a square root, so sparse levels still show.
    #[must_use]
    pub fn intensities(&self, counts: &[u32]) -> Vec<f32> {
        #[allow(clippy::cast_precision_loss)] // counts, for drawing
        let scale = self.scale() as f32;
        let mut out = Vec::with_capacity(counts.len());
        for level in (0..WAVEFORM_LEVELS).rev() {
            let row = &counts[level * WAVEFORM_COLUMNS..(level + 1) * WAVEFORM_COLUMNS];
            #[allow(clippy::cast_precision_loss)] // counts, for drawing
            out.extend(row.iter().map(|&c| (c as f32 / scale).min(1.0).sqrt()));
        }
        out
    }

    /// The count most cells stay below (the 99th percentile of the non-empty
    /// cells), for scaling the brightness of the plot.
    #[must_use]
    pub fn scale(&self) -> u32 {
        let mut counts: Vec<u32> = self.luma.iter().copied().filter(|&c| c > 0).collect();
        if counts.is_empty() {
            return 1;
        }
        counts.sort_unstable();
        counts[(counts.len() - 1) * 99 / 100].max(1)
    }
}

impl Histogram {
    /// Counts packed `R G B` pixels (a trailing partial pixel is ignored).
    #[must_use]
    pub fn of_rgb8(rgb: &[u8]) -> Self {
        let mut h = Self::default();
        for p in rgb.as_chunks::<3>().0 {
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

    /// Counts packed `R G B` pixels of an image `width` pixels wide,
    /// waveform and parade included.
    #[must_use]
    pub fn of_image(rgb: &[u8], width: usize) -> Self {
        Self { waveform: Waveform::of_rgb8(rgb, width), ..Self::of_rgb8(rgb) }
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
    fn the_waveform_counts_each_column_at_its_level() {
        // A 4 × 2 image: black on the left half, white on the right; one red pixel.
        let mut rgb = Vec::new();
        for y in 0..2 {
            for x in 0..4 {
                let v = if x < 2 { 0 } else { 255 };
                if (x, y) == (0, 1) {
                    rgb.extend([255, 0, 0]);
                } else {
                    rgb.extend([v, v, v]);
                }
            }
        }
        let h = Histogram::of_image(&rgb, 4);
        let w = &h.waveform;
        let top = WAVEFORM_LEVELS - 1;
        // Column 0 of the image is in waveform column 0, column 3 in the last quarter.
        assert_eq!(w.luma[0], 1, "one black pixel in the first column");
        assert_eq!(w.luma[top * WAVEFORM_COLUMNS + 3 * WAVEFORM_COLUMNS / 4], 2, "two white pixels");
        assert_eq!(w.red[top * WAVEFORM_COLUMNS], 1, "the red pixel is at the top of the red parade");
        assert_eq!(w.green[0], 2);
        assert_eq!(w.luma.iter().map(|&c| u64::from(c)).sum::<u64>(), 8);
        assert!(w.scale() >= 1);
        assert!(Histogram::of_rgb8(&rgb).waveform.is_empty());
        let cells = w.intensities(&w.luma);
        assert_eq!(cells.len(), WAVEFORM_COLUMNS * WAVEFORM_LEVELS);
        assert!(cells[3 * WAVEFORM_COLUMNS / 4] > 0.5, "white is in the top row");
        assert!(cells[(WAVEFORM_LEVELS - 1) * WAVEFORM_COLUMNS] > 0.5, "black is in the bottom row");
    }

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
