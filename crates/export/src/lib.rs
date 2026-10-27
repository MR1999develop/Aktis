//! Writes finished images to disk.
//!
//! Every write goes to a temporary file in the destination folder and is
//! renamed only when complete, so a crash or a full disk never leaves a
//! half-written file under the final name. Existing files are not
//! overwritten: a free name like `photo (2).jpg` is chosen instead.

mod icc;

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub use icc::srgb_profile;

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
    /// The pixel buffer does not hold `width × height × 3` bytes.
    BadBuffer,
    /// Quality outside 1–100.
    BadQuality(u8),
    /// The encoder failed.
    Encode(String),
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

/// Encodes 8-bit sRGB pixels (`R G B` per pixel, rows top to bottom) as a
/// JPEG with an embedded sRGB profile. Quality 90 and above keeps full
/// colour resolution (4:4:4); below, colour is subsampled 4:2:0.
///
/// # Errors
/// [`ExportError`] for invalid sizes, buffers or quality, or an encoder failure.
pub fn encode_jpeg(rgb: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>, ExportError> {
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
    encoder.add_icc_profile(&srgb_profile()).map_err(|e| ExportError::Encode(e.to_string()))?;
    encoder.encode(rgb, w, h, jpeg_encoder::ColorType::Rgb).map_err(|e| ExportError::Encode(e.to_string()))?;
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
        let d = std::env::temp_dir().join(format!("rne-export-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("create temp dir");
        d
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
    fn invalid_sizes_buffers_and_qualities_are_refused() {
        assert!(matches!(encode_jpeg(&[], 0, 1, 90), Err(ExportError::BadSize { .. })));
        assert!(matches!(encode_jpeg(&[], 70_000, 1, 90), Err(ExportError::BadSize { .. })));
        assert!(matches!(encode_jpeg(&[0; 5], 1, 2, 90), Err(ExportError::BadBuffer)));
        assert!(matches!(encode_jpeg(&[0; 3], 1, 1, 0), Err(ExportError::BadQuality(0))));
        assert!(matches!(encode_jpeg(&[0; 3], 1, 1, 101), Err(ExportError::BadQuality(101))));
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
