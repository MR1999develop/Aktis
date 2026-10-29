//! The output render: scene-referred linear light to display-referred sRGB
//! in [0, 1], through a film-like tone curve.
//!
//! The curve works on stops (log2 of the light relative to middle grey),
//! like Capture One's film curves: through the midtones every stop moves
//! the displayed value by the same amount, so an exposure change brightens
//! the whole picture evenly instead of mostly the highlights; the shadows
//! sink into a toe and the highlights roll off into a shoulder instead of
//! clipping. It is applied to the brightest channel and every channel is
//! scaled by the same factor, so hues are kept; in the shoulder colours
//! pale towards white as the curve flattens, and far above white they
//! fade to white, as film does. Colours outside sRGB are brought inside by
//! moving them towards the grey of the same luminance, never by clipping
//! one channel.

use core_model::{BaseCurve, Matrix3};

/// Middle grey of the scene (linear).
pub const GREY: f32 = 0.18;
/// Displayed value (sRGB-encoded) per stop through the midtones of Film Standard.
pub const SLOPE: f32 = 0.21;
/// Where Film Standard shows middle grey (sRGB-encoded): a little brighter
/// than its plain encoding (0.46), as Capture One's film curves do.
pub const FILM_GREY: f32 = 0.5;
/// Colour boost of Film Standard: film curves give livelier colours than a
/// plain tone curve.
pub const FILM_SATURATION: f32 = 1.15;
/// Stops above middle grey where Film Standard's shoulder starts (white is +2.47).
pub const SHOULDER_FROM: f32 = 2.0;
/// Brightest-channel value from which a colour starts fading to white…
pub const WHITE_FROM: f32 = 1.0;
/// …and at which it has turned fully white (3 stops over white).
pub const WHITE_AT: f32 = 8.0;
/// How much of the shoulder's compression also takes colour away: bright
/// colours pale towards white as they approach it, as on film and in Capture One,
/// instead of staying as saturated as in the midtones (exposure would
/// otherwise look like it adds saturation).
pub const HIGHLIGHT_FADE: f32 = 0.5;

/// The shape of one base curve: every curve passes through middle grey at
/// its displayed value, with `slope` through the midtones.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    /// Displayed (sRGB-encoded) value of middle grey.
    grey: f32,
    /// Displayed value per stop at middle grey.
    slope: f32,
    /// Stops above grey where the shoulder starts.
    shoulder_from: f32,
    /// How the toe reaches black: `None` exponentially (Film Standard);
    /// `Some(p)` with `p > 0` a power law that keeps the deep shadows
    /// brighter (less contrast there).
    toe_power: Option<f32>,
    /// Colour boost (1 = none).
    saturation: f32,
}

impl Shape {
    fn of(curve: BaseCurve) -> Self {
        let film = Self {
            grey: FILM_GREY,
            slope: SLOPE,
            shoulder_from: SHOULDER_FROM,
            toe_power: None,
            saturation: FILM_SATURATION,
        };
        match curve.resolved() {
            BaseCurve::FilmExtraShadow => Self { toe_power: Some(2.5), ..film },
            BaseCurve::FilmHighContrast => Self { slope: 0.24, shoulder_from: 1.6, saturation: 1.2, ..film },
            BaseCurve::LinearResponse => {
                Self { grey: grey_out(), slope: 0.15, shoulder_from: 2.3, toe_power: Some(4.0), saturation: 1.0 }
            }
            BaseCurve::Auto | BaseCurve::FilmStandard => film,
        }
    }
}

/// Middle grey as displayed: the sRGB encoding of [`GREY`], so midtones
/// keep their brightness.
fn grey_out() -> f32 {
    core_color::srgb_encode(GREY)
}

/// Film Standard on one value: scene-linear in, display-linear out (to be
/// sRGB-encoded). See [`curve_value`].
#[must_use]
pub fn film_curve(n: f32) -> f32 {
    curve_value(BaseCurve::FilmStandard, n)
}

/// Base curve `curve` on one value: scene-linear in, display-linear out (to
/// be sRGB-encoded). It works on stops from middle grey: a straight line
/// through the midtones, a toe into black and a shoulder towards white.
/// Monotonic, continuous with a continuous slope; black stays black and the
/// top approaches 1 without reaching it.
#[must_use]
pub fn curve_value(curve: BaseCurve, n: f32) -> f32 {
    if n.is_nan() || n <= 0.0 {
        return 0.0;
    }
    if n.is_infinite() {
        return 1.0;
    }
    let shape = Shape::of(curve);
    let e = (n / GREY).log2();
    let g = shape.grey;
    let s = shape.slope;
    let y = if e <= 0.0 {
        // Toe: the midtone slope at grey, fading to black.
        match shape.toe_power {
            None => g * (s / g * e).exp(),
            Some(p) => g * (1.0 - s / (g * p) * e).powf(-p),
        }
    } else if e <= shape.shoulder_from {
        g + s * e
    } else {
        // Shoulder: the same slope at its start, approaching 1.
        let y1 = g + s * shape.shoulder_from;
        let room = 1.0 - y1;
        y1 + room * (1.0 - (-(e - shape.shoulder_from) * s / room).exp())
    };
    core_color::srgb_decode(y.clamp(0.0, 1.0))
}

/// How far the curve has flattened at scene value `n`: 0 below the
/// shoulder, rising towards 1 as the slope falls to nothing.
#[must_use]
pub fn shoulder_compression(curve: BaseCurve, n: f32) -> f32 {
    if n.is_nan() || n <= 0.0 {
        return 0.0;
    }
    if n.is_infinite() {
        return 1.0;
    }
    let shape = Shape::of(curve);
    let e = (n / GREY).log2();
    if e <= shape.shoulder_from {
        return 0.0;
    }
    let room = 1.0 - (shape.grey + shape.slope * shape.shoulder_from);
    1.0 - (-(e - shape.shoulder_from) * shape.slope / room).exp()
}

/// Rec. 2020 luminance weights (the working space).
const LUMA: [f32; 3] = [0.2627, 0.6780, 0.0593];

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One pixel of the working space as linear sRGB in [0, 1] through Film
/// Standard; `to_srgb` is the matrix from the working space to linear sRGB.
#[must_use]
pub fn output_rgb(rgb: [f32; 3], to_srgb: &Matrix3) -> [f32; 3] {
    output_rgb_with(rgb, to_srgb, BaseCurve::FilmStandard)
}

/// [`output_rgb`] through base curve `curve`.
#[must_use]
pub fn output_rgb_with(rgb: [f32; 3], to_srgb: &Matrix3, curve: BaseCurve) -> [f32; 3] {
    let n = rgb[0].max(rgb[1]).max(rgb[2]);
    if n.is_nan() || n <= 0.0 || n.is_infinite() {
        return [0.0; 3];
    }
    let f = curve_value(curve, n);
    let scale = f / n;
    let kept = rgb.map(|v| v * scale);
    // The curve's colour boost, around the grey of the same luminance;
    // none left in the shoulder, where colours pale instead.
    let sat = 1.0 + (Shape::of(curve).saturation - 1.0) * (1.0 - shoulder_compression(curve, n));
    let ky = kept[0] * LUMA[0] + kept[1] * LUMA[1] + kept[2] * LUMA[2];
    let kept = kept.map(|v| ky + (v - ky) * sat);
    // Paler in the shoulder: the other channels rise towards the brightest,
    // so the colour whitens as on film and more light is never darker.
    let fade = HIGHLIGHT_FADE * shoulder_compression(curve, n);
    let top = kept[0].max(kept[1]).max(kept[2]);
    let paled = kept.map(|v| v + (top - v) * fade);
    // Towards white as the light grows far above white.
    let w = smoothstep(WHITE_FROM, WHITE_AT, n);
    let c = paled.map(|v| v + (f - v) * w);
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
    fn middle_grey_shows_a_little_brighter_than_its_plain_encoding() {
        assert!((displayed(GREY) - FILM_GREY).abs() < 1e-4);
        assert!(FILM_GREY > core_color::srgb_encode(GREY));
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
        assert!(white > 0.94 && white < 0.99, "white just below the top: {white}");
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
        assert!(out[0] > out[1] && out[1] > out[2], "still orange: {out:?}");
        assert!(ratio > 0.5 && ratio < 0.85, "not clipped to yellow: {out:?}");
        // Far above white: white.
        let out = output_rgb([60.0, 30.0, 6.0], &ID);
        assert!(out.iter().all(|v| *v > 0.98), "{out:?}");
        // More light is never darker.
        let a = output_rgb([1.0, 0.5, 0.1], &ID);
        let b = output_rgb([1.5, 0.75, 0.15], &ID);
        assert!(b.iter().zip(&a).all(|(b, a)| b >= a), "{a:?} {b:?}");
        assert_eq!(output_rgb([-0.1, -0.2, -0.3], &ID), [0.0; 3]);
    }

    #[test]
    fn more_exposure_does_not_add_saturation() {
        // The same orange, one stop brighter each time: as displayed, its
        // channels come closer (paler), never further apart.
        let spread = |k: f32| {
            let out = output_rgb([0.2 * k, 0.1 * k, 0.02 * k], &ID);
            out[2] / out[0]
        };
        let mut last = spread(1.0);
        for stop in 1..6 {
            let s = spread(2.0_f32.powi(stop));
            assert!(s >= last - 1e-4, "stop {stop}: {s} < {last}");
            last = s;
        }
        // Midtones are untouched by the fade.
        assert!(shoulder_compression(BaseCurve::FilmStandard, GREY).abs() < 1e-9);
        assert!(shoulder_compression(BaseCurve::FilmStandard, 4.0) > 0.8);
    }

    #[test]
    fn every_base_curve_keeps_grey_rises_steadily_and_stays_below_white() {
        for curve in BaseCurve::ALL {
            let grey = core_color::srgb_encode(curve_value(curve, GREY));
            assert!((0.45..=0.52).contains(&grey), "{curve:?} shows middle grey at {grey}");
            let mut last = curve_value(curve, 0.004);
            for i in 2..6000 {
                #[allow(clippy::cast_precision_loss)]
                let n = i as f32 * 0.004;
                let y = curve_value(curve, n);
                assert!(y >= last && y < 1.0, "{curve:?}: {n} → {y}");
                assert!(y - last < 0.01, "{curve:?}: no jump at {n}");
                last = y;
            }
            assert!(curve_value(curve, 1e-4) < 0.01, "{curve:?}: black stays near black");
        }
        assert_eq!(curve_value(BaseCurve::Auto, 0.05), film_curve(0.05), "Auto is Film Standard for now");
    }

    #[test]
    fn the_curves_differ_as_their_names_say() {
        let at = |c: BaseCurve, stops: f32| core_color::srgb_encode(curve_value(c, GREY * stops.exp2()));
        let (std, shadow, high, linear) = (
            BaseCurve::FilmStandard,
            BaseCurve::FilmExtraShadow,
            BaseCurve::FilmHighContrast,
            BaseCurve::LinearResponse,
        );
        // Extra Shadow: brighter deep shadows, the same midtones and highlights.
        assert!(at(shadow, -4.0) > at(std, -4.0) + 0.02);
        assert!((at(shadow, 1.0) - at(std, 1.0)).abs() < 1e-5);
        // High Contrast: darker shadows, brighter highlights.
        assert!(at(high, -2.0) < at(std, -2.0) && at(high, 1.5) > at(std, 1.5));
        // Linear Response: the flattest midtones, shadows kept, highlights gentler.
        assert!(at(linear, 1.0) - at(linear, -1.0) < at(std, 1.0) - at(std, -1.0));
        assert!(at(linear, -4.0) > at(std, -4.0) && at(linear, 2.0) < at(std, 2.0));
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
