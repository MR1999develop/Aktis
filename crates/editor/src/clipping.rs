//! The exposure warning: which pixels of the image on screen are burnt
//! (highlights) or blocked (shadows), to show them in a colour over the
//! photo. A view aid only: it never reaches the recipe or an export.
//!
//! It is measured on the displayed 8-bit sRGB values, after the output
//! render, because that is what an export of the same view writes.

/// What the warning says about one pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clip {
    /// Within range.
    None,
    /// At least one channel at or above the highlight threshold.
    Highlight,
    /// Every channel at or below the shadow threshold.
    Shadow,
}

/// Thresholds of the warning on 0–255 display values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// A channel at this value or above counts as burnt.
    pub highlight: u8,
    /// All channels at this value or below count as blocked; `None` shows
    /// only the highlights.
    pub shadow: Option<u8>,
}

impl Default for Thresholds {
    /// As in Capture One: highlights from 250, no shadow warning.
    fn default() -> Self {
        Self { highlight: 250, shadow: None }
    }
}

/// The verdict for one `[r, g, b]` pixel.
#[must_use]
pub fn classify(rgb: [u8; 3], t: Thresholds) -> Clip {
    if rgb.iter().any(|&v| v >= t.highlight) {
        Clip::Highlight
    } else if t.shadow.is_some_and(|s| rgb.iter().all(|&v| v <= s)) {
        Clip::Shadow
    } else {
        Clip::None
    }
}

/// The overlay for `rgb` (3 bytes per pixel): `highlight` and `shadow`
/// (RGBA, premultiplied) where the warning applies, transparent elsewhere.
/// Returns 4 bytes per pixel; an incomplete last pixel is ignored.
#[must_use]
pub fn overlay(rgb: &[u8], t: Thresholds, highlight: [u8; 4], shadow: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for p in rgb.chunks_exact(3) {
        let colour = match classify([p[0], p[1], p[2]], t) {
            Clip::Highlight => highlight,
            Clip::Shadow => shadow,
            Clip::None => [0; 4],
        };
        out.extend_from_slice(&colour);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_burnt_channel_is_a_highlight_and_only_all_dark_channels_a_shadow() {
        let t = Thresholds { shadow: Some(5), ..Thresholds::default() };
        assert_eq!(classify([255, 10, 10], t), Clip::Highlight, "a burnt red");
        assert_eq!(classify([249, 249, 249], t), Clip::None, "just under the threshold");
        assert_eq!(classify([250, 0, 0], t), Clip::Highlight, "at the threshold");
        assert_eq!(classify([0, 0, 0], t), Clip::Shadow);
        assert_eq!(classify([5, 5, 5], t), Clip::Shadow, "at the shadow threshold");
        assert_eq!(classify([0, 0, 6], t), Clip::None, "a deep blue is not blocked");
        let highlights_only = Thresholds { shadow: None, ..t };
        assert_eq!(classify([0, 0, 0], highlights_only), Clip::None);
    }

    #[test]
    fn the_overlay_has_four_bytes_per_pixel_and_is_clear_where_nothing_clips() {
        let rgb = [255, 255, 255, 128, 128, 128, 0, 0, 0, 7];
        let t = Thresholds { shadow: Some(5), ..Thresholds::default() };
        let o = overlay(&rgb, t, [200, 0, 0, 200], [0, 0, 200, 200]);
        assert_eq!(o, vec![200, 0, 0, 200, 0, 0, 0, 0, 0, 0, 200, 200]);
        assert!(overlay(&[], Thresholds::default(), [1; 4], [2; 4]).is_empty());
    }
}
