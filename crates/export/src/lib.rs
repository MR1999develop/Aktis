//! Writes finished images to disk.
//!
//! - [`encode`] turns an upright 16-bit sRGB image into a JPEG, PNG (8/16
//!   bit) or TIFF (8/16 bit) file with the sRGB profile and the chosen
//!   metadata (EXIF, XMP);
//! - [`draw_watermark`] draws a text watermark with a built-in font;
//! - [`NamePattern`], [`Destination`] and [`Size`] decide names, folders
//!   and sizes;
//! - [`write_atomic`] and [`non_destructive_path`] write safely.
//!
//! Every write goes to a temporary file in the destination folder and is
//! renamed only when complete, so a crash or a full disk never leaves a
//! half-written file under the final name. Existing files are not
//! overwritten: a free name like `photo (2).jpg` is chosen instead.

mod deflate;
mod font;
mod icc;
mod metadata;
mod naming;
mod options;
mod png;
mod tiff;
mod watermark;

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use icc::srgb_profile;
pub use metadata::{MetadataChoice, PhotoMetadata};
pub use naming::{Destination, NAME_TOKENS, NameFields, NamePattern, sanitize_stem};
pub use options::{BitDepth, ExportOptions, FileFormat, Format, Resize, Size};
pub use watermark::{Position, Watermark, draw_watermark};

/// Why an export failed.
#[derive(Debug)]
pub enum ExportError {
    /// Width or height is zero or above 65 535 (the JPEG limit).
    BadSize {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// The pixel buffer does not hold `width × height × 3` values.
    BadBuffer,
    /// Quality outside 1–100.
    BadQuality(u8),
    /// The encoder failed.
    Encode(String),
    /// The file would exceed what its format can hold (4 GB for TIFF).
    FileTooLarge,
    /// Writing or renaming the file failed.
    Io(std::io::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadSize { width, height } => {
                write!(f, "a JPEG can be at most 65535 pixels per side, the image is {width}\u{d7}{height}")
            }
            Self::BadBuffer => write!(f, "the image data do not match the image size"),
            Self::BadQuality(q) => write!(f, "JPEG quality must be between 1 and 100, not {q}"),
            Self::Encode(e) => write!(f, "JPEG encoding failed: {e}"),
            Self::FileTooLarge => write!(f, "the image is too large for this file format"),
            Self::Io(e) => write!(f, "cannot write the file: {e}"),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Upright sRGB pixels at 16 bits per channel: `R G B` per pixel, rows top
/// to bottom, values encoded with the sRGB curve (0–65 535).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage16 {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width × height × 3` values.
    pub pixels: Vec<u16>,
}

/// A 16-bit value as 8 bits, rounded.
#[must_use]
pub fn to_u8(v: u16) -> u8 {
    #[allow(clippy::cast_possible_truncation)] // (v·255 + 32767) / 65535 ≤ 255
    let b = ((u32::from(v) * 255 + 32_767) / 65_535) as u8;
    b
}

/// Encodes `image` as a file of `format` with the sRGB profile and
/// `metadata` (EXIF for every format, XMP for rating, keywords and
/// copyright). Returns the file's bytes.
///
/// # Errors
/// [`ExportError`] for invalid images or settings, or a file too large for its format.
pub fn encode(image: &RgbImage16, format: &Format, metadata: &PhotoMetadata) -> Result<Vec<u8>, ExportError> {
    let (w, h) = (image.width, image.height);
    if w == 0 || h == 0 {
        return Err(ExportError::BadSize { width: w, height: h });
    }
    if image.pixels.len() as u64 != u64::from(w) * u64::from(h) * 3 {
        return Err(ExportError::BadBuffer);
    }
    let xmp = metadata::xmp_packet(metadata);
    let icc = srgb_profile();
    match format.kind {
        FileFormat::Jpeg => {
            let rgb: Vec<u8> = image.pixels.iter().map(|&v| to_u8(v)).collect();
            let exif = metadata::exif_block(metadata, w, h)?;
            encode_jpeg_with(&rgb, w, h, format.jpeg_quality, exif.as_deref(), xmp.as_deref())
        }
        FileFormat::Png => {
            let exif = metadata::exif_block(metadata, w, h)?;
            png::encode_png(image, format.depth, &icc, exif.as_deref(), xmp.as_deref())
        }
        FileFormat::Tiff => {
            let ifd = metadata::exif_ifd(metadata, w, h);
            tiff::encode_tiff(image, format.depth, &icc, ifd, xmp.as_deref())
        }
    }
}

/// Encodes 8-bit sRGB pixels (`R G B` per pixel, rows top to bottom) as a
/// JPEG with an embedded sRGB profile. Quality 90 and above keeps full
/// colour resolution (4:4:4); below, colour is subsampled 4:2:0.
///
/// # Errors
/// [`ExportError`] for invalid sizes, buffers or quality, or an encoder failure.
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, ExportError> {
    encode_jpeg_with(rgb, width, height, quality, None, None)
}

/// [`encode_jpeg`] with an EXIF block (a TIFF structure) and an XMP packet.
/// An XMP packet too long for one JPEG segment is left out.
fn encode_jpeg_with(
    rgb: &[u8],
    width: u32,
    height: u32,
    quality: u8,
    exif: Option<&[u8]>,
    xmp: Option<&str>,
) -> Result<Vec<u8>, ExportError> {
    const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    let (w, h) = match (u16::try_from(width), u16::try_from(height)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => return Err(ExportError::BadSize { width, height }),
    };
    if rgb.len() != usize::from(w) * usize::from(h) * 3 {
        return Err(ExportError::BadBuffer);
    }
    if !(1..=100).contains(&quality) {
        return Err(ExportError::BadQuality(quality));
    }
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    if quality >= 90 {
        encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    }
    let encode_error = |e: jpeg_encoder::EncodingError| ExportError::Encode(e.to_string());
    if let Some(exif) = exif {
        encoder.add_exif_metadata(exif).map_err(encode_error)?;
    }
    encoder.add_icc_profile(&srgb_profile()).map_err(encode_error)?;
    if let Some(xmp) = xmp.filter(|x| XMP_HEADER.len() + x.len() <= 65_533) {
        let mut segment = XMP_HEADER.to_vec();
        segment.extend_from_slice(xmp.as_bytes());
        encoder.add_app_segment(1, segment).map_err(encode_error)?;
    }
    encoder.encode(rgb, w, h, jpeg_encoder::ColorType::Rgb).map_err(encode_error)?;
    Ok(out)
}

/// `path` if it does not exist, else the first free `stem (n).ext`, n ≥ 2.
#[must_use]
pub fn non_destructive_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

/// Writes `bytes` to `path` atomically: a temporary file in the same folder,
/// flushed to disk, then renamed. On error the temporary file is removed and
/// `path` is untouched.
///
/// # Errors
/// [`ExportError::Io`].
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(ExportError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aktis-export-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("create temp dir");
        d
    }

    /// The APP segments of a JPEG: (marker, payload).
    fn jpeg_segments(jpg: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out = Vec::new();
        let mut p = 2;
        while p + 4 <= jpg.len() && jpg[p] == 0xFF && (0xE0..=0xEF).contains(&jpg[p + 1]) {
            let len = usize::from(u16::from_be_bytes([jpg[p + 2], jpg[p + 3]]));
            out.push((jpg[p + 1], jpg[p + 4..p + 2 + len].to_vec()));
            p += 2 + len;
        }
        out
    }

    fn small_image() -> RgbImage16 {
        RgbImage16 { width: 4, height: 3, pixels: (0..36).map(|v| v * 1800).collect() }
    }

    fn metadata() -> PhotoMetadata {
        PhotoMetadata {
            model: Some("D3S".into()),
            captured: Some("2026:08:14 18:42:07".into()),
            rating: Some(3),
            keywords: vec!["lake".into()],
            copyright: Some("Ada".into()),
            ..PhotoMetadata::default()
        }
    }

    #[test]
    fn a_jpeg_starts_with_soi_carries_the_icc_profile_and_ends_with_eoi() {
        let rgb = vec![118_u8; 4 * 3 * 3];
        let jpg = encode_jpeg(&rgb, 4, 3, 92).expect("encodes");
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        assert_eq!(&jpg[jpg.len() - 2..], &[0xFF, 0xD9]);
        let icc_marker = b"ICC_PROFILE\0";
        assert!(jpg.windows(icc_marker.len()).any(|w| w == icc_marker), "no ICC segment");
    }

    #[test]
    fn a_jpeg_export_carries_exif_icc_and_xmp_segments() {
        let jpg = encode(&small_image(), &Format::default(), &metadata()).expect("encodes");
        let segments = jpeg_segments(&jpg);
        let exif = segments.iter().find(|(m, d)| *m == 0xE1 && d.starts_with(b"Exif\0\0")).expect("EXIF");
        let d = tiff::read::ifd0(&exif.1[6..]);
        assert_eq!(tiff::read::ascii(&d, 0x0110), "D3S");
        assert_eq!(tiff::read::number(&d, 0x0112), 1, "upright");
        assert!(segments.iter().any(|(m, d)| *m == 0xE2 && d.starts_with(b"ICC_PROFILE\0")));
        let xmp = segments.iter().find(|(m, d)| *m == 0xE1 && d.starts_with(b"http://ns.adobe.com/xap/1.0/\0"));
        let xmp = String::from_utf8(xmp.expect("XMP").1.clone()).expect("UTF-8");
        assert!(xmp.contains("xmp:Rating=\"3\"") && xmp.contains("<rdf:li>lake</rdf:li>"));

        let bare = encode(&small_image(), &Format::default(), &PhotoMetadata::default()).expect("encodes");
        assert!(!jpeg_segments(&bare).iter().any(|(m, _)| *m == 0xE1), "no metadata, no APP1");
    }

    #[test]
    fn png_and_tiff_exports_carry_their_metadata() {
        let png_format = Format { kind: FileFormat::Png, depth: BitDepth::Sixteen, ..Format::default() };
        let file = encode(&small_image(), &png_format, &metadata()).expect("encodes");
        let chunks = png::tests::chunks(&file);
        let exif = chunks.iter().find(|(k, _)| k == b"eXIf").expect("eXIf");
        assert_eq!(tiff::read::ascii(&tiff::read::ifd0(&exif.1), 0x8298), "Ada");
        assert!(chunks.iter().any(|(k, _)| k == b"iCCP"));
        assert!(chunks.iter().any(|(k, _)| k == b"iTXt"));

        let tiff_format = Format { kind: FileFormat::Tiff, depth: BitDepth::Eight, ..Format::default() };
        let file = encode(&small_image(), &tiff_format, &metadata()).expect("encodes");
        let d = tiff::read::ifd0(&file);
        assert_eq!(tiff::read::number(&d, 256), 4);
        assert_eq!(tiff::read::ascii(&d, 0x0110), "D3S");
        let exif = tiff::read::sub(&file, &d, 0x8769);
        assert_eq!(tiff::read::ascii(&exif, 0x9003), "2026:08:14 18:42:07");
        assert_eq!(d[&34_675].2, srgb_profile());
        assert!(d.contains_key(&700), "XMP");
    }

    #[test]
    fn sixteen_bits_round_to_eight() {
        assert_eq!([0, 128, 129, 32_767, 65_400, 65_535].map(to_u8), [0, 0, 1, 127, 254, 255]);
    }

    #[test]
    fn invalid_sizes_buffers_and_qualities_are_refused() {
        assert!(matches!(encode_jpeg(&[], 0, 1, 90), Err(ExportError::BadSize { .. })));
        assert!(matches!(encode_jpeg(&[], 70_000, 1, 90), Err(ExportError::BadSize { .. })));
        assert!(matches!(encode_jpeg(&[0; 5], 1, 2, 90), Err(ExportError::BadBuffer)));
        assert!(matches!(encode_jpeg(&[0; 3], 1, 1, 0), Err(ExportError::BadQuality(0))));
        assert!(matches!(encode_jpeg(&[0; 3], 1, 1, 101), Err(ExportError::BadQuality(101))));
        let bad = RgbImage16 { width: 2, height: 2, pixels: vec![0; 5] };
        assert!(matches!(encode(&bad, &Format::default(), &PhotoMetadata::default()), Err(ExportError::BadBuffer)));
    }

    #[test]
    fn existing_files_get_a_numbered_name_instead_of_being_overwritten() {
        let d = temp_dir("names");
        let p = d.join("photo.jpg");
        assert_eq!(non_destructive_path(&p), p);
        fs::write(&p, b"x").expect("write");
        assert_eq!(non_destructive_path(&p), d.join("photo (2).jpg"));
        fs::write(d.join("photo (2).jpg"), b"x").expect("write");
        assert_eq!(non_destructive_path(&p), d.join("photo (3).jpg"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn an_atomic_write_leaves_only_the_final_file() {
        let d = temp_dir("atomic");
        let p = d.join("out.jpg");
        write_atomic(&p, b"data").expect("write");
        assert_eq!(fs::read(&p).expect("read"), b"data");
        let names: Vec<_> = fs::read_dir(&d).expect("list").flatten().map(|e| e.file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file_behind() {
        let d = temp_dir("fail");
        // The destination is an existing directory: the rename fails.
        let target = d.join("taken");
        fs::create_dir(&target).expect("mkdir");
        fs::write(target.join("keep"), b"x").expect("write");
        assert!(write_atomic(&target, b"data").is_err());
        let names: Vec<_> = fs::read_dir(&d).expect("list").flatten().map(|e| e.file_name()).collect();
        assert_eq!(names, vec![std::ffi::OsString::from("taken")]);
        let _ = fs::remove_dir_all(&d);
    }
}
