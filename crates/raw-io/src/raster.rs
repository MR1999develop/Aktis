//! JPEG and TIFF pictures, read into the same [`RawFile`] as a RAW file so
//! they develop and edit the same way, as Capture One does: the pixels,
//! already demosaiced and in sRGB, are turned back into linear light and
//! given the sRGB primaries as their "camera" colours (white balance 1, the
//! D65 matrix), and [`CaptureInfo::rendered`] tells the renderer they
//! already have their tone curve.

use std::fs::File;
use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use core_model::{CalibratedMatrix, CaptureInfo, CfaPattern, Illuminant, Matrix3, Orientation, SensorImage};

use crate::exif::{self, Exif};
use crate::thumbnail::{Thumbnail, orient_rgb8};
use crate::{MAX_FILE_BYTES, PhotoInfo, RawFile, RawIoError, orientation_from_exif};

/// File extensions of the pictures read here, lower case.
pub const RASTER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "tif", "tiff"];

/// XYZ (D65) → linear sRGB, IEC 61966-2-1: the "camera" matrix of a picture.
const XYZ_TO_SRGB: Matrix3 = Matrix3([
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
]);

/// Whether `path` names a JPEG or TIFF picture.
#[must_use]
pub fn has_raster_extension(path: &Path) -> bool {
    extension_in(path, RASTER_EXTENSIONS)
}

/// Whether `path` names a JPEG picture.
#[must_use]
pub fn has_jpeg_extension(path: &Path) -> bool {
    extension_in(path, &["jpg", "jpeg"])
}

fn extension_in(path: &Path, list: &[&str]) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| list.iter().any(|r| r.eq_ignore_ascii_case(e)))
}

/// The Exif data of the picture at `path`, without decoding its pixels.
fn read_exif(path: &Path) -> Result<Exif, RawIoError> {
    let file = BufReader::new(File::open(path).map_err(RawIoError::Io)?);
    let exif = if has_jpeg_extension(path) { exif::from_jpeg(file) } else { exif::from_tiff(file) };
    Ok(exif.unwrap_or_default())
}

/// The shooting data of a picture.
///
/// # Errors
/// [`RawIoError::Io`] when the file cannot be read.
pub(crate) fn read_info(path: &Path) -> Result<PhotoInfo, RawIoError> {
    Ok(photo_info(&read_exif(path)?))
}

fn photo_info(e: &Exif) -> PhotoInfo {
    let lens = match (&e.lens_make, &e.lens_model) {
        (Some(make), Some(model)) if !model.starts_with(make.as_str()) => Some(format!("{make} {model}")),
        (_, Some(model)) => Some(model.clone()),
        (make, None) => make.clone(),
    };
    PhotoInfo {
        exposure_time: e.exposure_time,
        aperture: e.aperture,
        iso: e.iso,
        focal_length: e.focal_length,
        exposure_bias: e.exposure_bias,
        captured: e.captured.clone(),
        gps: None,
        lens,
        camera_rating: e.rating,
        artist: e.artist.clone(),
        copyright: e.copyright.clone(),
    }
}

/// Decodes the picture at `path`.
fn decode(path: &Path) -> Result<image::DynamicImage, RawIoError> {
    let bytes = std::fs::metadata(path).map_err(RawIoError::Io)?.len();
    if bytes > MAX_FILE_BYTES {
        return Err(RawIoError::TooLarge { bytes });
    }
    catch_unwind(AssertUnwindSafe(|| -> Result<image::DynamicImage, RawIoError> {
        let mut reader = image::ImageReader::open(path).map_err(RawIoError::Io)?;
        reader = reader.with_guessed_format().map_err(RawIoError::Io)?;
        // Large TIFF files pass the decoder's default memory limit.
        reader.no_limits();
        reader.decode().map_err(|e| RawIoError::Decode(e.to_string()))
    }))
    .map_err(|_| RawIoError::DecoderCrashed)?
}

/// sRGB-encoded value (0–1) to linear light.
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// The linear RGB pixels of `image`, three values per pixel.
fn linear_rgb(image: &image::DynamicImage) -> Vec<f32> {
    let eight_bit = matches!(
        image,
        image::DynamicImage::ImageLuma8(_)
            | image::DynamicImage::ImageLumaA8(_)
            | image::DynamicImage::ImageRgb8(_)
            | image::DynamicImage::ImageRgba8(_)
    );
    if eight_bit {
        let table: Vec<f32> = (0..=255_u8).map(|v| srgb_decode(f32::from(v) / 255.0)).collect();
        image.to_rgb8().into_raw().into_iter().map(|v| table[usize::from(v)]).collect()
    } else {
        image.to_rgb32f().into_raw().into_iter().map(|v| srgb_decode(v.clamp(0.0, 1.0))).collect()
    }
}

/// Reads and decodes a JPEG or TIFF picture.
///
/// # Errors
/// [`RawIoError`] when the file cannot be read or decoded.
pub(crate) fn read(path: &Path) -> Result<RawFile, RawIoError> {
    let exif = read_exif(path)?;
    let image = decode(path)?;
    let (width, height) = (image.width(), image.height());
    let data = linear_rgb(&image);
    drop(image);
    let sensor = SensorImage::new(width, height, data, CfaPattern::LinearRgb, [1.0; 3], XYZ_TO_SRGB)
        .map_err(RawIoError::Sensor)?;
    let make = exif.make.clone().unwrap_or_default();
    let model = exif.model.clone().unwrap_or_default();
    let mut notes = Vec::new();
    if exif.has_icc {
        notes.push("the picture's embedded colour profile is not applied yet: it is read as sRGB".to_owned());
    }
    let capture = CaptureInfo {
        camera_make: make.clone(),
        camera_model: model.clone(),
        clean_make: make.clone(),
        clean_model: model.clone(),
        // The lens corrections were made by the camera or the program that
        // wrote the picture: no lens profile is applied again.
        lens_make: None,
        lens_model: None,
        focal_length: exif.focal_length,
        aperture: exif.aperture,
        focus_distance: None,
        iso: exif.iso,
        color_matrices: vec![CalibratedMatrix { illuminant: Illuminant::D65, xyz_to_camera: XYZ_TO_SRGB }],
        rendered: true,
    };
    Ok(RawFile {
        sensor,
        make,
        model,
        orientation: exif.orientation.map_or(Orientation::Normal, orientation_from_exif),
        has_camera_white_balance: true,
        capture,
        info: photo_info(&exif),
        notes,
    })
}

/// A small upright preview of a picture, no larger than `max_side`.
///
/// # Errors
/// [`RawIoError`] when the file cannot be read or decoded.
pub(crate) fn read_thumbnail(path: &Path, max_side: u32) -> Result<Thumbnail, RawIoError> {
    let max_side = max_side.max(1);
    let orientation = read_exif(path)?.orientation.map_or(Orientation::Normal, orientation_from_exif);
    let image = decode(path)?;
    let image = if image.width().max(image.height()) > max_side { image.thumbnail(max_side, max_side) } else { image };
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let (width, height, rgb) = orient_rgb8(w, h, &rgb.into_raw(), orientation);
    if width == 0 || height == 0 {
        return Err(RawIoError::Decode("the picture is empty".to_owned()));
    }
    Ok(Thumbnail { width, height, rgb })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn pictures_are_known_by_their_extension() {
        assert!(has_raster_extension(Path::new("a.JPG")) && has_raster_extension(Path::new("b.tiff")));
        assert!(!has_raster_extension(Path::new("c.NEF")));
        assert!(has_jpeg_extension(Path::new("a.jpeg")) && !has_jpeg_extension(Path::new("b.tif")));
    }

    #[test]
    fn the_srgb_matrix_maps_d65_white_to_equal_channels() {
        let rgb = XYZ_TO_SRGB.apply([0.950_47, 1.0, 1.088_83]);
        assert!(rgb.iter().all(|v| (v - 1.0).abs() < 1e-3), "{rgb:?}");
        assert!((srgb_decode(1.0) - 1.0).abs() < 1e-6 && srgb_decode(0.0) == 0.0);
        assert!((srgb_decode(0.5) - 0.214).abs() < 1e-3);
    }

    #[test]
    fn a_jpeg_reads_as_linear_rgb_marked_as_a_finished_picture() {
        let dir = std::env::temp_dir().join(format!("rne-raster-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("grey.jpg");
        let img = image::RgbImage::from_pixel(8, 4, image::Rgb([128, 128, 128]));
        img.save(&path).unwrap();
        let file = read(&path).unwrap();
        assert!(file.capture.rendered && file.has_camera_white_balance);
        assert_eq!((file.sensor.width(), file.sensor.height()), (8, 4));
        let thumb = read_thumbnail(&path, 4).unwrap();
        assert_eq!((thumb.width, thumb.height), (4, 2));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
