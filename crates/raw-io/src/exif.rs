//! The Exif data of a JPEG or TIFF picture: camera, orientation, shooting
//! data and credits, read from the TIFF structure the Exif block is (inside
//! a JPEG's APP1 segment, or the TIFF file itself). Only what the editor
//! shows or uses; anything malformed is left out.

use std::io::{Read, Seek, SeekFrom};

use crate::tiff::TiffReader;

/// Exif tags read here.
mod tag {
    pub(super) const MAKE: u16 = 271;
    pub(super) const MODEL: u16 = 272;
    pub(super) const ORIENTATION: u16 = 274;
    pub(super) const DATE_TIME: u16 = 306;
    pub(super) const ARTIST: u16 = 315;
    pub(super) const RATING: u16 = 18246;
    pub(super) const COPYRIGHT: u16 = 33432;
    pub(super) const EXPOSURE_TIME: u16 = 33434;
    pub(super) const F_NUMBER: u16 = 33437;
    pub(super) const EXIF_IFD: u16 = 34665;
    pub(super) const ICC_PROFILE: u16 = 34675;
    pub(super) const ISO: u16 = 34855;
    pub(super) const DATE_TIME_ORIGINAL: u16 = 36867;
    pub(super) const EXPOSURE_BIAS: u16 = 37380;
    pub(super) const FOCAL_LENGTH: u16 = 37386;
    pub(super) const LENS_MAKE: u16 = 42035;
    pub(super) const LENS_MODEL: u16 = 42036;
}

/// What the Exif data says. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Exif {
    pub(crate) make: Option<String>,
    pub(crate) model: Option<String>,
    /// Exif orientation code, 1–8.
    pub(crate) orientation: Option<u16>,
    /// "2026:08:14 18:42:07": the capture time, else the file's date.
    pub(crate) captured: Option<String>,
    pub(crate) exposure_time: Option<(u32, u32)>,
    pub(crate) aperture: Option<f32>,
    pub(crate) iso: Option<u32>,
    pub(crate) focal_length: Option<f32>,
    pub(crate) exposure_bias: Option<f32>,
    pub(crate) lens_make: Option<String>,
    pub(crate) lens_model: Option<String>,
    pub(crate) artist: Option<String>,
    pub(crate) copyright: Option<String>,
    /// 0–5.
    pub(crate) rating: Option<u8>,
    /// The file embeds a colour profile (TIFF tag; JPEGs say it apart).
    pub(crate) has_icc: bool,
}

/// The Exif data of a TIFF structure (a TIFF file, or the Exif block of a JPEG).
pub(crate) fn from_tiff<R: Read + Seek>(reader: R) -> Option<Exif> {
    let mut tiff = TiffReader::new(reader)?;
    let ifd0 = tiff.first()?;
    let exif_ifd = tiff.first_unsigned(&ifd0, tag::EXIF_IFD).and_then(|o| tiff.ifd_at(o));
    #[allow(clippy::cast_possible_truncation)] // small Exif numbers
    let small = |v: f64| Some(v as f32).filter(|v| v.is_finite() && *v > 0.0);
    let mut e = Exif {
        make: tiff.ascii(&ifd0, tag::MAKE),
        model: tiff.ascii(&ifd0, tag::MODEL),
        orientation: tiff.first_unsigned(&ifd0, tag::ORIENTATION).and_then(|o| u16::try_from(o).ok()),
        artist: tiff.ascii(&ifd0, tag::ARTIST),
        copyright: tiff.ascii(&ifd0, tag::COPYRIGHT),
        rating: tiff.first_unsigned(&ifd0, tag::RATING).and_then(|r| u8::try_from(r.min(5)).ok()),
        has_icc: ifd0.get(tag::ICC_PROFILE).is_some(),
        ..Exif::default()
    };
    let file_date = tiff.ascii(&ifd0, tag::DATE_TIME);
    if let Some(ifd) = exif_ifd {
        e.captured = tiff.ascii(&ifd, tag::DATE_TIME_ORIGINAL);
        e.exposure_time = tiff.rational(&ifd, tag::EXPOSURE_TIME).filter(|(n, d)| *n > 0 && *d > 0);
        e.aperture = tiff.real(&ifd, tag::F_NUMBER).and_then(small);
        e.iso = tiff.first_unsigned(&ifd, tag::ISO).filter(|i| *i > 0);
        e.focal_length = tiff.real(&ifd, tag::FOCAL_LENGTH).and_then(small);
        #[allow(clippy::cast_possible_truncation)] // a few stops
        let bias = tiff.real(&ifd, tag::EXPOSURE_BIAS).map(|b| b as f32);
        e.exposure_bias = bias;
        e.lens_make = tiff.ascii(&ifd, tag::LENS_MAKE);
        e.lens_model = tiff.ascii(&ifd, tag::LENS_MODEL);
    }
    // A picture without a capture time (a scan, an export) has its file date.
    e.captured = e.captured.or(file_date).filter(|d| !d.starts_with("0000"));
    Some(e)
}

/// Largest JPEG header segment read (an APP segment is at most 64 KiB).
const MAX_SEGMENT: usize = 1 << 16;

/// The Exif block and whether a colour profile is embedded, from the
/// segments of a JPEG file before its image data.
pub(crate) fn from_jpeg<R: Read + Seek>(mut reader: R) -> Option<Exif> {
    let mut soi = [0_u8; 2];
    reader.seek(SeekFrom::Start(0)).ok()?;
    reader.read_exact(&mut soi).ok()?;
    if soi != [0xFF, 0xD8] {
        return None;
    }
    let mut exif = None;
    let mut has_icc = false;
    loop {
        let mut marker = [0_u8; 2];
        if reader.read_exact(&mut marker).is_err() || marker[0] != 0xFF {
            break;
        }
        // Start of scan or end of image: no more header segments.
        if marker[1] == 0xDA || marker[1] == 0xD9 {
            break;
        }
        // Markers without a length (padding, restart markers).
        if marker[1] == 0xFF || marker[1] == 0x01 || (0xD0..=0xD7).contains(&marker[1]) {
            continue;
        }
        let mut len = [0_u8; 2];
        reader.read_exact(&mut len).ok()?;
        let len = usize::from(u16::from_be_bytes(len)).checked_sub(2)?;
        if len > MAX_SEGMENT {
            return None;
        }
        let mut payload = vec![0_u8; len];
        reader.read_exact(&mut payload).ok()?;
        match marker[1] {
            0xE1 if exif.is_none() && payload.starts_with(b"Exif\0\0") => {
                exif = from_tiff(std::io::Cursor::new(payload[6..].to_vec()));
            }
            0xE2 if payload.starts_with(b"ICC_PROFILE\0") => has_icc = true,
            _ => {}
        }
    }
    let mut e = exif.unwrap_or_default();
    e.has_icc |= has_icc;
    Some(e)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io::Cursor;

    use super::*;
    use crate::tiff::tests::{TestIfd, Value, build};

    fn picture_tiff(big_endian: bool) -> Vec<u8> {
        // IFD0 points at the Exif IFD, which `build` places second.
        let ifd0_len = 2 + 12 * 4 + 4;
        let exif_at = u32::try_from(8 + ifd0_len).unwrap();
        build(
            big_endian,
            &[
                TestIfd {
                    entries: vec![
                        (tag::MAKE, Value::Ascii("FUJIFILM".into())),
                        (tag::ORIENTATION, Value::Short(vec![6])),
                        (tag::DATE_TIME, Value::Ascii("2026:01:02 03:04:05".into())),
                        (tag::EXIF_IFD, Value::Long(vec![exif_at])),
                    ],
                    sub_ifds: vec![],
                },
                TestIfd {
                    entries: vec![
                        (tag::EXPOSURE_TIME, Value::Rational(vec![(1, 250)])),
                        (tag::F_NUMBER, Value::Rational(vec![(28, 10)])),
                        (tag::ISO, Value::Short(vec![400])),
                        (tag::DATE_TIME_ORIGINAL, Value::Ascii("2026:10:29 10:00:00".into())),
                        (tag::EXPOSURE_BIAS, Value::SRational(vec![(-2, 3)])),
                        (tag::LENS_MODEL, Value::Ascii("XF35mmF1.4 R".into())),
                    ],
                    sub_ifds: vec![],
                },
            ],
        )
    }

    #[test]
    fn the_exif_block_gives_camera_orientation_and_shooting_data() {
        for big_endian in [false, true] {
            let e = from_tiff(Cursor::new(picture_tiff(big_endian))).unwrap();
            assert_eq!(e.make.as_deref(), Some("FUJIFILM"));
            assert_eq!(e.orientation, Some(6));
            assert_eq!(e.captured.as_deref(), Some("2026:10:29 10:00:00"), "the capture time wins over the file date");
            assert_eq!(e.exposure_time, Some((1, 250)));
            assert!((e.aperture.unwrap() - 2.8).abs() < 1e-6);
            assert_eq!(e.iso, Some(400));
            assert!((e.exposure_bias.unwrap() + 0.6667).abs() < 1e-3);
            assert_eq!(e.lens_model.as_deref(), Some("XF35mmF1.4 R"));
            assert_eq!(e.model, None);
        }
    }

    #[test]
    fn a_jpeg_gives_its_exif_block_and_says_when_it_has_a_profile() {
        let tiff = picture_tiff(false);
        let mut jpeg = vec![0xFF, 0xD8];
        let mut segment = |marker: u8, payload: &[u8]| {
            jpeg.extend_from_slice(&[0xFF, marker]);
            jpeg.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
            jpeg.extend_from_slice(payload);
        };
        segment(0xE0, b"JFIF\0\x01\x02");
        segment(0xE1, &[b"Exif\0\0".as_slice(), &tiff].concat());
        segment(0xE2, b"ICC_PROFILE\0\x01\x01data");
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xAB, 0xCD]);
        let e = from_jpeg(Cursor::new(jpeg.clone())).unwrap();
        assert_eq!(e.captured.as_deref(), Some("2026:10:29 10:00:00"));
        assert!(e.has_icc);
        // Cut anywhere: never a panic.
        for cut in 0..jpeg.len() {
            let _ = from_jpeg(Cursor::new(jpeg[..cut].to_vec()));
        }
        assert_eq!(from_jpeg(Cursor::new(b"not a jpeg".to_vec())), None);
    }
}
