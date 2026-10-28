//! Small upright previews for the filmstrip, from the JPEG the camera
//! embedded in the RAW file: no decoding of the sensor data, a few
//! milliseconds per file.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use core_model::Orientation;
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use crate::{RawIoError, orientation_from_exif};

/// 8-bit sRGB pixels of a preview: `R G B` per pixel, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 3` bytes.
    pub rgb: Vec<u8>,
}

/// The embedded preview of a RAW file, upright, no larger than `max_side`
/// on its longer side. The smaller embedded image is used when it is big
/// enough, the large preview otherwise.
///
/// # Errors
/// [`RawIoError`] when the file cannot be read or has no embedded preview.
pub fn read_thumbnail(path: &Path, max_side: u32) -> Result<Thumbnail, RawIoError> {
    let max_side = max_side.max(1);
    let source = RawSource::new(path).map_err(RawIoError::Io)?;
    let params = RawDecodeParams::default();
    let found = catch_unwind(AssertUnwindSafe(|| {
        let decoder = rawler::get_decoder(&source).map_err(|e| RawIoError::Decode(e.to_string()))?;
        let exif_orientation = decoder.raw_metadata(&source, &params).ok().and_then(|m| m.exif.orientation);
        let small = decoder.thumbnail_image(&source, &params).ok().flatten();
        let image = match small {
            Some(img) if img.width().max(img.height()) >= max_side => Some(img),
            other => decoder.preview_image(&source, &params).ok().flatten().or(other),
        };
        Ok::<_, RawIoError>((image, exif_orientation))
    }))
    .map_err(|_| RawIoError::DecoderCrashed)??;
    let (Some(image), exif_orientation) = found else {
        return Err(RawIoError::Decode("the file has no embedded preview".to_owned()));
    };
    let image = if image.width().max(image.height()) > max_side { image.thumbnail(max_side, max_side) } else { image };
    let rgb = image.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    let orientation = exif_orientation.map_or(Orientation::Normal, orientation_from_exif);
    let (width, height, rgb) = orient_rgb8(w, h, &rgb.into_raw(), orientation);
    if width == 0 || height == 0 {
        return Err(RawIoError::Decode("the embedded preview is empty".to_owned()));
    }
    Ok(Thumbnail { width, height, rgb })
}

/// A JPEG kept by the editor (the preview of an edited photo, already
/// upright), no larger than `max_side` on its longer side.
///
/// # Errors
/// [`RawIoError::Decode`] when the bytes are not a readable JPEG.
pub fn decode_jpeg(bytes: &[u8], max_side: u32) -> Result<Thumbnail, RawIoError> {
    let max_side = max_side.max(1);
    let image = catch_unwind(AssertUnwindSafe(|| image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)))
        .map_err(|_| RawIoError::DecoderCrashed)?
        .map_err(|e| RawIoError::Decode(e.to_string()))?;
    let image = if image.width().max(image.height()) > max_side { image.thumbnail(max_side, max_side) } else { image };
    let rgb = image.to_rgb8();
    let (width, height) = (rgb.width(), rgb.height());
    if width == 0 || height == 0 {
        return Err(RawIoError::Decode("the stored preview is empty".to_owned()));
    }
    Ok(Thumbnail { width, height, rgb: rgb.into_raw() })
}

/// Turns packed RGB pixels upright; returns the new width, height and pixels.
/// A buffer shorter than `w × h × 3` gives an empty image.
#[must_use]
pub fn orient_rgb8(w: u32, h: u32, rgb: &[u8], o: Orientation) -> (u32, u32, Vec<u8>) {
    let (wu, hu) = (w as usize, h as usize);
    if rgb.len() < wu * hu * 3 || wu == 0 || hu == 0 {
        return (0, 0, Vec::new());
    }
    let (ow, oh) = if o.swaps_axes() { (hu, wu) } else { (wu, hu) };
    let mut out = vec![0_u8; ow * oh * 3];
    for y in 0..oh {
        for x in 0..ow {
            // Source pixel shown at (x, y) of the upright image.
            let (sx, sy) = match o {
                Orientation::Normal => (x, y),
                Orientation::FlipHorizontal => (wu - 1 - x, y),
                Orientation::Rotate180 => (wu - 1 - x, hu - 1 - y),
                Orientation::FlipVertical => (x, hu - 1 - y),
                Orientation::Transpose => (y, x),
                Orientation::Rotate90 => (y, hu - 1 - x),
                Orientation::Transverse => (wu - 1 - y, hu - 1 - x),
                Orientation::Rotate270 => (wu - 1 - y, x),
            };
            let s = (sy * wu + sx) * 3;
            let d = (y * ow + x) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    #[allow(clippy::cast_possible_truncation)] // sizes come from u32 values
    (ow as u32, oh as u32, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3×2 image whose pixel (x, y) has red = 10·y + x.
    fn numbered() -> Vec<u8> {
        (0..2).flat_map(|y| (0..3).flat_map(move |x| [10 * y + x, 0, 0])).collect()
    }

    fn reds(w: u32, rgb: &[u8]) -> Vec<Vec<u8>> {
        rgb.chunks(w as usize * 3).map(|row| row.chunks(3).map(|p| p[0]).collect()).collect()
    }

    #[test]
    fn every_orientation_moves_pixels_like_the_editor_does() {
        let src = numbered();
        let cases = [
            (Orientation::Normal, vec![vec![0, 1, 2], vec![10, 11, 12]]),
            (Orientation::FlipHorizontal, vec![vec![2, 1, 0], vec![12, 11, 10]]),
            (Orientation::Rotate180, vec![vec![12, 11, 10], vec![2, 1, 0]]),
            (Orientation::FlipVertical, vec![vec![10, 11, 12], vec![0, 1, 2]]),
            (Orientation::Transpose, vec![vec![0, 10], vec![1, 11], vec![2, 12]]),
            (Orientation::Rotate90, vec![vec![10, 0], vec![11, 1], vec![12, 2]]),
            (Orientation::Transverse, vec![vec![12, 2], vec![11, 1], vec![10, 0]]),
            (Orientation::Rotate270, vec![vec![2, 12], vec![1, 11], vec![0, 10]]),
        ];
        for (o, expected) in cases {
            let (w, h, out) = orient_rgb8(3, 2, &src, o);
            assert_eq!(reds(w, &out), expected, "{o:?}");
            assert_eq!((w * h * 3) as usize, out.len());
        }
    }

    #[test]
    fn short_buffers_give_an_empty_image_instead_of_panicking() {
        assert_eq!(orient_rgb8(3, 2, &[0; 5], Orientation::Rotate90), (0, 0, Vec::new()));
        assert_eq!(orient_rgb8(0, 2, &[], Orientation::Normal), (0, 0, Vec::new()));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let r = read_thumbnail(Path::new("no/such/file.NEF"), 256);
        assert!(matches!(r, Err(RawIoError::Io(_))), "{r:?}");
    }
}
