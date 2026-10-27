//! The correction of one image: interpolated coefficients brought into the
//! image's own coordinates, ready to be evaluated per pixel.
//!
//! Coordinates follow lensfun (`lfModifier`): pixel centres are integers,
//! the origin is the optical centre, and the unit is the real focal length,
//! so one set of coefficients serves every resolution. The database's models
//! use other units (Hugin's half image height for distortion and TCA, the
//! half diagonal for vignetting, both on the calibration camera's sensor);
//! their coefficients are rescaled once here, as lensfun's
//! `rescale_polynomial_coefficients` does.
//!
//! For each pixel of the corrected image, [`LensCorrection::source`] gives
//! where to read each colour channel in the uncorrected image: distortion
//! first, then chromatic aberration on red and blue. Vignetting is a gain on
//! the uncorrected image, [`LensCorrection::vignetting_gain`].

use std::fmt;

use crate::interp;
use crate::model::{DistortionModel, Lens, TcaModel};

/// Diagonal of a 36×24 mm frame.
const FULL_FRAME_DIAGONAL: f64 = 43.266_615_305_567_87;

/// Smallest and largest automatic zoom: beyond these the data is not usable.
const ZOOM_RANGE: [f64; 2] = [0.25, 4.0];

/// What to correct, for which image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorrectionRequest {
    /// Crop factor of the camera (or of the in-camera crop) that took the image.
    pub crop_factor: f32,
    /// Focal length in mm, from the EXIF data.
    pub focal: f32,
    /// F-number, from the EXIF data; needed for vignetting.
    pub aperture: f32,
    /// Focus distance in metres; far away (1000) when unknown.
    pub distance: f32,
    /// Width of the image in pixels, as given to the correction (sensor orientation).
    pub width: u32,
    /// Height of the image in pixels.
    pub height: u32,
    /// Correct geometric distortion.
    pub distortion: bool,
    /// Correct transverse (lateral) chromatic aberration.
    pub tca: bool,
    /// Correct vignetting.
    pub vignetting: bool,
}

/// Why no correction could be prepared.
#[derive(Debug, Clone, PartialEq)]
pub enum CorrectionError {
    /// Crop factor, focal length or image size is not positive.
    BadRequest(String),
}

impl fmt::Display for CorrectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(m) => write!(f, "lens correction not possible: {m}"),
        }
    }
}

impl std::error::Error for CorrectionError {}

/// Which corrections a [`LensCorrection`] actually applies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// Distortion data was found and is applied.
    pub distortion: bool,
    /// Chromatic aberration data was found and is applied.
    pub tca: bool,
    /// Vignetting data was found and is applied.
    pub vignetting: bool,
}

impl Applied {
    /// `true` when nothing is corrected.
    #[must_use]
    pub fn is_empty(self) -> bool {
        !(self.distortion || self.tca || self.vignetting)
    }
}

/// The correction of one image.
#[derive(Debug, Clone, PartialEq)]
pub struct LensCorrection {
    width: u32,
    height: u32,
    /// Pixels → focal-length units.
    norm_scale: f64,
    /// Optical centre in focal-length units, from the top-left pixel centre.
    center: [f64; 2],
    distortion: Option<(DistortionModel, [f64; 3])>,
    tca: Option<(TcaModel, [f64; 6])>,
    vignetting: Option<[f64; 3]>,
    /// Magnification of the corrected image (> 1 crops in).
    zoom: f64,
}

impl LensCorrection {
    /// Prepares the correction of an image taken with `lens`. Corrections the
    /// database has no data for (or no data valid for this sensor size) are
    /// left out; see [`Self::applied`]. With distortion or TCA, the zoom is
    /// set so that the corrected image has no empty borders.
    ///
    /// # Errors
    /// [`CorrectionError`] when crop factor, focal length or size are not positive.
    pub fn new(lens: &Lens, req: &CorrectionRequest) -> Result<Self, CorrectionError> {
        let bad = |m: &str| Err(CorrectionError::BadRequest(m.to_owned()));
        if !(req.crop_factor.is_finite() && req.crop_factor > 0.0) {
            return bad("the camera's crop factor is unknown");
        }
        if !(req.focal.is_finite() && req.focal > 0.0) {
            return bad("the focal length is unknown");
        }
        if req.width == 0 || req.height == 0 {
            return bad("the image is empty");
        }
        let (crop, focal) = (req.crop_factor, req.focal);

        // Lensfun measures sizes between the outer pixel centres.
        let w1 = f64::from(req.width.max(2) - 1);
        let h1 = f64::from(req.height.max(2) - 1);

        // The real focal length sets the unit, whether or not distortion is corrected.
        let interpolated = interp::distortion(lens, crop, focal);
        let real_focal = interpolated.map_or(f64::from(focal), |(d, _)| f64::from(d.real_focal));
        let norm_scale = FULL_FRAME_DIAGONAL / f64::from(crop) / (w1 + 1.0).hypot(h1 + 1.0) / real_focal;
        let size = w1.min(h1);
        let center = [
            (w1 / 2.0 + size / 2.0 * f64::from(lens.center[0])) * norm_scale,
            (h1 / 2.0 + size / 2.0 * f64::from(lens.center[1])) * norm_scale,
        ];

        // Hugin's unit: half the height of the calibration camera's frame.
        let hugin_scaling = |set_crop: f32, aspect: f32| {
            let half_height_mm = FULL_FRAME_DIAGONAL / f64::from(set_crop) / f64::from(aspect).hypot(1.0) / 2.0;
            real_focal / half_height_mm
        };

        let distortion = interpolated.filter(|_| req.distortion).map(|(d, set)| {
            let hs = hugin_scaling(set.crop_factor, set.aspect_ratio);
            let t = d.terms.map(f64::from);
            let terms = match d.model {
                DistortionModel::Poly3 => {
                    let dd = 1.0 - t[0];
                    [t[0] * hs.powi(2) / dd.powi(3), 0.0, 0.0]
                }
                DistortionModel::Poly5 => [t[0] * hs.powi(2), t[1] * hs.powi(4), 0.0],
                DistortionModel::PtLens => {
                    let dd = 1.0 - t[0] - t[1] - t[2];
                    [t[0] * hs.powi(3) / dd.powi(4), t[1] * hs.powi(2) / dd.powi(3), t[2] * hs / dd.powi(2)]
                }
            };
            (d.model, terms)
        });

        let tca = if req.tca { interp::tca(lens, crop, focal) } else { None }.map(|(t, set)| {
            let hs = hugin_scaling(set.crop_factor, set.aspect_ratio);
            let mut terms = t.terms.map(f64::from);
            if t.model == TcaModel::Poly3 {
                terms[2] *= hs;
                terms[3] *= hs;
                terms[4] *= hs * hs;
                terms[5] *= hs * hs;
            }
            (t.model, terms)
        });

        let distance = if req.distance.is_finite() && req.distance > 0.0 { req.distance } else { 1000.0 };
        let vignetting = if req.vignetting && req.aperture.is_finite() && req.aperture > 0.0 {
            interp::vignetting(lens, crop, focal, req.aperture, distance)
        } else {
            None
        }
        .map(|(v, set)| {
            // The "pa" model's unit is the half diagonal of the calibration frame.
            let hs = real_focal / (FULL_FRAME_DIAGONAL / f64::from(set.crop_factor) / 2.0);
            let k = v.terms.map(f64::from);
            [k[0] * hs.powi(2), k[1] * hs.powi(4), k[2] * hs.powi(6)]
        });

        let mut c =
            Self { width: req.width, height: req.height, norm_scale, center, distortion, tca, vignetting, zoom: 1.0 };
        if c.applied().distortion || c.applied().tca {
            c.zoom = c.auto_zoom();
        }
        Ok(c)
    }

    /// The corrections this instance applies.
    #[must_use]
    pub fn applied(&self) -> Applied {
        Applied {
            distortion: self.distortion.is_some(),
            tca: self.tca.is_some(),
            vignetting: self.vignetting.is_some(),
        }
    }

    /// `true` when pixels move (distortion or TCA), so the image must be resampled.
    #[must_use]
    pub fn moves_pixels(&self) -> bool {
        self.distortion.is_some() || self.tca.is_some() || (self.zoom - 1.0).abs() > 1e-9
    }

    /// Image width the correction was prepared for.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Image height the correction was prepared for.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Magnification of the corrected image; above 1 the borders are cropped.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // a zoom factor near 1
    pub fn zoom(&self) -> f32 {
        self.zoom as f32
    }

    /// Replaces the automatic zoom (1 keeps the original scale, with empty
    /// or stretched borders where the correction pulls the image inwards).
    pub fn set_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() && zoom > 0.0 {
            self.zoom = f64::from(zoom);
        }
    }

    /// Multiplier that removes the vignetting at pixel `(x, y)` of the
    /// uncorrected image (1 at the centre, above 1 towards darker corners).
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // the gain is used in f32 pixel maths
    pub fn vignetting_gain(&self, x: f32, y: f32) -> f32 {
        let Some(k) = self.vignetting else { return 1.0 };
        let u = f64::from(x) * self.norm_scale - self.center[0];
        let v = f64::from(y) * self.norm_scale - self.center[1];
        let r2 = u * u + v * v;
        let c = 1.0 + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2;
        // A calibration that darkens to nothing would blow up the corners.
        if c > 1e-3 { (1.0 / c) as f32 } else { 1.0 }
    }

    /// Where to read red, green and blue in the uncorrected image for pixel
    /// `(x, y)` of the corrected one, in pixel coordinates.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // pixel coordinates fit in f32
    pub fn source(&self, x: f32, y: f32) -> [[f32; 2]; 3] {
        let u = [
            (f64::from(x) * self.norm_scale - self.center[0]) / self.zoom,
            (f64::from(y) * self.norm_scale - self.center[1]) / self.zoom,
        ];
        self.map_normalized(u).map(|p| {
            [((p[0] + self.center[0]) / self.norm_scale) as f32, ((p[1] + self.center[1]) / self.norm_scale) as f32]
        })
    }

    /// Distortion, then TCA, in normalised coordinates (lensfun's order).
    fn map_normalized(&self, u: [f64; 2]) -> [[f64; 2]; 3] {
        let d = match self.distortion {
            None => u,
            Some((model, k)) => {
                let r2 = u[0] * u[0] + u[1] * u[1];
                let f = match model {
                    DistortionModel::Poly3 => 1.0 + k[0] * r2,
                    DistortionModel::Poly5 => 1.0 + k[0] * r2 + k[1] * r2 * r2,
                    DistortionModel::PtLens => {
                        let r = r2.sqrt();
                        k[0] * r2 * r + k[1] * r2 + k[2] * r + 1.0
                    }
                };
                [u[0] * f, u[1] * f]
            }
        };
        match self.tca {
            None => [d, d, d],
            Some((TcaModel::Linear, k)) => [[d[0] * k[0], d[1] * k[0]], d, [d[0] * k[1], d[1] * k[1]]],
            Some((TcaModel::Poly3, k)) => {
                let r2 = d[0] * d[0] + d[1] * d[1];
                let r = r2.sqrt();
                let red = k[4] * r2 + k[2] * r + k[0];
                let blue = k[5] * r2 + k[3] * r + k[1];
                [[d[0] * red, d[1] * red], d, [d[0] * blue, d[1] * blue]]
            }
        }
    }

    /// `true` when every channel of normalised point `u` lands inside the image.
    fn inside(&self, u: [f64; 2]) -> bool {
        let w1 = f64::from(self.width.max(2) - 1);
        let h1 = f64::from(self.height.max(2) - 1);
        let eps = 1e-6;
        self.map_normalized(u).iter().all(|p| {
            let x = (p[0] + self.center[0]) / self.norm_scale;
            let y = (p[1] + self.center[1]) / self.norm_scale;
            x.is_finite() && y.is_finite() && x >= -eps && y >= -eps && x <= w1 + eps && y <= h1 + eps
        })
    }

    /// The smallest zoom that leaves no border of the corrected image without
    /// data: along rays from the optical centre to points of the border, find
    /// how far one can go and still read inside the image.
    fn auto_zoom(&self) -> f64 {
        let w1 = f64::from(self.width.max(2) - 1);
        let h1 = f64::from(self.height.max(2) - 1);
        const STEPS: u32 = 64;
        let mut border = Vec::with_capacity(4 * STEPS as usize);
        for i in 0..STEPS {
            let s = f64::from(i) / f64::from(STEPS);
            border.extend([[s * w1, 0.0], [w1, s * h1], [(1.0 - s) * w1, h1], [0.0, (1.0 - s) * h1]]);
        }
        let mut zoom = ZOOM_RANGE[0];
        for p in border {
            let u = [p[0] * self.norm_scale - self.center[0], p[1] * self.norm_scale - self.center[1]];
            if u[0] == 0.0 && u[1] == 0.0 {
                continue;
            }
            let at = |t: f64| [u[0] * t, u[1] * t];
            // The first t at which the ray leaves the image: march outwards
            // (strong distortion can fold the ray back inside further out),
            // then refine by bisection.
            let t_max = 1.0 / ZOOM_RANGE[0];
            let step = t_max / 400.0;
            let mut lo = 0.0_f64;
            let mut hi = None;
            while lo + step <= t_max {
                if self.inside(at(lo + step)) {
                    lo += step;
                } else {
                    hi = Some(lo + step);
                    break;
                }
            }
            if let Some(mut hi) = hi {
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    if self.inside(at(mid)) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
            }
            let needed = if lo > 0.0 { 1.0 / lo } else { ZOOM_RANGE[1] };
            zoom = zoom.max(needed);
        }
        // Lensfun's margin against the gaps between the test points.
        (zoom * 1.001).clamp(ZOOM_RANGE[0], ZOOM_RANGE[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CalibrationSet, Distortion, LensType, Name, Tca, Vignetting};

    fn lens(set: CalibrationSet) -> Lens {
        Lens {
            maker: Name::new("Test"),
            model: Name::new("Prime 50mm f/2"),
            mounts: vec!["M".to_owned()],
            lens_type: LensType::Rectilinear,
            focal_range: [50.0, 50.0],
            aperture_range: [2.0, 16.0],
            center: [0.0, 0.0],
            calibrations: vec![set],
        }
    }

    fn empty_set() -> CalibrationSet {
        CalibrationSet { crop_factor: 1.0, aspect_ratio: 1.5, distortion: vec![], tca: vec![], vignetting: vec![] }
    }

    fn request(w: u32, h: u32) -> CorrectionRequest {
        CorrectionRequest {
            crop_factor: 1.0,
            focal: 50.0,
            aperture: 2.0,
            distance: 10.0,
            width: w,
            height: h,
            distortion: true,
            tca: true,
            vignetting: true,
        }
    }

    fn with_distortion(model: DistortionModel, terms: [f32; 3]) -> Lens {
        let mut s = empty_set();
        let real_focal = match model {
            DistortionModel::PtLens => 50.0 * (1.0 - terms[0] - terms[1] - terms[2]),
            DistortionModel::Poly3 => 50.0 * (1.0 - terms[0]),
            DistortionModel::Poly5 => 50.0,
        };
        s.distortion.push(Distortion { model, focal: 50.0, real_focal, terms });
        lens(s)
    }

    #[test]
    fn without_data_nothing_moves_and_nothing_is_brightened() {
        let c = LensCorrection::new(&lens(empty_set()), &request(300, 200)).expect("valid");
        assert!(c.applied().is_empty() && !c.moves_pixels());
        assert_eq!(c.source(17.0, 33.0), [[17.0, 33.0]; 3]);
        assert!((c.vignetting_gain(0.0, 0.0) - 1.0).abs() < 1e-7);
    }

    #[test]
    fn the_optical_centre_stays_in_place() {
        let mut c =
            LensCorrection::new(&with_distortion(DistortionModel::PtLens, [0.01, -0.04, 0.02]), &request(301, 201))
                .expect("valid");
        c.set_zoom(1.0);
        let s = c.source(150.0, 100.0);
        assert!(s.iter().all(|p| (p[0] - 150.0).abs() < 1e-3 && (p[1] - 100.0).abs() < 1e-3), "{s:?}");
    }

    #[test]
    fn poly3_and_the_equivalent_ptlens_move_pixels_identically() {
        let a = LensCorrection::new(&with_distortion(DistortionModel::Poly3, [-0.03, 0.0, 0.0]), &request(600, 400))
            .expect("valid");
        let b = LensCorrection::new(&with_distortion(DistortionModel::PtLens, [0.0, -0.03, 0.0]), &request(600, 400))
            .expect("valid");
        for (x, y) in [(0.0, 0.0), (599.0, 0.0), (420.0, 333.0), (10.0, 390.0)] {
            let (pa, pb) = (a.source(x, y)[1], b.source(x, y)[1]);
            assert!((pa[0] - pb[0]).abs() < 1e-3 && (pa[1] - pb[1]).abs() < 1e-3, "{pa:?} vs {pb:?}");
        }
    }

    #[test]
    fn coefficients_are_brought_into_focal_length_units_as_lensfun_does() {
        // Hand computation of one point for poly3, k1 = -0.02, 3:2 full frame, 50 mm.
        let k1 = -0.02_f64;
        let c = LensCorrection::new(&with_distortion(DistortionModel::Poly3, [-0.02, 0.0, 0.0]), &request(3000, 2000))
            .expect("valid");
        let mut c = c;
        c.set_zoom(1.0);
        let real_focal = 50.0 * (1.0 - k1);
        let ns = FULL_FRAME_DIAGONAL / 3000.0_f64.hypot(2000.0) / real_focal;
        let hs = real_focal / (FULL_FRAME_DIAGONAL / 1.5_f64.hypot(1.0) / 2.0); // = real_focal / 12 mm
        let k = k1 * hs * hs / (1.0 - k1).powi(3);
        let (x, y) = (2900.0_f64, 1800.0_f64);
        let u = [x * ns - 2999.0 / 2.0 * ns, y * ns - 1999.0 / 2.0 * ns];
        let f = 1.0 + k * (u[0] * u[0] + u[1] * u[1]);
        let want = [(u[0] * f) / ns + 2999.0 / 2.0, (u[1] * f) / ns + 1999.0 / 2.0];
        #[allow(clippy::cast_possible_truncation)]
        let got = c.source(x as f32, y as f32)[1];
        assert!((f64::from(got[0]) - want[0]).abs() < 1e-2 && (f64::from(got[1]) - want[1]).abs() < 1e-2);
        // Barrel distortion (k1 < 0): corrected corners read closer to the centre.
        assert!(got[0] < 2900.0 && got[1] < 1800.0, "{got:?}");
    }

    #[test]
    fn the_automatic_zoom_leaves_no_empty_border() {
        for terms in [[0.0, -0.05, 0.0], [0.0, 0.04, 0.0], [0.02, -0.06, 0.01]] {
            let c = LensCorrection::new(&with_distortion(DistortionModel::PtLens, terms), &request(400, 300))
                .expect("valid");
            for i in 0..=400 {
                #[allow(clippy::cast_precision_loss)]
                let x = i as f32;
                for (px, py) in [(x, 0.0), (x, 299.0), (0.0, x * 0.7475), (399.0, x * 0.7475)] {
                    for s in c.source(px.min(399.0), py) {
                        let inside = s[0] >= -0.01 && s[1] >= -0.01 && s[0] <= 399.01 && s[1] <= 299.01;
                        assert!(inside, "terms {terms:?}, zoom {}, ({px}, {py}) reads {s:?}", c.zoom());
                    }
                }
            }
        }
    }

    #[test]
    fn pincushion_zooms_out_and_barrel_zooms_in() {
        // Barrel (b < 0): corners read inwards, so the image can be shown a little smaller…
        let barrel =
            LensCorrection::new(&with_distortion(DistortionModel::PtLens, [0.0, -0.05, 0.0]), &request(400, 300))
                .expect("valid");
        // …pincushion (b > 0): corners read outside, so it must be enlarged.
        let pincushion =
            LensCorrection::new(&with_distortion(DistortionModel::PtLens, [0.0, 0.05, 0.0]), &request(400, 300))
                .expect("valid");
        assert!(barrel.zoom() < 1.0 && pincushion.zoom() > 1.0, "{} {}", barrel.zoom(), pincushion.zoom());
    }

    #[test]
    fn chromatic_aberration_moves_red_and_blue_but_not_green() {
        let mut s = empty_set();
        s.tca.push(Tca { model: TcaModel::Linear, focal: 50.0, terms: [1.002, 0.998, 0.0, 0.0, 0.0, 0.0] });
        let mut c = LensCorrection::new(&lens(s), &request(1001, 801)).expect("valid");
        c.set_zoom(1.0);
        let [r, g, b] = c.source(1000.0, 800.0);
        assert!((g[0] - 1000.0).abs() < 1e-3 && (g[1] - 800.0).abs() < 1e-3, "{g:?}");
        // Red is read 0.2 % further from the centre (500, 400), blue 0.2 % closer.
        assert!((r[0] - (500.0 + 500.0 * 1.002)).abs() < 1e-2, "{r:?}");
        assert!((b[1] - (400.0 + 400.0 * 0.998)).abs() < 1e-2, "{b:?}");
    }

    #[test]
    fn vignetting_brightens_the_corners_only() {
        let mut s = empty_set();
        s.vignetting.push(Vignetting { focal: 50.0, aperture: 2.0, distance: 10.0, terms: [-0.4, 0.0, 0.0] });
        let c = LensCorrection::new(&lens(s), &request(301, 201)).expect("valid");
        assert!(c.applied().vignetting && !c.moves_pixels());
        assert!((c.vignetting_gain(150.0, 100.0) - 1.0).abs() < 1e-6);
        // r = 1 at the corner of a full-frame 3:2 image: gain 1 / (1 − 0.4).
        let corner = c.vignetting_gain(0.0, 0.0);
        assert!((corner - 1.0 / 0.6).abs() < 2e-2, "{corner}");
        assert!(c.vignetting_gain(75.0, 50.0) < corner);
    }

    #[test]
    fn impossible_requests_are_refused() {
        let l = lens(empty_set());
        let mut r = request(10, 10);
        r.focal = 0.0;
        assert!(LensCorrection::new(&l, &r).is_err());
        let mut r = request(10, 10);
        r.crop_factor = f32::NAN;
        assert!(LensCorrection::new(&l, &r).is_err());
        assert!(LensCorrection::new(&l, &request(0, 10)).is_err());
    }
}
