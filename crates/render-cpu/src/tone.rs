//! The output render: scene-referred linear light to display-referred sRGB
//! in [0, 1], through a film-like tone curve.
//!
//! The curve works on stops (log2 of the light relative to middle grey),
//! like Capture One's film curves: through the midtones every stop moves
//! the displayed value by the same amount, so an exposure change brightens
//! the whole picture evenly instead of mostly the highlights; the shadows
//! sink into a toe and the highlights roll off into a shoulder instead of
//! clipping. It is applied to the brightest channel and every channel is
//! scaled by the same factor, so hues are kept; colours far above white
//! fade to white as film does. Colours outside sRGB are brought inside by
//! moving them towards the grey of the same luminance, never by clipping
//! one channel.

use core_model::Matrix3;

/// Middle grey of the scene (linear).
pub const GREY: f32 = 0.18;
/// Displayed value (sRGB-encoded) per stop through the midtones.
pub const SLOPE: f32 = 0.2;
/// Stops above middle grey where the shoulder starts (white is +2.47).
pub const SHOULDER_FROM: f32 = 2.0;
/// Brightest-channel value from which a colour starts fading to white…
pub const WHITE_FROM: f32 = 1.0;
/// …and at which it has turned fully white (3 stops over white).
pub const WHITE_AT: f32 = 8.0;

/// Middle grey as displayed: the sRGB encoding of [`GREY`], so midtones
/// keep their brightness.
fn grey_out() -> f32 {
    core_color::srgb_encode(GREY)
}

/// The tone curve on one value: scene-linear in, display-linear out (to be
/// sRGB-encoded). Monotonic, continuous with a continuous slope; black stays
/// black and the top approaches 1 without reaching it.
#[must_use]
pub fn film_curve(n: f32) -> f32 {
    if n.is_nan() || n <= 0.0 {
        return 0.0;
    }
    if n.is_infinite() {
        return 1.0;
    }
    let e = (n / GREY).log2();
    let g = grey_out();
    let y = if e <= 0.0 {
        // Toe: same slope as the midtones at grey, fading to black.
        g * (SLOPE / g * e).exp()
    } else if e <= SHOULDER_FROM {
        g + SLOPE * e
    } else {
        // Shoulder: same slope at its start, approaching 1.
        let y1 = g + SLOPE * SHOULDER_FROM;
        let room = 1.0 - y1;
        y1 + room * (1.0 - (-(e - SHOULDER_FROM) * SLOPE / room).exp())
    };
    core_color::srgb_decode(y.clamp(0.0, 1.0))
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One pixel of the working space as linear sRGB in [0, 1]; `to_srgb` is
/// the matrix from the working space to linear sRGB.
#[must_use]
pub fn output_rgb(rgb: [f32; 3], to_srgb: &Matrix3) -> [f32; 3] {
    let n = rgb[0].max(rgb[1]).max(rgb[2]);
    if n.is_nan() || n <= 0.0 || n.is_infinite() {
        return [0.0; 3];
    }
    let f = film_curve(n);
    let scale = f / n;
    // Towards white as the light grows far above white.
    let w = smoothstep(WHITE_FROM, WHITE_AT, n);
    let c = rgb.map(|v| {
        let kept = v * scale;
        kept + (f - kept) * w
    });
    gamut_map(to_srgb.apply(c))
}

/// Brings linear sRGB inside [0, 1] by desaturating towards the grey of the
/// same luminance; non-finite values become black.
#[must_use]
pub fn gamut_map(rgb: [f32; 3]) -> [f32; 3] {
    if !rgb.iter().all(|v| v.is_finite()) {
        return [0.0; 3];
    }
    let y = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).clamp(0.0, 1.0);
    let lo = rgb[0].min(rgb[1]).min(rgb[2]);
    let hi = rgb[0].max(rgb[1]).max(rgb[2]);
    let mut t: f32 = 1.0;
    if lo < 0.0 && y - lo > 0.0 {
        t = t.min(y / (y - lo));
    }
    if hi > 1.0 && hi - y > 0.0 {
        t = t.min((1.0 - y) / (hi - y));
    }
    rgb.map(|v| (y + (v - y) * t).clamp(0.0, 1.0))
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact results of exact inputs
mod tests {
    use super::*;

    const ID: Matrix3 = Matrix3::IDENTITY;

    fn displayed(n: f32) -> f32 {
        core_color::srgb_encode(film_curve(n))
    }

    #[test]
    fn middle_grey_keeps_its_brightness() {
        assert!((film_curve(GREY) - GREY).abs() < 1e-4);
    }

    #[test]
    fn every_stop_through_the_midtones_moves_the_display_by_the_same_amount() {
        let step = |e: f32| displayed(GREY * (e + 1.0).exp2()) - displayed(GREY * e.exp2());
        for e in [0.0_f32, 0.5, 1.0] {
            assert!((step(e) - SLOPE).abs() < 0.005, "stop {e}: {}", step(e));
        }
        // The shadows compress gently into the toe, as film does, but still separate.
        assert!(step(-1.0) < SLOPE && step(-1.0) > 0.12);
        assert!(step(-4.0) > 0.03);
    }

    #[test]
    fn the_curve_is_monotonic_and_continuous_and_rolls_off_highlights() {
        let mut last = 0.0;
        for i in 1..4000 {
            #[allow(clippy::cast_precision_loss)]
            let n = i as f32 * 0.004;
            let y = film_curve(n);
            assert!(y >= last && y < 1.0, "{n} → {y}");
            assert!(y - last < 0.01, "no jump at {n}");
            last = y;
        }
        let white = displayed(1.0);
        assert!(white > 0.9 && white < 0.96, "white just below the top: {white}");
        assert!(displayed(8.0) > 0.99, "three stops over white is white");
        assert!(displayed(0.001) < 0.03, "deep shadows stay dark");
        assert_eq!(film_curve(0.0), 0.0);
        assert_eq!(film_curve(-1.0), 0.0);
        assert_eq!(film_curve(f32::NAN), 0.0);
    }

    #[test]
    fn bright_colours_keep_their_hue_then_fade_to_white() {
        // An orange one stop over white: channel ratios kept, not clipped to yellow.
        let orange = [2.0, 1.0, 0.2];
        let out = output_rgb(orange, &ID);
        assert!(out[0] < 1.0);
        let ratio = out[1] / out[0];
        assert!(ratio > 0.5 && ratio < 0.6, "the hue is kept: {out:?}");
        // Far above white: white.
        let out = output_rgb([60.0, 30.0, 6.0], &ID);
        assert!(out.iter().all(|v| *v > 0.98), "{out:?}");
        // More light is never darker.
        let a = output_rgb([1.0, 0.5, 0.1], &ID);
        let b = output_rgb([1.5, 0.75, 0.15], &ID);
        assert!(b.iter().zip(&a).all(|(b, a)| b >= a));
        assert_eq!(output_rgb([-0.1, -0.2, -0.3], &ID), [0.0; 3]);
    }

    #[test]
    fn out_of_gamut_colours_are_desaturated_not_clipped() {
        let out = gamut_map([1.2, 0.4, -0.1]);
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(out[2].abs() < 1e-6 || out[0] > 1.0 - 1e-6, "one channel touches the edge: {out:?}");
        assert_eq!(gamut_map([f32::NAN, 0.0, 0.0]), [0.0; 3]);
        assert_eq!(gamut_map([-0.2, -0.1, -0.3]), [0.0; 3]);
        assert_eq!(gamut_map([0.25, 0.5, 0.75]), [0.25, 0.5, 0.75]);
    }
}
