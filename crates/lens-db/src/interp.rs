//! Calibration data for any focal length, aperture and distance, by the same
//! interpolation lensfun uses (`lfLens::Interpolate*`), so corrections match
//! other lensfun-based programs.
//!
//! Distortion and TCA: a cubic Hermite spline over focal length through the
//! two nearest samples on each side, with the coefficients multiplied by the
//! focal length before interpolating (they follow a 1/f law). Outside the
//! measured range the nearest sample is used unchanged. Vignetting: inverse
//! distance weighting (power 3.5) over focal length, 1/aperture and
//! 1/distance; no result when every sample is far away.

use crate::model::{CalibrationSet, Distortion, Lens, Tca, Vignetting};

/// The calibration set to use for an image with crop factor `crop`. Data
/// measured on a smaller sensor does not cover the image's corners, so
/// lensfun accepts a set only when `crop / set.crop ≥ 0.96` and prefers the
/// set whose sensor is closest in size (the smallest such ratio).
fn pick_set(lens: &Lens, crop: f32, has: impl Fn(&CalibrationSet) -> bool) -> Option<&CalibrationSet> {
    lens.calibrations
        .iter()
        .filter(|s| has(s))
        .map(|s| (crop / s.crop_factor, s))
        .filter(|(r, _)| *r >= 0.96)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
}

/// Cubic Hermite through `y2` (t = 0) and `y3` (t = 1) with Catmull-Rom
/// tangents; a missing outer point gives a secant tangent (lensfun's
/// `_lf_interpolate`).
fn hermite(y1: Option<f32>, y2: f32, y3: f32, y4: Option<f32>, t: f32) -> f32 {
    let tg2 = y1.map_or(y3 - y2, |y1| (y3 - y1) * 0.5);
    let tg3 = y4.map_or(y3 - y2, |y4| (y4 - y2) * 0.5);
    let t2 = t * t;
    let t3 = t2 * t;
    (2.0 * t3 - 3.0 * t2 + 1.0) * y2 + (t3 - 2.0 * t2 + t) * tg2 + (-2.0 * t3 + 3.0 * t2) * y3 + (t3 - t2) * tg3
}

/// The two nearest samples above `focal` (nearest first) and the two below.
/// `Err(sample)` when one sits exactly at `focal`.
#[allow(clippy::type_complexity)] // a local helper's tuple, named at the call sites
fn neighbours<T: Copy>(
    samples: &[T],
    focal: f32,
    focal_of: impl Fn(&T) -> f32,
) -> Result<([Option<T>; 2], [Option<T>; 2]), T> {
    if let Some(exact) = samples.iter().find(|s| (focal_of(s) - focal).abs() < 1e-4) {
        return Err(*exact);
    }
    let mut above: Vec<T> = samples.iter().copied().filter(|s| focal_of(s) > focal).collect();
    let mut below: Vec<T> = samples.iter().copied().filter(|s| focal_of(s) < focal).collect();
    above.sort_by(|a, b| focal_of(a).total_cmp(&focal_of(b)));
    below.sort_by(|a, b| focal_of(b).total_cmp(&focal_of(a)));
    Ok(([above.first().copied(), above.get(1).copied()], [below.first().copied(), below.get(1).copied()]))
}

/// Interpolated distortion for `focal`, with the set it came from.
pub(crate) fn distortion(lens: &Lens, crop: f32, focal: f32) -> Option<(Distortion, &CalibrationSet)> {
    let set = pick_set(lens, crop, |s| !s.distortion.is_empty())?;
    let (above, below) = match neighbours(&set.distortion, focal, |d| d.focal) {
        Err(exact) => return Some((exact, set)),
        Ok(n) => n,
    };
    let (p1, p2) = match (above[0], below[0]) {
        (Some(a), Some(b)) => (a, b),
        (Some(only), None) | (None, Some(only)) => return Some((only, set)),
        (None, None) => return None,
    };
    let (p0, p3) = (above[1], below[1]);
    let t = (focal - p1.focal) / (p2.focal - p1.focal);
    let mut out = Distortion { model: p1.model, focal, real_focal: 0.0, terms: [0.0; 3] };
    out.real_focal = hermite(p0.map(|p| p.real_focal), p1.real_focal, p2.real_focal, p3.map(|p| p.real_focal), t);
    for i in 0..3 {
        // Coefficients scale with 1/f: interpolate k·f, then divide by f.
        let v = |d: &Distortion| d.terms[i] * d.focal;
        out.terms[i] = hermite(p0.as_ref().map(v), v(&p1), v(&p2), p3.as_ref().map(v), t) / focal;
    }
    Some((out, set))
}

/// Interpolated chromatic aberration for `focal`, with the set it came from.
pub(crate) fn tca(lens: &Lens, crop: f32, focal: f32) -> Option<(Tca, &CalibrationSet)> {
    let set = pick_set(lens, crop, |s| !s.tca.is_empty())?;
    let (above, below) = match neighbours(&set.tca, focal, |t| t.focal) {
        Err(exact) => return Some((exact, set)),
        Ok(n) => n,
    };
    let (p1, p2) = match (above[0], below[0]) {
        (Some(a), Some(b)) => (a, b),
        (Some(only), None) | (None, Some(only)) => return Some((only, set)),
        (None, None) => return None,
    };
    let (p0, p3) = (above[1], below[1]);
    let t = (focal - p1.focal) / (p2.focal - p1.focal);
    let mut out = Tca { model: p1.model, focal, terms: [0.0; 6] };
    for i in 0..6 {
        // The zeroth-order terms (close to 1) stay constant; the others follow 1/f.
        let scale = |f: f32| if i < 2 { 1.0 } else { f };
        let v = |s: &Tca| s.terms[i] * scale(s.focal);
        out.terms[i] = hermite(p0.as_ref().map(v), v(&p1), v(&p2), p3.as_ref().map(v), t) / scale(focal);
    }
    Some((out, set))
}

/// Interpolated vignetting, with the set it came from.
pub(crate) fn vignetting(
    lens: &Lens,
    crop: f32,
    focal: f32,
    aperture: f32,
    distance: f32,
) -> Option<(Vignetting, &CalibrationSet)> {
    let set = pick_set(lens, crop, |s| !s.vignetting.is_empty())?;
    let range = crate::matching::lens_parameters(lens);
    let span = range.max_focal - range.min_focal;
    let norm_f = |f: f32| if span == 0.0 { f - range.min_focal } else { (f - range.min_focal) / span };
    let point = |f: f32, a: f32, d: f32| [norm_f(f), 4.0 / a, 0.1 / d];
    let target = point(focal, aperture, distance);

    let mut sum = [0.0_f32; 3];
    let mut total = 0.0_f32;
    let mut nearest = f32::MAX;
    for v in &set.vignetting {
        let p = point(v.focal, v.aperture, v.distance);
        let dist = ((p[0] - target[0]).powi(2) + (p[1] - target[1]).powi(2) + (p[2] - target[2]).powi(2)).sqrt();
        if dist < 0.0001 {
            return Some((*v, set));
        }
        nearest = nearest.min(dist);
        let w = 1.0 / dist.powf(3.5);
        for (s, k) in sum.iter_mut().zip(v.terms) {
            *s += w * k;
        }
        total += w;
    }
    // Every sample too far away: better no correction than a wild guess.
    if nearest > 1.0 || total <= 0.0 || !total.is_finite() {
        return None;
    }
    Some((Vignetting { focal, aperture, distance, terms: sum.map(|s| s / total) }, set))
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // samples are returned unchanged
mod tests {
    use super::*;
    use crate::model::{DistortionModel, LensType, Name, TcaModel};

    fn lens(sets: Vec<CalibrationSet>) -> Lens {
        Lens {
            maker: Name::new("Test"),
            model: Name::new("Zoom 20-100mm f/4"),
            mounts: vec!["M".to_owned()],
            lens_type: LensType::Rectilinear,
            focal_range: [20.0, 100.0],
            aperture_range: [4.0, 22.0],
            center: [0.0, 0.0],
            calibrations: sets,
        }
    }

    fn set(crop: f32) -> CalibrationSet {
        CalibrationSet { crop_factor: crop, aspect_ratio: 1.5, distortion: vec![], tca: vec![], vignetting: vec![] }
    }

    fn poly3(focal: f32, k1: f32) -> Distortion {
        Distortion { model: DistortionModel::Poly3, focal, real_focal: focal * (1.0 - k1), terms: [k1, 0.0, 0.0] }
    }

    #[test]
    fn the_hermite_spline_passes_through_its_points_and_is_linear_on_a_line() {
        assert!((hermite(Some(0.0), 1.0, 2.0, Some(3.0), 0.0) - 1.0).abs() < 1e-6);
        assert!((hermite(Some(0.0), 1.0, 2.0, Some(3.0), 1.0) - 2.0).abs() < 1e-6);
        assert!((hermite(Some(0.0), 1.0, 2.0, Some(3.0), 0.25) - 1.25).abs() < 1e-6);
        assert!((hermite(None, 1.0, 2.0, None, 0.5) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn distortion_following_one_over_f_is_interpolated_exactly() {
        // k1 = 1/f: k1·f is constant, so the spline is exact at any focal length.
        let mut s = set(1.0);
        s.distortion = [20.0_f32, 35.0, 50.0, 100.0].map(|f| poly3(f, 0.5 / f)).to_vec();
        let l = lens(vec![s]);
        let (d, _) = distortion(&l, 1.0, 42.0).expect("data");
        assert!((d.terms[0] - 0.5 / 42.0).abs() < 1e-6, "{d:?}");
        assert_eq!(d.focal, 42.0);
        // Exact sample and outside the range: samples unchanged.
        assert_eq!(distortion(&l, 1.0, 35.0).map(|(d, _)| d), Some(poly3(35.0, 0.5 / 35.0)));
        assert_eq!(distortion(&l, 1.0, 10.0).map(|(d, _)| d), Some(poly3(20.0, 0.5 / 20.0)));
    }

    #[test]
    fn calibrations_from_a_larger_sensor_are_preferred_and_smaller_ones_refused() {
        let mut ff = set(1.0);
        ff.distortion = vec![poly3(50.0, -0.01)];
        let mut apsc = set(1.5);
        apsc.distortion = vec![poly3(50.0, -0.02)];
        let l = lens(vec![ff, apsc]);
        // An APS-C image uses the APS-C data; a full-frame image only the full-frame data.
        assert_eq!(distortion(&l, 1.53, 50.0).map(|(d, _)| d.terms[0]), Some(-0.02));
        assert_eq!(distortion(&l, 1.0, 50.0).map(|(d, _)| d.terms[0]), Some(-0.01));
        let only_apsc = lens(vec![l.calibrations[1].clone()]);
        assert!(distortion(&only_apsc, 1.0, 50.0).is_none());
    }

    #[test]
    fn tca_keeps_the_constant_term_and_scales_the_others() {
        let mut s = set(1.0);
        s.tca = [20.0_f32, 100.0]
            .map(|f| Tca { model: TcaModel::Poly3, focal: f, terms: [1.001, 0.999, 0.0, 0.0, 0.2 / f, -0.1 / f] })
            .to_vec();
        let (t, _) = tca(&lens(vec![s]), 1.0, 60.0).expect("data");
        assert!((t.terms[0] - 1.001).abs() < 1e-6 && (t.terms[1] - 0.999).abs() < 1e-6, "{t:?}");
        assert!((t.terms[4] - 0.2 / 60.0).abs() < 1e-6 && (t.terms[5] + 0.1 / 60.0).abs() < 1e-6, "{t:?}");
    }

    #[test]
    fn vignetting_weights_nearby_samples_and_refuses_distant_ones() {
        let mut s = set(1.0);
        let v = |a: f32, k: f32| Vignetting { focal: 50.0, aperture: a, distance: 10.0, terms: [k, 0.0, 0.0] };
        s.vignetting = vec![v(4.0, -0.6), v(8.0, -0.2)];
        let l = lens(vec![s]);
        assert_eq!(vignetting(&l, 1.0, 50.0, 4.0, 10.0).map(|(v, _)| v.terms[0]), Some(-0.6));
        let (mid, _) = vignetting(&l, 1.0, 50.0, 5.33, 10.0).expect("between samples");
        assert!(mid.terms[0] > -0.6 && mid.terms[0] < -0.2, "{mid:?}");
        // f/1 is far from both samples on the 4/aperture axis.
        assert!(vignetting(&l, 1.0, 50.0, 1.0, 10.0).is_none());
    }
}
