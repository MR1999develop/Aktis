//! The fixed development stage, on the CPU: sensor data → linear working space.
//!
//! Order (fixed by the sensor's physics and the optics): white balance on the
//! raw mosaic, demosaic, clipped highlights made neutral
//! ([`neutralise_clipped`]), lens corrections in camera RGB (vignetting, then
//! distortion and lateral chromatic aberration, which move each colour
//! channel differently), camera profile into the working space. Bayer
//! sensors are demosaiced bilinearly, X-Trans sensors with an edge-aware
//! green and colour differences ([`xtrans`]). This is the reference
//! implementation; quality demosaicing moves to the GPU engine in phase 3,
//! tested against this one.
//!
//! [`prepare`] chooses the profile and the lens correction for an image from
//! its shooting data; [`develop_with`] applies them.

mod lens;
pub mod prepare;
mod xtrans;

use std::fmt;

use core_color::{ColorProfile, ProfileError};
use core_model::{CfaPattern, DevelopSettings, ImageError, ImageF32, Rgba, SensorImage, WhiteBalance, WorkingSpace};
use lens_db::LensCorrection;

/// Why the development stage could not run.
#[derive(Debug, Clone, PartialEq)]
pub enum DevelopError {
    /// The camera colour matrix cannot be inverted.
    DegenerateCameraMatrix,
    /// The camera profile cannot be used.
    Profile(ProfileError),
    /// The lens correction was prepared for another image size.
    LensSizeMismatch,
    /// The output image could not be created.
    Image(ImageError),
}

impl fmt::Display for DevelopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DegenerateCameraMatrix => write!(f, "the camera colour matrix in the file is not usable"),
            Self::Profile(e) => write!(f, "camera profile: {e}"),
            Self::LensSizeMismatch => write!(f, "the lens correction was prepared for another image size"),
            Self::Image(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for DevelopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Image(e) => Some(e),
            Self::Profile(e) => Some(e),
            _ => None,
        }
    }
}

/// What the development stage uses besides the sensor data and the settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevelopInputs<'a> {
    /// Camera profile; `None` uses the colour matrix stored with the sensor data.
    pub profile: Option<&'a ColorProfile>,
    /// Lens correction prepared for the sensor's size; `None` corrects nothing.
    pub lens: Option<&'a LensCorrection>,
}

/// Develops sensor data into a linear image in `space`, with the sensor's
/// own colour matrix and no lens correction.
///
/// # Errors
/// [`DevelopError`] for an unusable colour matrix.
pub fn develop(
    sensor: &SensorImage,
    settings: &DevelopSettings,
    space: WorkingSpace,
) -> Result<ImageF32, DevelopError> {
    develop_with(sensor, settings, space, &DevelopInputs::default())
}

/// Develops sensor data into a linear image in `space`, with a camera
/// profile and lens corrections.
///
/// # Errors
/// [`DevelopError`] for an unusable colour matrix or profile, or a lens
/// correction for another image size.
pub fn develop_with(
    sensor: &SensorImage,
    settings: &DevelopSettings,
    space: WorkingSpace,
    inputs: &DevelopInputs<'_>,
) -> Result<ImageF32, DevelopError> {
    let wb = match settings.white_balance {
        WhiteBalance::AsShot => sensor.as_shot_wb(),
        WhiteBalance::Multipliers(m) => m,
    };
    let w = sensor.width() as usize;
    let h = sensor.height() as usize;
    if let Some(c) = inputs.lens
        && (c.width() != sensor.width() || c.height() != sensor.height())
    {
        return Err(DevelopError::LensSizeMismatch);
    }

    // Camera RGB, white-balanced.
    let data = sensor.data();
    let mut camera: Vec<[f32; 3]> = match sensor.cfa() {
        CfaPattern::Bayer(pattern) => {
            // White balance on the mosaic, before interpolation mixes the channels.
            let balanced: Vec<f32> =
                data.iter().enumerate().map(|(i, v)| v * wb[pattern.channel_at(i % w, i / w)]).collect();
            let mut out = vec![[0.0_f32; 3]; w * h];
            lens::parallel_rows(&mut out, w, |first, rows| {
                for (i, px) in rows.iter_mut().enumerate() {
                    let (x, y) = (i % w, first + i / w);
                    *px = demosaic_bilinear_at(&balanced, w, h, x, y, |xx, yy| pattern.channel_at(xx, yy));
                }
            });
            out
        }
        CfaPattern::LinearRgb => data.chunks_exact(3).map(|c| [c[0] * wb[0], c[1] * wb[1], c[2] * wb[2]]).collect(),
        CfaPattern::XTrans(pattern) => {
            // White balance on the mosaic here too; the 6×6 tile starts at the image's corner.
            let channel = |x: usize, y: usize| usize::from(pattern[y % 6][x % 6]);
            let balanced: Vec<f32> = data.iter().enumerate().map(|(i, v)| v * wb[channel(i % w, i / w)]).collect();
            xtrans::demosaic(&balanced, w, h, pattern)
        }
    };

    // Highlights where the sensor saturated: neutral, not the colour of the multipliers.
    for px in &mut camera {
        *px = neutralise_clipped(*px, wb);
    }

    // Lens corrections, in camera RGB where each channel is still one colour.
    if let Some(c) = inputs.lens {
        if c.applied().vignetting {
            lens::devignette(&mut camera, w, c);
        }
        if c.moves_pixels() {
            camera = lens::resample(&camera, w, h, c);
        }
    }

    // Camera RGB → working space.
    let mut pixels: Vec<Rgba> = vec![[0.0, 0.0, 0.0, 1.0]; w * h];
    match inputs.profile {
        Some(profile) => {
            let t = profile.transform(wb, space).map_err(DevelopError::Profile)?;
            lens::parallel_rows(&mut pixels, w, |first, rows| {
                for (i, px) in rows.iter_mut().enumerate() {
                    let rgb = t.apply(camera[first * w + i]);
                    *px = [rgb[0], rgb[1], rgb[2], 1.0];
                }
            });
        }
        None => {
            let to_working = core_color::camera_to_working(sensor.xyz_to_camera(), space)
                .ok_or(DevelopError::DegenerateCameraMatrix)?;
            for (px, cam) in pixels.iter_mut().zip(&camera) {
                let rgb = to_working.apply(*cam);
                *px = [rgb[0], rgb[1], rgb[2], 1.0];
            }
        }
    }
    ImageF32::from_pixels(sensor.width(), sensor.height(), space, pixels).map_err(DevelopError::Image)
}

/// Raw level (relative to the white level) from which a pixel starts
/// turning neutral…
pub const CLIP_FROM: f32 = 0.90;
/// …and at which it is fully neutral: the sensor saturated.
pub const CLIP_AT: f32 = 0.99;

/// One white-balanced camera pixel with its saturated highlights made neutral.
///
/// A saturated photosite reads the white level whatever the light, so after
/// white balance a burnt highlight takes the colour of the multipliers
/// (typically pink or magenta) instead of white; lowering the exposure or
/// recovering highlights would show it. Where any channel, back on the
/// sensor's scale, nears the white level, the pixel is blended towards the
/// neutral of its brightest channel: what the camera could not measure
/// becomes white, and unclipped colours, even bright ones, are untouched.
#[must_use]
pub fn neutralise_clipped(px: [f32; 3], wb: [f32; 3]) -> [f32; 3] {
    let raw = |c: usize| px[c] / wb[c].max(f32::MIN_POSITIVE);
    let level = raw(0).max(raw(1)).max(raw(2));
    if level.is_nan() || level <= CLIP_FROM {
        return px; // NaN is left to the output render
    }
    let t = ((level - CLIP_FROM) / (CLIP_AT - CLIP_FROM)).clamp(0.0, 1.0);
    let t = t * t * (3.0 - 2.0 * t);
    let white = px[0].max(px[1]).max(px[2]);
    px.map(|v| v + (white - v) * t)
}

/// Mirrors an index at the borders without repeating the edge sample
/// ("reflect-101"): -1 → 1, n → n-2.
fn reflect(i: isize, n: usize) -> usize {
    let n = n as isize;
    if n == 1 {
        return 0;
    }
    let r = if i < 0 {
        -i
    } else if i >= n {
        2 * n - 2 - i
    } else {
        i
    };
    usize::try_from(r.clamp(0, n - 1)).unwrap_or(0)
}

/// Bilinear demosaic of one pixel: each missing channel is the mean of the
/// same-channel photosites in the 3×3 neighbourhood. Borders are mirrored,
/// which keeps the Bayer phase intact.
fn demosaic_bilinear_at(
    mosaic: &[f32],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    channel_at: impl Fn(usize, usize) -> usize,
) -> [f32; 3] {
    let own = channel_at(x, y);
    let mut sum = [0.0_f32; 3];
    let mut count = [0_u32; 3];
    for dy in -1_isize..=1 {
        for dx in -1_isize..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let xx = reflect(x as isize + dx, w);
            let yy = reflect(y as isize + dy, h);
            // Mirroring can land on the centre itself in 1-pixel-wide images.
            if xx == x && yy == y {
                continue;
            }
            let c = channel_at(xx, yy);
            sum[c] += mosaic[yy * w + xx];
            count[c] += 1;
        }
    }
    let centre = mosaic[y * w + x];
    let mut out = [0.0; 3];
    for c in 0..3 {
        out[c] = if c == own {
            centre
        } else if count[c] > 0 {
            #[allow(clippy::cast_precision_loss)] // at most 8
            let n = count[c] as f32;
            sum[c] / n
        } else {
            // Tiny images may lack a channel entirely: assume neutral.
            centre
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::{BayerPattern, Matrix3};

    /// A sensor where every photosite of channel c holds `values[c]`.
    fn sensor(w: u32, h: u32, pattern: BayerPattern, values: [f32; 3], wb: [f32; 3]) -> SensorImage {
        let data = (0..(w * h) as usize).map(|i| values[pattern.channel_at(i % w as usize, i / w as usize)]).collect();
        SensorImage::new(w, h, data, CfaPattern::Bayer(pattern), wb, Matrix3::IDENTITY).expect("valid sensor")
    }

    fn assert_all_close(img: &ImageF32, want: [f32; 3], tol: f32) {
        for (i, p) in img.pixels().iter().enumerate() {
            for c in 0..3 {
                assert!((p[c] - want[c]).abs() <= tol, "pixel {i}: {p:?}, want {want:?}");
            }
        }
    }

    #[test]
    fn a_grey_scene_develops_to_the_same_grey_everywhere_including_borders() {
        for pattern in [BayerPattern::Rggb, BayerPattern::Bggr, BayerPattern::Grbg, BayerPattern::Gbrg] {
            let s = sensor(7, 5, pattern, [0.4, 0.4, 0.4], [1.0, 1.0, 1.0]);
            let img = develop(&s, &DevelopSettings::default(), WorkingSpace::LinearRec2020).expect("develops");
            assert_all_close(&img, [0.4; 3], 1e-4);
        }
    }

    #[test]
    fn as_shot_white_balance_neutralises_a_tinted_grey() {
        // The camera saw a grey card as R 0.25, G 0.5, B 1.0 and recorded multipliers 2, 1, 0.5.
        let s = sensor(6, 6, BayerPattern::Rggb, [0.25, 0.5, 1.0], [2.0, 1.0, 0.5]);
        let img = develop(&s, &DevelopSettings::default(), WorkingSpace::LinearSrgb).expect("develops");
        assert_all_close(&img, [0.5; 3], 1e-4);
    }

    #[test]
    fn explicit_multipliers_replace_the_as_shot_ones() {
        let s = sensor(4, 4, BayerPattern::Rggb, [0.5, 0.5, 0.5], [2.0, 1.0, 0.5]);
        let settings =
            DevelopSettings { white_balance: WhiteBalance::Multipliers([1.0, 1.0, 1.0]), ..DevelopSettings::default() };
        let img = develop(&s, &settings, WorkingSpace::LinearRec2020).expect("develops");
        assert_all_close(&img, [0.5; 3], 1e-4);
    }

    #[test]
    fn a_burnt_highlight_develops_to_white_not_to_the_colour_of_the_multipliers() {
        // Every photosite at the white level; multipliers as a daylight camera records them.
        let s = sensor(6, 6, BayerPattern::Rggb, [1.0, 1.0, 1.0], [2.1, 1.0, 1.5]);
        let img = develop(&s, &DevelopSettings::default(), WorkingSpace::LinearSrgb).expect("develops");
        let p = img.pixels()[14];
        assert!((p[0] - p[1]).abs() < 1e-4 && (p[1] - p[2]).abs() < 1e-4, "neutral: {p:?}");
        assert!(p[0] >= 2.0, "as bright as its brightest channel: {p:?}");
    }

    #[test]
    fn bright_colours_the_sensor_measured_keep_their_colour() {
        // A strong red below saturation on every channel develops exactly like
        // the same red one stop darker, scaled: nothing was neutralised.
        let wb = [2.1, 1.0, 1.5];
        let develop_red = |k: f32| {
            let s = sensor(6, 6, BayerPattern::Rggb, [0.85 * k, 0.2 * k, 0.1 * k], wb);
            develop(&s, &DevelopSettings::default(), WorkingSpace::LinearSrgb).expect("develops")
        };
        let (bright, dark) = (develop_red(1.0), develop_red(0.5));
        for (b, d) in bright.pixels().iter().zip(dark.pixels()) {
            for c in 0..3 {
                assert!((b[c] - 2.0 * d[c]).abs() < 1e-4, "{b:?} vs 2 × {d:?}");
            }
        }
    }

    #[test]
    fn clipping_turns_neutral_gradually_and_never_darkens() {
        let wb = [2.0, 1.0, 1.5];
        let mut last = neutralise_clipped([0.5 * 2.0, 0.4, 0.3 * 1.5], wb);
        for i in 0..=100 {
            #[allow(clippy::cast_precision_loss)]
            let r = 0.85 + 0.15 * i as f32 / 100.0;
            let px = neutralise_clipped([r * 2.0, 0.4, 0.3 * 1.5], wb);
            assert!(px[0] >= r * 2.0 - 1e-5, "the brightest channel is kept: {px:?}");
            assert!(px[1] >= 0.4 - 1e-6 && px[2] >= 0.45 - 1e-6, "never darker: {px:?}");
            assert!((px[1] - last[1]).abs() < 0.15, "no jump at {r}: {last:?} → {px:?}");
            last = px;
        }
        assert!((last[0] - last[1]).abs() < 1e-5 && (last[1] - last[2]).abs() < 1e-5, "{last:?}");
        let dark = neutralise_clipped([0.2, 0.3, 0.1], wb);
        assert!(dark.iter().zip([0.2, 0.3, 0.1]).all(|(a, b)| (a - b).abs() < 1e-7), "{dark:?}");
    }

    #[test]
    fn tiny_sensors_do_not_panic() {
        for (w, h) in [(1, 1), (2, 1), (1, 3), (2, 2)] {
            let s = sensor(w, h, BayerPattern::Rggb, [0.3, 0.3, 0.3], [1.0; 3]);
            let img = develop(&s, &DevelopSettings::default(), WorkingSpace::LinearRec2020).expect("develops");
            assert_eq!((img.width(), img.height()), (w, h));
        }
    }

    #[test]
    fn linear_rgb_data_skip_demosaic_but_get_white_balance() {
        // Two pixels of a tinted grey, already demosaiced (Linear DNG / ProRAW).
        let data = vec![0.25, 0.5, 1.0, 0.125, 0.25, 0.5];
        let s = SensorImage::new(2, 1, data, CfaPattern::LinearRgb, [2.0, 1.0, 0.5], Matrix3::IDENTITY)
            .expect("valid sensor");
        let img = develop(&s, &DevelopSettings::default(), WorkingSpace::LinearSrgb).expect("develops");
        let p = img.pixels();
        assert!(p[0][..3].iter().all(|v| (v - 0.5).abs() < 1e-4), "{:?}", p[0]);
        assert!(p[1][..3].iter().all(|v| (v - 0.25).abs() < 1e-4), "{:?}", p[1]);
    }

    /// The X-Trans tile of Fujifilm cameras, 0 R, 1 G, 2 B.
    const X_TRANS: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];

    #[test]
    fn x_trans_sensors_develop_with_white_balance_on_the_mosaic() {
        // The camera saw a grey card as R 0.25, G 0.5, B 1.0 and recorded multipliers 2, 1, 0.5.
        let (w, h) = (13_u32, 11_u32);
        let values = [0.25, 0.5, 1.0];
        let data = (0..(w * h) as usize)
            .map(|i| values[usize::from(X_TRANS[i / w as usize % 6][i % w as usize % 6])])
            .collect();
        let s = SensorImage::new(w, h, data, CfaPattern::XTrans(X_TRANS), [2.0, 1.0, 0.5], Matrix3::IDENTITY)
            .expect("valid sensor");
        let img = develop(&s, &DevelopSettings::default(), WorkingSpace::LinearSrgb).expect("develops");
        assert_eq!((img.width(), img.height()), (w, h));
        assert_all_close(&img, [0.5; 3], 1e-4);
    }

    fn vignetting_lens() -> lens_db::LensDatabase {
        lens_db::LensDatabase::from_xml(
            r#"<lensdatabase><lens><maker>T</maker><model>T 50mm f/2</model><mount>M</mount><cropfactor>1</cropfactor>
            <calibration><vignetting model="pa" focal="50" aperture="2" distance="10" k1="-0.5" k2="0" k3="0"/>
            <distortion model="poly3" focal="50" k1="0"/></calibration></lens></lensdatabase>"#,
        )
        .expect("database")
    }

    fn request(w: u32, h: u32) -> lens_db::CorrectionRequest {
        lens_db::CorrectionRequest {
            crop_factor: 1.0,
            focal: 50.0,
            aperture: 2.0,
            distance: 10.0,
            width: w,
            height: h,
            distortion: true,
            tca: true,
            vignetting: true,
            adjust: lens_db::Adjustments::default(),
        }
    }

    #[test]
    fn vignetting_correction_brightens_the_corners_of_a_flat_field() {
        let db = vignetting_lens();
        let s = sensor(30, 20, BayerPattern::Rggb, [0.3, 0.3, 0.3], [1.0; 3]);
        let c = lens_db::LensCorrection::new(&db.lenses()[0], &request(30, 20)).expect("correction");
        let inputs = DevelopInputs { profile: None, lens: Some(&c) };
        let img =
            develop_with(&s, &DevelopSettings::default(), WorkingSpace::LinearRec2020, &inputs).expect("develops");
        let centre = img.get(15, 10).expect("pixel")[1];
        let corner = img.get(0, 0).expect("pixel")[1];
        assert!((centre - 0.3).abs() < 0.01, "{centre}");
        assert!(corner > centre * 1.3, "corner {corner}, centre {centre}");
    }

    #[test]
    fn a_lens_correction_for_another_size_is_refused() {
        let db = vignetting_lens();
        let s = sensor(30, 20, BayerPattern::Rggb, [0.3, 0.3, 0.3], [1.0; 3]);
        let c = lens_db::LensCorrection::new(&db.lenses()[0], &request(31, 20)).expect("correction");
        let inputs = DevelopInputs { profile: None, lens: Some(&c) };
        let r = develop_with(&s, &DevelopSettings::default(), WorkingSpace::LinearRec2020, &inputs);
        assert_eq!(r.map(|_| ()), Err(DevelopError::LensSizeMismatch));
    }

    #[test]
    fn a_profile_keeps_a_balanced_grey_grey() {
        let m = Matrix3::from_rows([[0.82, -0.3, -0.07], [-0.43, 1.2, 0.25], [-0.04, 0.13, 0.58]]);
        let profile = ColorProfile::from_matrices(
            "Test",
            &[core_model::CalibratedMatrix { illuminant: core_model::Illuminant::D65, xyz_to_camera: m }],
        )
        .expect("profile");
        // The camera saw a grey card as R 0.25, G 0.5, B 1.0 and recorded multipliers 2, 1, 0.5.
        let s = sensor(6, 6, BayerPattern::Rggb, [0.25, 0.5, 1.0], [2.0, 1.0, 0.5]);
        let inputs = DevelopInputs { profile: Some(&profile), lens: None };
        let img =
            develop_with(&s, &DevelopSettings::default(), WorkingSpace::LinearRec2020, &inputs).expect("develops");
        assert_all_close(&img, [0.5; 3], 1e-3);
    }

    #[test]
    fn reflect_mirrors_without_repeating_the_edge() {
        assert_eq!([-2, -1, 0, 4, 5, 6].map(|i| reflect(i, 5)), [2, 1, 0, 4, 3, 2]);
        assert_eq!(reflect(-1, 1), 0);
    }
}
