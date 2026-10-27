//! The fixed development stage, on the CPU: sensor data → linear working space.
//!
//! Order (fixed by the sensor's physics): white balance on the raw mosaic,
//! demosaic, camera matrix into the working space. This is the reference
//! implementation for the tracer bullet; quality demosaicing (and X-Trans)
//! moves to the GPU engine in phase 3, tested against this one.

use std::fmt;

use core_model::{CfaPattern, DevelopSettings, ImageError, ImageF32, Rgba, SensorImage, WhiteBalance, WorkingSpace};

/// Why the development stage could not run.
#[derive(Debug, Clone, PartialEq)]
pub enum DevelopError {
    /// The sensor uses a layout this build cannot demosaic yet (X-Trans).
    UnsupportedCfa,
    /// The camera colour matrix cannot be inverted.
    DegenerateCameraMatrix,
    /// The output image could not be created.
    Image(ImageError),
}

impl fmt::Display for DevelopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCfa => write!(f, "this sensor layout (X-Trans) is not supported yet"),
            Self::DegenerateCameraMatrix => write!(f, "the camera colour matrix in the file is not usable"),
            Self::Image(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for DevelopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Image(e) => Some(e),
            _ => None,
        }
    }
}

/// Develops sensor data into a linear image in `space`.
///
/// # Errors
/// [`DevelopError`] for X-Trans sensors (not yet supported) or an unusable colour matrix.
pub fn develop(
    sensor: &SensorImage,
    settings: &DevelopSettings,
    space: WorkingSpace,
) -> Result<ImageF32, DevelopError> {
    let to_working =
        core_color::camera_to_working(sensor.xyz_to_camera(), space).ok_or(DevelopError::DegenerateCameraMatrix)?;
    let wb = match settings.white_balance {
        WhiteBalance::AsShot => sensor.as_shot_wb(),
        WhiteBalance::Multipliers(m) => m,
    };

    let w = sensor.width() as usize;
    let h = sensor.height() as usize;
    let data = sensor.data();
    let mut pixels: Vec<Rgba> = Vec::with_capacity(w * h);

    match sensor.cfa() {
        CfaPattern::Bayer(pattern) => {
            // White balance on the mosaic, before interpolation mixes the channels.
            let balanced: Vec<f32> =
                data.iter().enumerate().map(|(i, v)| v * wb[pattern.channel_at(i % w, i / w)]).collect();
            for y in 0..h {
                for x in 0..w {
                    let cam = demosaic_bilinear_at(&balanced, w, h, x, y, |xx, yy| pattern.channel_at(xx, yy));
                    let rgb = to_working.apply(cam);
                    pixels.push([rgb[0], rgb[1], rgb[2], 1.0]);
                }
            }
        }
        CfaPattern::LinearRgb => {
            for cam in data.chunks_exact(3) {
                let rgb = to_working.apply([cam[0] * wb[0], cam[1] * wb[1], cam[2] * wb[2]]);
                pixels.push([rgb[0], rgb[1], rgb[2], 1.0]);
            }
        }
        CfaPattern::XTrans(_) => return Err(DevelopError::UnsupportedCfa),
    }
    ImageF32::from_pixels(sensor.width(), sensor.height(), space, pixels).map_err(DevelopError::Image)
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
        let settings = DevelopSettings { white_balance: WhiteBalance::Multipliers([1.0, 1.0, 1.0]) };
        let img = develop(&s, &settings, WorkingSpace::LinearRec2020).expect("develops");
        assert_all_close(&img, [0.5; 3], 1e-4);
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

    #[test]
    fn x_trans_is_reported_as_not_supported_yet() {
        let s = SensorImage::new(6, 6, vec![0.0; 36], CfaPattern::XTrans([[1; 6]; 6]), [1.0; 3], Matrix3::IDENTITY)
            .expect("valid sensor");
        assert_eq!(
            develop(&s, &DevelopSettings::default(), WorkingSpace::LinearRec2020),
            Err(DevelopError::UnsupportedCfa)
        );
    }

    #[test]
    fn reflect_mirrors_without_repeating_the_edge() {
        assert_eq!([-2, -1, 0, 4, 5, 6].map(|i| reflect(i, 5)), [2, 1, 0, 4, 3, 2]);
        assert_eq!(reflect(-1, 1), 0);
    }
}
