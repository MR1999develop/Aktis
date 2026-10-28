//! Looking closely at the image on screen, as in a photo viewer: the
//! eyedropper (hold the right button: the colour under the pointer) and the
//! loupe (hold the middle button: a magnified circle). Both read the 8-bit
//! image on screen.

/// Side of the loupe in points.
pub const LOUPE_SIDE: usize = 320;

/// Magnification of the loupe over the current on-screen size.
pub const LOUPE_FACTOR: f32 = 4.0;

/// Background outside the image in the loupe.
const OUTSIDE: [u8; 4] = [0x14, 0x14, 0x14, 255];

/// The 8-bit colour at pixel `(x, y)` of a packed RGB image `width` wide.
#[must_use]
pub fn pixel(rgb: &[u8], width: usize, x: usize, y: usize) -> Option<[u8; 3]> {
    if x >= width {
        return None;
    }
    let i = (y * width + x) * 3;
    rgb.get(i..i + 3).map(|p| [p[0], p[1], p[2]])
}

/// Hue (degrees), saturation and lightness (percent) of an 8-bit colour.
#[must_use]
pub fn hsl(rgb: [u8; 3]) -> (u16, u8, u8) {
    let [r, g, b] = rgb.map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d <= f32::EPSILON {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if r >= g && r >= b {
            60.0 * ((g - b) / d).rem_euclid(6.0)
        } else if g >= b {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (h, s)
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0–360 and 0–100
    let out = (
        (h.round() as u16) % 360,
        (s * 100.0).round().clamp(0.0, 100.0) as u8,
        (l * 100.0).round().clamp(0.0, 100.0) as u8,
    );
    out
}

/// The loupe's pixels: [`LOUPE_SIDE`]² RGBA bytes of the packed RGB image
/// (`width` × `height`) around `center` (image pixels), magnified `mag`
/// times (screen points per image pixel), nearest pixel, dark outside the
/// image.
#[must_use]
pub fn loupe_patch(rgb: &[u8], width: usize, height: usize, center: [f32; 2], mag: f32) -> Vec<u8> {
    let side = LOUPE_SIDE;
    let mut out = Vec::with_capacity(side * side * 4);
    #[allow(clippy::cast_precision_loss)] // a small size
    let half = side as f32 / 2.0;
    let step = 1.0 / mag.max(0.05);
    for ly in 0..side {
        #[allow(clippy::cast_precision_loss)] // a small size
        let sy = (center[1] + (ly as f32 - half + 0.5) * step).floor();
        for lx in 0..side {
            #[allow(clippy::cast_precision_loss)] // a small size
            let sx = (center[0] + (lx as f32 - half + 0.5) * step).floor();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked to be inside
            let inside = (sx >= 0.0 && sy >= 0.0)
                .then_some((sx as usize, sy as usize))
                .filter(|(x, y)| *x < width && *y < height);
            match inside.and_then(|(x, y)| pixel(rgb, width, x, y)) {
                Some([r, g, b]) => out.extend([r, g, b, 255]),
                None => out.extend(OUTSIDE),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_of_known_colours() {
        assert_eq!(hsl([255, 0, 0]), (0, 100, 50));
        assert_eq!(hsl([0, 255, 0]), (120, 100, 50));
        assert_eq!(hsl([0, 0, 255]), (240, 100, 50));
        assert_eq!(hsl([128, 128, 128]), (0, 0, 50));
        assert_eq!(hsl([255, 255, 255]), (0, 0, 100));
    }

    #[test]
    fn the_loupe_magnifies_around_the_centre_and_is_dark_outside() {
        // 4 × 2 image: pixel (1, 0) red, the rest grey.
        let mut rgb = vec![100_u8; 4 * 2 * 3];
        rgb[3..6].copy_from_slice(&[255, 0, 0]);
        let patch = loupe_patch(&rgb, 4, 2, [1.5, 0.5], 100.0);
        assert_eq!(patch.len(), LOUPE_SIDE * LOUPE_SIDE * 4);
        let at = |x: usize, y: usize| &patch[(y * LOUPE_SIDE + x) * 4..(y * LOUPE_SIDE + x) * 4 + 4];
        assert_eq!(at(LOUPE_SIDE / 2, LOUPE_SIDE / 2), &[255, 0, 0, 255], "the centre is the red pixel");
        assert_eq!(at(LOUPE_SIDE / 2 + 60, LOUPE_SIDE / 2), &[100, 100, 100, 255], "one pixel to the right");
        assert_eq!(at(LOUPE_SIDE / 2, 0), &OUTSIDE, "above the image");
        assert_eq!(pixel(&rgb, 4, 4, 0), None);
    }
}
