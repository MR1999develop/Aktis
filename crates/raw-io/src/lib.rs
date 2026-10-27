//! Reads camera RAW and DNG files into [`SensorImage`] and [`CaptureInfo`].
//!
//! The decoder (`rawler`) stays behind this crate: nothing else in the
//! workspace sees its types. Black and white levels are applied here, keeping
//! values below black (noise) instead of clipping them, and the image is cut
//! to the area the camera declares as valid. Shooting data (lens, focal
//! length, aperture) and the camera's colour matrices for every calibration
//! illuminant are read too, for lens corrections and camera profiles.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use core_model::{
    BayerPattern, CalibratedMatrix, CaptureInfo, CfaPattern, Illuminant as LightSource, Matrix3, Orientation,
    SensorError, SensorImage,
};
use rawler::RawImageData;
use rawler::decoders::{Decoder, RawDecodeParams, RawMetadata};
use rawler::imgop::xyz::Illuminant;
use rawler::rawimage::RawPhotometricInterpretation;
use rawler::rawsource::RawSource;

/// Largest file accepted, checked before reading (1 GiB; real RAW files are far smaller).
pub const MAX_FILE_BYTES: u64 = 1 << 30;

/// File extensions of the RAW formats offered in open dialogs, lower case.
/// Only a convenience for the dialog filter: [`read_raw`] recognises a file
/// by its content, whatever its name.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf",
    "pef", "raf", "rw2", "rwl", "sr2", "srf", "srw",
];

/// Whether `path` has one of the [`RAW_EXTENSIONS`] (any case).
#[must_use]
pub fn has_raw_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| RAW_EXTENSIONS.iter().any(|r| r.eq_ignore_ascii_case(e)))
}

/// Why a RAW file could not be read.
#[derive(Debug)]
pub enum RawIoError {
    /// The file could not be opened or its size read.
    Io(std::io::Error),
    /// The file is larger than [`MAX_FILE_BYTES`].
    TooLarge {
        /// Size of the file in bytes.
        bytes: u64,
    },
    /// The decoder does not know this camera or format, or the file is damaged.
    Decode(String),
    /// The decoder stopped unexpectedly on this file.
    DecoderCrashed,
    /// Monochrome sensors are not supported yet.
    Monochrome,
    /// The colour filter is neither RGB Bayer, X-Trans nor linear RGB.
    UnsupportedLayout(String),
    /// The file has no usable colour matrix.
    NoColorMatrix,
    /// Black and white levels leave no usable range.
    BadLevels,
    /// The decoded data do not form a valid sensor image.
    Sensor(SensorError),
}

impl fmt::Display for RawIoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read the file: {e}"),
            Self::TooLarge { bytes } => {
                write!(f, "file of {} MB is larger than the {} MB limit", bytes >> 20, MAX_FILE_BYTES >> 20)
            }
            Self::Decode(msg) => write!(f, "cannot decode the RAW file: {msg}"),
            Self::DecoderCrashed => write!(f, "the RAW decoder stopped on this file"),
            Self::Monochrome => write!(f, "monochrome RAW files are not supported yet"),
            Self::UnsupportedLayout(name) => write!(f, "sensor layout '{name}' is not supported"),
            Self::NoColorMatrix => write!(f, "the file has no colour matrix for this camera"),
            Self::BadLevels => write!(f, "black and white levels in the file are not usable"),
            Self::Sensor(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RawIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Sensor(e) => Some(e),
            _ => None,
        }
    }
}

/// A decoded RAW file.
#[derive(Debug)]
pub struct RawFile {
    /// Normalised sensor data, cut to the valid area.
    pub sensor: SensorImage,
    /// Camera maker, cleaned up ("Sony").
    pub make: String,
    /// Camera model, cleaned up ("ILCE-7RM5").
    pub model: String,
    /// How to turn the image upright.
    pub orientation: Orientation,
    /// `false` when the file had no white balance and neutral multipliers were used.
    pub has_camera_white_balance: bool,
    /// Camera, lens, shooting settings and colour calibration.
    pub capture: CaptureInfo,
}

/// Reads and decodes a RAW or DNG file.
///
/// # Errors
/// [`RawIoError`] when the file cannot be read, is too large, or its content is unsupported.
pub fn read_raw(path: &Path) -> Result<RawFile, RawIoError> {
    let bytes = std::fs::metadata(path).map_err(RawIoError::Io)?.len();
    if bytes > MAX_FILE_BYTES {
        return Err(RawIoError::TooLarge { bytes });
    }
    let source = RawSource::new(path).map_err(RawIoError::Io)?;
    let params = RawDecodeParams::default();
    // The decoder has unfinished branches that panic on rare files; a bad file
    // must fail on its own, never take the application down.
    let raw = catch_unwind(AssertUnwindSafe(|| rawler::decode(&source, &params)))
        .map_err(|_| RawIoError::DecoderCrashed)?
        .map_err(|e| RawIoError::Decode(e.to_string()))?;
    // Shooting data is optional: a file whose metadata cannot be read still develops.
    let metadata = catch_unwind(AssertUnwindSafe(|| {
        let decoder = rawler::get_decoder(&source).ok()?;
        Decoder::raw_metadata(&*decoder, &source, &params).ok()
    }))
    .ok()
    .flatten();
    convert(&raw, metadata.as_ref())
}

fn convert(raw: &rawler::RawImage, metadata: Option<&RawMetadata>) -> Result<RawFile, RawIoError> {
    let area = valid_area(raw.width, raw.height, rect(raw.crop_area), rect(raw.active_area));
    let (cfa, cpp) = match &raw.photometric {
        RawPhotometricInterpretation::BlackIsZero => return Err(RawIoError::Monochrome),
        RawPhotometricInterpretation::Cfa(config) => {
            let colors = |dy: usize, dx: usize| config.cfa.color_at(area.y + dy, area.x + dx);
            (cfa_pattern(config.cfa.width, config.cfa.height, &config.cfa.to_string(), colors)?, 1)
        }
        RawPhotometricInterpretation::LinearRaw if raw.cpp == 3 => (CfaPattern::LinearRgb, 3),
        RawPhotometricInterpretation::LinearRaw => {
            return Err(RawIoError::UnsupportedLayout(format!("linear, {} channels", raw.cpp)));
        }
    };

    let levels = Levels::new(&raw.blacklevel.as_bayer_array(), &raw.whitelevel.as_bayer_array(), cpp, raw)?;
    let data = match &raw.data {
        RawImageData::Integer(v) => scale(v, raw.width, cpp, area, &levels, |s| f32::from(*s)),
        RawImageData::Float(v) => scale(v, raw.width, cpp, area, &levels, |s| *s),
    }
    .ok_or_else(|| RawIoError::Decode("pixel data shorter than declared".to_owned()))?;

    let has_wb = raw.wb_coeffs[..3].iter().all(|m| m.is_finite() && *m > 0.0);
    let wb = if has_wb { [raw.wb_coeffs[0], raw.wb_coeffs[1], raw.wb_coeffs[2]] } else { [1.0; 3] };

    let sensor = SensorImage::new(
        u32::try_from(area.w).map_err(|_| RawIoError::BadLevels)?,
        u32::try_from(area.h).map_err(|_| RawIoError::BadLevels)?,
        data,
        cfa,
        wb,
        color_matrix(raw)?,
    )
    .map_err(RawIoError::Sensor)?;

    let capture = capture_info(raw, metadata);
    Ok(RawFile {
        sensor,
        make: raw.clean_make.clone(),
        model: raw.clean_model.clone(),
        orientation: orientation(raw.orientation),
        has_camera_white_balance: has_wb,
        capture,
    })
}

/// Shooting data and colour calibration of the file.
#[allow(clippy::cast_possible_truncation)] // illuminant codes are below 256
fn capture_info(raw: &rawler::RawImage, metadata: Option<&RawMetadata>) -> CaptureInfo {
    let exif = metadata.map(|m| &m.exif);
    let text = |s: Option<&String>| s.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    let aperture = exif.and_then(|e| e.fnumber.and_then(|r| ratio(r.n, r.d))).or_else(|| {
        // APEX aperture value: f-number = 2^(Av / 2).
        exif.and_then(|e| e.aperture_value.and_then(|r| ratio(r.n, r.d))).map(|av| 2_f32.powf(av / 2.0))
    });
    // EXIF stores "infinity" as 0xFFFFFFFF/1 and "unknown" as 0.
    let focus_distance = exif
        .and_then(|e| e.subject_distance)
        .and_then(|r| if r.n == u32::MAX { Some(1000.0) } else { ratio(r.n, r.d) });
    let mut color_matrices: Vec<CalibratedMatrix> = raw
        .color_matrix
        .iter()
        .filter_map(|(illuminant, flat)| {
            Some(CalibratedMatrix {
                illuminant: LightSource::from_code(*illuminant as u16),
                xyz_to_camera: matrix_from_flat(flat)?,
            })
        })
        .collect();
    // HashMap order is arbitrary: sort for reproducible results.
    color_matrices.sort_by(|a, b| {
        let t = |m: &CalibratedMatrix| m.illuminant.temperature().unwrap_or(f32::MAX);
        t(a).total_cmp(&t(b))
    });
    CaptureInfo {
        camera_make: raw.make.trim().to_owned(),
        camera_model: raw.model.trim().to_owned(),
        clean_make: raw.clean_make.clone(),
        clean_model: raw.clean_model.clone(),
        lens_make: text(exif.and_then(|e| e.lens_make.as_ref())),
        lens_model: text(exif.and_then(|e| e.lens_model.as_ref())),
        focal_length: exif.and_then(|e| e.focal_length.and_then(|r| ratio(r.n, r.d))),
        aperture,
        focus_distance,
        color_matrices,
    }
}

/// `n / d` when both are positive and the result is finite.
#[allow(clippy::cast_precision_loss)] // EXIF rationals are small numbers
fn ratio(n: u32, d: u32) -> Option<f32> {
    (n > 0 && d > 0).then(|| n as f32 / d as f32).filter(|v| v.is_finite())
}

/// A rectangle in sensor coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Area {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

fn rect(r: Option<rawler::imgop::Rect>) -> Option<Area> {
    r.map(|r| Area { x: r.p.x, y: r.p.y, w: r.d.w, h: r.d.h })
}

/// The area to keep: the camera's default crop, else the active area, else
/// the whole frame. An area that does not fit in the frame is ignored.
fn valid_area(width: usize, height: usize, crop: Option<Area>, active: Option<Area>) -> Area {
    let fits = |a: &Area| a.w > 0 && a.h > 0 && a.x + a.w <= width && a.y + a.h <= height;
    crop.filter(fits).or(active.filter(fits)).unwrap_or(Area { x: 0, y: 0, w: width, h: height })
}

/// Our CFA description from the decoder's: colour index (0 R, 1 G, 2 B) at
/// row `dy`, column `dx` from the top-left of the kept area.
fn cfa_pattern(
    width: usize,
    height: usize,
    name: &str,
    colors: impl Fn(usize, usize) -> usize,
) -> Result<CfaPattern, RawIoError> {
    let unsupported = || RawIoError::UnsupportedLayout(name.to_owned());
    match (width, height) {
        (2, 2) => {
            let tile = [[colors(0, 0), colors(0, 1)], [colors(1, 0), colors(1, 1)]];
            let pattern = match tile {
                [[0, 1], [1, 2]] => BayerPattern::Rggb,
                [[2, 1], [1, 0]] => BayerPattern::Bggr,
                [[1, 0], [2, 1]] => BayerPattern::Grbg,
                [[1, 2], [0, 1]] => BayerPattern::Gbrg,
                _ => return Err(unsupported()),
            };
            Ok(CfaPattern::Bayer(pattern))
        }
        (6, 6) => {
            let mut p = [[0_u8; 6]; 6];
            for (dy, row) in p.iter_mut().enumerate() {
                for (dx, cell) in row.iter_mut().enumerate() {
                    *cell = u8::try_from(colors(dy, dx)).ok().filter(|c| *c <= 2).ok_or_else(unsupported)?;
                }
            }
            Ok(CfaPattern::XTrans(p))
        }
        _ => Err(unsupported()),
    }
}

/// Black and white levels per CFA position (2×2) or per channel (linear RGB).
#[derive(Debug, Clone, PartialEq)]
struct Levels {
    black: [f32; 4],
    range: [f32; 4],
}

impl Levels {
    fn new(black: &[f32; 4], white: &[f32; 4], cpp: usize, raw: &rawler::RawImage) -> Result<Self, RawIoError> {
        // Linear RGB: one level per channel when the file gives them, else the first.
        let (black, white) = if cpp == 3 {
            let pick = |v: Vec<f32>| {
                let first = v.first().copied().unwrap_or(0.0);
                [0, 1, 2, 3].map(|i| if v.len() >= 3 { v.get(i).copied().unwrap_or(first) } else { first })
            };
            (pick(raw.blacklevel.as_vec()), pick(raw.whitelevel.as_vec()))
        } else {
            (*black, *white)
        };
        let mut range = [0.0; 4];
        for i in 0..4 {
            range[i] = white[i] - black[i];
            if !(range[i].is_finite() && range[i] > 0.0) {
                return Err(RawIoError::BadLevels);
            }
        }
        Ok(Self { black, range })
    }

    /// Index of the level for sample `c` of the pixel at frame position (x, y).
    fn index(cpp: usize, x: usize, y: usize, c: usize) -> usize {
        if cpp == 3 { c } else { (y % 2) * 2 + (x % 2) }
    }
}

/// Cuts `area` out of the frame and normalises it: `(v - black) / (white - black)`.
/// Values below black stay negative. Returns `None` when `src` is too short.
fn scale<T>(
    src: &[T],
    frame_width: usize,
    cpp: usize,
    area: Area,
    levels: &Levels,
    to_f32: impl Fn(&T) -> f32,
) -> Option<Vec<f32>> {
    let mut out = Vec::with_capacity(area.w * area.h * cpp);
    for y in area.y..area.y + area.h {
        let row = src.get((y * frame_width + area.x) * cpp..(y * frame_width + area.x + area.w) * cpp)?;
        for (i, s) in row.iter().enumerate() {
            let x = area.x + i / cpp;
            let k = Levels::index(cpp, x, y, i % cpp);
            out.push((to_f32(s) - levels.black[k]) / levels.range[k]);
        }
    }
    Some(out)
}

/// The XYZ (D65) → camera matrix: the D65 one when present, else daylight,
/// else the first the file offers.
fn color_matrix(raw: &rawler::RawImage) -> Result<Matrix3, RawIoError> {
    let preferred = [Illuminant::D65, Illuminant::Daylight, Illuminant::D55, Illuminant::FineWeather, Illuminant::D50];
    let flat = raw
        .color_matrix_find_first(preferred)
        .map(|(_, m)| m)
        .or_else(|| raw.color_matrix.values().next().cloned())
        .ok_or(RawIoError::NoColorMatrix)?;
    matrix_from_flat(&flat).ok_or(RawIoError::NoColorMatrix)
}

/// First three rows of a flat row-major matrix with three columns; `None`
/// when it is too short, not finite or all zero.
fn matrix_from_flat(flat: &[f32]) -> Option<Matrix3> {
    let v = flat.get(..9)?;
    if v.iter().any(|x| !x.is_finite()) || v.iter().all(|x| *x == 0.0) {
        return None;
    }
    Some(Matrix3::from_rows([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]))
}

fn orientation(o: rawler::Orientation) -> Orientation {
    use rawler::Orientation as R;
    match o {
        R::Normal | R::Unknown => Orientation::Normal,
        R::HorizontalFlip => Orientation::FlipHorizontal,
        R::Rotate180 => Orientation::Rotate180,
        R::VerticalFlip => Orientation::FlipVertical,
        R::Transpose => Orientation::Transpose,
        R::Rotate90 => Orientation::Rotate90,
        R::Transverse => Orientation::Transverse,
        R::Rotate270 => Orientation::Rotate270,
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // values are exact in these small cases
mod tests {
    use super::*;

    #[test]
    fn the_default_crop_wins_over_the_active_area_and_bad_areas_are_ignored() {
        let crop = Area { x: 2, y: 2, w: 10, h: 8 };
        let active = Area { x: 1, y: 1, w: 12, h: 10 };
        assert_eq!(valid_area(16, 12, Some(crop), Some(active)), crop);
        assert_eq!(valid_area(16, 12, None, Some(active)), active);
        let outside = Area { x: 10, y: 0, w: 10, h: 5 };
        assert_eq!(valid_area(16, 12, Some(outside), None), Area { x: 0, y: 0, w: 16, h: 12 });
    }

    #[test]
    fn bayer_patterns_are_read_at_the_crop_origin() {
        // An RGGB sensor cut at an odd column starts with G R / B G.
        let rggb = |row: usize, col: usize| [[0, 1], [1, 2]][row % 2][col % 2];
        let odd_column = cfa_pattern(2, 2, "RGGB", |dy, dx| rggb(dy, dx + 1)).expect("bayer");
        assert_eq!(odd_column, CfaPattern::Bayer(BayerPattern::Grbg));
        let even = cfa_pattern(2, 2, "RGGB", rggb).expect("bayer");
        assert_eq!(even, CfaPattern::Bayer(BayerPattern::Rggb));
    }

    #[test]
    fn non_rgb_filters_are_refused() {
        let cygm = cfa_pattern(2, 2, "CYGM", |r, c| [[3, 5], [1, 4]][r][c]);
        assert!(matches!(cygm, Err(RawIoError::UnsupportedLayout(_))));
        let odd_size = cfa_pattern(4, 4, "XXXX", |_, _| 0);
        assert!(matches!(odd_size, Err(RawIoError::UnsupportedLayout(_))));
    }

    #[test]
    fn x_trans_tiles_are_copied() {
        let p = cfa_pattern(6, 6, "XTRANS", |r, c| (r + c) % 3).expect("x-trans");
        let CfaPattern::XTrans(t) = p else { panic!("expected X-Trans, got {p:?}") };
        assert_eq!(t[1][1], 2);
        assert_eq!(t[5][0], 2);
    }

    #[test]
    fn scaling_uses_the_level_of_each_cfa_position_and_keeps_noise_below_black() {
        let levels = Levels { black: [100.0, 200.0, 200.0, 300.0], range: [1000.0, 1000.0, 1000.0, 1000.0] };
        // 4×2 frame, keep the 2×2 area at x=1: positions (1,0) (2,0) / (1,1) (2,1).
        let src: Vec<u16> = vec![0, 1200, 600, 0, 0, 1200, 250, 0];
        let area = Area { x: 1, y: 0, w: 2, h: 2 };
        let out = scale(&src, 4, 1, area, &levels, |s| f32::from(*s)).expect("long enough");
        // (1,0) uses black[1]=200; (2,0) black[0]=100; (1,1) black[3]=300; (2,1) black[2]=200.
        assert_eq!(out, vec![1.0, 0.5, 0.9, 0.05]);

        let below = scale(&[50_u16], 1, 1, Area { x: 0, y: 0, w: 1, h: 1 }, &levels, |s| f32::from(*s));
        assert_eq!(below, Some(vec![-0.05]));
    }

    #[test]
    fn short_pixel_data_is_reported_instead_of_panicking() {
        let levels = Levels { black: [0.0; 4], range: [1.0; 4] };
        let r = scale(&[0.5_f32; 3], 2, 1, Area { x: 0, y: 0, w: 2, h: 2 }, &levels, |s| *s);
        assert_eq!(r, None);
    }

    #[test]
    fn colour_matrices_need_nine_finite_non_zero_values() {
        assert!(matrix_from_flat(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]).is_some());
        assert!(matrix_from_flat(&[1.0; 8]).is_none());
        assert!(matrix_from_flat(&[0.0; 9]).is_none());
        assert!(matrix_from_flat(&[f32::NAN; 9]).is_none());
    }

    #[test]
    fn exif_rationals_become_numbers_only_when_meaningful() {
        assert_eq!(ratio(28, 10), Some(2.8));
        assert_eq!(ratio(0, 1), None);
        assert_eq!(ratio(5, 0), None);
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let r = read_raw(Path::new("this/file/does/not/exist.dng"));
        assert!(matches!(r, Err(RawIoError::Io(_))), "{r:?}");
    }

    #[test]
    fn raw_extensions_match_in_any_case_and_nothing_else() {
        assert!(has_raw_extension(Path::new("a/b/DSC_0001.NEF")));
        assert!(has_raw_extension(Path::new("photo.arw")));
        assert!(has_raw_extension(Path::new("with space/IMG 1.Cr3")));
        assert!(!has_raw_extension(Path::new("photo.jpg")));
        assert!(!has_raw_extension(Path::new("nef")));
        assert!(!has_raw_extension(Path::new("")));
        assert!(RAW_EXTENSIONS.iter().all(|e| e.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())));
    }
}
