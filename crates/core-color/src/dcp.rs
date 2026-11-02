//! Reads DNG camera profiles (`.dcp` files, DNG specification 1.4+).
//!
//! A DCP file is a TIFF-like container (byte order `II` or `MM`, magic
//! `RC` instead of 42) with one directory of DNG profile tags. Every count
//! and offset is checked against the file size before reading, because
//! profiles come from outside the editor.

use std::fmt;

use core_model::{Illuminant, Matrix3};

use crate::profile::{Calibration, ColorProfile, HsvTable, ProfileError, ProfileSource};

/// Largest profile file accepted (Adobe's largest are a few MB).
pub const MAX_DCP_BYTES: usize = 32 << 20;

const UNIQUE_CAMERA_MODEL: u16 = 50708;
const COLOR_MATRIX_1: u16 = 50721;
const COLOR_MATRIX_2: u16 = 50722;
const CALIBRATION_ILLUMINANT_1: u16 = 50778;
const CALIBRATION_ILLUMINANT_2: u16 = 50779;
const PROFILE_NAME: u16 = 50936;
const HUE_SAT_MAP_DIMS: u16 = 50937;
const HUE_SAT_MAP_DATA_1: u16 = 50938;
const HUE_SAT_MAP_DATA_2: u16 = 50939;
const TONE_CURVE: u16 = 50940;
const COPYRIGHT: u16 = 50942;
const FORWARD_MATRIX_1: u16 = 50964;
const FORWARD_MATRIX_2: u16 = 50965;
const LOOK_TABLE_DIMS: u16 = 50981;
const LOOK_TABLE_DATA: u16 = 50982;
const HUE_SAT_MAP_ENCODING: u16 = 51107;
const LOOK_TABLE_ENCODING: u16 = 51108;
const BASELINE_EXPOSURE_OFFSET: u16 = 51109;

/// Why a DCP file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DcpError {
    /// Not a DCP file (wrong header).
    NotADcp,
    /// The file is larger than [`MAX_DCP_BYTES`].
    TooLarge,
    /// A directory entry points outside the file or has an unexpected type.
    Damaged(String),
    /// The profile's contents are not usable.
    Profile(ProfileError),
}

impl fmt::Display for DcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADcp => write!(f, "not a DCP camera profile"),
            Self::TooLarge => write!(f, "profile file larger than {} MB", MAX_DCP_BYTES >> 20),
            Self::Damaged(m) => write!(f, "damaged profile: {m}"),
            Self::Profile(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for DcpError {}

#[derive(Debug, Clone, Copy)]
struct Entry {
    typ: u16,
    count: u32,
    /// Offset of the value bytes in the file (inline values point into the entry).
    at: usize,
}

struct Reader<'a> {
    bytes: &'a [u8],
    big_endian: bool,
}

impl Reader<'_> {
    fn slice(&self, at: usize, len: usize) -> Result<&[u8], DcpError> {
        at.checked_add(len)
            .and_then(|end| self.bytes.get(at..end))
            .ok_or_else(|| DcpError::Damaged(format!("{len} bytes at {at} are outside the file")))
    }

    fn u16(&self, at: usize) -> Result<u16, DcpError> {
        let b = self.slice(at, 2)?;
        let a = [b[0], b[1]];
        Ok(if self.big_endian { u16::from_be_bytes(a) } else { u16::from_le_bytes(a) })
    }

    fn u32(&self, at: usize) -> Result<u32, DcpError> {
        let b = self.slice(at, 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Ok(if self.big_endian { u32::from_be_bytes(a) } else { u32::from_le_bytes(a) })
    }

    fn type_size(typ: u16) -> Option<usize> {
        Some(match typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        })
    }

    /// Value `i` of an entry as a number, whatever its numeric type.
    fn number(&self, e: Entry, i: usize) -> Result<f64, DcpError> {
        let size = Self::type_size(e.typ).ok_or_else(|| DcpError::Damaged(format!("unknown type {}", e.typ)))?;
        let at = e.at + i * size;
        Ok(match e.typ {
            1 | 7 => f64::from(self.slice(at, 1)?[0]),
            6 => f64::from(i8::from_ne_bytes([self.slice(at, 1)?[0]])),
            3 => f64::from(self.u16(at)?),
            8 => f64::from(i16::from_ne_bytes(self.u16(at)?.to_ne_bytes())),
            4 => f64::from(self.u32(at)?),
            9 => f64::from(i32::from_ne_bytes(self.u32(at)?.to_ne_bytes())),
            11 => f64::from(f32::from_bits(self.u32(at)?)),
            5 | 10 => {
                let (n, d) = (self.u32(at)?, self.u32(at + 4)?);
                let (n, d) = if e.typ == 10 {
                    (f64::from(i32::from_ne_bytes(n.to_ne_bytes())), f64::from(i32::from_ne_bytes(d.to_ne_bytes())))
                } else {
                    (f64::from(n), f64::from(d))
                };
                if d == 0.0 { 0.0 } else { n / d }
            }
            12 => {
                let hi = u64::from(self.u32(at)?);
                let lo = u64::from(self.u32(at + 4)?);
                let bits = if self.big_endian { (hi << 32) | lo } else { (lo << 32) | hi };
                f64::from_bits(bits)
            }
            _ => return Err(DcpError::Damaged(format!("unknown type {}", e.typ))),
        })
    }

    fn numbers(&self, e: Entry) -> Result<Vec<f64>, DcpError> {
        (0..e.count as usize).map(|i| self.number(e, i)).collect()
    }

    fn text(&self, e: Entry) -> Result<String, DcpError> {
        let raw = self.slice(e.at, e.count as usize)?;
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        Ok(String::from_utf8_lossy(&raw[..end]).trim().to_owned())
    }
}

/// Reads a DCP file's bytes into a profile. `source` records where it came from.
///
/// # Errors
/// [`DcpError`] when the bytes are not a usable DCP profile.
pub fn read_dcp(bytes: &[u8], source: ProfileSource) -> Result<ColorProfile, DcpError> {
    if bytes.len() > MAX_DCP_BYTES {
        return Err(DcpError::TooLarge);
    }
    let big_endian = match bytes.get(..4) {
        Some(b"IIRC") => false,
        Some(b"MMCR") => true,
        _ => return Err(DcpError::NotADcp),
    };
    let r = Reader { bytes, big_endian };
    let ifd = r.u32(4)? as usize;
    let count = usize::from(r.u16(ifd)?);
    let mut entries: Vec<(u16, Entry)> = Vec::with_capacity(count);
    for i in 0..count {
        let base = ifd + 2 + i * 12;
        let tag = r.u16(base)?;
        let typ = r.u16(base + 2)?;
        let n = r.u32(base + 4)?;
        let Some(size) = Reader::type_size(typ) else { continue };
        let len = size.checked_mul(n as usize).ok_or_else(|| DcpError::Damaged("entry too large".to_owned()))?;
        let at = if len <= 4 { base + 8 } else { r.u32(base + 8)? as usize };
        r.slice(at, len)?;
        entries.push((tag, Entry { typ, count: n, at }));
    }
    let get = |tag: u16| entries.iter().find(|(t, _)| *t == tag).map(|(_, e)| *e);

    let matrix = |tag: u16| -> Result<Option<Matrix3>, DcpError> {
        let Some(e) = get(tag) else { return Ok(None) };
        let v = r.numbers(e)?;
        if v.len() != 9 || v.iter().any(|x| !x.is_finite()) || v.iter().all(|x| *x == 0.0) {
            return Ok(None);
        }
        #[allow(clippy::cast_possible_truncation)] // f64 → f32 is the matrix precision
        let m = Matrix3::from_rows([
            [v[0] as f32, v[1] as f32, v[2] as f32],
            [v[3] as f32, v[4] as f32, v[5] as f32],
            [v[6] as f32, v[7] as f32, v[8] as f32],
        ]);
        Ok(Some(m))
    };
    let illuminant = |tag: u16| -> Result<Option<Illuminant>, DcpError> {
        match get(tag) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a 16-bit code
            Some(e) => Ok(Some(Illuminant::from_code(r.number(e, 0)?.clamp(0.0, 65535.0) as u16))),
            None => Ok(None),
        }
    };
    let dims = |tag: u16| -> Result<Option<[u32; 3]>, DcpError> {
        let Some(e) = get(tag) else { return Ok(None) };
        let v = r.numbers(e)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked range
        let d = |x: f64| if (0.0..=65535.0).contains(&x) { x as u32 } else { 0 };
        Ok(match v.as_slice() {
            [h, s] => Some([d(*h), d(*s), 1]),
            [h, s, val, ..] => Some([d(*h), d(*s), d(*val)]),
            _ => None,
        })
    };
    let floats = |tag: u16| -> Result<Option<Vec<f32>>, DcpError> {
        let Some(e) = get(tag) else { return Ok(None) };
        if e.typ != 11 {
            return Err(DcpError::Damaged(format!("tag {tag} is not stored as floats")));
        }
        // Checked against the file size above; also bounded by the table limit.
        if e.count as usize > crate::profile::MAX_TABLE_ENTRIES * 3 {
            return Err(DcpError::Damaged(format!("tag {tag} is too large")));
        }
        #[allow(clippy::cast_possible_truncation)] // floats stored as f32
        let values = r.numbers(e)?.into_iter().map(|x| x as f32).collect();
        Ok(Some(values))
    };
    let encoding_is_srgb = |tag: u16| -> Result<bool, DcpError> {
        Ok(match get(tag) {
            Some(e) => (r.number(e, 0)? - 1.0).abs() < 0.5,
            None => false,
        })
    };

    let hsm_dims = dims(HUE_SAT_MAP_DIMS)?;
    let hsm_srgb = encoding_is_srgb(HUE_SAT_MAP_ENCODING)?;
    let table = |data: Option<Vec<f32>>, dims: Option<[u32; 3]>, srgb: bool| -> Result<Option<HsvTable>, DcpError> {
        match (data, dims) {
            (Some(data), Some(dims)) => HsvTable::new(dims, &data, srgb)
                .map(Some)
                .ok_or_else(|| DcpError::Damaged("table size does not match its dimensions".to_owned())),
            _ => Ok(None),
        }
    };

    let mut calibrations = Vec::new();
    for (cm, fm, ill, hsm) in [
        (COLOR_MATRIX_1, FORWARD_MATRIX_1, CALIBRATION_ILLUMINANT_1, HUE_SAT_MAP_DATA_1),
        (COLOR_MATRIX_2, FORWARD_MATRIX_2, CALIBRATION_ILLUMINANT_2, HUE_SAT_MAP_DATA_2),
    ] {
        if let Some(color_matrix) = matrix(cm)? {
            calibrations.push(Calibration {
                // A single-illuminant profile may omit the illuminant; D65 is the common default.
                illuminant: illuminant(ill)?.unwrap_or(Illuminant::D65),
                color_matrix,
                forward_matrix: matrix(fm)?,
                hue_sat_map: table(floats(hsm)?, hsm_dims, hsm_srgb)?,
            });
        }
    }

    let look_table = table(floats(LOOK_TABLE_DATA)?, dims(LOOK_TABLE_DIMS)?, encoding_is_srgb(LOOK_TABLE_ENCODING)?)?;
    let tone_curve = match floats(TONE_CURVE)? {
        Some(v) if v.len() >= 4 && v.len() % 2 == 0 => Some(v.as_chunks::<2>().0.to_vec()),
        _ => None,
    };

    let camera = get(UNIQUE_CAMERA_MODEL).map(|e| r.text(e)).transpose()?.unwrap_or_default();
    let name = match get(PROFILE_NAME).map(|e| r.text(e)).transpose()? {
        Some(n) if !n.is_empty() => n,
        _ => match &source {
            ProfileSource::Dcp(path) => path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            ProfileSource::Builtin => String::new(),
        },
    };
    let mut profile =
        ColorProfile::new(name, camera, source, calibrations, look_table, tone_curve).map_err(DcpError::Profile)?;
    profile.copyright = get(COPYRIGHT).map(|e| r.text(e)).transpose()?.filter(|c| !c.is_empty());
    if let Some(e) = get(BASELINE_EXPOSURE_OFFSET) {
        let v = r.number(e, 0)?;
        if v.is_finite() {
            #[allow(clippy::cast_possible_truncation)] // a few stops
            let stops = v as f32;
            profile.baseline_exposure_offset = stops;
        }
    }
    Ok(profile)
}

/// Writes a minimal DCP file (little-endian), for tests and for exporting
/// the built-in matrices.
#[must_use]
pub fn write_dcp(profile: &ColorProfile) -> Vec<u8> {
    let mut tags: Vec<(u16, u16, Vec<u8>, u32)> = Vec::new(); // (tag, type, bytes, count)
    let ascii = |s: &str| {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    };
    let srational = |m: &Matrix3| {
        let mut b = Vec::with_capacity(72);
        for v in m.0.iter().flatten() {
            #[allow(clippy::cast_possible_truncation)] // 4 decimals of a matrix entry
            let n = (f64::from(*v) * 10_000.0).round() as i32;
            b.extend_from_slice(&n.to_le_bytes());
            b.extend_from_slice(&10_000_i32.to_le_bytes());
        }
        b
    };
    let code = |i: Illuminant| -> u16 { (1..=255).find(|c| Illuminant::from_code(*c) == i).unwrap_or(0) };
    let len = |b: &[u8]| u32::try_from(b.len()).unwrap_or(u32::MAX);
    let camera = ascii(&profile.camera);
    tags.push((UNIQUE_CAMERA_MODEL, 2, camera.clone(), len(&camera)));
    let name = ascii(&profile.name);
    tags.push((PROFILE_NAME, 2, name.clone(), len(&name)));
    for (i, c) in profile.calibrations().iter().enumerate().take(2) {
        let (cm, ill, fm) = if i == 0 {
            (COLOR_MATRIX_1, CALIBRATION_ILLUMINANT_1, FORWARD_MATRIX_1)
        } else {
            (COLOR_MATRIX_2, CALIBRATION_ILLUMINANT_2, FORWARD_MATRIX_2)
        };
        tags.push((cm, 10, srational(&c.color_matrix), 9));
        tags.push((ill, 3, code(c.illuminant).to_le_bytes().to_vec(), 1));
        if let Some(f) = &c.forward_matrix {
            tags.push((fm, 10, srational(f), 9));
        }
    }
    tags.sort_by_key(|t| t.0);

    let mut out = b"IIRC".to_vec();
    out.extend_from_slice(&8_u32.to_le_bytes());
    let count = u16::try_from(tags.len()).unwrap_or(u16::MAX);
    let data_start = 8 + 2 + tags.len() * 12 + 4;
    let mut data = Vec::new();
    out.extend_from_slice(&count.to_le_bytes());
    for (tag, typ, bytes, n) in &tags {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&typ.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        if bytes.len() <= 4 {
            let mut inline = bytes.clone();
            inline.resize(4, 0);
            out.extend_from_slice(&inline);
        } else {
            let offset = u32::try_from(data_start + data.len()).unwrap_or(u32::MAX);
            out.extend_from_slice(&offset.to_le_bytes());
            data.extend_from_slice(bytes);
            if data.len() % 2 == 1 {
                data.push(0);
            }
        }
    }
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_model::CalibratedMatrix;

    fn sample() -> ColorProfile {
        let m = |s: f32| Matrix3::from_rows([[0.8 * s, -0.3, -0.07], [-0.43, 1.2, 0.25], [-0.04, 0.13, 0.58]]);
        ColorProfile::from_matrices(
            "Sony ILCE-7RM5",
            &[
                CalibratedMatrix { illuminant: Illuminant::StandardA, xyz_to_camera: m(1.1) },
                CalibratedMatrix { illuminant: Illuminant::D65, xyz_to_camera: m(1.0) },
            ],
        )
        .expect("profile")
    }

    #[test]
    fn a_written_profile_reads_back() {
        let p = sample();
        let bytes = write_dcp(&p);
        let back = read_dcp(&bytes, ProfileSource::Builtin).expect("reads");
        assert_eq!(back.camera, "Sony ILCE-7RM5");
        assert_eq!(back.name, "Built-in");
        assert_eq!(back.calibrations().len(), 2);
        assert_eq!(back.calibrations()[0].illuminant, Illuminant::StandardA);
        for (a, b) in back.calibrations().iter().zip(p.calibrations()) {
            for (x, y) in a.color_matrix.0.iter().flatten().zip(b.color_matrix.0.iter().flatten()) {
                assert!((x - y).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn damaged_files_are_refused_without_panicking() {
        assert_eq!(read_dcp(b"II*\0", ProfileSource::Builtin).map(|_| ()), Err(DcpError::NotADcp));
        let mut bytes = write_dcp(&sample());
        // Point the directory outside the file.
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read_dcp(&bytes, ProfileSource::Builtin), Err(DcpError::Damaged(_))));
        // Every truncation fails cleanly.
        let full = write_dcp(&sample());
        for n in 0..full.len() {
            let _ = read_dcp(&full[..n], ProfileSource::Builtin);
        }
    }

    #[test]
    fn a_profile_without_matrices_is_refused() {
        let empty = ColorProfile::from_matrices("X", &[]);
        assert_eq!(empty.map(|_| ()), Err(ProfileError::NoColorMatrix));
    }
}
